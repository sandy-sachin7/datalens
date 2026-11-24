use crate::cli::output_opts::{CommandOutcome, OutputOpts};
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
) -> Result<CommandOutcome, DeltaLensError> {
    let since_ts = if let Some(since_str) = since {
        let nd = chrono::NaiveDate::parse_from_str(&since_str, "%Y-%m-%d").map_err(|e| {
            DeltaLensError::InvalidInput(format!(
                "Invalid --since date '{since_str}' (expected YYYY-MM-DD): {e}"
            ))
        })?;
        let ndt = nd.and_hms_opt(0, 0, 0).ok_or_else(|| {
            DeltaLensError::InvalidInput("Invalid --since date value".to_string())
        })?;
        Some(Utc.from_utc_datetime(&ndt).timestamp_millis())
    } else {
        None
    };

    let until_ts = if let Some(until_str) = until {
        let nd = chrono::NaiveDate::parse_from_str(&until_str, "%Y-%m-%d").map_err(|e| {
            DeltaLensError::InvalidInput(format!(
                "Invalid --until date '{until_str}' (expected YYYY-MM-DD): {e}"
            ))
        })?;
        // Include the entire final second of the day through 23:59:59.999 UTC.
        let ndt = nd.and_hms_milli_opt(23, 59, 59, 999).ok_or_else(|| {
            DeltaLensError::InvalidInput("Invalid --until date value".to_string())
        })?;
        Some(Utc.from_utc_datetime(&ndt).timestamp_millis())
    } else {
        None
    };

    let operations = op.map(|o| {
        o.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
    });

    let (storage, root) = storage_for(path_str)?;
    let reader = DeltaLogReader::new(storage, &root)?;
    let max_v = reader.current_version()?;
    let (entries, warnings) = reader.read_all_with_warnings()?;

    if opts.verbose {
        eprintln!(
            "[verbose] audit commits_read={} warnings={}",
            entries.len(),
            warnings.len()
        );
        for w in &warnings {
            eprintln!("[warning] {}:{}: {}", w.path, w.line, w.message);
        }
    }

    let num_actions: usize = entries.iter().map(|c| c.actions.len()).sum();
    let lineage_entries = LineageTracer::trace(&entries);

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
        render_json(&filtered_entries)?;
    } else {
        let renderer = TableRenderer::new(opts.plain, opts.no_header);
        renderer.render_audit(path_str, &filtered_entries)?;
    }

    Ok(CommandOutcome {
        num_commits: entries.len(),
        num_actions,
        table_version: max_v,
        exit_code: 0,
    })
}
