/// Output/rendering options passed to every subcommand handler.
/// Bundles the three global flags so functions stay under clippy's
/// `too_many_arguments` limit (max 7).
#[derive(Clone, Copy, Debug)]
pub struct OutputOpts {
    pub json: bool,
    pub plain: bool,
    pub no_header: bool,
}
