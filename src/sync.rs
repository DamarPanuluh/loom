//! Sync — the structural recompute and ripple engine.
//!
//! Plane: orchestration over the store. Sync is content-hash based:
//! - re-extracts derived facets for files whose content changed,
//! - ripples staleness to asserted edges that depended on a *real* change
//!   (a file with a PRIOR hash that now differs — a missing prior hash is a
//!   rebuild/first-extract, not a change, so it never falsely stales),
//! - rebuilds the derived finding plane deterministically every run.
//!
//! This is the second half of INV-2: `store.wipe_derived()` then `sync` yields a
//! byte-identical derived plane (deterministic ids + sentinel timestamps + a
//! pure extraction), and ripples nothing because no prior hashes remain.

use crate::model::{
    EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass,
};
use crate::store::Store;
use crate::Result;
use std::collections::BTreeSet;
use std::path::Path;

/// Summary of one sync run.
#[derive(Debug, Default, Clone)]
pub struct SyncReport {
    pub files_scanned: usize,
    pub files_changed: usize,
    /// Registered files that previously had content and have now disappeared.
    pub files_deleted: usize,
    pub edges_staled: usize,
    /// Realizing groundings NOT re-opened by a file change because the change
    /// did not touch the symbol their locator names (and no cited evidence in
    /// the file was rewritten) — the payoff of symbol-scoped staleness.
    pub edges_spared: usize,
    pub validations_reset: usize,
    /// Interface surfaces whose backing code changed this run (integration monitoring).
    pub surfaces_affected: usize,
    /// Contracts (validations exercising an affected surface) reset to `not_run`.
    pub contracts_reset: usize,
    pub findings: usize,
    /// Wiki pages marked stale because a documented intent's meaning, code, or
    /// proof drifted since the page was last recorded.
    pub wiki_staled: usize,
    pub missing: Vec<String>,
}






/// Run a full sync against the graph rooted at `root`. Orchestrates the
/// registered code-seed derivers to recompute the derived plane, then ripples
/// the artifact changes they report — the engine names no extraction type, so
/// unplugging a deriver leaves this loop intact and rippling correctly.
pub fn run(store: &Store, root: &Path) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let mut changed_paths: BTreeSet<String> = BTreeSet::new();
    let mut seen_surfaces: BTreeSet<String> = BTreeSet::new();
    for deriver in crate::seed::sync_derivers() {
        for change in deriver.derive(store, root, &mut report)? {
            if let Some(node) = store.get_node(&change.artifact_id)? {
                changed_paths.insert(node.name);
            }
            ripple_surface_contracts(
                store,
                &change.artifact_id,
                &change.cause,
                &mut seen_surfaces,
                &mut report,
            )?;
        }
    }
    report.surfaces_affected = seen_surfaces.len();
    ripple_artifact_drift(store, root, &mut report)?;
    ripple_runner_drift(store, root, &mut report)?;
    ripple_wiki_drift(store, &mut report)?;
    // Grade every proof from its own shape. Derived, so it is recomputed here
    // rather than trusted from whoever registered the validation — the string
    // this replaced was supplied by the caller, and `loom journey add`
    // hardcoded the top of the scale.
    crate::proofstrength::recompute(store, root)?;
    // Wantedness earned from evidence. Recomputed AFTER proof strength, which
    // one of its three conjuncts reads.
    crate::ratification::recompute(store)?;
    rebuild_smell_findings(store, &mut report)?;
    // The re-verification pass. One question — "does the thing this fact points
    // at still say what it said?" — asked of every anchor in the graph.
    //
    // This replaced a ~470-line decision table that asked a DIFFERENT question
    // per edge kind: does a realizing grounding ripple to its intent, does that
    // intent ripple to its relates edges, do those ripple one-sidedly or only
    // when both endpoints moved, and so on. Every row of it was a hand-written
    // guess at what a change could invalidate. Anchors make the guess
    // unnecessary: a fact points at something, and either that thing still
    // holds or it does not. Nothing has to know WHY it might not.
    //
    // Symbol-scoped sparing survives the deletion because it was never really
    // about the ripple: a locator Run re-resolves its symbol, so an unrelated
    // edit in the same file leaves it standing on its own.
    let pass = store.reverify_all(&changed_paths)?;
    report.edges_staled += pass.demoted;
    report.edges_spared += pass.spared;
    report.validations_reset += pass.validations_reset;
    Ok(report)
}

