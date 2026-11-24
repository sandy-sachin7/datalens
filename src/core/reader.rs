use crate::core::model::action::Action;
use crate::core::parser::checkpoint::{parse_parquet_checkpoint, LastCheckpoint};
use crate::core::parser::commit::{CommitParser, ParseWarning};
use crate::core::storage::StorageBackend;
use crate::error::DeltaLensError;
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

/// One version entry: its number, file path, and parsed actions.
#[derive(Debug, Clone)]
pub struct CommitEntry {
    pub version: u64,
    pub path: String,
    pub actions: Vec<Action>,
}

/// Complete checkpoint representation discovered in `_delta_log/`.
#[derive(Debug, Clone)]
pub enum CheckpointKind {
    ClassicParquet(String),
    MultiPartParquet {
        total_parts: u32,
        parts: Vec<String>,
    },
    V2Json(String),
    V2Parquet(String),
}

/// A Delta log compaction file covering `[start_version, end_version]`.
/// Its actions are returned as one entry at `end_version` because the file does not
/// preserve per-commit attribution.
#[derive(Debug, Clone)]
pub struct CompactedFile {
    pub start_version: u64,
    pub end_version: u64,
    pub path: String,
}

/// Cached inventory of `_delta_log/` produced by a single directory listing.
#[derive(Debug, Clone, Default)]
pub struct LogInventory {
    pub commits: BTreeMap<u64, String>,
    pub checkpoints: BTreeMap<u64, CheckpointKind>,
    pub compactions: Vec<CompactedFile>,
    pub last_checkpoint_hint: Option<u64>,
}

impl LogInventory {
    pub fn max_version(&self) -> Option<u64> {
        let max_commit = self.commits.keys().next_back().copied();
        let max_ckpt = self.checkpoints.keys().next_back().copied();
        let max_compact = self.compactions.iter().map(|c| c.end_version).max();
        [max_commit, max_ckpt, max_compact]
            .into_iter()
            .flatten()
            .max()
    }

    pub fn min_version(&self) -> Option<u64> {
        let min_commit = self.commits.keys().next().copied();
        let min_ckpt = self.checkpoints.keys().next().copied();
        let min_compact = self.compactions.iter().map(|c| c.start_version).min();
        [min_commit, min_ckpt, min_compact]
            .into_iter()
            .flatten()
            .min()
    }

    /// Return the latest complete checkpoint version `<= target`.
    pub fn latest_checkpoint_le(&self, target: u64) -> Option<(u64, &CheckpointKind)> {
        self.checkpoints
            .range(..=target)
            .next_back()
            .map(|(v, k)| (*v, k))
    }
}

/// Checkpoint-accelerated log slice for reconstructing state at `target_version`.
#[derive(Debug, Clone)]
pub struct SnapshotLog {
    pub target_version: u64,
    pub checkpoint_version: Option<u64>,
    pub checkpoint_actions: Vec<Action>,
    pub commits: Vec<CommitEntry>,
    pub warnings: Vec<ParseWarning>,
    pub files_read: usize,
}

/// Entry point for all Delta log reading operations.
pub struct DeltaLogReader {
    pub table_root: String,
    pub log_subpath: String,
    pub storage: Box<dyn StorageBackend>,
    inventory_cache: OnceLock<LogInventory>,
}

impl DeltaLogReader {
    /// Create a reader for a Delta table using the given storage backend.
    pub fn new(storage: Box<dyn StorageBackend>, table_root: &str) -> Result<Self, DeltaLensError> {
        let log_subpath = "_delta_log".to_string();

        if !storage.is_dir(&log_subpath)? {
            return Err(DeltaLensError::NotADeltaTable(table_root.to_string()));
        }

        Ok(Self {
            table_root: table_root.to_string(),
            log_subpath,
            storage,
            inventory_cache: OnceLock::new(),
        })
    }

    /// Discover and cache all commits, checkpoints, and compaction files with a SINGLE listing.
    pub fn inventory(&self) -> Result<&LogInventory, DeltaLensError> {
        if let Some(inv) = self.inventory_cache.get() {
            return Ok(inv);
        }

        let entries = self.storage.list(&self.log_subpath)?;
        let (mut inv, has_last_checkpoint) = parse_inventory_entries(entries);

        inv.last_checkpoint_hint = if has_last_checkpoint {
            self.storage
                .read(&format!("{}/_last_checkpoint", self.log_subpath))
                .ok()
                .and_then(|bytes| LastCheckpoint::parse_bytes(&bytes).ok())
                .map(|lc| lc.version)
        } else {
            None
        };

        Ok(self.inventory_cache.get_or_init(|| inv))
    }

    /// Attempt an offset-accelerated partial inventory starting from `_last_checkpoint`
    /// so snapshot reads on large cloud tables avoid listing tens of thousands of old JSON files.
    /// Returns `Ok(None)` to fall back to a full listing if:
    /// - full inventory is already cached,
    /// - `_last_checkpoint` is missing, unreadable, invalid, or points to version 0,
    /// - `target_version` precedes `_last_checkpoint`,
    /// - the storage backend does not support offset listing, or
    /// - `_last_checkpoint` is stale/incomplete (no valid complete checkpoint found).
    fn try_snapshot_inventory(
        &self,
        target_version: Option<u64>,
    ) -> Result<Option<LogInventory>, DeltaLensError> {
        if self.inventory_cache.get().is_some() {
            return Ok(None);
        }

        let last_ckpt_path = format!("{}/_last_checkpoint", self.log_subpath);
        let Ok(bytes) = self.storage.read(&last_ckpt_path) else {
            return Ok(None);
        };
        let Ok(last_ckpt) = LastCheckpoint::parse_bytes(&bytes) else {
            return Ok(None);
        };
        let hint_v = last_ckpt.version;
        if hint_v == 0 {
            return Ok(None);
        }
        if let Some(target) = target_version {
            if target < hint_v {
                return Ok(None);
            }
        }

        let offset_name = format!("{:020}", hint_v);
        let Some(entries) = self
            .storage
            .list_from_offset(&self.log_subpath, &offset_name)?
        else {
            return Ok(None);
        };

        let (mut partial_inv, _) = parse_inventory_entries(entries);
        partial_inv.last_checkpoint_hint = Some(hint_v);

        let Some(max_v) = partial_inv.max_version() else {
            return Ok(None);
        };
        let check_bound = target_version.map_or(max_v, |t| t.min(max_v));
        if partial_inv.latest_checkpoint_le(check_bound).is_none() {
            return Ok(None);
        }

        Ok(Some(partial_inv))
    }

    /// Current (maximum) version in the log across commits, checkpoints, and compactions.
    pub fn current_version(&self) -> Result<u64, DeltaLensError> {
        let inv = self.inventory()?;
        inv.max_version().ok_or(DeltaLensError::EmptyTable)
    }

