use clap::Parser;
use deltalens::cli::args::{Cli, Commands};
use deltalens::cli::output_opts::OutputOpts;
use deltalens::core::config::DeltalensConfig;
use deltalens::core::metrics::MetricsRecorder;
use std::process;

fn main() {
    let cli = Cli::parse();

    let opts = OutputOpts {
        json: cli.json,
        plain: cli.plain,
        no_header: cli.no_header,
    };

    let app_version = cli_version();

    let result = match &cli.command {
        Commands::Inspect { path, version } => {
            let recorder = MetricsRecorder::new("inspect");
            let r = deltalens::cli::commands::inspect::execute(
                path,
                *version,
                cli.json,
                cli.plain,
                cli.no_header,
            );
            if DeltalensConfig::load().telemetry && r.is_ok() {
                recorder.finish(0, 0, 0, &app_version);
            }
            r
        }
        Commands::Diff {
            path,
            v1,
            v2,
            schema_only,
            files_only,
        } => {
            deltalens::cli::commands::diff::execute(path, *v1, *v2, *schema_only, *files_only, opts)
        }
        Commands::Lineage {
            path,
            last,
            since,
            op,
            user,
        } => deltalens::cli::commands::lineage::execute(
            path,
            *last,
            since.clone(),
            op.clone(),
            user.clone(),
            opts,
        ),
        Commands::Audit {
            path,
            since,
            until,
            op,
            user,
        } => deltalens::cli::commands::audit::execute(
            path,
            since.clone(),
            until.clone(),
            op.clone(),
            user.clone(),
            opts,
        ),
        Commands::Schema { path, history, at } => {
            let recorder = MetricsRecorder::new("schema");
            let r = deltalens::cli::commands::schema::execute(
                path,
                *history,
                *at,
                cli.json,
                cli.plain,
                cli.no_header,
            );
            if DeltalensConfig::load().telemetry && r.is_ok() {
                recorder.finish(0, 0, 0, &app_version);
            }
            r
        }
        Commands::Config { action } => match action {
            deltalens::cli::args::ConfigAction::Show => deltalens::cli::commands::config::show(),
            deltalens::cli::args::ConfigAction::Path => deltalens::cli::commands::config::path(),
            deltalens::cli::args::ConfigAction::Metrics => {
                deltalens::cli::commands::config::metrics()
            }
            deltalens::cli::args::ConfigAction::Set { key, value } => {
                deltalens::cli::commands::config::set(key, value)
            }
        },
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}

fn cli_version() -> String {
    format!("deltalens v{}", env!("CARGO_PKG_VERSION"))
}