/// Reset the contracts that exercise an interface surface this file backs.
///
/// The one dependency in the graph that is NOT evidence: a contract does not
/// cite the surface's backing code, it reaches it at run time through a `calls`
/// edge. No anchor points there, so re-verification cannot see it, and it stays
/// an explicit walk. Idempotent — a contract already at `not_run` is neither
/// reset nor counted.
fn ripple_surface_contracts(
    store: &Store,
    cf_id: &str,
    cause: &str,
    seen_surfaces: &mut BTreeSet<String>,
    report: &mut SyncReport,
) -> Result<()> {
    // Integration-monitoring ripple: cf is the `to` of an `exposes` edge; the
    // surface is its `from`. Reset the contracts that `call` each surface.
    for ex_edge in store.edges_with(Some(EdgeKind::Exposes), None, Some(cf_id))? {
        let surface_id = ex_edge.from_id;
        if !seen_surfaces.insert(surface_id.clone()) {
            continue; // already rippled via another changed file this run
        }
        for call in store.edges_with(Some(EdgeKind::Calls), None, Some(&surface_id))? {
            // Only a contract that was actually proven needs re-checking; one
            // already at `not_run` is unchanged, so it is neither reset nor
            // counted (avoids overstating "need re-verification").
            let was_proven = store
                .get_node(&call.from_id)?
                .map(|n| n.status != "not_run")
                .unwrap_or(false);
            if !was_proven {
                continue;
            }
            if store.stale_edge(&call.id, cause)? {
                report.edges_staled += 1;
            }
            store.reset_validation_status_for_sync(&call.from_id)?;
            report.contracts_reset += 1;
            // Fold into the headline reset tally so the summary line can never
            // read `0 validations reset` next to a reset contract.
            report.validations_reset += 1;
            // The contract proves intents via `validates`; reset those so the
            // intent's proof reads as unproven, mirroring the implements ripple.
            for v in store.edges_with(Some(EdgeKind::Validates), Some(&call.from_id), None)? {
                if store.stale_edge(&v.id, cause)? {
                    report.edges_staled += 1;
                }
            }
        }
    }
    Ok(())
}





















/// Pass 2b: ripple drift of a JourneyProof validation's `body.artifact` file.
///
/// A validation may point at a contract JSON / journey YAML / runner file via
/// `body.artifact`. Those paths are not necessarily registered CodeFiles, so
/// the structural pass cannot see them. Track a derived `artifact_hash` facet
/// per such validation and, when the file changes or disappears, stale its
/// `validates` edges and reset a proven validation to `not_run` — so a stale
/// artifact cannot keep a user-visible intent "proven" and silence the journey
/// smell. Mirrors the codefile content_hash convergence (INV-2).
fn ripple_artifact_drift(store: &Store, root: &Path, report: &mut SyncReport) -> Result<()> {
    let validations = store.list_nodes(Some(NodeType::Validation), usize::MAX)?;
    for val in validations {
        let Some(artifact) = val.body.get("artifact").and_then(|v| v.as_str()) else {
            continue;
        };
        let path = root.join(artifact);
        let prior = store.get_facet(&val.id, TargetKind::Node, "artifact_hash")?;
        let current = match std::fs::read_to_string(&path) {
            Ok(c) => Some(crate::artifact::fingerprint(&c)),
            Err(_) => None,
        };
        // No prior hash and no current file: never-hashed + absent → nothing to
        // ripple (first observation of a not-yet-present artifact).
        let drifted = match (prior.as_deref(), current.as_deref()) {
            (Some(p), Some(c)) => p != c,
            (Some(_), None) => true, // file disappeared
            _ => false,
        };
        // Refresh the derived hash so a wipe+rebuild converges (INV-2).
        match &current {
            Some(h) => store.set_facet(
                &val.id,
                TargetKind::Node,
                "artifact_hash",
                h,
                TruthClass::Derived,
            )?,
            None => store.clear_facet(&val.id, TargetKind::Node, "artifact_hash")?,
        }
        if !drifted {
            continue;
        }
        let cause = if current.is_some() {
            format!("artifact {artifact} changed")
        } else {
            format!("artifact {artifact} disappeared")
        };
        for e in store.edges_with(Some(EdgeKind::Validates), Some(&val.id), None)? {
            if store.stale_edge(&e.id, &cause)? {
                report.edges_staled += 1;
            }
        }
        // Only count/reset a validation that was actually proven; one already
        // at `not_run` is unchanged, so it is neither reset nor counted.
        if val.status != "not_run" {
            store.reset_validation_status_for_sync(&val.id)?;
            report.validations_reset += 1;
        }
    }
    Ok(())
}