    /// Return the actual latest complete checkpoint version `<= max_version`, if any exists.
    pub fn latest_checkpoint_version(
        &self,
        max_version: Option<u64>,
    ) -> Result<Option<u64>, DeltaLensError> {
        let inv = self.inventory()?;
        let bound = max_version.unwrap_or(u64::MAX);
        Ok(inv.latest_checkpoint_le(bound).map(|(v, _)| v))
    }

    /// Read a checkpoint-accelerated snapshot log at `target_version` (default: latest).
    /// Starts from the latest complete checkpoint <= target_version (if one exists) and
    /// replays only subsequent commits (leveraging `.compacted.json` when available).
    /// On cloud object stores, uses `_last_checkpoint` + `list_with_offset` when valid.
    pub fn read_snapshot_log(
        &self,
        target_version: Option<u64>,
    ) -> Result<SnapshotLog, DeltaLensError> {
        if let Some(partial_inv) = self.try_snapshot_inventory(target_version)? {
            return self.read_snapshot_log_from_inventory(&partial_inv, target_version);
        }
        let inv = self.inventory()?;
        self.read_snapshot_log_from_inventory(inv, target_version)
    }

    fn read_snapshot_log_from_inventory(
        &self,
        inv: &LogInventory,
        target_version: Option<u64>,
    ) -> Result<SnapshotLog, DeltaLensError> {
        let max_v = inv.max_version().ok_or(DeltaLensError::EmptyTable)?;
        let target = match target_version {
            Some(v) => {
                if v > max_v {
                    return Err(DeltaLensError::VersionNotFound(v));
                }
                v
            }
            None => max_v,
        };

        let mut warnings = Vec::new();
        let mut files_read = 0usize;

        // Pick latest complete checkpoint <= target.
        let (checkpoint_version, checkpoint_actions, start_after) =
            if let Some((ckpt_v, kind)) = inv.latest_checkpoint_le(target) {
                let (actions, ckpt_warnings, ckpt_files) = self.read_checkpoint(kind)?;
                warnings.extend(ckpt_warnings);
                files_read += ckpt_files;
                (Some(ckpt_v), actions, ckpt_v.checked_add(1))
            } else {
                // No checkpoint <= target; must reconstruct from version 0.
                let min_v = inv.min_version().ok_or(DeltaLensError::EmptyTable)?;
                if target < min_v {
                    return Err(DeltaLensError::VersionNotFound(target));
                }
                if min_v > 0 && !inv.commits.contains_key(&0) {
                    return Err(DeltaLensError::VersionNotFound(0));
                }
                (None, Vec::new(), Some(0))
            };

        let mut commits = Vec::new();
        if let Some(start_v) = start_after {
            if start_v <= target {
                let (delta_commits, delta_warnings, delta_files) =
                    self.read_contiguous_range(inv, start_v, target, true)?;
                commits = delta_commits;
                warnings.extend(delta_warnings);
                files_read += delta_files;
            }
        }

        Ok(SnapshotLog {
            target_version: target,
            checkpoint_version,
            checkpoint_actions,
            commits,
            warnings,
            files_read,
        })
    }

    /// Read full history starting from version 0 when available, or seeded from the
    /// earliest available checkpoint when pre-checkpoint commits have expired (so
    /// `schema --history` retains the pre-pruning schema baseline).
    pub fn read_history_snapshot_log(&self) -> Result<SnapshotLog, DeltaLensError> {
        let inv = self.inventory()?;
        let max_v = inv.max_version().ok_or(DeltaLensError::EmptyTable)?;

        let first_commit_v = inv
            .commits
            .keys()
            .next()
            .copied()
            .into_iter()
            .chain(inv.compactions.iter().map(|c| c.start_version))
            .min();

        let seed_checkpoint = if first_commit_v == Some(0) {
            None
        } else {
            first_commit_v
                .and_then(|fv| inv.latest_checkpoint_le(fv))
                .or_else(|| inv.checkpoints.iter().next().map(|(v, k)| (*v, k)))
        };

        if let Some((ckpt_v, kind)) = seed_checkpoint {
            let (checkpoint_actions, mut warnings, mut files_read) = self.read_checkpoint(kind)?;
            let mut commits = Vec::new();
            if let Some(start_v) = ckpt_v.checked_add(1) {
                if start_v <= max_v {
                    let (delta_commits, delta_warnings, delta_files) =
                        self.read_contiguous_range(inv, start_v, max_v, true)?;
                    commits = delta_commits;
                    warnings.extend(delta_warnings);
                    files_read += delta_files;
                }
            }
            Ok(SnapshotLog {
                target_version: max_v,
                checkpoint_version: Some(ckpt_v),
                checkpoint_actions,
                commits,
                warnings,
                files_read,
            })
        } else {
            let (commits, warnings) = self.read_all_with_warnings()?;
            let files_read = commits.len();
            Ok(SnapshotLog {
                target_version: max_v,
                checkpoint_version: None,
                checkpoint_actions: Vec::new(),
                commits,
                warnings,
                files_read,
            })
        }
    }

