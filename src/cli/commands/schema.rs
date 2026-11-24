use crate::cli::output_opts::{CommandOutcome, OutputOpts};
use crate::core::analyzer::schema::SchemaTracker;
use crate::core::reader::DeltaLogReader;
use crate::core::storage::storage_for;
use crate::error::DeltaLensError;
use crate::render::json::render_json;
use crate::render::table::TableRenderer;

pub fn execute(
    path_str: &str,
    history: bool,
    at: Option<u64>,
    opts: OutputOpts,
) -> Result<CommandOutcome, DeltaLensError> {
    let (storage, root) = storage_for(path_str)?;
    let reader = DeltaLogReader::new(storage, &root)?;
    let max_v = reader.current_version()?;

    if let Some(version) = at {
        let snap_log = reader.read_snapshot_log(Some(version))?;
        if opts.verbose {
            eprintln!(
                "[verbose] schema --at v{} checkpoint={:?} files_read={} warnings={}",
                version,
                snap_log.checkpoint_version,
                snap_log.files_read,
                snap_log.warnings.len()
            );
            for w in &snap_log.warnings {
                eprintln!("[warning] {}:{}: {}", w.path, w.line, w.message);
            }
        }
        let num_actions: usize = snap_log.checkpoint_actions.len()
            + snap_log
                .commits
                .iter()
                .map(|c| c.actions.len())
                .sum::<usize>();
        let snapshot = SchemaTracker::from_snapshot(&snap_log).ok_or_else(|| {
            DeltaLensError::MissingMetadata(format!(
                "No schema metadata found at version {version}"
            ))
        })?;

        if opts.json {
            render_json(&snapshot)?;
        } else {
            let renderer = TableRenderer::new(opts.plain, opts.no_header);
            renderer.render_schema_snapshot(path_str, &snapshot)?;
        }

        Ok(CommandOutcome {
            num_commits: snap_log.files_read,
            num_actions,
            table_version: version,
            exit_code: 0,
        })
    } else if history {
        let (entries, warnings) = reader.read_all_with_warnings()?;
        if opts.verbose {
            eprintln!(
                "[verbose] schema --history commits_read={} warnings={}",
                entries.len(),
                warnings.len()
            );
            for w in &warnings {
                eprintln!("[warning] {}:{}: {}", w.path, w.line, w.message);
            }
        }
        let num_actions: usize = entries.iter().map(|c| c.actions.len()).sum();
        let evolution = SchemaTracker::history(&entries);

        if opts.json {
            render_json(&evolution)?;
        } else {
            let renderer = TableRenderer::new(opts.plain, opts.no_header);
            renderer.render_schema_history(path_str, &evolution)?;
        }

        Ok(CommandOutcome {
            num_commits: entries.len(),
            num_actions,
            table_version: max_v,
            exit_code: 0,
        })
    } else {
        let snap_log = reader.read_snapshot_log(None)?;
        if opts.verbose {
            eprintln!(
                "[verbose] schema current=v{} checkpoint={:?} files_read={} warnings={}",
                snap_log.target_version,
                snap_log.checkpoint_version,
                snap_log.files_read,
                snap_log.warnings.len()
            );
            for w in &snap_log.warnings {
                eprintln!("[warning] {}:{}: {}", w.path, w.line, w.message);
            }
        }
        let num_actions: usize = snap_log.checkpoint_actions.len()
            + snap_log
                .commits
                .iter()
                .map(|c| c.actions.len())
                .sum::<usize>();
        let snapshot = SchemaTracker::from_snapshot(&snap_log).ok_or_else(|| {
            DeltaLensError::MissingMetadata("No schema found in table metadata".to_string())
        })?;

        if opts.json {
            render_json(&snapshot)?;
        } else {
            let renderer = TableRenderer::new(opts.plain, opts.no_header);
            renderer.render_schema_snapshot(path_str, &snapshot)?;
        }

        Ok(CommandOutcome {
            num_commits: snap_log.files_read,
            num_actions,
            table_version: snap_log.target_version,
            exit_code: 0,
        })
    }
}
