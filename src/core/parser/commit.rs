use crate::core::model::action::Action;
use crate::error::DeltaLensError;
use std::path::Path;

/// A structured, non-fatal parse warning collected while parsing a log file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseWarning {
    pub path: String,
    pub line: usize,
    pub message: String,
}

/// Output of parsing a commit or NDJSON checkpoint file.
#[derive(Debug, Default)]
pub struct ParseResult {
    pub actions: Vec<Action>,
    pub warnings: Vec<ParseWarning>,
}

const KNOWN_ACTION_KEYS: &[&str] = &[
    "add",
    "remove",
    "metaData",
    "protocol",
    "commitInfo",
    "cdc",
    "txn",
    "domainMetadata",
    "sidecar",
    "checkpointMetadata",
];

/// Parses a single Delta commit or NDJSON checkpoint file.
/// Each line is a separate JSON action (newline-delimited JSON).
pub struct CommitParser;

impl CommitParser {
    /// Parse all actions from a file on disk.
    pub fn parse_file(path: &Path) -> Result<Vec<Action>, DeltaLensError> {
        let data = std::fs::read(path).map_err(DeltaLensError::Io)?;
        let path_str = path.to_string_lossy();
        Ok(Self::parse_bytes_with_warnings(&data, &path_str)?.actions)
    }

    /// Parse all actions from raw bytes (e.g., from a storage backend).
    pub fn parse_bytes(data: &[u8], path_str: &str) -> Result<Vec<Action>, DeltaLensError> {
        Ok(Self::parse_bytes_with_warnings(data, path_str)?.actions)
    }

    /// Parse all actions from raw bytes, returning structured parse warnings instead of
    /// writing to stderr from Rayon worker threads.
    pub fn parse_bytes_with_warnings(
        data: &[u8],
        path_str: &str,
    ) -> Result<ParseResult, DeltaLensError> {
        let mut actions = Vec::new();
        let mut warnings = Vec::new();

        for (idx, raw_line) in data.split(|&b| b == b'\n').enumerate() {
            let line = trim_ascii_whitespace(raw_line);
            if line.is_empty() {
                continue;
            }

            // Direct deserialization from &[u8] to Action enum (zero per-line String allocation).
            match serde_json::from_slice::<Action>(line) {
                Ok(action) => actions.push(action),
                Err(e) => {
                    // Distinguish valid unknown extension actions (silently ignored per Delta spec)
                    // from genuinely malformed lines or corrupted known actions.
                    if is_unknown_extension_action(line) {
                        continue;
                    }
                    warnings.push(ParseWarning {
                        path: path_str.to_string(),
                        line: idx + 1,
                        message: e.to_string(),
                    });
                }
            }
        }

        Ok(ParseResult { actions, warnings })
    }
}

fn trim_ascii_whitespace(mut bytes: &[u8]) -> &[u8] {
    while let Some(&first) = bytes.first() {
        if first.is_ascii_whitespace() {
            bytes = &bytes[1..];
        } else {
            break;
        }
    }
    while let Some(&last) = bytes.last() {
        if last.is_ascii_whitespace() {
            bytes = &bytes[..bytes.len() - 1];
        } else {
            break;
        }
    }
    bytes
}

fn is_unknown_extension_action(line: &[u8]) -> bool {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_slice::<serde_json::Value>(line)
    else {
        return false;
    };
    if map.len() != 1 {
        return false;
    }
    let Some((key, _)) = map.iter().next() else {
        return false;
    };
    !KNOWN_ACTION_KEYS.contains(&key.as_str())
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
    fn test_parse_null_tolerant_actions() {
        let payload = concat!(
            r#"{"add":{"path":"p1.parquet","size":100,"modificationTime":10,"dataChange":null,"partitionValues":null,"stats":null,"tags":{"k1":null,"k2":"v2"}}}"#,
            "\n",
            r#"{"metaData":{"id":"t1","format":null,"schemaString":"{\"type\":\"struct\",\"fields\":[{\"name\":\"id\",\"type\":\"integer\",\"nullable\":true,\"metadata\":null}]}","partitionColumns":null,"configuration":{"delta.appendOnly":null}}}"#,
            "\n",
            r#"{"commitInfo":{"timestamp":1700000000000,"operation":"WRITE","operationParameters":null,"notebook":{"notebookId":"1234567890"},"job":{"jobId":42,"jobName":"nightly_etl"}}}"#,
            "\n",
            r#"{"domainMetadata":{"domain":"delta.clustering","configuration":"{\"clusteringColumns\":[[\"id\"]]}","removed":false}}"#,
            "\n",
            r#"{"txn":{"appId":"stream-1","version":7}}"#,
            "\n"
        );

        let parsed =
            CommitParser::parse_bytes_with_warnings(payload.as_bytes(), "test.json").unwrap();
        assert!(
            parsed.warnings.is_empty(),
            "Expected zero warnings, got: {:?}",
            parsed.warnings
        );
        assert_eq!(parsed.actions.len(), 5);
        assert!(matches!(parsed.actions[0], Action::Add(_)));
        assert!(matches!(parsed.actions[1], Action::MetaData(_)));
        assert!(matches!(parsed.actions[2], Action::CommitInfo(_)));
        assert!(matches!(parsed.actions[3], Action::DomainMetadata(_)));
        assert!(matches!(parsed.actions[4], Action::Txn(_)));
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
    fn test_skips_unknown_actions_without_warning() {
        let payload = br#"{"unknownFutureAction":{"foo":"bar"}}"#;
        let parsed = CommitParser::parse_bytes_with_warnings(payload, "test.json").unwrap();
        assert_eq!(parsed.actions.len(), 0);
        assert!(parsed.warnings.is_empty());
    }

    #[test]
    fn test_skips_malformed_lines_with_structured_warning() {
        let payload = concat!(
            "this is not json {{{\n",
            r#"{"add":{"path":"p.parquet","size":100,"modificationTime":0,"dataChange":true,"partitionValues":{}}}"#,
            "\n"
        );

        let parsed =
            CommitParser::parse_bytes_with_warnings(payload.as_bytes(), "bad.json").unwrap();
        assert_eq!(parsed.actions.len(), 1);
        assert_eq!(parsed.warnings.len(), 1);
        assert_eq!(parsed.warnings[0].path, "bad.json");
        assert_eq!(parsed.warnings[0].line, 1);
    }
}
