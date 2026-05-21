use crate::core::model::action::Action;
use crate::core::parser::commit::CommitParser;
use crate::error::DeltaLensError;
use rayon::prelude::*;
use std::path::{Path, PathBuf};

/// One version entry: its number, file path, and parsed actions.
pub struct CommitEntry {
    pub version: u64,
    #[allow(dead_code)]
    pub path: PathBuf,
    pub actions: Vec<Action>,
}

/// Entry point for all Delta log reading operations.
pub struct DeltaLogReader {
    #[allow(dead_code)]
    pub table_path: PathBuf,
    pub log_path: PathBuf,
}

impl DeltaLogReader {
    /// Validate and open a Delta table at the given path.
    pub fn new(table_path: &Path) -> Result<Self, DeltaLensError> {
        let log_path = table_path.join("_delta_log");

        if !log_path.exists() {
            return Err(DeltaLensError::NotADeltaTable(
                table_path.to_string_lossy().to_string(),
            ));
        }

        Ok(Self {
            table_path: table_path.to_path_buf(),
            log_path,
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
            .map(|(version, path)| {
                let actions = CommitParser::parse_file(path)?;
                Ok(CommitEntry {
                    version: *version,
                    path: path.clone(),
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
            .map(|(version, path)| {
                let actions = CommitParser::parse_file(path)?;
                Ok(CommitEntry {
                    version: *version,
                    path: path.clone(),
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
    ) -> Result<Vec<(u64, PathBuf)>, DeltaLensError> {
        let mut files = Vec::new();

        for entry in std::fs::read_dir(&self.log_path)? {
            let entry = entry?;
            let path = entry.path();

            // Only JSON commit files — skip .parquet checkpoints and _last_checkpoint
            if let Some(ext) = path.extension() {
                if ext != "json" {
                    continue;
                }
            } else {
                continue;
            }

            if let Some(version) = Self::extract_version(&path) {
                let in_range = match (from, to) {
                    (Some(f), Some(t)) => version >= f && version <= t,
                    (Some(f), None) => version >= f,
                    (None, Some(t)) => version <= t,
                    (None, None) => true,
                };

                if in_range {
                    files.push((version, path));
                }
            }
        }

        files.sort_by_key(|(v, _)| *v);
        Ok(files)
    }

    /// Extract version number from filename: `0000000000000000142.json` → `142`
    fn extract_version(path: &Path) -> Option<u64> {
        path.file_stem()?.to_str()?.parse::<u64>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn make_commit(dir: &std::path::Path, version: u64, content: &str) {
        let filename = format!("{:020}.json", version);
        let mut f = std::fs::File::create(dir.join(filename)).unwrap();
        writeln!(f, "{}", content).unwrap();
    }

    #[test]
    fn test_reject_non_delta_path() {
        let tmp = TempDir::new().unwrap();
        let result = DeltaLogReader::new(tmp.path());
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

        let reader = DeltaLogReader::new(tmp.path()).unwrap();
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

        let reader = DeltaLogReader::new(tmp.path()).unwrap();
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

        let reader = DeltaLogReader::new(tmp.path()).unwrap();
        let entries = reader.read_last(3).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].version, 7);
        assert_eq!(entries[2].version, 9);
    }
}