    /// Read commit and compaction files in version range `[from, to]` (inclusive).
    pub fn read_range(
        &self,
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<Vec<CommitEntry>, DeltaLensError> {
        let (entries, _warnings) = self.read_range_with_warnings(from, to)?;
        Ok(entries)
    }

    /// Read commit and compaction files in `[from, to]` with parse warnings.
    pub fn read_range_with_warnings(
        &self,
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<(Vec<CommitEntry>, Vec<ParseWarning>), DeltaLensError> {
        if let (Some(start), Some(end)) = (from, to) {
            if start > end {
                return Err(DeltaLensError::InvalidVersionRange(start, end));
            }
        }

        let inv = self.inventory()?;
        let first_available = inv
            .commits
            .keys()
            .next()
            .copied()
            .into_iter()
            .chain(inv.compactions.iter().map(|c| c.start_version))
            .min();
        let last_available = inv
            .commits
            .keys()
            .next_back()
            .copied()
            .into_iter()
            .chain(inv.compactions.iter().map(|c| c.end_version))
            .max();

        let (Some(first_available), Some(last_available)) = (first_available, last_available)
        else {
            return if let Some(requested) = from.or(to) {
                Err(DeltaLensError::VersionNotFound(requested))
            } else {
                Ok((Vec::new(), Vec::new()))
            };
        };

        let start = from.unwrap_or(first_available);
        let end = to.unwrap_or(last_available);
        if from.is_some() && start > last_available {
            return Err(DeltaLensError::VersionNotFound(start));
        }
        if to.is_some() && end > last_available {
            return Err(DeltaLensError::VersionNotFound(end));
        }
        if from.is_none() && to.is_some() && end < first_available {
            return Err(DeltaLensError::VersionNotFound(end));
        }
        if start > end {
            return Ok((Vec::new(), Vec::new()));
        }
        if start < first_available {
            return Err(DeltaLensError::VersionGap {
                expected: start,
                found: first_available,
            });
        }

        let (entries, warnings, _) = self.read_contiguous_range(inv, start, end, true)?;
        Ok((entries, warnings))
    }

    /// Read all available commits and compaction files.
    pub fn read_all(&self) -> Result<Vec<CommitEntry>, DeltaLensError> {
        self.read_range(None, None)
    }

    /// Read all available commits and compaction files with structured warnings.
    pub fn read_all_with_warnings(
        &self,
    ) -> Result<(Vec<CommitEntry>, Vec<ParseWarning>), DeltaLensError> {
        self.read_range_with_warnings(None, None)
    }

    /// Read only the latest N commit or compaction records.
    pub fn read_last(&self, n: usize) -> Result<Vec<CommitEntry>, DeltaLensError> {
        let (entries, _warnings) = self.read_last_with_warnings(n)?;
        Ok(entries)
    }

    /// Read only the latest N unique commit or compaction records with parse warnings.
    /// Individual `.json` commits always take precedence over `.compacted.json` files for
    /// versions they cover; a compaction is included only if it covers at least one version
    /// that has no individual commit file.
    pub fn read_last_with_warnings(
        &self,
        n: usize,
    ) -> Result<(Vec<CommitEntry>, Vec<ParseWarning>), DeltaLensError> {
        if n == 0 {
            return Ok((Vec::new(), Vec::new()));
        }
        let inv = self.inventory()?;

        // Individual `.json` commits take precedence over compactions for versions they cover.
        let mut records: BTreeMap<u64, String> = inv
            .commits
            .iter()
            .map(|(&version, path)| (version, path.clone()))
            .collect();

        // `inv.compactions` is sorted by `(start_version ASC, end_version DESC)`.
        // Skip any compaction completely subsumed by an earlier compaction, and only include
        // compactions that cover at least one version lacking an individual commit file.
        let mut max_compacted_end: Option<u64> = None;
        for compaction in &inv.compactions {
            if max_compacted_end.is_some_and(|max_end| compaction.end_version <= max_end) {
                continue;
            }
            max_compacted_end = Some(compaction.end_version);

            if let Some(uncovered_v) = highest_uncovered_version(
                &inv.commits,
                compaction.start_version,
                compaction.end_version,
            ) {
                records
                    .entry(uncovered_v)
                    .or_insert_with(|| compaction.path.clone());
            }
        }

        if records.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }

        let mut selected: Vec<(u64, String)> = records.into_iter().rev().take(n).collect();
        selected.reverse();
        self.load_and_parse_commits(&selected)
    }

    fn read_contiguous_range(
        &self,
        inv: &LogInventory,
        start_v: u64,
        end_v: u64,
        allow_compactions: bool,
    ) -> Result<(Vec<CommitEntry>, Vec<ParseWarning>, usize), DeltaLensError> {
        let mut planned: Vec<(u64, String)> = Vec::new();
        let mut v = start_v;

        while v <= end_v {
            // Prefer the largest compaction file starting at `v` that fits within `end_v`.
            if allow_compactions {
                if let Some(comp) = inv
                    .compactions
                    .iter()
                    .filter(|c| c.start_version == v && c.end_version <= end_v)
                    .max_by_key(|c| c.end_version)
                {
                    // A compacted file has no per-commit version attribution. Keep its
                    // actions together and label the entry with the covered end version.
                    planned.push((comp.end_version, comp.path.clone()));
                    match comp.end_version.checked_add(1) {
                        Some(next) => {
                            v = next;
                            continue;
                        }
                        None => break,
                    }
                }
            }

            if let Some(path) = inv.commits.get(&v) {
                planned.push((v, path.clone()));
                match v.checked_add(1) {
                    Some(next) => v = next,
                    None => break,
                }
            } else {
                // Report exact gap if a higher version exists in commits or compactions,
                // or VersionNotFound if nothing higher exists in the requested range.
                let next_commit = inv.commits.range(v..=end_v).next().map(|(&ver, _)| ver);
                let next_compaction = inv
                    .compactions
                    .iter()
                    .filter(|c| c.start_version > v && c.start_version <= end_v)
                    .map(|c| c.start_version)
                    .min();
                if let Some(next_found) = [next_commit, next_compaction].into_iter().flatten().min()
                {
                    return Err(DeltaLensError::VersionGap {
                        expected: v,
                        found: next_found,
                    });
                }
                return Err(DeltaLensError::VersionNotFound(v));
            }
        }

        let files_read = planned.len();
        let (entries, warnings) = self.load_and_parse_commits(&planned)?;
        Ok((entries, warnings, files_read))
    }

    fn read_checkpoint(
        &self,
        kind: &CheckpointKind,
    ) -> Result<(Vec<Action>, Vec<ParseWarning>, usize), DeltaLensError> {
        let mut actions = Vec::new();
        let mut warnings = Vec::new();
        let mut files_read = 0usize;

        match kind {
            CheckpointKind::ClassicParquet(path) | CheckpointKind::V2Parquet(path) => {
                let bytes = self.storage.read(path)?;
                files_read += 1;
                let parsed = parse_parquet_checkpoint(&bytes, path)?;
                actions.extend(parsed.actions);
                warnings.extend(parsed.warnings);
            }
            CheckpointKind::MultiPartParquet { parts, .. } => {
                let raw = self.storage.read_many(parts)?;
                files_read += raw.len();
                let parsed_results: Result<Vec<_>, DeltaLensError> = raw
                    .par_iter()
                    .map(|(path, bytes)| parse_parquet_checkpoint(bytes, path))
                    .collect();
                for pr in parsed_results? {
                    actions.extend(pr.actions);
                    warnings.extend(pr.warnings);
                }
            }
            CheckpointKind::V2Json(path) => {
                let bytes = self.storage.read(path)?;
                files_read += 1;
                let parsed = CommitParser::parse_bytes_with_warnings(&bytes, path)?;
                actions.extend(parsed.actions);
                warnings.extend(parsed.warnings);
            }
        }

        // Resolve V2 checkpoint sidecar files below `_delta_log/_sidecars/`.
        let sidecar_paths: Vec<String> = actions
            .iter()
            .filter_map(|action| match action {
                Action::Sidecar(sidecar) => Some(sidecar.path.as_str()),
                _ => None,
            })
            .map(|path| sidecar_storage_path(&self.log_subpath, path))
            .collect::<Result<Vec<_>, _>>()?;

        if !sidecar_paths.is_empty() {
            let sidecar_blobs = self.storage.read_many(&sidecar_paths)?;
            files_read += sidecar_blobs.len();
            let parsed_sidecars: Result<Vec<_>, DeltaLensError> = sidecar_blobs
                .par_iter()
                .map(|(path, bytes)| parse_parquet_checkpoint(bytes, path))
                .collect();
            for pr in parsed_sidecars? {
                actions.extend(pr.actions);
                warnings.extend(pr.warnings);
            }
        }

        Ok((actions, warnings, files_read))
    }

    fn load_and_parse_commits(
        &self,
        files: &[(u64, String)],
    ) -> Result<(Vec<CommitEntry>, Vec<ParseWarning>), DeltaLensError> {
        if files.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }

        let version_by_path: HashMap<&str, u64> =
            files.iter().map(|(v, p)| (p.as_str(), *v)).collect();
        let paths: Vec<String> = files.iter().map(|(_, p)| p.clone()).collect();

        // Batch read via backend (bounded Tokio async on cloud, Rayon on local disk).
        let raw_files = self.storage.read_many(&paths)?;

        // Parse JSON in parallel with Rayon (CPU-bound, zero nested Tokio calls!).
        let parsed: Result<Vec<(CommitEntry, Vec<ParseWarning>)>, DeltaLensError> = raw_files
            .par_iter()
            .map(|(path, bytes)| {
                let version = *version_by_path.get(path.as_str()).ok_or_else(|| {
                    DeltaLensError::Storage(format!("Unexpected path returned by storage: {path}"))
                })?;
                let res = CommitParser::parse_bytes_with_warnings(bytes, path)?;
                Ok((
                    CommitEntry {
                        version,
                        path: path.clone(),
                        actions: res.actions,
                    },
                    res.warnings,
                ))
            })
            .collect();

        let mut pairs = parsed?;
        pairs.sort_by_key(|(entry, _)| entry.version);

        let mut entries = Vec::with_capacity(pairs.len());
        let mut warnings = Vec::new();
        for (entry, w) in pairs {
            entries.push(entry);
            warnings.extend(w);
        }

        Ok((entries, warnings))
    }
}

