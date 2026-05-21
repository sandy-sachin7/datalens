use crate::core::analyzer::diff::DiffAnalyzer;
use crate::core::reader::DeltaLogReader;
use crate::error::DeltaLensError;
use crate::render::json::render_json;
use crate::render::table::TableRenderer;
use std::path::Path;

pub fn execute(
    path: &Path,
    v1: u64,
    v2: u64,
    schema_only: bool,
    files_only: bool,
    json: bool,
    plain: bool,
    _no_header: bool,
) -> Result<(), DeltaLensError> {
    let reader = DeltaLogReader::new(path)?;
    let max_v = reader.current_version()?;
    if v1 > max_v {
        return Err(DeltaLensError::VersionNotFound(v1));
    }
    if v2 > max_v {
        return Err(DeltaLensError::VersionNotFound(v2));
    }
    if v1 > v2 {
        return Err(DeltaLensError::InvalidVersionRange(v1, v2));
    }

    let entries_before = reader.read_range(None, Some(v1))?;
    let entries_after = reader.read_range(Some(v1 + 1), Some(v2))?;

    let diff = DiffAnalyzer::diff(v1, &entries_before, &entries_after);

    if json {
        render_json(&diff);
    } else {
        let renderer = TableRenderer::new(plain);
        renderer.render_diff(path, &diff, schema_only, files_only);
    }

    Ok(())
}
