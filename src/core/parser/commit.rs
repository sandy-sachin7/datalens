use crate::core::model::action::Action;
use crate::error::DeltaLensError;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Parses a single Delta commit JSON file.
/// Each line is a separate JSON action (newline-delimited JSON).
pub struct CommitParser;

impl CommitParser {
    /// Parse all actions from a commit file.
    /// Unknown action types are silently skipped.
    /// Malformed lines emit a warning and are skipped (never hard-fail).
    pub fn parse_file(path: &Path) -> Result<Vec<Action>, DeltaLensError> {
        let file = File::open(path).map_err(DeltaLensError::Io)?;
        let reader = BufReader::new(file);
        let path_str = path.to_string_lossy();
        Self::parse_reader(reader, &path_str)
    }

    /// Parse all actions from raw bytes (e.g., from a storage backend).
    pub fn parse_bytes(data: &[u8], path_str: &str) -> Result<Vec<Action>, DeltaLensError> {
        let reader = BufReader::new(data);
        Self::parse_reader(reader, path_str)
    }

    /// Internal: parse actions from any BufRead source.
    fn parse_reader<R: BufRead>(reader: R, path_str: &str) -> Result<Vec<Action>, DeltaLensError> {
        let mut actions = Vec::new();

        for (line_num, line) in reader.lines().enumerate() {
            let line = line.map_err(DeltaLensError::Io)?;

            if line.trim().is_empty() {
                continue;
            }

            // Direct deserialization to Action enum for maximum P99 performance.
            // Eliminates intermediate serde_json::Value allocation and sub-tree cloning.
            match serde_json::from_str::<Action>(&line) {
                Ok(action) => actions.push(action),
                Err(e) => {
                    let err_msg = e.to_string();
                    // If it is just an unknown variant type, we skip it silently
                    // (since Delta tables allow custom/extension action tags).
                    if err_msg.contains("unknown variant") {
                        continue;
                    }
                    eprintln!(
                        "Warning: Skipping malformed/unparseable action at {} (line {}): {}",
                        path_str, line_num, e
                    );
                }
            }
        }

        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_add_action() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"{{"add":{{"path":"part-0001.parquet","size":1024,"modificationTime":1700000000000,"dataChange":true,"partitionValues":{{}}}}}}"#
        )
        .unwrap();

        let actions = CommitParser::parse_file(f.path()).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Add(_)));
    }

    #[test]
    fn test_parse_commit_info() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"{{"commitInfo":{{"timestamp":1700000000000,"operation":"WRITE","operationParameters":{{}}}}}}"#
        )
        .unwrap();

        let actions = CommitParser::parse_file(f.path()).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::CommitInfo(_)));
    }

    #[test]
    fn test_skips_empty_lines() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f).unwrap();
        writeln!(
            f,
            r#"{{"add":{{"path":"p.parquet","size":100,"modificationTime":0,"dataChange":true,"partitionValues":{{}}}}}}"#
        )
        .unwrap();

        let actions = CommitParser::parse_file(f.path()).unwrap();
        assert_eq!(actions.len(), 1);
    }

    #[test]
    fn test_skips_unknown_actions() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, r#"{{"unknownFutureAction":{{"foo":"bar"}}}}"#).unwrap();

        let actions = CommitParser::parse_file(f.path()).unwrap();
        assert_eq!(actions.len(), 0);
    }

    #[test]
    fn test_skips_malformed_lines_and_continues() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "this is not json {{{{").unwrap();
        writeln!(
            f,
            r#"{{"add":{{"path":"p.parquet","size":100,"modificationTime":0,"dataChange":true,"partitionValues":{{}}}}}}"#
        )
        .unwrap();

        // Should not error — should skip bad line, return the good action
        let actions = CommitParser::parse_file(f.path()).unwrap();
        assert_eq!(actions.len(), 1);
    }
}
