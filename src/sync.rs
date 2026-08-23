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

use crate::model::{EdgeKind, InspectionStatus, NodeType, TargetKind};
use crate::store::Store;
use crate::Result;
use serde::Serialize;
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
    /// The grounding-plane share of [`Self::edges_staled`]: how many
    /// `implements` edges this run re-opened. `edges_staled` is a union across
    /// every plane, so its value depends on how much of the validation plane
    /// happens to be settled in the graph at hand — the same source change
    /// re-opens one grounding in a locally proven graph and a hundred-odd
    /// edges in a freshly imported one, where every imported proof is still
    /// prose. Anything asserting "this change moved exactly N groundings" —
    /// a Journey fixture, a release gate — has to read this instead.
    pub groundings_staled: usize,
    /// Realizing groundings NOT re-opened by a file change because the change
    /// did not touch the symbol their locator names (and no cited evidence in
    /// the file was rewritten) — the payoff of symbol-scoped staleness.
    pub edges_spared: usize,
    pub validations_reset: usize,
    /// Evidence spans re-anchored to new coordinates (a move, not a rewrite) —
    /// journaled as `evidence_reanchor`, no re-verdict demanded.
    pub evidence_reanchored: usize,
    /// Interface surfaces whose backing code changed this run (integration monitoring).
    pub surfaces_affected: usize,
    /// Contracts (validations exercising an affected surface) reset to `not_run`.
    pub contracts_reset: usize,
    pub findings: usize,
    /// Wiki pages marked stale because a documented intent's meaning, code, or
    /// proof drifted since the page was last recorded.
    pub wiki_staled: usize,
    pub missing: Vec<String>,
    /// The exact edges this run re-opened, in deterministic id order. The count
    /// alone is not actionable: anything that asserts on `edges_staled` — a
    /// Journey fixture, a release gate, an operator reading `sync --json` — has
    /// to know WHICH claim moved before it can decide whether the ripple was
    /// the expected one.
    pub staled_edges: BTreeSet<String>,
}

/// One strict source-anchor locator that a read-only sync preview can no
/// longer resolve. The locator module owns syntax and cardinality policy; sync
/// carries only the affected graph edge and the exact resolver error.
#[derive(Debug, Clone, Serialize)]
pub struct AnchorStaleness {
    pub edge_id: String,
    pub edge_kind: EdgeKind,
    pub codefile_id: String,
    pub path: String,
    pub locator: String,
    pub cause: String,
}

/// Read-only answer to “would structural sync observe different repository
/// state?”. It deliberately does not run derivers or write a freshness stamp.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncPreview {
    pub fresh: bool,
    pub changed_files: Vec<String>,
    pub missing_files: Vec<String>,
    pub unregistered_glob_matches: Vec<String>,
    pub invalid_anchors: Vec<AnchorStaleness>,
}

impl SyncPreview {
    pub fn affected_paths(&self) -> Vec<String> {
        let mut paths = self.changed_files.clone();
        paths.extend(self.missing_files.iter().cloned());
        paths.extend(self.unregistered_glob_matches.iter().cloned());
        paths.extend(
            self.invalid_anchors
                .iter()
                .map(|anchor| anchor.path.clone()),
        );
        paths.sort();
        paths.dedup();
        paths
    }

    pub fn evidence_lines(&self) -> Vec<String> {
        if self.fresh {
            return vec!["registered repository content matches the synchronized graph".into()];
        }
        let mut evidence = Vec::new();
        evidence.extend(
            self.changed_files
                .iter()
                .map(|path| format!("content changed since sync: {path}")),
        );
        evidence.extend(
            self.missing_files
                .iter()
                .map(|path| format!("registered file missing: {path}")),
        );
        evidence.extend(
            self.unregistered_glob_matches
                .iter()
                .map(|path| format!("remembered glob has unregistered file: {path}")),
        );
        evidence.extend(self.invalid_anchors.iter().map(|anchor| {
            format!(
                "anchor invalid on {} edge {} at {}: {}",
                anchor.edge_kind, anchor.edge_id, anchor.path, anchor.cause
            )
        }));
        evidence.sort();
        evidence
    }
}