/// Self-healing ripple for typed-runner coverage. A `journey_coverage` node may
/// carry `runner_ref` / `test_ref` pointing at the code that proves its flow
/// (path or `path::symbol`). Those files are not necessarily registered
/// CodeFiles, so the structural pass never sees them — meaning a developer can
/// edit (and break) the runner while the coverage's proof stays green. Track a
/// derived hash per ref and, on a real change or disappearance, stale the
/// covered intent's journey `validates` edges and reset a proven journey
/// validation to `not_run` — so the proof re-enters the validate queue and the
/// coverage flips to uncovered until it is re-run. Mirrors `ripple_artifact_drift`.
///
/// Only explicit path refs are content-tracked; a free-text ref (no on-disk
/// path component) is left to `journey coverage drift`'s existence check.
fn ripple_runner_drift(store: &Store, root: &Path, report: &mut SyncReport) -> Result<()> {
    let coverages = store.list_nodes(Some(NodeType::JourneyCoverage), usize::MAX)?;
    for cov in coverages {
        let mut causes = BTreeSet::new();
        for field in ["runner_ref", "test_ref"] {
            let Some(reference) = cov.body.get(field).and_then(|v| v.as_str()) else {
                continue;
            };
            // The on-disk path is the ref up to an optional `::symbol` locator.
            let rel_path = reference.split("::").next().unwrap_or(reference);
            if rel_path.is_empty() {
                continue;
            }
            let facet_key = format!("{field}_hash");
            let prior = store.get_facet(&cov.id, TargetKind::Node, &facet_key)?;
            let current = match std::fs::read_to_string(root.join(rel_path)) {
                Ok(c) => Some(crate::artifact::fingerprint(&c)),
                Err(_) => None,
            };
            // Seed on first observation (no prior hash) — never stale on it.
            let this_drifted = match (prior.as_deref(), current.as_deref()) {
                (Some(p), Some(c)) => p != c,
                (Some(_), None) => true, // file disappeared
                _ => false,
            };
            match &current {
                Some(h) => store.set_facet(
                    &cov.id,
                    TargetKind::Node,
                    &facet_key,
                    h,
                    TruthClass::Derived,
                )?,
                None => store.clear_facet(&cov.id, TargetKind::Node, &facet_key)?,
            }
            if this_drifted {
                let cause = if current.is_some() {
                    format!("{field} {rel_path} changed")
                } else {
                    format!("{field} {rel_path} disappeared")
                };
                causes.insert(cause);
            }
        }
        if causes.is_empty() {
            continue;
        }
        let cause = causes.iter().cloned().collect::<Vec<_>>().join("; ");
        // Find the covered intent (Covers: coverage → intent) and stale only the
        // journey proof(s) this coverage actually stands behind, so a sibling
        // proof for the same intent isn't disturbed. When the coverage declares
        // a `contract_artifact`, match it to the validation's `body.artifact`;
        // otherwise fall back to the intent's current passing L5/L6 journey
        // proofs. Restricting to passing L5/L6 keeps an already-unproven or
        // shallow validation untouched.
        let Some(cover_edge) = store
            .edges_with(Some(EdgeKind::Covers), Some(&cov.id), None)?
            .into_iter()
            .next()
        else {
            continue;
        };
        let coverage_artifact = cov.body.get("contract_artifact").and_then(|v| v.as_str());
        for e in store.edges_with(Some(EdgeKind::Validates), None, Some(&cover_edge.to_id))? {
            let Some(val) = store.get_node(&e.from_id)? else {
                continue;
            };
            let is_journey = val.body.get("proof_kind").and_then(|v| v.as_str()) == Some("journey");
            if !is_journey {
                continue;
            }
            // Only a currently-proven proof is worth re-opening.
            if val.status != "passed" {
                continue;
            }
            // If the coverage names a specific artifact, only stale the proof
            // backed by that same artifact.
            if let Some(want) = coverage_artifact {
                let proof_artifact = val.body.get("artifact").and_then(|v| v.as_str());
                if proof_artifact != Some(want) {
                    continue;
                }
            }
            if store.stale_edge(&e.id, &cause)? {
                report.edges_staled += 1;
            }
            store.reset_validation_status_for_sync(&val.id)?;
            report.validations_reset += 1;
        }
    }
    Ok(())
}

