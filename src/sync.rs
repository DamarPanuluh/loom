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

use crate::extract::{extract, Extraction, Role};
use crate::model::{EdgeKind, Node, NodeType, TargetKind, TruthClass};
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
    pub validations_reset: usize,
    /// Interface surfaces whose backing code changed this run (integration monitoring).
    pub surfaces_affected: usize,
    /// Contracts (validations exercising an affected surface) reset to `not_run`.
    pub contracts_reset: usize,
    pub findings: usize,
    pub missing: Vec<String>,
}

/// A built-in structural CodeRule and the finding kinds it covers.
struct BuiltinRule {
    key: &'static str,
    name: &'static str,
    category: &'static str,
    description: &'static str,
}

const BUILTIN_RULES: &[BuiltinRule] = &[
    BuiltinRule {
        key: "max-file-size",
        name: "max-file-size",
        category: "size",
        description: "a file should not exceed a maintainable line count",
    },
    BuiltinRule {
        key: "complex-symbol",
        name: "complex-symbol",
        category: "complexity",
        description: "a function should stay below a cognitive complexity threshold",
    },
    BuiltinRule {
        key: "no-panic-marker",
        name: "no-panic-marker",
        category: "safety",
        description: "production source should not panic at a boundary (unwrap/panic!)",
    },
];

const MAX_FILE_LOC: usize = 600;
const MAX_COMPLEXITY: u32 = 20;

/// Run a full sync against the graph rooted at `root`.
pub fn run(store: &Store, root: &Path) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let rules = ensure_builtin_rules(store)?;
    let codefiles = store.codefiles()?;
    let changed_intents = sync_structural(store, root, &codefiles, &mut report)?;
    ripple_changed_intents(store, &changed_intents, &mut report)?;
    ripple_artifact_drift(store, root, &mut report)?;
    ripple_runner_drift(store, root, &mut report)?;
    rebuild_findings(store, root, &codefiles, &rules, &mut report)?;
    Ok(report)
}

/// Pass 1: detect content changes, recompute derived facets, and gather the set
/// of intents whose grounding changed (the ripple seed).
fn sync_structural(
    store: &Store,
    root: &Path,
    codefiles: &[Node],
    report: &mut SyncReport,
) -> Result<BTreeSet<String>> {
    let mut changed_intents: BTreeSet<String> = BTreeSet::new();
    let mut seen_surfaces: BTreeSet<String> = BTreeSet::new();
    for cf in codefiles {
        report.files_scanned += 1;
        let path = root.join(&cf.name);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => {
                report.missing.push(cf.name.clone());
                // A registered file that previously had content and is now gone
                // is a deletion: ripple its dependents ONCE, then clear the
                // derived content_hash. Clearing makes the next sync (prior=None)
                // skip it, and makes the incremental state converge with a clean
                // wipe+rebuild (which also has no hash for a missing file) — INV-2.
                if store
                    .get_facet(&cf.id, TargetKind::Node, "content_hash")?
                    .is_some()
                {
                    report.files_deleted += 1;
                    ripple_codefile(
                        store,
                        &cf.id,
                        &mut changed_intents,
                        &mut seen_surfaces,
                        report,
                    )?;
                    store.clear_facet(&cf.id, TargetKind::Node, "content_hash")?;
                }
                continue;
            }
        };
        let ex = extract(&cf.name, &content);
        let prior = store.get_facet(&cf.id, TargetKind::Node, "content_hash")?;
        let current = ex.content_hash.clone();
        if prior.as_deref() == Some(current.as_str()) {
            continue; // unchanged
        }
        write_facets(store, &cf.id, &ex)?;
        // Ripple only on a REAL change (prior hash existed and differs).
        if prior.is_some() {
            report.files_changed += 1;
            ripple_codefile(
                store,
                &cf.id,
                &mut changed_intents,
                &mut seen_surfaces,
                report,
            )?;
        }
    }
    report.surfaces_affected = seen_surfaces.len();
    Ok(changed_intents)
}

