use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "deltalens")]
#[command(
    about = "Standalone CLI to inspect Delta Lake tables without Spark, JVM, or Python",
    long_about = None
)]
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

    /// Show debug info and parse warnings on stderr
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Table health report
    Inspect {
        /// Path to the Delta table (local path or cloud URI: s3://, gs://, az://, abfss://)
        path: String,

        /// Inspect at specific version (default: latest)
        #[arg(long)]
        version: Option<u64>,

        /// Exit with status code 2 if any table health issues are detected (CI gate)
        #[arg(long)]
        fail_on_issues: bool,
    },
    /// Version diff
    Diff {
        /// Path to the Delta table (local path or cloud URI)
        path: String,

        /// Starting version
        #[arg(long, required = true)]
        v1: u64,

        /// Ending version
        #[arg(long, required = true)]
        v2: u64,

        /// Show only schema changes
        #[arg(long, conflicts_with = "files_only")]
        schema_only: bool,

        /// Show only file-level changes
        #[arg(long, conflicts_with = "schema_only")]
        files_only: bool,

        /// Exit with status code 2 if breaking schema changes are detected (CI gate)
        #[arg(long)]
        fail_on_breaking: bool,
    },

    /// Operation lineage
    Lineage {
        /// Path to the Delta table (local path or cloud URI)
        path: String,

        /// Show last N matching commits (default: 20)
        #[arg(long)]
        last: Option<usize>,

        /// Show commits since date (YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,

        /// Filter by operation type(s) (comma-separated)
        #[arg(long)]
        op: Option<String>,

        /// Filter by user or writer name
        #[arg(long)]
        user: Option<String>,
    },

    /// Filtered audit trail
    Audit {
        /// Path to the Delta table (local path or cloud URI)
        path: String,

        /// Start date filter (YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,

        /// End date filter (YYYY-MM-DD, inclusive through 23:59:59.999 UTC)
        #[arg(long)]
        until: Option<String>,

        /// Comma-separated operation filter
        #[arg(long)]
        op: Option<String>,

        /// Filter by user or writer name
        #[arg(long)]
        user: Option<String>,
    },

    /// Schema evolution history
    Schema {
        /// Path to the Delta table (local path or cloud URI)
        path: String,

        /// Show full evolution history (default: current only)
        #[arg(long, conflicts_with = "at")]
        history: bool,

        /// Show schema at specific version
        #[arg(long, conflicts_with = "history")]
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