/// Inspect repository/sync drift without mutating graph or filesystem state.
/// Both this preview and [`run`] consume the same strict anchor plan, keeping
/// the definition of anchor freshness local to one implementation.
pub fn preview(store: &Store, root: &Path) -> Result<SyncPreview> {
    let codefiles = store.codefiles()?;
    let mut changed_files = Vec::new();
    let mut missing_files = Vec::new();
    let existing: BTreeSet<String> = codefiles.iter().map(|file| file.name.clone()).collect();

    for file in &codefiles {
        let path = root.join(&file.name);
        let content = match std::fs::read_to_string(&path) {
            Ok(content) if crate::fsglob::contains(root, &path) => content,
            _ => {
                missing_files.push(file.name.clone());
                continue;
            }
        };
        let current = crate::extract::fnv1a(&content);
        let stored = store.get_facet(&file.id, TargetKind::Node, "content_hash")?;
        if stored.as_deref() != Some(current.as_str()) {
            changed_files.push(file.name.clone());
        }
    }

    let ignored = crate::fsglob::matcher(store.ignore_globs()?)?;
    let globs: Vec<String> = store
        .get_meta("codefile_globs")?
        .map(|raw| serde_json::from_str(&raw))
        .transpose()?
        .unwrap_or_default();
    let mut unregistered_glob_matches = Vec::new();
    for glob in globs {
        for path in crate::fsglob::expand(root, &glob)? {
            if !existing.contains(&path) && !ignored.is_match(&path) {
                unregistered_glob_matches.push(path);
            }
        }
    }

    changed_files.sort();
    changed_files.dedup();
    missing_files.sort();
    missing_files.dedup();
    unregistered_glob_matches.sort();
    unregistered_glob_matches.dedup();
    let invalid_anchors = plan_anchor_staleness(store)?;
    let fresh = changed_files.is_empty()
        && missing_files.is_empty()
        && unregistered_glob_matches.is_empty()
        && invalid_anchors.is_empty();
    Ok(SyncPreview {
        fresh,
        changed_files,
        missing_files,
        unregistered_glob_matches,
        invalid_anchors,
    })
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
    ripple_compiled_journey_drift(store, &mut report)?;
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
    //
    // ORDER: this runs BEFORE the derived recomputes below (proof strength,
    // ratification, wiki freshness, smell findings). Each of those READS edge
    // and validation status — the very state this pass settles by re-opening a
    // proof whose anchor broke. Grading first and re-opening after left the
    // derived plane one sync stale (a proof graded S3 this run, demoted next),
    // so a second sync produced a different graph — a fixpoint violation of
    // INV-2. Settling status first makes one sync converge.
    // Explicit dependency ripples above may already have moved an edge to
    // `needs_reverification` before its evidence fact is rechecked here. The
    // re-verifier reports demoted facts, so adding that raw count would count
    // the same edge twice. Snapshot the settled boundary and count only edge
    // IDs whose status actually transitions during this pass.
    let stale_before_reverify: BTreeSet<String> = store
        .list_edges(None, usize::MAX)?
        .into_iter()
        .filter(|edge| edge.status == InspectionStatus::NeedsReverification)
        .map(|edge| edge.id)
        .collect();
    let pass = store.reverify_all(&changed_paths)?;
    let stale_after_reverify: BTreeSet<String> = store
        .list_edges(None, usize::MAX)?
        .into_iter()
        .filter(|edge| edge.status == InspectionStatus::NeedsReverification)
        .map(|edge| edge.id)
        .collect();
    let newly_stale = stale_after_reverify.difference(&stale_before_reverify);
    for edge_id in newly_stale {
        report.edges_staled += 1;
        report.staled_edges.insert(edge_id.clone());
    }
    report.edges_spared += pass.spared;
    report.validations_reset += pass.validations_reset;
    report.evidence_reanchored += pass.reanchored;
    ripple_validation_evidence_drift(store, &changed_paths, &mut report)?;
    let anchor_staleness = plan_anchor_staleness(store)?;
    apply_anchor_staleness(store, &anchor_staleness, &mut report)?;
    // AFTER the pass, deliberately. The anchor machinery is more precise —
    // it distinguishes a redefined symbol from a missing one and spares
    // untouched groundings — so it gets first say, and this is the backstop
    // for anchors it never re-checks at all: a locator whose verdict was
    // settled on a cited span, which carries no locator Run to re-resolve.
    // Running it first double-counted an edge both mechanisms would stale.
    ripple_locator_drift(store, root, &mut report)?;
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
    // Derived from the recorded set rather than counted at each staling site:
    // every pass above reaches `staled_edges` through a different door, and a
    // plane counter maintained per-door is one door away from being wrong.
    report.groundings_staled = grounding_share(store, &report.staled_edges)?;
    Ok(report)
}

