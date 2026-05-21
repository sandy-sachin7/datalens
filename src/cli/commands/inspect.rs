use crate::core::analyzer::health::HealthAnalyzer;
use crate::core::reader::DeltaLogReader;
use crate::core::storage::storage_for;
use crate::error::DeltaLensError;
use crate::render::table::TableRenderer;

pub fn execute(
    path_str: &str,
    version: Option<u64>,
    json: bool,
    plain: bool,
    _no_header: bool,
) -> Result<(), DeltaLensError> {
    let (storage, root) = storage_for(path_str)?;
    let reader = DeltaLogReader::new(storage, &root)?;

    let entries = if let Some(v) = version {
        reader.read_range(None, Some(v))?
    } else {
        reader.read_all()?
    };

    let stats = HealthAnalyzer::analyze(&entries);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&stats).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        let renderer = TableRenderer::new(plain);
        renderer.render_inspect(path_str, &stats);
    }

    Ok(())
}
