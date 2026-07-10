//! Debt-feed argument shapes (`loom debt` / `loom debt promote`).
//!
//! Plane: surface — argument shape only. These enums declare flag names,
//! defaults, and help text; every handler lives in `crate::commands`. Nothing
//! here opens a store, resolves a graph, or contains logic beyond clap parsing.

use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum DebtCmd {
    /// Promote a live debt cluster into one asserted Finding for triage.
    ///
    /// The statistical cluster stays an unstored advisory signal (INV-3); this
    /// writes a separate asserted fact and never converts or suppresses the feed.
    Promote {
        /// Exact cluster id (`c…`) or a unique prefix from `loom debt`.
        cluster_id: String,
        /// Substantive evidence for the promotion (not a placeholder).
        #[arg(long)]
        evidence: String,
        /// Confidence in `[0.0, 1.0]` (default 0.7).
        #[arg(long, default_value_t = 0.7)]
        confidence: f64,
    },
}