/// How many of `staled` are grounding-plane (`implements`) edges. An edge that
/// vanished between staling and here simply does not count.
fn grounding_share(store: &Store, staled: &BTreeSet<String>) -> Result<usize> {
    let mut groundings = 0;
    for edge_id in staled {
        if store
            .get_edge(edge_id)?
            .is_some_and(|edge| edge.kind == EdgeKind::Implements)
        {
            groundings += 1;
        }
    }
    Ok(groundings)
}

/// Reset validations whose explicit Validation→CodeFile S3 entry surface
/// changed. `exercises` is evidence provenance, not a verdict-bearing claim, so
/// the validation itself is the state that expires.
fn ripple_validation_evidence_drift(
    store: &Store,
    changed_paths: &BTreeSet<String>,
    report: &mut SyncReport,
) -> Result<()> {
    let mut reset = BTreeSet::new();
    let files_by_path: std::collections::BTreeMap<String, String> = store
        .list_nodes(Some(NodeType::CodeFile), usize::MAX)?
        .into_iter()
        .map(|node| (node.name, node.id))
        .collect();
    for path in changed_paths {
        let Some(file_id) = files_by_path.get(path) else {
            continue;
        };
        for edge in store.edges_with(Some(EdgeKind::Exercises), None, Some(file_id))? {
            if !reset.insert(edge.from_id.clone()) {
                continue;
            }
            let was_run = store
                .get_node(&edge.from_id)?
                .is_some_and(|validation| validation.status != "not_run");
            if was_run {
                stale_validation_closure(
                    store,
                    &edge.from_id,
                    &format!("validation evidence file '{}' changed", path),
                    false,
                    report,
                )?;
            }
        }
    }
    Ok(())
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
            stale_validation_closure(store, &call.from_id, cause, true, report)?;
            report.contracts_reset += 1;
        }
    }
    Ok(())
}

fn stale_validation_closure(
    store: &Store,
    validation_id: &str,
    cause: &str,
    stale_calls: bool,
    report: &mut SyncReport,
) -> Result<()> {
    for kind in [EdgeKind::Proves, EdgeKind::Validates] {
        for edge in store.edges_with(Some(kind), Some(validation_id), None)? {
            if store.stale_edge(&edge.id, cause)? {
                report.edges_staled += 1;
                report.staled_edges.insert(edge.id.clone());
            }
        }
    }
    if stale_calls {
        for edge in store.edges_with(Some(EdgeKind::Calls), Some(validation_id), None)? {
            if store.stale_edge(&edge.id, cause)? {
                report.edges_staled += 1;
                report.staled_edges.insert(edge.id.clone());
            }
        }
    }
    if store
        .get_node(validation_id)?
        .is_some_and(|validation| validation.status != "not_run")
    {
        store.reset_validation_status_for_sync(validation_id)?;
        report.validations_reset += 1;
    }
    Ok(())
}

