use thiserror::Error;

#[derive(Error, Debug)]
pub enum DeltaLensError {
    #[error("Not a Delta table: {0}\n       No _delta_log directory found.\n       Are you sure this is a Delta Lake table path?")]
    NotADeltaTable(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[allow(dead_code)]
    #[error("Parse error in {path} at line {line}: {source}")]
    ParseError {
        path: String,
        line: usize,
        source: serde_json::Error,
    },

    #[error("Version {0} not found in log")]
    VersionNotFound(u64),

    #[error("Invalid version range: v{0} > v{1}")]
    InvalidVersionRange(u64, u64),

    #[error("JSON deserialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Empty table: no commits found")]
    EmptyTable,

    #[error("Storage error: {0}")]
    Storage(String),
}
