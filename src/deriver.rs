//! Deriver seam — sync orchestrates registered derivers to produce derived facts.
//!
//! Plane: engine contract. The engine owns change detection and ripple (the
//! staleness plane); a deriver owns turning an artifact's content into derived
//! facets and findings, and querying its own artifacts. sync loops the
//! registered derivers, then ripples the [`ArtifactChange`]s they report — so
//! unplugging a deriver leaves sync compiling and rippling correctly, it simply
//! yields fewer derived facts. The concrete derivers (tree-sitter extraction,
//! external scan adapters) live in the code seed (`crate::seed`), never in the
//! engine — the contract here names nothing file-specific.

/// Which named regions of an artifact changed, when the deriver can tell.
/// Region keys are opaque to the engine (the code seed uses symbol names);
/// the ripple logic only ever asks "does this claim's locator resolve to a
/// known region, and did that region change?".
#[derive(Debug, Clone, Default)]
pub struct RegionDiff {
    /// Region keys whose fingerprint differs between the prior and the new
    /// extraction — including keys present in only one of the two (added or
    /// removed regions count as changed).
    pub changed: std::collections::BTreeSet<String>,
    /// Every region key known on either side of the diff. A locator that
    /// resolves to none of these gets file-scoped (conservative) staling.
    pub known: std::collections::BTreeSet<String>,
}

/// One artifact change a deriver detected, for the engine to ripple. The engine
/// never inspects `content` for meaning — it hands it to the ripple logic that
/// checks whether asserted seam locators still resolve.
#[derive(Debug, Clone)]
pub struct ArtifactChange {
    /// The changed artifact node's id.
    pub artifact_id: String,
    /// A human-readable cause stamped onto every re-opened dependent claim.
    pub cause: String,
    /// The new content, or `None` when the artifact disappeared.
    pub content: Option<String>,
    /// Region-scoped diff of the change, or `None` when the deriver cannot
    /// compare (artifact gone, no prior fingerprints, unsupported format).
    /// `None` means dependents ripple file-scoped, exactly as before.
    pub regions: Option<RegionDiff>,
}

/// A deriver recomputes one slice of the derived plane from the artifacts on
/// disk and reports which artifacts changed so the engine can ripple. It knows
/// which artifacts it owns (by its artifact class) and queries them itself.
pub trait Deriver {
    /// Stable registry key.
    fn name(&self) -> &str;
    /// Whether sync auto-runs this deriver. Cheap structural derivation runs on
    /// every sync; expensive external adapters (linters) are on-demand.
    fn runs_on_sync(&self) -> bool;
    /// Recompute this deriver's derived facts (facets + findings), updating
    /// `report`, and return the artifact changes the engine must ripple.
    fn derive(
        &self,
        store: &crate::store::Store,
        root: &std::path::Path,
        report: &mut crate::sync::SyncReport,
    ) -> crate::Result<Vec<ArtifactChange>>;
}