/// Ripple one codefile's change or disappearance to everything that depended on
/// it: the `implements` groundings (seeding the pass-2 intent ripple), and the
/// interface surfaces it backed → the contracts that exercise them. Idempotent
/// on already-stale edges, so a still-missing file re-rippling is harmless.
fn ripple_codefile(
    store: &Store,
    cf_id: &str,
    changed_intents: &mut BTreeSet<String>,
    seen_surfaces: &mut BTreeSet<String>,
    report: &mut SyncReport,
) -> Result<()> {
    for e in store.edges_with(Some(EdgeKind::Implements), None, Some(cf_id))? {
        if store.stale_edge(&e.id)? {
            report.edges_staled += 1;
        }
        changed_intents.insert(e.from_id.clone());
    }
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
            if store.stale_edge(&call.id)? {
                report.edges_staled += 1;
            }
            store.set_node_status(&call.from_id, "not_run")?;
            report.contracts_reset += 1;
            // Fold into the headline reset tally so the summary line can never
            // read `0 validations reset` next to a reset contract.
            report.validations_reset += 1;
            // The contract proves intents via `validates`; reset those so the
            // intent's proof reads as unproven, mirroring the implements ripple.
            for v in store.edges_with(Some(EdgeKind::Validates), Some(&call.from_id), None)? {
                if store.stale_edge(&v.id)? {
                    report.edges_staled += 1;
                }
            }
        }
    }
    Ok(())
}

/// Pass 2: ripple staleness from changed intents to the edges that depend on
/// them — governs, validates (also resetting the proof), and relationships.
fn ripple_changed_intents(
    store: &Store,
    changed_intents: &BTreeSet<String>,
    report: &mut SyncReport,
) -> Result<()> {
    for intent in changed_intents {
        for e in store.edges_with(Some(EdgeKind::Governs), None, Some(intent))? {
            if store.stale_edge(&e.id)? {
                report.edges_staled += 1;
            }
        }
        for e in store.edges_with(Some(EdgeKind::Validates), None, Some(intent))? {
            if store.stale_edge(&e.id)? {
                report.edges_staled += 1;
            }
            // Reset the linked Validation's last_result.
            store.set_node_status(&e.from_id, "not_run")?;
            report.validations_reset += 1;
        }
        for kind in [
            EdgeKind::Relates,
            EdgeKind::Requires,
            EdgeKind::ScenarioOf,
            EdgeKind::VariantOf,
            EdgeKind::Triggers,
            EdgeKind::Sequence,
        ] {
            for e in store.edges_with(Some(kind), Some(intent), None)? {
                if store.stale_edge(&e.id)? {
                    report.edges_staled += 1;
                }
            }
            for e in store.edges_with(Some(kind), None, Some(intent))? {
                if store.stale_edge(&e.id)? {
                    report.edges_staled += 1;
                }
            }
        }
    }
    Ok(())
}

/// Pass 2b: ripple drift of a JourneyProof validation's `body.artifact` file.
///
/// A validation may point at a contract JSON / saga YAML / runner file via
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
            Ok(c) => Some(crate::extract::fnv1a(&c)),
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
        for e in store.edges_with(Some(EdgeKind::Validates), Some(&val.id), None)? {
            if store.stale_edge(&e.id)? {
                report.edges_staled += 1;
            }
        }
        // Only count/reset a validation that was actually proven; one already
        // at `not_run` is unchanged, so it is neither reset nor counted.
        if val.status != "not_run" {
            store.set_node_status(&val.id, "not_run")?;
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
        let mut drifted = false;
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
                Ok(c) => Some(crate::extract::fnv1a(&c)),
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
            drifted |= this_drifted;
        }
        if !drifted {
            continue;
        }
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
            let is_l5_plus = matches!(
                val.body.get("proof_level").and_then(|v| v.as_str()),
                Some("L5") | Some("L6")
            );
            if !is_journey || !is_l5_plus {
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
            if store.stale_edge(&e.id)? {
                report.edges_staled += 1;
            }
            store.set_node_status(&val.id, "not_run")?;
            report.validations_reset += 1;
        }
    }
    Ok(())
}