/// The freshness fingerprint of a wiki page's scope: a hash over every intent
/// the page `documents` — the intent's meaning (lifecycle + updated_at), the
/// content of the files that realize it, and whether it is proven. It changes
/// iff something the page describes changed, so a recorded page stays fresh
/// exactly until its subject drifts. The page's prose/layout is never hashed —
/// the graph governs freshness, not form.
pub fn wiki_scope_hash(store: &Store, page_id: &str) -> Result<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut docs = store.edges_with(Some(EdgeKind::Documents), Some(page_id), None)?;
    docs.sort_by(|a, b| a.to_id.cmp(&b.to_id));
    for d in docs {
        let Some(intent) = store.get_node(&d.to_id)? else {
            continue;
        };
        parts.push(format!(
            "i:{}:{}:{}",
            intent.id, intent.status, intent.updated_at
        ));
        let mut files: Vec<String> = Vec::new();
        for g in store.realizing_groundings(&intent.id)? {
            let hash = store
                .get_facet(&g.to_id, TargetKind::Node, "content_hash")?
                .unwrap_or_default();
            files.push(format!("{}={hash}", g.to_id));
        }
        files.sort();
        parts.push(format!("f:[{}]", files.join(",")));
        let proven = store
            .edges_with(Some(EdgeKind::Validates), None, Some(&intent.id))?
            .iter()
            .any(|v| v.status == InspectionStatus::Passing);
        parts.push(format!("p:{proven}"));
    }
    Ok(crate::artifact::fingerprint(&parts.join("|")))
}

/// Pass 2d: mark a wiki page stale when its documented scope drifted since it was
/// recorded. Mirrors the artifact/runner drift gates — a recorded page's stored
/// `scope_hash` is compared to the live one; a `draft` page (never recorded
/// fresh) is left alone. loom curates freshness; an agent rewrites the prose.
fn ripple_wiki_drift(store: &Store, report: &mut SyncReport) -> Result<()> {
    for page in store.list_nodes(Some(NodeType::WikiPage), usize::MAX)? {
        if page.status == "draft" || page.status == "stale" {
            continue;
        }
        let current = wiki_scope_hash(store, &page.id)?;
        let stored = page
            .body
            .get("scope_hash")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if current != stored {
            store.set_node_status(&page.id, "stale")?;
            report.wiki_staled += 1;
        }
    }
    Ok(())
}

/// Pass 3b: materialize structural smells as derived Finding nodes. Smells
/// stay computed-on-read for `loom smells`, but the materialized finding gives
/// the triage queue a servable item and `loom finding verdict` a stable id
/// whose asserted adjudication survives every rebuild — the same wipe/re-derive
/// convergence cycle as the other structural findings.
fn rebuild_smell_findings(store: &Store, report: &mut SyncReport) -> Result<()> {
    for s in crate::signal::smells(store)? {
        store.add_derived_node(
            NodeType::Finding,
            &crate::signal::smell_det_key(&s.identity),
            &s.message,
            &s.remedy,
            &s.kind,
            serde_json::json!({ "kind": s.kind, "category": "smell", "identity": s.identity }),
        )?;
        report.findings += 1;
    }
    Ok(())
}
