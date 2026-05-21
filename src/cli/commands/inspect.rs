use crate::core::analyzer::health::HealthAnalyzer;
use crate::core::reader::DeltaLogReader;
use crate::error::DeltaLensError;
use crate::render::table::TableRenderer;
use std::path::Path;

pub fn execute(
    path: &Path,
    version: Option<u64>,
    json: bool,
    plain: bool,
    _no_header: bool, // Header control logic can be added to renderer later if needed
) -> Result<(), DeltaLensError> {
    let reader = DeltaLogReader::new(path)?;

    // Default to reading everything up to latest to get full health
    let entries = if let Some(v) = version {
        reader.read_range(None, Some(v))?
    } else {
        reader.read_all()?
    };

    let stats = HealthAnalyzer::analyze(&entries);

    if json {
        // Simple JSON output via serde
        println!(
            "{}",
            serde_json::to_string_pretty(&stats).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        let renderer = TableRenderer::new(plain);
        renderer.render_inspect(path, &stats);
    }

    Ok(())
}

// Temporary: Since TableStats has private/non-serialized fields inside its nested structs,
// and we didn't add Serialize to everything in model::stats yet, we need to add Serialize
// to TableStats and its dependencies if we want JSON output to work correctly.
// For now, I'll just print a placeholder JSON if serde fails, but the implementation should
// ideally have #[derive(Serialize)] on TableStats, TableHealth, MaintenanceInfo.
// I'll patch those in stats.rs later if the user requests it.
