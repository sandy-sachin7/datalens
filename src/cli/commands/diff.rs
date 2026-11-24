use crate::cli::output_opts::{CommandOutcome, OutputOpts};
use crate::core::analyzer::diff::DiffAnalyzer;
use crate::core::reader::DeltaLogReader;
use crate::core::replay::TableState;
use crate::core::storage::storage_for;
use crate::error::DeltaLensError;
use crate::render::json::render_json;
use crate::render::table::TableRenderer;

pub fn execute(
    path_str: &str,
    v1: u64,
    v2: u64,
    schema_only: bool,
    files_only: bool,
    fail_on_breaking: bool,
    opts: OutputOpts,
) -> Result<CommandOutcome, DeltaLensError> {
    if v1 > v2 {
        return Err(DeltaLensError::InvalidVersionRange(v1, v2));
    }

    let (storage, root) = storage_for(path_str)?;
    let reader = DeltaLogReader::new(storage, &root)?;

    // Reconstruct state at v1 and read v1+1..=v2 in a single discovery pass,
    // preserving `_last_checkpoint` offset listing on large cloud tables.
    let (snap_v1, entries_after, after_warnings) = reader.read_diff_logs(v1, v2)?;
    let state_v1 = TableState::from_snapshot(&snap_v1);

    if opts.verbose {
        eprintln!(
            "[verbose] diff v{}..v{} v1_checkpoint={:?} after_commits={} warnings={}",
            v1,
            v2,
            snap_v1.checkpoint_version,
            entries_after.len(),
            snap_v1.warnings.len() + after_warnings.len()
        );
        for w in snap_v1.warnings.iter().chain(after_warnings.iter()) {
            eprintln!("[warning] {}:{}: {}", w.path, w.line, w.message);
        }
    }

    let num_actions: usize = snap_v1.checkpoint_actions.len()
        + snap_v1
            .commits
            .iter()
            .map(|c| c.actions.len())
            .sum::<usize>()
        + entries_after.iter().map(|c| c.actions.len()).sum::<usize>();

    let diff = DiffAnalyzer::diff_from_state(v1, state_v1, &entries_after);

    if opts.json {
        render_json(&diff)?;
    } else {
        let renderer = TableRenderer::new(opts.plain, opts.no_header);
        renderer.render_diff(path_str, &diff, schema_only, files_only)?;
    }

    let exit_code = if fail_on_breaking && diff.has_breaking_schema_changes {
        2
    } else {
        0
    };

    Ok(CommandOutcome {
        num_commits: snap_v1.files_read + entries_after.len(),
        num_actions,
        table_version: v2,
        exit_code,
    })
}
