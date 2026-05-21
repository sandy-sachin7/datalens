use crate::core::analyzer::schema::SchemaTracker;
use crate::core::reader::DeltaLogReader;
use crate::error::DeltaLensError;
use crate::render::json::render_json;
use crate::render::table::TableRenderer;
use std::path::Path;

pub fn execute(
    path: &Path,
    history: bool,
    at: Option<u64>,
    json: bool,
    plain: bool,
    _no_header: bool,
) -> Result<(), DeltaLensError> {
    let reader = DeltaLogReader::new(path)?;
    let max_v = reader.current_version()?;

    if let Some(version) = at {
        if version > max_v {
            return Err(DeltaLensError::VersionNotFound(version));
        }
        let entries = reader.read_range(None, Some(version))?;
        let snapshot = SchemaTracker::at_version(&entries, version)
            .ok_or_else(|| DeltaLensError::Storage(format!("No schema found at version {}", version)))?;

        if json {
            render_json(&snapshot);
        } else {
            let renderer = TableRenderer::new(plain);
            renderer.render_schema_snapshot(path, &snapshot);
        }
    } else if history {
        let entries = reader.read_all()?;
        let evolution = SchemaTracker::history(&entries);

        if json {
            render_json(&evolution);
        } else {
            let renderer = TableRenderer::new(plain);
            renderer.render_schema_history(path, &evolution);
        }
    } else {
        let entries = reader.read_all()?;
        let snapshot = SchemaTracker::current_schema(&entries)
            .ok_or_else(|| DeltaLensError::Storage("No schema found in table metadata".to_string()))?;

        if json {
            render_json(&snapshot);
        } else {
            let renderer = TableRenderer::new(plain);
            renderer.render_schema_snapshot(path, &snapshot);
        }
    }

    Ok(())
}
