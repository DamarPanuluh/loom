use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum CodefileCmd {
    /// Register a codefile by path.
    Add {
        path: String,
        /// Register as observed (monitored upstream code): sync scans it and
        /// surface/contract staleness still ripples, but it carries no
        /// ownership, coverage, or build obligations. Re-adding an existing
        /// file with this flag marks it observed.
        #[arg(long)]
        observed: bool,
    },
    /// Issue a stable source anchor for a one-based source line without editing source or graph state.
    Anchor {
        path: String,
        #[arg(
            long,
            value_name = "LINE",
            conflicts_with = "at_symbol",
            required_unless_present = "at_symbol"
        )]
        at_line: Option<usize>,
        /// Name the declaration instead of its line. A line moves whenever
        /// anything above it changes; a declaration name does not.
        #[arg(long, value_name = "SYMBOL", conflicts_with = "at_line")]
        at_symbol: Option<String>,
    },
    /// Re-expand every glob ever registered and add any newly-appeared files
    /// (e.g. an endpoint an upstream just added). Run before `loom sync`.
    Rescan,
    /// Unregister a codefile (e.g. the file was deleted/renamed/split on
    /// disk). With live asserted edges pointing at it and no --successor,
    /// refuses and lists every blocker. With --successor, each such edge is
    /// retargeted in place (keeping its verdict history) before the node is
    /// removed — one recorded graph operation for a rename/split.
    Remove {
        key: String,
        /// Successor codefile that now carries this file's behavior. Must be
        /// registered (`loom codefile add <path>` first).
        #[arg(long)]
        successor: Option<String>,
    },
    /// Show a codefile.
    Show { key: String },
    /// List codefiles.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}