fn parse_inventory_entries(entries: Vec<String>) -> (LogInventory, bool) {
    let mut commits = BTreeMap::new();
    let mut classic_ckpts: BTreeMap<u64, String> = BTreeMap::new();
    let mut v2_json_ckpts: BTreeMap<u64, String> = BTreeMap::new();
    let mut v2_parquet_ckpts: BTreeMap<u64, String> = BTreeMap::new();
    let mut multipart_parts: HashMap<(u64, u32), BTreeMap<u32, String>> = HashMap::new();
    let mut compactions = Vec::new();
    let mut has_last_checkpoint = false;

    for path_str in entries {
        let Some(name) = basename(&path_str) else {
            continue;
        };
        if name == "_last_checkpoint" {
            has_last_checkpoint = true;
            continue;
        }
        if let Some(v) = parse_commit_filename(name) {
            commits.insert(v, path_str);
            continue;
        }
        if let Some((start, end)) = parse_compacted_filename(name) {
            compactions.push(CompactedFile {
                start_version: start,
                end_version: end,
                path: path_str,
            });
            continue;
        }
        if let Some(ckpt) = parse_checkpoint_filename(name) {
            match ckpt {
                ParsedCheckpointFile::Classic(v) => {
                    classic_ckpts.insert(v, path_str);
                }
                ParsedCheckpointFile::MultiPart {
                    version,
                    part,
                    total,
                } => {
                    multipart_parts
                        .entry((version, total))
                        .or_default()
                        .insert(part, path_str);
                }
                ParsedCheckpointFile::V2Json(v) => {
                    v2_json_ckpts.insert(v, path_str);
                }
                ParsedCheckpointFile::V2Parquet(v) => {
                    v2_parquet_ckpts.insert(v, path_str);
                }
            }
        }
    }

    let mut checkpoints: BTreeMap<u64, CheckpointKind> = BTreeMap::new();
    for (v, p) in classic_ckpts {
        checkpoints.insert(v, CheckpointKind::ClassicParquet(p));
    }
    for ((version, total), parts_map) in multipart_parts {
        if total > 0
            && parts_map.len() == total as usize
            && (1..=total).all(|i| parts_map.contains_key(&i))
        {
            let parts = (1..=total)
                .filter_map(|i| parts_map.get(&i).cloned())
                .collect();
            checkpoints.insert(
                version,
                CheckpointKind::MultiPartParquet {
                    total_parts: total,
                    parts,
                },
            );
        }
    }
    // V2 checkpoints take precedence if both exist for the same version.
    for (v, p) in v2_parquet_ckpts {
        checkpoints.insert(v, CheckpointKind::V2Parquet(p));
    }
    for (v, p) in v2_json_ckpts {
        checkpoints.insert(v, CheckpointKind::V2Json(p));
    }

    compactions.sort_by_key(|c| (c.start_version, std::cmp::Reverse(c.end_version)));

    (
        LogInventory {
            commits,
            checkpoints,
            compactions,
            last_checkpoint_hint: None,
        },
        has_last_checkpoint,
    )
}

/// Return the highest version in `[start, end]` that does not have an individual commit file,
/// or `None` if every version in `[start, end]` is already covered by `commits`.
fn highest_uncovered_version(commits: &BTreeMap<u64, String>, start: u64, end: u64) -> Option<u64> {
    let mut candidate = end;
    for (&v, _) in commits.range(start..=end).rev() {
        if v < candidate {
            return Some(candidate);
        }
        if candidate == start {
            return None;
        }
        candidate = candidate.checked_sub(1)?;
    }
    Some(candidate)
}

fn sidecar_storage_path(log_subpath: &str, sidecar_path: &str) -> Result<String, DeltaLensError> {
    let relative = std::path::Path::new(sidecar_path);
    if sidecar_path.is_empty()
        || sidecar_path.contains('\\')
        || sidecar_path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(DeltaLensError::Checkpoint(format!(
            "Invalid V2 checkpoint sidecar path: {sidecar_path}"
        )));
    }

    Ok(format!("{log_subpath}/_sidecars/{sidecar_path}"))
}

enum ParsedCheckpointFile {
    Classic(u64),
    MultiPart { version: u64, part: u32, total: u32 },
    V2Json(u64),
    V2Parquet(u64),
}

fn basename(path: &str) -> Option<&str> {
    path.rsplit(['/', '\\']).next().filter(|s| !s.is_empty())
}

fn is_20_digits(s: &str) -> bool {
    s.len() == 20 && s.bytes().all(|b| b.is_ascii_digit())
}

/// Strict 20-digit zero-padded Delta commit filename: `00000000000000000142.json`.
pub fn parse_commit_filename(filename: &str) -> Option<u64> {
    let stem = filename.strip_suffix(".json")?;
    if !is_20_digits(stem) {
        return None;
    }
    stem.parse::<u64>().ok()
}

