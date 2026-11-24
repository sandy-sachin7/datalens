use clap::Parser;
use deltalens::cli::args::{Cli, Commands, ConfigAction};
use deltalens::cli::output_opts::{CommandOutcome, OutputOpts};
use deltalens::core::config::DeltalensConfig;
use deltalens::core::metrics::MetricsRecorder;
use std::process;

fn main() {
    let cli = Cli::parse();

    let opts = OutputOpts {
        json: cli.json,
        plain: cli.plain,
        no_header: cli.no_header,
        verbose: cli.verbose,
    };

    let app_version = cli_version();
    let telemetry_enabled = DeltalensConfig::load().telemetry;

    let (cmd_name, result): (Option<&str>, Result<CommandOutcome, _>) = match &cli.command {
        Commands::Inspect {
            path,
            version,
            fail_on_issues,
        } => {
            let recorder = MetricsRecorder::new("inspect");
            let r =
                deltalens::cli::commands::inspect::execute(path, *version, *fail_on_issues, opts);
            record_if_enabled(telemetry_enabled, &recorder, &r, &app_version);
            (Some("inspect"), r)
        }
        Commands::Diff {
            path,
            v1,
            v2,
            schema_only,
            files_only,
            fail_on_breaking,
        } => {
            let recorder = MetricsRecorder::new("diff");
            let r = deltalens::cli::commands::diff::execute(
                path,
                *v1,
                *v2,
                *schema_only,
                *files_only,
                *fail_on_breaking,
                opts,
            );
            record_if_enabled(telemetry_enabled, &recorder, &r, &app_version);
            (Some("diff"), r)
        }
        Commands::Lineage {
            path,
            last,
            since,
            op,
            user,
        } => {
            let recorder = MetricsRecorder::new("lineage");
            let r = deltalens::cli::commands::lineage::execute(
                path,
                *last,
                since.clone(),
                op.clone(),
                user.clone(),
                opts,
            );
            record_if_enabled(telemetry_enabled, &recorder, &r, &app_version);
            (Some("lineage"), r)
        }
        Commands::Audit {
            path,
            since,
            until,
            op,
            user,
        } => {
            let recorder = MetricsRecorder::new("audit");
            let r = deltalens::cli::commands::audit::execute(
                path,
                since.clone(),
                until.clone(),
                op.clone(),
                user.clone(),
                opts,
            );
            record_if_enabled(telemetry_enabled, &recorder, &r, &app_version);
            (Some("audit"), r)
        }
        Commands::Schema { path, history, at } => {
            let recorder = MetricsRecorder::new("schema");
            let r = deltalens::cli::commands::schema::execute(path, *history, *at, opts);
            record_if_enabled(telemetry_enabled, &recorder, &r, &app_version);
            (Some("schema"), r)
        }
        Commands::Config { action } => {
            let r = match action {
                ConfigAction::Show => deltalens::cli::commands::config::show(opts),
                ConfigAction::Path => deltalens::cli::commands::config::path(opts),
                ConfigAction::Metrics => deltalens::cli::commands::config::metrics(opts),
                ConfigAction::Set { key, value } => {
                    deltalens::cli::commands::config::set(key, value, opts)
                }
            };
            (None, r)
        }
    };

    match result {
        Ok(outcome) => {
            if outcome.exit_code != 0 {
                if opts.verbose {
                    eprintln!(
                        "[verbose] command {:?} exited with gate code {}",
                        cmd_name.unwrap_or("config"),
                        outcome.exit_code
                    );
                }
                process::exit(outcome.exit_code);
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    }
}

fn record_if_enabled(
    enabled: bool,
    recorder: &MetricsRecorder,
    result: &Result<CommandOutcome, deltalens::error::DeltaLensError>,
    app_version: &str,
) {
    if !enabled {
        return;
    }
    if let Ok(outcome) = result {
        recorder.finish(
            outcome.num_commits,
            outcome.num_actions,
            outcome.table_version,
            app_version,
        );
    }
}

fn cli_version() -> String {
    format!("deltalens v{}", env!("CARGO_PKG_VERSION"))
}
