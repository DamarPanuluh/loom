//! Coverage truth — the read-only definition of code ownership gaps.
//!
//! Plane: computed-on-read over registered CodeFiles and asserted grounding.
//! This module owns the predicate shared by diagnostics, maturity, and work
//! routing so those projections cannot disagree about whether a file is owned.

use crate::extract::Role;
use crate::model::{EdgeKind, GroundingRole, Node};
use crate::store::Store;
use crate::Result;

/// Whether a CodeFile is registered as observed: monitored upstream code that
/// stays in the sync/surface/contract plane but carries no ownership, coverage,
/// or build obligations. The per-file counterpart of the graph-level observed
/// mode. Asserted at registration (`codefile add --observed`), never touched by
/// sync — derivers write facets, not the body.
pub(crate) fn codefile_observed(n: &Node) -> bool {
    n.body
        .get("observed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Registered CodeFiles with no owning `implements` edge, not matched by a
/// coverage-exclusion glob (`loom ignore`), and not registered as observed.
/// This is the single definition of the coverage gap: the diagnostic, the
/// `realized` maturity gate, and the `coverage` work queue all read it, so they
/// can never disagree. Sorted by name for a stable next-item.
pub(crate) fn unowned_codefiles(store: &Store) -> Result<Vec<Node>> {
    let ignore = crate::fsglob::matcher(store.ignore_globs()?)?;
    let mut unowned = Vec::new();
    for cf in store.codefiles()? {
        if ignore.is_match(&cf.name) {
            continue; // deliberately outside the tracked surface
        }
        if codefile_observed(&cf) {
            continue; // monitored upstream — no ownership obligation
        }
        // A TEST file is never realized by a behavior — it verifies one, and
        // demanding a realizing owner for it would mean `tests/` could only be
        // registered by permanently reddening coverage. That is exactly why
        // 22.8k lines of this repo's evidence backbone stayed outside the graph
        // while coverage reported 67/67 owned.
        if Role::detect(&cf.name) == Role::Test {
            let mut verified = false;
            for e in store.edges_with(Some(EdgeKind::Implements), None, Some(&cf.id))? {
                if !store.edge_superseded(&e.id)?
                    && store.grounding_role(&e.id)? == GroundingRole::Verifies
                {
                    verified = true;
                    break;
                }
            }
            if !verified {
                unowned.push(cf);
            }
            continue;
        }
        if store.realizing_implementers(&cf.id)?.is_empty() {
            unowned.push(cf);
        }
    }
    unowned.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(unowned)
}

/// Names of all CodeFiles currently in the coverage gap.
pub fn unowned_names(store: &Store) -> Result<Vec<String>> {
    Ok(unowned_codefiles(store)?
        .into_iter()
        .map(|n| n.name)
        .collect())
}

/// `(registered, owned, unowned_names, observed)` after coverage exclusions.
/// Ignored files are dropped from every bucket and observed files count only in
/// the `observed` bucket, so `registered == owned + unowned`.
///
/// Counts from [`unowned_codefiles`] rather than re-deriving the rule. It used
/// to carry its own copy, which meant the "single definition of the coverage
/// gap" was two definitions that agreed only by coincidence — and when the test
/// file rule landed in one of them, `loom coverage` and the `covered` rung
/// disagreed about the same files.
pub fn code_ownership_summary(store: &Store) -> Result<(usize, usize, Vec<String>, usize)> {
    let ignore = crate::fsglob::matcher(store.ignore_globs()?)?;
    let unowned: Vec<String> = unowned_codefiles(store)?
        .into_iter()
        .map(|n| n.name)
        .collect();
    let mut owned = 0usize;
    let mut observed = 0usize;
    for cf in store.codefiles()? {
        if ignore.is_match(&cf.name) {
            continue;
        }
        if codefile_observed(&cf) {
            observed += 1;
            continue;
        }
        if !unowned.contains(&cf.name) {
            owned += 1;
        }
    }
    Ok((owned + unowned.len(), owned, unowned, observed))
}
