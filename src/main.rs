use clap::Parser;
use deltalens::cli::args::{Cli, Commands};
use std::process;

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Inspect { path, version } => {
            deltalens::cli::commands::inspect::execute(&path, version, cli.json, cli.plain, cli.no_header)
        }
        Commands::Diff {
            path,
            v1,
            v2,
            schema_only,
            files_only,
        } => deltalens::cli::commands::diff::execute(
            &path,
            v1,
            v2,
            schema_only,
            files_only,
            cli.json,
            cli.plain,
            cli.no_header,
        ),
        Commands::Lineage {
            path,
            last,
            since,
            op,
            user,
        } => deltalens::cli::commands::lineage::execute(
            &path,
            last,
            since,
            op,
            user,
            cli.json,
            cli.plain,
            cli.no_header,
        ),
        Commands::Audit {
            path,
            since,
            until,
            op,
            user,
        } => deltalens::cli::commands::audit::execute(
            &path,
            since,
            until,
            op,
            user,
            cli.json,
            cli.plain,
            cli.no_header,
        ),
        Commands::Schema { path, history, at } => deltalens::cli::commands::schema::execute(
            &path,
            history,
            at,
            cli.json,
            cli.plain,
            cli.no_header,
        ),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}
