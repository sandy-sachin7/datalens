use crate::cli::output_opts::{CommandOutcome, OutputOpts};
use crate::core::analyzer::health::HealthAnalyzer;
use crate::core::reader::DeltaLogReader;
use crate::core::storage::storage_for;
use crate::error::DeltaLensError;
use crate::render::json::render_json;
use crate::render::table::TableRenderer;

pub fn execute(
    path_str: &str,
    version: Option<u64>,
    fail_on_issues: bool,
    opts: OutputOpts,
) -> Result<CommandOutcome, DeltaLensError> {
    let (storage, root) = storage_for(path_str)?;
    let reader = DeltaLogReader::new(storage, &root)?;
    let snapshot = reader.read_snapshot_log(version)?;

    if opts.verbose {
        eprintln!(
            "[verbose] inspect target=v{} checkpoint={:?} files_read={} commits_replayed={} warnings={}",
            snapshot.target_version,
            snapshot.checkpoint_version,
            snapshot.files_read,
            snapshot.commits.len(),
            snapshot.warnings.len()
        );
        for w in &snapshot.warnings {
            eprintln!("[warning] {}:{}: {}", w.path, w.line, w.message);
        }
    }

    let num_actions: usize = snapshot.checkpoint_actions.len()
        + snapshot
            .commits
            .iter()
            .map(|c| c.actions.len())
            .sum::<usize>();
    let stats = HealthAnalyzer::analyze_snapshot(&snapshot);

    if opts.json {
        render_json(&stats)?;
    } else {
        let renderer = TableRenderer::new(opts.plain, opts.no_header);
        renderer.render_inspect(path_str, &stats)?;
    }

    let exit_code = if fail_on_issues && !stats.health_issues().is_empty() {
        2
    } else {
        0
    };

    Ok(CommandOutcome {
        num_commits: snapshot.files_read,
        num_actions,
        table_version: stats.current_version,
        exit_code,
    })
}
