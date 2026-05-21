use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "deltalens")]
#[command(about = "Zero-dependency CLI for Delta Lake observability", long_about = None)]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Output as JSON (machine-readable, for piping)
    #[arg(long, global = true)]
    pub json: bool,

    /// No ANSI colors (for CI/CD logs)
    #[arg(long, global = true)]
    pub plain: bool,

    /// Skip table headers
    #[arg(long, global = true)]
    pub no_header: bool,

    /// Show debug info
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Table health report
    Inspect {
        /// Path to the Delta table (local path or s3:// URI)
        path: String,

        /// Inspect at specific version (default: latest)
        #[arg(long)]
        version: Option<u64>,
    },
    /// Version diff
    Diff {
        /// Path to the Delta table (local path or s3:// URI)
        path: String,

        /// Starting version
        #[arg(long, required = true)]
        v1: u64,

        /// Ending version
        #[arg(long, required = true)]
        v2: u64,

        /// Show only schema changes
        #[arg(long)]
        schema_only: bool,

        /// Show only file-level changes
        #[arg(long)]
        files_only: bool,
    },

    /// Operation lineage
    Lineage {
        /// Path to the Delta table (local path or s3:// URI)
        path: String,

        /// Show last N commits (default: 20)
        #[arg(long)]
        last: Option<usize>,

        /// Show commits since date (YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,

        /// Filter by operation type(s)
        #[arg(long)]
        op: Option<String>,

        /// Filter by user or writer name
        #[arg(long)]
        user: Option<String>,
    },

    /// Filtered audit trail
    Audit {
        /// Path to the Delta table (local path or s3:// URI)
        path: String,

        /// Start date filter (YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,

        /// End date filter (YYYY-MM-DD)
        #[arg(long)]
        until: Option<String>,

        /// Comma-separated operation filter
        #[arg(long)]
        op: Option<String>,

        /// Filter by user
        #[arg(long)]
        user: Option<String>,
    },

    /// Schema evolution history
    Schema {
        /// Path to the Delta table (local path or s3:// URI)
        path: String,

        /// Show full evolution history (default: current only)
        #[arg(long)]
        history: bool,

        /// Show schema at specific version
        #[arg(long)]
        at: Option<u64>,
    },

    /// Manage configuration and telemetry
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Show current configuration
    Show,
    /// Show config file path
    Path,
    /// Show collected performance metrics
    Metrics,
    /// Set a config value (e.g., 'set telemetry true')
    Set { key: String, value: String },
}