/// Invalidate compiler output when its semantic or reusable-surface inputs no
/// longer match the graph. Raw authored artifact paths are intentionally absent:
/// the compiler contract is the Proves+Calls+Exercises closure and its hashes.
fn ripple_compiled_journey_drift(store: &Store, report: &mut SyncReport) -> Result<()> {
    for proves in store.edges_with(Some(EdgeKind::Proves), None, None)? {
        let Some(validation) = store.get_node(&proves.from_id)? else {
            continue;
        };
        let Some(journey) = store.get_node(&proves.to_id)? else {
            continue;
        };
        if validation.body.get("type").and_then(|value| value.as_str()) != Some("journey") {
            continue;
        }
        let journey_hash = journey
            .body
            .get("semantic_hash")
            .and_then(|value| value.as_str());
        let surface_hash = crate::journey::surface_projection_hash(store, &journey)?;
        let body_current = journey_hash.is_some()
            && validation
                .body
                .get("journey_hash")
                .and_then(|value| value.as_str())
                == journey_hash
            && surface_hash.is_some()
            && validation
                .body
                .get("surface_hash")
                .and_then(|value| value.as_str())
                == surface_hash.as_deref()
            && validation
                .body
                .get("profile")
                .and_then(|value| value.as_str())
                == Some("proof")
            && validation
                .body
                .get("compiler_version")
                .and_then(|value| value.as_str())
                == Some(crate::journey::JOURNEY_COMPILER_VERSION);
        // The compiled Exercises topology and its provenance facets must agree
        // exactly with the canonical projection of the accepted surface.
        // A missing/stale projection or any semantic disagreement — forged,
        // malformed, or obsolete provenance — stales the closure through the
        // normal compiler-owned mechanism instead of letting grading carry a
        // topology nobody accepted.
        let topology_current = match crate::journey_exercises::expected_projection(store, &journey)
        {
            Ok(projection) => {
                crate::journey_exercises::topology_problems(store, &validation.id, &projection)?
                    .is_empty()
            }
            Err(_) => false,
        };

        let mut accepted_surfaces = BTreeSet::new();
        if let Some(hash) = journey_hash {
            for surface in store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)? {
                if matches!(
                    surface.status,
                    InspectionStatus::Uninspected | InspectionStatus::Passing
                ) && store
                    .get_facet(&surface.id, TargetKind::Edge, "journey_hash")?
                    .as_deref()
                    == Some(hash)
                {
                    accepted_surfaces.insert(surface.to_id);
                }
            }
        }
        let calls_current = store
            .edges_with(Some(EdgeKind::Calls), Some(&validation.id), None)?
            .into_iter()
            .any(|call| {
                matches!(
                    call.status,
                    InspectionStatus::Uninspected | InspectionStatus::Passing
                ) && accepted_surfaces.contains(&call.to_id)
            });
        let exercises_current = store
            .edges_with(Some(EdgeKind::Exercises), Some(&validation.id), None)?
            .into_iter()
            .any(|edge| {
                matches!(
                    edge.status,
                    InspectionStatus::Uninspected | InspectionStatus::Passing
                )
            });
        if body_current && calls_current && exercises_current && topology_current {
            continue;
        }
        stale_validation_closure(
            store,
            &validation.id,
            "compiled Journey inputs or proof topology drifted",
            true,
            report,
        )?;
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
/// Re-open a grounding whose locator names nothing.
///
/// A locator is the promise "this behavior lives at this symbol". Nothing was
/// checking it. `recheck` expires a LOCATOR run when its symbol stops
/// resolving, but a verdict recorded with a cited span carries no such run —
/// so a grounding could name a deleted symbol and stay `passing` forever. On
/// this repository that was 13 live claims at confidence 0.95 pointing at
/// symbols that no longer existed, including four left by a hard-cut, while
/// `doctor` reported clean and the ladder showed `grounded` met.
///
/// A locator that opens with `module` is a WHOLE-FILE scope, not a symbol —
/// the long-standing convention here for "the behavior is this file". Those
/// are left alone; requiring them to resolve would reject 39 legitimate
/// groundings to catch 13 broken ones.
fn ripple_locator_drift(store: &Store, root: &Path, report: &mut SyncReport) -> Result<()> {
    for edge in store.edges_with(Some(EdgeKind::Implements), None, None)? {
        if store.edge_superseded(&edge.id)? {
            continue;
        }
        // Only a REALIZES grounding promises a symbol. A `consumes` locator
        // names a seam — an interface string the consumer calls, which is not a
        // definition and will never resolve as one; `recheck` re-runs those
        // through their own Seam arm. `configures` and `verifies` make no
        // symbol claim either. Judging them here would stale every seam
        // grounding on every sync, which ring11 catches.
        if store.grounding_role(&edge.id)? != crate::model::GroundingRole::Realizes {
            continue;
        }
        stale_unresolved_locator(store, root, &edge, false, report)?;
    }
    // `exercises` provenance makes the same symbol promise a realizing
    // grounding does, and nothing else re-checks it: the anchor machinery
    // covers only anchor-form locators, and intact-looking provenance is
    // deliberately not queue work — so this ripple must fire even on a
    // never-inspected edge or the repair reaches no lane. Compiler-owned
    // Journey topology is excepted: its provenance is policed against the
    // accepted surface's canonical projection, and its door is journey
    // compile/run.
    for edge in store.edges_with(Some(EdgeKind::Exercises), None, None)? {
        if crate::completeness::compiler_owned_proof_edge(store, &edge)?.is_some() {
            continue;
        }
        stale_unresolved_locator(store, root, &edge, true, report)?;
    }
    Ok(())
}

fn stale_unresolved_locator(
    store: &Store,
    root: &Path,
    edge: &crate::model::Edge,
    include_uninspected: bool,
    report: &mut SyncReport,
) -> Result<()> {
    let Some(locator) = store.edge_locator(&edge.id)? else {
        return Ok(());
    };
    let locator = locator.trim();
    if locator.is_empty()
        || crate::locator::is_module_scope(locator)
        || crate::locator::is_anchor_locator(locator)
    {
        return Ok(());
    }
    let Some(file) = store.get_node(&edge.to_id)? else {
        return Ok(());
    };
    // A missing FILE is already someone else's ripple; only judge the
    // symbol when the file is there to look in.
    if !root.join(&file.name).exists() {
        return Ok(());
    }
    // `resolve_locator` returns Some whenever the FILE is readable — the
    // cardinality is carried separately, which is why `unique_locator_probe`
    // reads match_count rather than the Option. A locator that matched
    // nothing is the broken case; matching several is ambiguous but still
    // points at real code, and the ripple already spares those.
    let matched = crate::runner::resolve_locator(root, &file.name, Some(locator))
        .map(|r| r.match_count)
        .unwrap_or(0);
    if matched > 0 {
        return Ok(());
    }
    // Name the anchor that fell: a cause that says only "a locator broke"
    // leaves the reader to work out which one, and ring13 requires the
    // symbol or the file by name.
    let cause = format!("locator '{locator}' names no symbol in {}", file.name);
    if store.stale_edge(&edge.id, &cause)?
        || (include_uninspected && store.stale_uninspected_edge(&edge.id, &cause)?)
    {
        report.edges_staled += 1;
        report.staled_edges.insert(edge.id.clone());
    }
    Ok(())
}

/// Resolve every anchor-form locator through the locator module. Sync never
/// parses marker syntax or guesses cardinality itself.
fn plan_anchor_staleness(store: &Store) -> Result<Vec<AnchorStaleness>> {
    let mut stale = Vec::new();
    for kind in [EdgeKind::Implements, EdgeKind::Exposes, EdgeKind::Exercises] {
        for edge in store.edges_with(Some(kind), None, None)? {
            if kind == EdgeKind::Implements && store.edge_superseded(&edge.id)? {
                continue;
            }
            let Some(locator) = store.edge_locator(&edge.id)? else {
                continue;
            };
            if !crate::locator::is_anchor_locator(&locator) {
                continue;
            }
            let Some(codefile) = store.get_node(&edge.to_id)? else {
                continue;
            };
            if codefile.node_type != NodeType::CodeFile {
                continue;
            }
            if let Err(error) = crate::locator::validate_for_codefile(store, &codefile, &locator) {
                stale.push(AnchorStaleness {
                    edge_id: edge.id,
                    edge_kind: kind,
                    codefile_id: codefile.id,
                    path: codefile.name,
                    locator,
                    cause: error.to_string(),
                });
            }
        }
    }
    stale.sort_by(|a, b| a.edge_id.cmp(&b.edge_id));
    Ok(stale)
}

fn apply_anchor_staleness(
    store: &Store,
    stale: &[AnchorStaleness],
    report: &mut SyncReport,
) -> Result<()> {
    let mut reset_validations = BTreeSet::new();
    for anchor in stale {
        if store.stale_edge(&anchor.edge_id, &anchor.cause)? {
            report.edges_staled += 1;
            report.staled_edges.insert(anchor.edge_id.clone());
        }
        match anchor.edge_kind {
            EdgeKind::Exercises => {
                let Some(edge) = store.get_edge(&anchor.edge_id)? else {
                    continue;
                };
                // A broken anchor on never-inspected provenance was invisible:
                // the closure reset below re-fires every sync while the edge
                // carrying the actual defect stayed uninspected and unqueued.
                // Surface it as the analyze work it is; compiler-owned Journey
                // topology keeps its own door.
                if edge.status == InspectionStatus::Uninspected
                    && crate::completeness::compiler_owned_proof_edge(store, &edge)?.is_none()
                    && store.stale_uninspected_edge(&edge.id, &anchor.cause)?
                {
                    report.edges_staled += 1;
                    report.staled_edges.insert(edge.id.clone());
                }
                if reset_validations.insert(edge.from_id.clone()) {
                    stale_validation_closure(store, &edge.from_id, &anchor.cause, false, report)?;
                }
            }
            EdgeKind::Exposes => {
                let Some(edge) = store.get_edge(&anchor.edge_id)? else {
                    continue;
                };
                for call in store.edges_with(Some(EdgeKind::Calls), None, Some(&edge.from_id))? {
                    if reset_validations.insert(call.from_id.clone()) {
                        stale_validation_closure(
                            store,
                            &call.from_id,
                            &anchor.cause,
                            true,
                            report,
                        )?;
                        report.contracts_reset += 1;
                    }
                }
            }
            EdgeKind::Implements => {}
            _ => unreachable!("anchor staleness is planned only for locator-bearing edges"),
        }
    }
    Ok(())
}

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
            // loom-stability-exempt: marks a wiki page stale
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
