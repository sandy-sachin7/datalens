use crate::core::model::action::{
    Action, AddAction, DomainMetadataAction, MetaDataAction, ProtocolAction, RemoveAction,
    TxnAction,
};
use crate::core::model::stats::{
    parse_delta_interval_ms, TombstoneInfo, DEFAULT_DELETED_FILE_RETENTION_MS,
};
use crate::core::reader::{CommitEntry, SnapshotLog};
use std::collections::{HashMap, HashSet};

/// Canonical Delta file key: `(percent_decoded_path, deletion_vector_unique_id)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileKey {
    pub path: String,
    pub dv_unique_id: Option<String>,
}

impl FileKey {
    pub fn from_add(add: &AddAction) -> Self {
        Self {
            path: canonicalize_path(&add.path),
            dv_unique_id: add.deletion_vector.as_ref().map(|dv| dv.unique_id()),
        }
    }

    pub fn from_remove(remove: &RemoveAction) -> Self {
        Self {
            path: canonicalize_path(&remove.path),
            dv_unique_id: remove.deletion_vector.as_ref().map(|dv| dv.unique_id()),
        }
    }
}

/// Percent-decode RFC 3986 escapes in relative Delta file paths and strip leading `./`.
pub fn canonicalize_path(path: &str) -> String {
    let decoded = percent_decode_utf8(path);
    decoded.trim_start_matches("./").to_string()
}