/// Delta log compaction filename: `<20-digits>.<20-digits>.compacted.json`.
pub fn parse_compacted_filename(filename: &str) -> Option<(u64, u64)> {
    let stem = filename.strip_suffix(".compacted.json")?;
    let (a, b) = stem.split_once('.')?;
    if !is_20_digits(a) || !is_20_digits(b) {
        return None;
    }
    let start = a.parse::<u64>().ok()?;
    let end = b.parse::<u64>().ok()?;
    if start <= end {
        Some((start, end))
    } else {
        None
    }
}

/// Parse classic, multi-part, and V2 JSON/Parquet checkpoint filenames.
fn parse_checkpoint_filename(filename: &str) -> Option<ParsedCheckpointFile> {
    let (ver_str, rest) = filename.split_once(".checkpoint.")?;
    if !is_20_digits(ver_str) {
        return None;
    }
    let version = ver_str.parse::<u64>().ok()?;

    if rest == "parquet" {
        return Some(ParsedCheckpointFile::Classic(version));
    }

    if let Some(stem) = rest.strip_suffix(".parquet") {
        // Multi-part: <10 digits>.<10 digits>
        if let Some((p_str, t_str)) = stem.split_once('.') {
            if p_str.len() == 10
                && t_str.len() == 10
                && p_str.bytes().all(|b| b.is_ascii_digit())
                && t_str.bytes().all(|b| b.is_ascii_digit())
            {
                let part = p_str.parse::<u32>().ok()?;
                let total = t_str.parse::<u32>().ok()?;
                if part >= 1 && part <= total {
                    return Some(ParsedCheckpointFile::MultiPart {
                        version,
                        part,
                        total,
                    });
                }
            }
            return None;
        }
        // V2 Parquet: <uuid>.parquet
        if !stem.is_empty() && !stem.contains('.') {
            return Some(ParsedCheckpointFile::V2Parquet(version));
        }
        return None;
    }

    if let Some(uuid) = rest.strip_suffix(".json") {
        if !uuid.is_empty() && !uuid.contains('.') {
            return Some(ParsedCheckpointFile::V2Json(version));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::storage::LocalStorage;
    use arrow_array::builder::{
        BooleanBuilder, Int32Builder, Int64Builder, StringBuilder, StructBuilder,
    };
    use arrow_array::RecordBatch;
    use arrow_schema::{DataType as ArrowDataType, Field, Fields, Schema};
    use parquet::arrow::ArrowWriter;
    use std::io::Write;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn make_commit(dir: &std::path::Path, version: u64, content: &str) {
        let filename = format!("{:020}.json", version);
        let mut f = std::fs::File::create(dir.join(filename)).unwrap();
        writeln!(f, "{}", content).unwrap();
    }

    fn create_reader(tmp: &TempDir) -> DeltaLogReader {
        let root = tmp.path().to_string_lossy().to_string();
        let storage = Box::new(LocalStorage::new(root.clone()));
        DeltaLogReader::new(storage, &root).unwrap()
    }

    fn write_parquet_add_file(path: &std::path::Path, file_path: &str, size: i64) {
        let add_fields = Fields::from(vec![
            Field::new("path", ArrowDataType::Utf8, false),
            Field::new("size", ArrowDataType::Int64, false),
            Field::new("modificationTime", ArrowDataType::Int64, false),
            Field::new("dataChange", ArrowDataType::Boolean, false),
        ]);
        let proto_fields = Fields::from(vec![
            Field::new("minReaderVersion", ArrowDataType::Int32, false),
            Field::new("minWriterVersion", ArrowDataType::Int32, false),
        ]);
        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "protocol",
                ArrowDataType::Struct(proto_fields.clone()),
                true,
            ),
            Field::new("add", ArrowDataType::Struct(add_fields.clone()), true),
        ]));

        let mut pb = StructBuilder::new(
            proto_fields,
            vec![Box::new(Int32Builder::new()), Box::new(Int32Builder::new())],
        );
        pb.field_builder::<Int32Builder>(0).unwrap().append_value(1);
        pb.field_builder::<Int32Builder>(1).unwrap().append_value(2);
        pb.append(true);

        let mut ab = StructBuilder::new(
            add_fields,
            vec![
                Box::new(StringBuilder::new()),
                Box::new(Int64Builder::new()),
                Box::new(Int64Builder::new()),
                Box::new(BooleanBuilder::new()),
            ],
        );
        ab.field_builder::<StringBuilder>(0)
            .unwrap()
            .append_value(file_path);
        ab.field_builder::<Int64Builder>(1)
            .unwrap()
            .append_value(size);
        ab.field_builder::<Int64Builder>(2)
            .unwrap()
            .append_value(1700000000000);
        ab.field_builder::<BooleanBuilder>(3)
            .unwrap()
            .append_value(false);
        ab.append(true);

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(pb.finish()), Arc::new(ab.finish())],
        )
        .unwrap();

        let file = std::fs::File::create(path).unwrap();
        let mut writer = ArrowWriter::try_new(file, schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
    }

    #[test]
    fn test_reject_non_delta_path() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        let storage = Box::new(LocalStorage::new(root.clone()));
        let result = DeltaLogReader::new(storage, &root);
        assert!(matches!(result, Err(DeltaLensError::NotADeltaTable(_))));
    }

    #[test]
    fn test_strict_20_digit_filenames() {
        assert_eq!(parse_commit_filename("00000000000000000042.json"), Some(42));
        assert_eq!(parse_commit_filename("42.json"), None);
        assert_eq!(parse_commit_filename("00000000000000000042.crc"), None);
        assert_eq!(
            parse_compacted_filename("00000000000000000001.00000000000000000005.compacted.json"),
            Some((1, 5))
        );
    }

    #[test]
    fn test_discovers_commits_in_order() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        std::fs::create_dir(&log_dir).unwrap();

        let action = r#"{"commitInfo":{"timestamp":1700000000000,"operation":"WRITE","operationParameters":{}}}"#;
        make_commit(&log_dir, 0, action);
        make_commit(&log_dir, 1, action);
        make_commit(&log_dir, 2, action);
        // Non-20-digit stray JSON file must be ignored
        std::fs::write(log_dir.join("999.json"), action).unwrap();

        let reader = create_reader(&tmp);
        let entries = reader.read_all().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].version, 0);
        assert_eq!(entries[2].version, 2);
    }

    #[test]
    fn test_detects_version_gap() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        std::fs::create_dir(&log_dir).unwrap();
        let action = r#"{"commitInfo":{"timestamp":1700000000000,"operation":"WRITE"}}"#;
        make_commit(&log_dir, 0, action);
        make_commit(&log_dir, 2, action);

        let reader = create_reader(&tmp);
        let err = reader.read_snapshot_log(Some(2)).unwrap_err();
        assert!(matches!(
            err,
            DeltaLensError::VersionGap {
                expected: 1,
                found: 2
            }
        ));
    }

    #[test]
    fn test_reads_classic_parquet_checkpoint_with_expired_commits() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        std::fs::create_dir(&log_dir).unwrap();

        // Commits 0..=9 are expired (absent), checkpoint exists at v10, commit 11 follows.
        write_parquet_add_file(
            &log_dir.join("00000000000000000010.checkpoint.parquet"),
            "part-from-ckpt.parquet",
            5000,
        );
        std::fs::write(
            log_dir.join("_last_checkpoint"),
            r#"{"version":10,"size":2}"#,
        )
        .unwrap();
        make_commit(
            &log_dir,
            11,
            r#"{"add":{"path":"part-v11.parquet","size":3000,"modificationTime":1700000002000,"dataChange":true,"partitionValues":{}}}"#,
        );

        let reader = create_reader(&tmp);
        assert_eq!(reader.current_version().unwrap(), 11);
        assert_eq!(reader.latest_checkpoint_version(None).unwrap(), Some(10));

        let snap = reader.read_snapshot_log(None).unwrap();
        assert_eq!(snap.checkpoint_version, Some(10));
        assert_eq!(snap.commits.len(), 1);
        assert_eq!(snap.commits[0].version, 11);
        assert!(snap
            .checkpoint_actions
            .iter()
            .any(|a| matches!(a, Action::Add(_))));
    }

    #[test]
    fn test_reads_multipart_checkpoint() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        std::fs::create_dir(&log_dir).unwrap();

        write_parquet_add_file(
            &log_dir.join("00000000000000000020.checkpoint.0000000001.0000000002.parquet"),
            "part-a.parquet",
            1000,
        );
        write_parquet_add_file(
            &log_dir.join("00000000000000000020.checkpoint.0000000002.0000000002.parquet"),
            "part-b.parquet",
            2000,
        );

        let reader = create_reader(&tmp);
        let snap = reader.read_snapshot_log(None).unwrap();
        assert_eq!(snap.checkpoint_version, Some(20));
        let add_count = snap
            .checkpoint_actions
            .iter()
            .filter(|a| matches!(a, Action::Add(_)))
            .count();
        assert_eq!(add_count, 2);
    }

    #[test]
    fn test_reads_v2_json_checkpoint_with_sidecar_and_compacted_json() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        let sidecars = log_dir.join("_sidecars");
        std::fs::create_dir_all(&sidecars).unwrap();

        write_parquet_add_file(&sidecars.join("sc-1.parquet"), "from-sidecar.parquet", 7777);

        let v2_ckpt = concat!(
            r#"{"checkpointMetadata":{"version":2}}"#,
            "\n",
            r#"{"protocol":{"minReaderVersion":3,"minWriterVersion":7,"readerFeatures":["v2Checkpoint"],"writerFeatures":["v2Checkpoint"]}}"#,
            "\n",
            r#"{"sidecar":{"path":"sc-1.parquet","sizeInBytes":100,"modificationTime":1000}}"#,
            "\n"
        );
        std::fs::write(
            log_dir.join("00000000000000000005.checkpoint.abcd-1234.json"),
            v2_ckpt,
        )
        .unwrap();

        // Compacted file covers versions 6..=8 (without needing individual 6, 7, 8 JSON files)
        std::fs::write(
            log_dir.join("00000000000000000006.00000000000000000008.compacted.json"),
            r#"{"add":{"path":"from-compacted.parquet","size":999,"modificationTime":2000,"dataChange":true,"partitionValues":{}}}"#,
        )
        .unwrap();

        let reader = create_reader(&tmp);
        assert_eq!(reader.current_version().unwrap(), 8);
        let snap = reader.read_snapshot_log(Some(8)).unwrap();
        assert_eq!(snap.checkpoint_version, Some(5));
        assert!(snap
            .checkpoint_actions
            .iter()
            .any(|a| matches!(a, Action::Add(add) if add.path == "from-sidecar.parquet")));
        assert_eq!(snap.commits.len(), 1);
        assert_eq!(snap.commits[0].version, 8);
    }

    #[test]
    fn test_reads_v2_parquet_checkpoint_with_sidecar() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        let sidecars = log_dir.join("_sidecars");
        std::fs::create_dir_all(&sidecars).unwrap();

        write_parquet_add_file(
            &sidecars.join("sc-v2p.parquet"),
            "from-v2-parquet-sidecar.parquet",
            8888,
        );

        let sidecar_fields = Fields::from(vec![
            Field::new("path", ArrowDataType::Utf8, false),
            Field::new("sizeInBytes", ArrowDataType::Int64, false),
            Field::new("modificationTime", ArrowDataType::Int64, false),
        ]);
        let schema = Arc::new(Schema::new(vec![Field::new(
            "sidecar",
            ArrowDataType::Struct(sidecar_fields.clone()),
            true,
        )]));
        let mut sb = StructBuilder::new(
            sidecar_fields,
            vec![
                Box::new(StringBuilder::new()),
                Box::new(Int64Builder::new()),
                Box::new(Int64Builder::new()),
            ],
        );
        sb.field_builder::<StringBuilder>(0)
            .unwrap()
            .append_value("sc-v2p.parquet");
        sb.field_builder::<Int64Builder>(1)
            .unwrap()
            .append_value(123);
        sb.field_builder::<Int64Builder>(2)
            .unwrap()
            .append_value(1000);
        sb.append(true);

        let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(sb.finish())]).unwrap();
        let f = std::fs::File::create(
            log_dir.join("00000000000000000007.checkpoint.550e8400-e29b-41d4.parquet"),
        )
        .unwrap();
        let mut writer = ArrowWriter::try_new(f, schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();

        let reader = create_reader(&tmp);
        let snap = reader.read_snapshot_log(Some(7)).unwrap();
        assert_eq!(snap.checkpoint_version, Some(7));
        assert!(snap.checkpoint_actions.iter().any(
            |a| matches!(a, Action::Add(add) if add.path == "from-v2-parquet-sidecar.parquet")
        ));
    }

    #[test]
    fn test_version_range_filter() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        std::fs::create_dir(&log_dir).unwrap();

        let action = r#"{"commitInfo":{"timestamp":1700000000000,"operation":"WRITE","operationParameters":{}}}"#;
        for v in 0..10 {
            make_commit(&log_dir, v, action);
        }

        let reader = create_reader(&tmp);
        let entries = reader.read_range(Some(3), Some(6)).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].version, 3);
        assert_eq!(entries[3].version, 6);
    }

    #[test]
    fn test_range_requires_every_requested_version() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        std::fs::create_dir(&log_dir).unwrap();
        let action = r#"{"commitInfo":{"timestamp":1700000000000,"operation":"WRITE"}}"#;
        make_commit(&log_dir, 1, action);
        make_commit(&log_dir, 2, action);

        let reader = create_reader(&tmp);
        assert!(matches!(
            reader.read_range(Some(0), Some(2)),
            Err(DeltaLensError::VersionGap {
                expected: 0,
                found: 1
            })
        ));
        assert!(matches!(
            reader.read_range(Some(1), Some(3)),
            Err(DeltaLensError::VersionNotFound(3))
        ));

        let later_history = TempDir::new().unwrap();
        let later_log = later_history.path().join("_delta_log");
        std::fs::create_dir(&later_log).unwrap();
        make_commit(&later_log, 5, action);
        make_commit(&later_log, 6, action);
        let later_reader = create_reader(&later_history);
        assert!(matches!(
            later_reader.read_range(None, Some(4)),
            Err(DeltaLensError::VersionNotFound(4))
        ));
    }

    #[test]
    fn test_range_reads_compaction_files_instead_of_omitting_them() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        std::fs::create_dir(&log_dir).unwrap();
        std::fs::write(
            log_dir.join("00000000000000000000.00000000000000000001.compacted.json"),
            r#"{"add":{"path":"from-compaction.parquet","size":123,"modificationTime":1700000000000,"dataChange":true,"partitionValues":{}}}"#,
        )
        .unwrap();
        make_commit(
            &log_dir,
            2,
            r#"{"add":{"path":"from-v2.parquet","size":456,"modificationTime":1700000001000,"dataChange":true,"partitionValues":{}}}"#,
        );

        let reader = create_reader(&tmp);
        let entries = reader.read_range(Some(0), Some(2)).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].version, 1);
        assert!(entries[0].actions.iter().any(
            |action| matches!(action, Action::Add(add) if add.path == "from-compaction.parquet")
        ));
        assert_eq!(entries[1].version, 2);

        let latest = reader.read_last(2).unwrap();
        assert_eq!(latest.len(), 2);
        assert_eq!(latest[0].version, 1);
        assert_eq!(latest[1].version, 2);
    }

    #[test]
    fn test_sidecar_path_traversal_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(
            log_dir.join("00000000000000000000.checkpoint.1234.json"),
            concat!(
                r#"{"checkpointMetadata":{"version":0}}"#,
                "\n",
                r#"{"sidecar":{"path":"../../outside.parquet","sizeInBytes":1,"modificationTime":0}}"#,
                "\n"
            ),
        )
        .unwrap();

        let reader = create_reader(&tmp);
        assert!(matches!(
            reader.read_snapshot_log(Some(0)),
            Err(DeltaLensError::Checkpoint(message))
                if message.contains("Invalid V2 checkpoint sidecar path")
        ));
    }

    #[test]
    fn test_read_last() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        std::fs::create_dir(&log_dir).unwrap();

        let action = r#"{"commitInfo":{"timestamp":1700000000000,"operation":"WRITE","operationParameters":{}}}"#;
        for v in 0..10 {
            make_commit(&log_dir, v, action);
        }

        let reader = create_reader(&tmp);
        let entries = reader.read_last(3).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].version, 7);
        assert_eq!(entries[2].version, 9);
    }

    #[test]
    fn test_read_last_keeps_commits_covered_by_a_compaction() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        std::fs::create_dir(&log_dir).unwrap();

        let action = r#"{"commitInfo":{"timestamp":1700000000000,"operation":"WRITE","operationParameters":{}}}"#;
        for v in 0..10 {
            make_commit(&log_dir, v, action);
        }
        std::fs::write(
            log_dir.join("00000000000000000000.00000000000000000010.compacted.json"),
            r#"{"add":{"path":"compacted.parquet","size":123,"modificationTime":1700000000000,"dataChange":true,"partitionValues":{}}}"#,
        )
        .unwrap();

        let reader = create_reader(&tmp);
        let entries = reader.read_last(3).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].version, 8);
        assert_eq!(entries[1].version, 9);
        assert_eq!(entries[2].version, 10);
    }

    #[test]
    fn test_read_last_deduplicates_overlapping_compaction_when_commits_exist() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        std::fs::create_dir(&log_dir).unwrap();

        let action = r#"{"commitInfo":{"timestamp":1700000000000,"operation":"WRITE","operationParameters":{}}}"#;
        for v in 0..=10 {
            make_commit(&log_dir, v, action);
        }
        // Coexisting overlapping compaction covering 0..=9 must NOT duplicate version 9!
        std::fs::write(
            log_dir.join("00000000000000000000.00000000000000000009.compacted.json"),
            r#"{"add":{"path":"compacted.parquet","size":123,"modificationTime":1700000000000,"dataChange":true,"partitionValues":{}}}"#,
        )
        .unwrap();

        let reader = create_reader(&tmp);
        let entries = reader.read_last(3).unwrap();
        let versions: Vec<u64> = entries.iter().map(|e| e.version).collect();
        assert_eq!(versions, vec![8, 9, 10]);
        assert!(entries[1].path.ends_with("00000000000000000009.json"));
    }

    #[test]
    fn test_contiguous_range_detects_gap_before_compaction() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        std::fs::create_dir(&log_dir).unwrap();

        make_commit(
            &log_dir,
            0,
            r#"{"commitInfo":{"timestamp":1700000000000,"operation":"WRITE"}}"#,
        );
        // Commit 1 is missing, compaction covers 2..=3.
        std::fs::write(
            log_dir.join("00000000000000000002.00000000000000000003.compacted.json"),
            r#"{"add":{"path":"c.parquet","size":123,"modificationTime":1700000000000,"dataChange":true,"partitionValues":{}}}"#,
        )
        .unwrap();

        let reader = create_reader(&tmp);
        assert!(matches!(
            reader.read_range(Some(0), Some(3)),
            Err(DeltaLensError::VersionGap {
                expected: 1,
                found: 2
            })
        ));
    }

    #[test]
    fn test_read_history_snapshot_log_seeds_earliest_checkpoint_when_commits_pruned() {
        use crate::core::analyzer::schema::SchemaTracker;

        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("_delta_log");
        std::fs::create_dir(&log_dir).unwrap();

        // Commits 0..=9 were pruned; V2 JSON checkpoint at v10 holds baseline schema `[id: long]`.
        let ckpt_v10 = concat!(
            r#"{"checkpointMetadata":{"version":2}}"#,
            "\n",
            r#"{"metaData":{"id":"t","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}}]}","partitionColumns":[],"configuration":{},"createdTime":1700000000000}}"#,
            "\n"
        );
        std::fs::write(
            log_dir.join("00000000000000000010.checkpoint.uuid-10.json"),
            ckpt_v10,
        )
        .unwrap();

        // Commit v11 adds column `email`.
        make_commit(
            &log_dir,
            11,
            concat!(
                r#"{"metaData":{"id":"t","format":{"provider":"parquet","options":{}},"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"long\",\"nullable\":false,\"metadata\":{}},{\"name\":\"email\",\"type\":\"string\",\"nullable\":true,\"metadata\":{}}]}","partitionColumns":[],"configuration":{},"createdTime":1700000000000}}"#,
                "\n",
                r#"{"commitInfo":{"timestamp":1700000005000,"operation":"WRITE"}}"#
            ),
        );

        let reader = create_reader(&tmp);
        let snap_log = reader.read_history_snapshot_log().unwrap();
        assert_eq!(snap_log.checkpoint_version, Some(10));
        assert_eq!(snap_log.target_version, 11);

        let history = SchemaTracker::history_from_snapshot(&snap_log);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].version, 11);
        assert_eq!(history[0].added_columns, vec!["email".to_string()]);
    }

    #[cfg(feature = "cloud")]
    #[test]
    fn test_last_checkpoint_accelerated_cloud_listing_and_fallbacks() {
        use crate::core::storage::ObjectStoreBackend;
        use bytes::Bytes;
        use object_store::memory::InMemory;
        use object_store::path::Path as ObjectStorePath;
        use object_store::ObjectStore;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingStorage {
            inner: ObjectStoreBackend,
            full_list_calls: Arc<AtomicUsize>,
            offset_list_calls: Arc<AtomicUsize>,
        }

        impl StorageBackend for CountingStorage {
            fn list(&self, dir_path: &str) -> Result<Vec<String>, DeltaLensError> {
                self.full_list_calls.fetch_add(1, Ordering::SeqCst);
                self.inner.list(dir_path)
            }

            fn list_from_offset(
                &self,
                dir_path: &str,
                start_after_name: &str,
            ) -> Result<Option<Vec<String>>, DeltaLensError> {
                self.offset_list_calls.fetch_add(1, Ordering::SeqCst);
                self.inner.list_from_offset(dir_path, start_after_name)
            }

            fn read(&self, file_path: &str) -> Result<Vec<u8>, DeltaLensError> {
                self.inner.read(file_path)
            }

            fn read_many(
                &self,
                file_paths: &[String],
            ) -> Result<Vec<(String, Vec<u8>)>, DeltaLensError> {
                self.inner.read_many(file_paths)
            }

            fn exists(&self, path: &str) -> Result<bool, DeltaLensError> {
                self.inner.exists(path)
            }

            fn is_dir(&self, path: &str) -> Result<bool, DeltaLensError> {
                self.inner.is_dir(path)
            }

            fn root_display(&self) -> &str {
                self.inner.root_display()
            }
        }

        let mem = Arc::new(InMemory::new());
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            for v in 0..=19u64 {
                let key = format!("warehouse/t/_delta_log/{:020}.json", v);
                let payload = format!(
                    r#"{{"add":{{"path":"f-{v}.parquet","size":100,"modificationTime":1700000000000,"dataChange":true,"partitionValues":{{}}}}}}"#
                );
                mem.put(&ObjectStorePath::from(key), Bytes::from(payload).into())
                    .await
                    .unwrap();
            }
            // V2 JSON checkpoint at v15 + _last_checkpoint pointing to v15
            let ckpt = concat!(
                r#"{"checkpointMetadata":{"version":2}}"#,
                "\n",
                r#"{"add":{"path":"ckpt-15.parquet","size":500,"modificationTime":1700000000000,"dataChange":false,"partitionValues":{}}}"#,
                "\n"
            );
            mem.put(
                &ObjectStorePath::from(
                    "warehouse/t/_delta_log/00000000000000000015.checkpoint.abcd.json",
                ),
                Bytes::from_static(ckpt.as_bytes()).into(),
            )
            .await
            .unwrap();
            // Nested sidecar file must be ignored by non-recursive list_from_offset
            mem.put(
                &ObjectStorePath::from("warehouse/t/_delta_log/_sidecars/ignored.parquet"),
                Bytes::from_static(b"unused").into(),
            )
            .await
            .unwrap();
            mem.put(
                &ObjectStorePath::from("warehouse/t/_delta_log/_last_checkpoint"),
                Bytes::from_static(br#"{"version":15,"size":2}"#).into(),
            )
            .await
            .unwrap();
        });

        let make_reader = |store: Arc<InMemory>| {
            let full_calls = Arc::new(AtomicUsize::new(0));
            let offset_calls = Arc::new(AtomicUsize::new(0));
            let backend = ObjectStoreBackend::from_object_store(
                store,
                "s3",
                "bucket",
                "warehouse/t",
                "s3://bucket/warehouse/t".to_string(),
            );
            let counting = CountingStorage {
                inner: backend,
                full_list_calls: Arc::clone(&full_calls),
                offset_list_calls: Arc::clone(&offset_calls),
            };
            let reader =
                DeltaLogReader::new(Box::new(counting), "s3://bucket/warehouse/t").unwrap();
            (reader, full_calls, offset_calls)
        };

        // 1. Latest snapshot uses list_from_offset and NEVER calls full list()!
        let (reader, full_calls, offset_calls) = make_reader(Arc::clone(&mem));
        let snap = reader.read_snapshot_log(None).unwrap();
        assert_eq!(snap.target_version, 19);
        assert_eq!(snap.checkpoint_version, Some(15));
        assert_eq!(snap.commits.len(), 4); // 16, 17, 18, 19
        assert_eq!(offset_calls.load(Ordering::SeqCst), 1);
        assert_eq!(full_calls.load(Ordering::SeqCst), 0);

        // 2. Historical version < hint (v10 < 15) falls back to full list()!
        let (reader_hist, full_calls_hist, offset_calls_hist) = make_reader(Arc::clone(&mem));
        let hist_snap = reader_hist.read_snapshot_log(Some(10)).unwrap();
        assert_eq!(hist_snap.target_version, 10);
        assert_eq!(hist_snap.checkpoint_version, None);
        assert_eq!(hist_snap.commits.len(), 11); // 0..=10
        assert_eq!(offset_calls_hist.load(Ordering::SeqCst), 0);
        assert_eq!(full_calls_hist.load(Ordering::SeqCst), 1);

        // 3. Stale _last_checkpoint (points to non-existent v99) falls back to full list()!
        rt.block_on(async {
            mem.put(
                &ObjectStorePath::from("warehouse/t/_delta_log/_last_checkpoint"),
                Bytes::from_static(br#"{"version":99,"size":2}"#).into(),
            )
            .await
            .unwrap();
        });
        let (reader_stale, full_calls_stale, offset_calls_stale) = make_reader(Arc::clone(&mem));
        let stale_snap = reader_stale.read_snapshot_log(None).unwrap();
        assert_eq!(stale_snap.target_version, 19);
        assert_eq!(stale_snap.checkpoint_version, Some(15));
        assert_eq!(offset_calls_stale.load(Ordering::SeqCst), 1);
        assert_eq!(full_calls_stale.load(Ordering::SeqCst), 1);
    }
}
