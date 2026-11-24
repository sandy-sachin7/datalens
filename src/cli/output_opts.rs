/// Output and execution options passed to every subcommand handler.
#[derive(Clone, Copy, Debug, Default)]
pub struct OutputOpts {
    pub json: bool,
    pub plain: bool,
    pub no_header: bool,
    pub verbose: bool,
}

/// Telemetry & exit-status summary returned by CLI command handlers.
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandOutcome {
    pub num_commits: usize,
    pub num_actions: usize,
    pub table_version: u64,
    /// Process exit code (`0` = success, `2` = CI gate failed).
    pub exit_code: i32,
}