fn percent_decode_utf8(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Build a canonical partition key string from `partitionValues`.
pub fn partition_key(pv: &HashMap<String, Option<String>>) -> String {
    let mut kv: Vec<_> = pv.iter().collect();
    kv.sort_by_key(|(k, _)| k.as_str());
    kv.iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                k,
                v.as_deref().unwrap_or("__HIVE_DEFAULT_PARTITION__")
            )
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Delta metrics computed while applying a single commit onto `TableState`.
#[derive(Debug, Clone, Default)]
pub struct CommitDelta {
    pub logical_files_added: i64,
    pub logical_files_removed: i64,
    pub logical_bytes_added: i64,
    pub logical_bytes_removed: i64,
    pub physical_files_added: i64,
    pub physical_files_removed: i64,
    pub physical_bytes_added: i64,
    pub physical_bytes_removed: i64,
}

/// Protocol-compliant materialized state of a Delta Lake table at a point in time.
#[derive(Debug, Clone, Default)]
pub struct TableState {
    pub version: u64,
    pub checkpoint_version: Option<u64>,
    pub active_files: HashMap<FileKey, AddAction>,
    pub active_tombstones: HashMap<FileKey, RemoveAction>,
    pub seen_partitions: HashSet<String>,
    pub metadata: Option<MetaDataAction>,
    pub protocol: Option<ProtocolAction>,
    pub transactions: HashMap<String, TxnAction>,
    pub domain_metadata: HashMap<String, DomainMetadataAction>,
}

impl TableState {
    /// Materialize a `TableState` from a checkpoint-aware `SnapshotLog`.
    pub fn from_snapshot(snapshot: &SnapshotLog) -> Self {
        let mut state = Self::default();
        if let Some(ckpt_v) = snapshot.checkpoint_version {
            state.apply_checkpoint(ckpt_v, &snapshot.checkpoint_actions);
        }
        for commit in &snapshot.commits {
            state.apply_commit(commit);
        }
        state.version = snapshot.target_version;
        state
    }

    /// Materialize a `TableState` directly from a slice of ordered `CommitEntry`s.
    pub fn from_commits(entries: &[CommitEntry]) -> Self {
        let mut state = Self::default();
        for entry in entries {
            state.apply_commit(entry);
        }
        state
    }

    /// Apply checkpoint actions (active `Add`s, retained `Remove` tombstones, metadata, protocol, etc.).
    pub fn apply_checkpoint(&mut self, version: u64, actions: &[Action]) {
        self.version = version;
        self.checkpoint_version = Some(version);

        // Apply non-file metadata and removes first, then adds so live files always win.
        for action in actions {
            match action {
                Action::Remove(r) => {
                    let key = FileKey::from_remove(r);
                    if let Some(pv) = &r.partition_values {
                        if !pv.is_empty() {
                            self.seen_partitions.insert(partition_key(pv));
                        }
                    }
                    if r.deletion_timestamp.is_some() {
                        self.active_tombstones.insert(key, r.clone());
                    }
                }
                Action::MetaData(m) => self.metadata = Some(m.clone()),
                Action::Protocol(p) => self.protocol = Some(p.clone()),
                Action::Txn(t) => {
                    self.transactions.insert(t.app_id.clone(), t.clone());
                }
                Action::DomainMetadata(dm) => {
                    if dm.removed {
                        self.domain_metadata.remove(&dm.domain);
                    } else {
                        self.domain_metadata.insert(dm.domain.clone(), dm.clone());
                    }
                }
                _ => {}
            }
        }

        for action in actions {
            if let Action::Add(a) = action {
                let key = FileKey::from_add(a);
                if !a.partition_values.is_empty() {
                    self.seen_partitions
                        .insert(partition_key(&a.partition_values));
                }
                self.active_tombstones.remove(&key);
                self.active_files.insert(key, a.clone());
            }
        }
    }

    /// Apply one commit in strict Delta Lake semantics:
    /// Within the same commit, `Remove` actions are processed BEFORE `Add` actions
    /// so same-commit remove+add (Deletion Vectors, `RESTORE`, re-add) never deletes a live file.
    pub fn apply_commit(&mut self, entry: &CommitEntry) -> CommitDelta {
        self.version = entry.version;
        let mut delta = CommitDelta::default();

        // Phase 1: Metadata, Protocol, Txn, DomainMetadata, and Remove actions.
        for action in &entry.actions {
            match action {
                Action::Remove(r) => {
                    let key = FileKey::from_remove(r);
                    let prior = self.active_files.remove(&key);

                    let resolved_size = r.size.or_else(|| prior.as_ref().map(|a| a.size));
                    let resolved_pv = r
                        .partition_values
                        .clone()
                        .or_else(|| prior.as_ref().map(|a| a.partition_values.clone()));

                    if let Some(pv) = &resolved_pv {
                        if !pv.is_empty() {
                            self.seen_partitions.insert(partition_key(pv));
                        }
                    }

                    let bytes = resolved_size.unwrap_or(0);
                    delta.physical_files_removed = delta.physical_files_removed.saturating_add(1);
                    delta.physical_bytes_removed =
                        delta.physical_bytes_removed.saturating_add(bytes);
                    if r.data_change {
                        delta.logical_files_removed = delta.logical_files_removed.saturating_add(1);
                        delta.logical_bytes_removed =
                            delta.logical_bytes_removed.saturating_add(bytes);
                    }

                    let mut enriched = r.clone();
                    enriched.size = resolved_size;
                    enriched.partition_values = resolved_pv;
                    if enriched.deletion_timestamp.is_some() {
                        self.active_tombstones.insert(key, enriched);
                    }
                }
                Action::MetaData(m) => self.metadata = Some(m.clone()),
                Action::Protocol(p) => self.protocol = Some(p.clone()),
                Action::Txn(t) => {
                    self.transactions.insert(t.app_id.clone(), t.clone());
                }
                Action::DomainMetadata(dm) => {
                    if dm.removed {
                        self.domain_metadata.remove(&dm.domain);
                    } else {
                        self.domain_metadata.insert(dm.domain.clone(), dm.clone());
                    }
                }
                _ => {}
            }
        }

        // Phase 2: Add actions (always take precedence over same-commit Remove for the same key).
        for action in &entry.actions {
            if let Action::Add(a) = action {
                let key = FileKey::from_add(a);
                if !a.partition_values.is_empty() {
                    self.seen_partitions
                        .insert(partition_key(&a.partition_values));
                }
                self.active_tombstones.remove(&key);
                self.active_files.insert(key, a.clone());

                delta.physical_files_added = delta.physical_files_added.saturating_add(1);
                delta.physical_bytes_added = delta.physical_bytes_added.saturating_add(a.size);
                if a.data_change {
                    delta.logical_files_added = delta.logical_files_added.saturating_add(1);
                    delta.logical_bytes_added = delta.logical_bytes_added.saturating_add(a.size);
                }
            }
        }

        delta
    }

    /// Set of partition keys that currently have at least one live active file.
    pub fn active_partitions(&self) -> HashSet<String> {
        let mut parts = HashSet::new();
        for add in self.active_files.values() {
            if !add.partition_values.is_empty() {
                parts.insert(partition_key(&add.partition_values));
            }
        }
        parts
    }

    /// Number of historically seen partitions that currently have zero active files.
    pub fn empty_partition_count(&self) -> usize {
        let active = self.active_partitions();
        self.seen_partitions.difference(&active).count()
    }

    /// Validate the materialized protocol against DeltaLens's supported reader features.
    pub fn validate_protocol(&self) -> Result<(), crate::error::DeltaLensError> {
        if let Some(protocol) = &self.protocol {
            protocol.validate_reader_support()?;
        }
        Ok(())
    }

    /// Extract Liquid Clustering columns from `domainMetadata` (`delta.clustering`).
    pub fn clustering_columns(&self) -> Vec<String> {
        let Some(dm) = self.domain_metadata.get("delta.clustering") else {
            return Vec::new();
        };
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&dm.configuration) else {
            return Vec::new();
        };
        let Some(cols) = val.get("clusteringColumns").and_then(|v| v.as_array()) else {
            return Vec::new();
        };
        cols.iter()
            .filter_map(|col| {
                if let Some(parts) = col.as_array() {
                    let joined: Vec<&str> = parts.iter().filter_map(|p| p.as_str()).collect();
                    if joined.is_empty() {
                        None
                    } else {
                        Some(joined.join("."))
                    }
                } else {
                    col.as_str().map(String::from)
                }
            })
            .collect()
    }

    /// Parse `delta.deletedFileRetentionDuration` from table metadata, defaulting to 7 days.
    pub fn deleted_file_retention_ms(&self) -> i64 {
        self.metadata
            .as_ref()
            .and_then(|m| {
                m.configuration
                    .get("delta.deletedFileRetentionDuration")
                    .or_else(|| {
                        m.configuration
                            .iter()
                            .find(|(k, _)| {
                                k.eq_ignore_ascii_case("delta.deletedFileRetentionDuration")
                            })
                            .map(|(_, v)| v)
                    })
                    .and_then(|v| v.as_deref())
                    .and_then(parse_delta_interval_ms)
            })
            .unwrap_or(DEFAULT_DELETED_FILE_RETENTION_MS)
    }

    /// Summarize retained soft-deleted file tombstones using default retention without expiration check.
    pub fn tombstone_info(&self) -> TombstoneInfo {
        self.tombstone_info_with_retention(self.deleted_file_retention_ms(), None)
    }

    /// Summarize retained soft-deleted file tombstones against the given retention window
    /// and optional reference timestamp (epoch ms).
    pub fn tombstone_info_with_retention(
        &self,
        retention_duration_ms: i64,
        reference_ts: Option<i64>,
    ) -> TombstoneInfo {
        let active_tombstones = self.active_tombstones.len();
        let reclaimable_bytes: i64 = self
            .active_tombstones
            .values()
            .filter_map(|r| r.size)
            .fold(0i64, i64::saturating_add);
        let oldest_tombstone_timestamp = self
            .active_tombstones
            .values()
            .filter_map(|r| r.deletion_timestamp)
            .min();

        let mut expired_tombstones = 0usize;
        let mut expired_reclaimable_bytes = 0i64;
        if let Some(ref_ts) = reference_ts {
            for r in self.active_tombstones.values() {
                if let Some(del_ts) = r.deletion_timestamp {
                    if del_ts > 0 && ref_ts.saturating_sub(del_ts) > retention_duration_ms {
                        expired_tombstones += 1;
                        expired_reclaimable_bytes =
                            expired_reclaimable_bytes.saturating_add(r.size.unwrap_or(0));
                    }
                }
            }
        }

        TombstoneInfo {
            active_tombstones,
            reclaimable_bytes,
            oldest_tombstone_timestamp,
            expired_tombstones,
            expired_reclaimable_bytes,
            retention_duration_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::action::DeletionVectorDescriptor;

    fn make_add(path: &str, size: i64, dv: Option<DeletionVectorDescriptor>) -> Action {
        Action::Add(AddAction {
            path: path.to_string(),
            size,
            modification_time: 1000,
            data_change: true,
            partition_values: HashMap::new(),
            stats: None,
            tags: None,
            deletion_vector: dv,
        })
    }

    fn make_remove(path: &str, dv: Option<DeletionVectorDescriptor>) -> Action {
        Action::Remove(RemoveAction {
            path: path.to_string(),
            deletion_timestamp: Some(2000),
            data_change: true,
            extended_file_metadata: None,
            size: None,
            partition_values: None,
            tags: None,
            deletion_vector: dv,
        })
    }

    #[test]
    fn test_same_commit_add_before_remove_preserves_live_file() {
        let mut state = TableState::default();
        // Even if Add appears BEFORE Remove in the JSON commit (e.g., RESTORE or DV update),
        // Remove is applied before Add so the live file is preserved.
        let commit = CommitEntry {
            version: 0,
            path: "00000000000000000000.json".to_string(),
            actions: vec![
                make_add("part-0.parquet", 1024, None),
                make_remove("part-0.parquet", None),
            ],
        };
        state.apply_commit(&commit);
        assert_eq!(state.active_files.len(), 1);
        assert_eq!(state.active_tombstones.len(), 0);
    }

    #[test]
    fn test_deletion_vector_keying_and_update() {
        let mut state = TableState::default();
        let c0 = CommitEntry {
            version: 0,
            path: "0.json".to_string(),
            actions: vec![make_add("part-0.parquet", 4096, None)],
        };
        state.apply_commit(&c0);
        assert_eq!(state.active_files.len(), 1);

        let dv1 = DeletionVectorDescriptor {
            storage_type: "u".to_string(),
            path_or_inline_dv: "vBn[lx{q8@P<9BNH/isA".to_string(),
            offset: Some(1),
            size_in_bytes: 36,
            cardinality: 5,
        };
        // Commit 1 adds DV1 (Add before Remove in JSON order!)
        let c1 = CommitEntry {
            version: 1,
            path: "1.json".to_string(),
            actions: vec![
                make_add("part-0.parquet", 4096, Some(dv1.clone())),
                make_remove("part-0.parquet", None),
            ],
        };
        let delta = state.apply_commit(&c1);
        assert_eq!(state.active_files.len(), 1);
        let active = state.active_files.values().next().unwrap();
        assert_eq!(active.deletion_vector, Some(dv1));
        // Remove had size: None, enriched from prior Add (4096)
        assert_eq!(delta.logical_bytes_removed, 4096);
        assert_eq!(state.tombstone_info().active_tombstones, 1);
        assert_eq!(state.tombstone_info().reclaimable_bytes, 4096);
    }

    #[test]
    fn test_percent_encoded_path_canonicalization() {
        let mut state = TableState::default();
        let c0 = CommitEntry {
            version: 0,
            path: "0.json".to_string(),
            actions: vec![make_add("region=us%20east/part%2D0.parquet", 2048, None)],
        };
        let c1 = CommitEntry {
            version: 1,
            path: "1.json".to_string(),
            actions: vec![make_remove("region=us east/part-0.parquet", None)],
        };
        state.apply_commit(&c0);
        assert_eq!(state.active_files.len(), 1);
        let delta = state.apply_commit(&c1);
        assert_eq!(state.active_files.len(), 0);
        assert_eq!(delta.logical_bytes_removed, 2048);
    }

    #[test]
    fn test_liquid_clustering_domain_metadata() {
        let mut state = TableState::default();
        let c0 = CommitEntry {
            version: 0,
            path: "0.json".to_string(),
            actions: vec![Action::DomainMetadata(DomainMetadataAction {
                domain: "delta.clustering".to_string(),
                configuration: r#"{"clusteringColumns":[["user_id"],["event","ts"]]}"#.to_string(),
                removed: false,
            })],
        };
        state.apply_commit(&c0);
        assert_eq!(state.clustering_columns(), vec!["user_id", "event.ts"]);
    }

    #[test]
    fn test_saturating_arithmetic_on_huge_file_sizes() {
        let huge = i64::MAX / 2;
        let mut state = TableState::default();
        let c0 = CommitEntry {
            version: 0,
            path: "0.json".to_string(),
            actions: vec![
                make_add("a.parquet", huge, None),
                make_add("b.parquet", huge, None),
                make_add("c.parquet", huge, None),
            ],
        };
        let add_delta = state.apply_commit(&c0);
        assert_eq!(add_delta.logical_bytes_added, i64::MAX);
        assert_eq!(add_delta.physical_bytes_added, i64::MAX);

        let c1 = CommitEntry {
            version: 1,
            path: "1.json".to_string(),
            actions: vec![
                make_remove("a.parquet", None),
                make_remove("b.parquet", None),
                make_remove("c.parquet", None),
            ],
        };
        let rem_delta = state.apply_commit(&c1);
        assert_eq!(rem_delta.logical_bytes_removed, i64::MAX);
        assert_eq!(rem_delta.physical_bytes_removed, i64::MAX);

        let tombs = state.tombstone_info();
        assert_eq!(tombs.active_tombstones, 3);
        assert_eq!(tombs.reclaimable_bytes, i64::MAX);
    }
}
