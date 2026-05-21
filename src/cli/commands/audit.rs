use crate::cli::output_opts::OutputOpts;
use crate::core::analyzer::lineage::{LineageFilter, LineageTracer};
use crate::core::reader::DeltaLogReader;
use crate::core::storage::storage_for;
use crate::error::DeltaLensError;
use crate::render::json::render_json;
use crate::render::table::TableRenderer;
use chrono::{TimeZone, Utc};

pub fn execute(
    path_str: &str,
    since: Option<String>,
    until: Option<String>,
    op: Option<String>,
    user: Option<String>,
    opts: OutputOpts,
) -> Result<(), DeltaLensError> {
    let (storage, root) = storage_for(path_str)?;
    let reader = DeltaLogReader::new(storage, &root)?;
    let entries = reader.read_all()?;

    let lineage_entries = LineageTracer::trace(&entries);

    let since_ts = if let Some(since_str) = since {
        let nd = chrono::NaiveDate::parse_from_str(&since_str, "%Y-%m-%d").map_err(|e| {
            DeltaLensError::Storage(format!("Invalid date format (expected YYYY-MM-DD): {}", e))
        })?;
        let ndt = nd
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| DeltaLensError::Storage("Invalid date value".to_string()))?;
        Some(Utc.from_utc_datetime(&ndt).timestamp_millis())
    } else {
        None
    };

    let until_ts = if let Some(until_str) = until {
        let nd = chrono::NaiveDate::parse_from_str(&until_str, "%Y-%m-%d").map_err(|e| {
            DeltaLensError::Storage(format!("Invalid date format (expected YYYY-MM-DD): {}", e))
        })?;
        let ndt = nd
            .and_hms_opt(23, 59, 59)
            .ok_or_else(|| DeltaLensError::Storage("Invalid date value".to_string()))?;
        Some(Utc.from_utc_datetime(&ndt).timestamp_millis())
    } else {
        None
    };

    let operations = op.map(|o| {
        o.split(',')
            .map(|s| s.trim().to_string())
            .collect::<Vec<_>>()
    });

    let filter = LineageFilter {
        since_timestamp: since_ts,
        until_timestamp: until_ts,
        operations,
        user,
    };

    let filtered_entries: Vec<_> = filter
        .apply(&lineage_entries)
        .into_iter()
        .cloned()
        .collect();

    if opts.json {
        render_json(&filtered_entries);
    } else {
        let renderer = TableRenderer::new(opts.plain);
        renderer.render_audit(path_str, &filtered_entries);
    }

    Ok(())
}
