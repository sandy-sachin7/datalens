use crate::core::model::action::Action;
use crate::core::parser::commit::CommitParser;
use crate::core::storage::StorageBackend;
use crate::error::DeltaLensError;
use rayon::prelude::*;

/// One version entry: its number, file path, and parsed actions.
pub struct CommitEntry {
    pub version: u64,
    #[allow(dead_code)]
    pub path: String,
    pub actions: Vec<Action>,
}

/// Entry point for all Delta log reading operations.
pub struct DeltaLogReader {
    #[allow(dead_code)]
    pub table_root: String,
    pub log_subpath: String,
    pub storage: Box<dyn StorageBackend>,
}

impl DeltaLogReader {
    /// Create a reader for a Delta table using the given storage backend.
    /// `table_root` is the root URI/path of the table (e.g. "/data/table" or "s3://bucket/table").
    pub fn new(storage: Box<dyn StorageBackend>, table_root: &str) -> Result<Self, DeltaLensError> {
        let log_subpath = "_delta_log".to_string();

        // Validate that the log directory exists
        if !storage.exists(&log_subpath)? {
            return Err(DeltaLensError::NotADeltaTable(table_root.to_string()));
        }

        Ok(Self {
            table_root: table_root.to_string(),
            log_subpath,
            storage,
        })
    }

    /// Read all commit files in version range [from, to] (inclusive).
    /// Files are parsed IN PARALLEL via rayon — the core performance lever.
    pub fn read_range(
        &self,
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<Vec<CommitEntry>, DeltaLensError> {
        let log_files = self.discover_log_files(from, to)?;

        if log_files.is_empty() {
            return Ok(vec![]);
        }

        // Each JSON file is independently parseable — embarrassingly parallel.
        let mut entries: Vec<CommitEntry> = log_files
            .par_iter()
            .map(|(version, file_path)| {
                let data = self.storage.read(file_path)?;
                let actions = CommitParser::parse_bytes(&data, file_path)?;
                Ok(CommitEntry {
                    version: *version,
                    path: file_path.clone(),
                    actions,
                })
            })
            .collect::<Result<Vec<_>, DeltaLensError>>()?;

        // Sort by version ascending after parallel collection
        entries.sort_by_key(|e| e.version);
        Ok(entries)
    }

    /// Read all commits from the beginning of the log.
    pub fn read_all(&self) -> Result<Vec<CommitEntry>, DeltaLensError> {
        self.read_range(None, None)
    }

    /// Read only the latest N commits. Used for `lineage --last N`.
    pub fn read_last(&self, n: usize) -> Result<Vec<CommitEntry>, DeltaLensError> {
        let all_files = self.discover_log_files(None, None)?;
        let last_n: Vec<_> = all_files.into_iter().rev().take(n).collect();

        let mut entries: Vec<CommitEntry> = last_n
            .par_iter()
            .map(|(version, file_path)| {
                let data = self.storage.read(file_path)?;
                let actions = CommitParser::parse_bytes(&data, file_path)?;
                Ok(CommitEntry {
                    version: *version,
                    path: file_path.clone(),
                    actions,
                })
            })
            .collect::<Result<Vec<_>, DeltaLensError>>()?;

        entries.sort_by_key(|e| e.version);
        Ok(entries)
    }

    /// Current (maximum) version in the log.
    pub fn current_version(&self) -> Result<u64, DeltaLensError> {
        let files = self.discover_log_files(None, None)?;
        files
            .last()
            .map(|(v, _)| *v)
            .ok_or(DeltaLensError::EmptyTable)
    }

    /// Discover all JSON commit files and return (version, path) sorted ascending.
    fn discover_log_files(
        &self,
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<Vec<(u64, String)>, DeltaLensError> {
        let entries = self.storage.list(&self.log_subpath)?;

        let mut files = Vec::new();
        for path_str in entries {
            // Only JSON commit files — skip .parquet checkpoints and _last_checkpoint
            if !path_str.ends_with(".json") {
                continue;
            }

            if let Some(version) = Self::extract_version(&path_str) {
                let in_range = match (from, to) {
                    (Some(f), Some(t)) => version >= f && version <= t,
                    (Some(f), None) => version >= f,
                    (None, Some(t)) => version <= t,
                    (None, None) => true,
                };

                if in_range {
                    files.push((version, path_str));
                }
            }
        }

        files.sort_by_key(|(v, _)| *v);
        Ok(files)
    }

    /// Extract version number from filename: `0000000000000000142.json` → `142`
    fn extract_version(path_str: &str) -> Option<u64> {
        // Handle paths like "/abs/path/00042.json" or "s3://bucket/.../00042.json"
        let filename = std::path::Path::new(path_str).file_stem()?.to_str()?;
        filename.parse::<u64>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::storage::LocalStorage;
    use std::io::Write;
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

    #[test]
    fn test_reject_non_delta_path() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        let storage = Box::new(LocalStorage::new(root.clone()));
        let result = DeltaLogReader::new(storage, &root);
        assert!(matches!(result, Err(DeltaLensError::NotADeltaTable(_))));
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

        let reader = create_reader(&tmp);
        let entries = reader.read_all().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].version, 0);
        assert_eq!(entries[2].version, 2);
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
}