/// Pass 3: rebuild the derived finding plane deterministically (wipe + re-derive
/// so unchanged input yields a byte-identical plane — INV-2).
fn rebuild_findings(
    store: &Store,
    root: &Path,
    codefiles: &[Node],
    rules: &std::collections::HashMap<&'static str, String>,
    report: &mut SyncReport,
) -> Result<()> {
    store.wipe_derived_graph()?;
    for cf in codefiles {
        let path = root.join(&cf.name);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let ex = extract(&cf.name, &content);
        for f in derive_findings(&cf.name, &ex) {
            let rule_id = match rules.get(f.rule) {
                Some(id) => id,
                None => continue,
            };
            let det_key = format!("{}:{}:{}", f.kind, cf.name, f.symbol);
            let node = store.add_derived_node(
                NodeType::Finding,
                &det_key,
                &f.title,
                &f.detail,
                f.kind,
                serde_json::json!({ "kind": f.kind, "symbol": f.symbol }),
            )?;
            store.add_derived_edge(EdgeKind::Flags, &node.id, &cf.id)?;
            store.add_derived_edge(EdgeKind::Assesses, &node.id, rule_id)?;
            report.findings += 1;
        }
    }
    Ok(())
}

fn write_facets(store: &Store, cf_id: &str, ex: &Extraction) -> Result<()> {
    let d = TruthClass::Derived;
    store.set_facet(cf_id, TargetKind::Node, "language", ex.language.as_str(), d)?;
    store.set_facet(cf_id, TargetKind::Node, "role", ex.role.as_str(), d)?;
    store.set_facet(cf_id, TargetKind::Node, "loc", &ex.loc.to_string(), d)?;
    store.set_facet(cf_id, TargetKind::Node, "content_hash", &ex.content_hash, d)?;
    store.set_facet(
        cf_id,
        TargetKind::Node,
        "symbol_count",
        &ex.symbols.len().to_string(),
        d,
    )?;
    let imports = serde_json::to_string(&ex.imports).unwrap_or_else(|_| "[]".into());
    store.set_facet(cf_id, TargetKind::Node, "imports", &imports, d)?;
    Ok(())
}

fn ensure_builtin_rules(store: &Store) -> Result<std::collections::HashMap<&'static str, String>> {
    let mut map = std::collections::HashMap::new();
    for r in BUILTIN_RULES {
        let node = store.upsert_builtin_node(
            NodeType::CodeRule,
            r.key,
            r.name,
            r.description,
            serde_json::json!({ "category": r.category }),
        )?;
        map.insert(r.key, node.id);
    }
    Ok(map)
}

/// A derived finding descriptor (pre-persistence).
struct FindingDesc {
    kind: &'static str,
    symbol: String,
    title: String,
    detail: String,
    rule: &'static str,
}

fn derive_findings(path: &str, ex: &Extraction) -> Vec<FindingDesc> {
    let mut out = Vec::new();
    // oversized_file — language-agnostic, loc based.
    if ex.loc > MAX_FILE_LOC {
        out.push(FindingDesc {
            kind: "oversized_file",
            symbol: String::new(),
            title: format!("{path} is oversized"),
            detail: format!("{} lines (> {MAX_FILE_LOC})", ex.loc),
            rule: "max-file-size",
        });
    }
    // complex_symbol — per function over the threshold (source files only).
    if ex.role == Role::Source {
        for s in &ex.symbols {
            if s.complexity > MAX_COMPLEXITY {
                out.push(FindingDesc {
                    kind: "complex_symbol",
                    symbol: s.name.clone(),
                    title: format!("{}::{} is complex", path, s.name),
                    detail: format!("complexity ~{} (> {MAX_COMPLEXITY})", s.complexity),
                    rule: "complex-symbol",
                });
            }
        }
    }
    // panic_marker — production unwrap()/panic! sites (AST-counted; excludes test
    // modules and string/comment text).
    if ex.role == Role::Source && ex.panic_sites > 0 {
        out.push(FindingDesc {
            kind: "panic_marker",
            symbol: String::new(),
            title: format!("{path} panics at a boundary"),
            detail: format!("{} unwrap()/panic! site(s)", ex.panic_sites),
            rule: "no-panic-marker",
        });
    }
    out
}
