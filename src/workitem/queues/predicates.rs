use super::super::rank_lifecycle;
use crate::lane::Lane;
use crate::model::{Edge, EdgeKind, InspectionStatus, Node, NodeType, TargetKind};
use crate::store::Store;
use crate::Result;

/// Why `intent_id` is not ready to build yet, one phrase per blocker.
///
/// Two relations gate readiness, and they are deliberately NOT the same thing:
///
/// * `requires` — this behavior cannot function without the target. That is
///   also the completeness `prerequisites` axis, and the two must keep
///   agreeing: a dependent the scorecard calls unmet is one the build lane
///   must not serve.
/// * `sequence` — this behavior is the step AFTER the target in a flow. It may
///   be perfectly complete as a specification while its predecessor is
///   unbuilt; what it is not is the next thing to build. So sequence gates
///   routing and deliberately does NOT touch the completeness axis —
///   ordering is not incompleteness.
///
/// Both read the same direction as everywhere else in the graph: the FROM side
/// stands on the TO side. A `requires` target must be realized; a `sequence`
/// predecessor remains the deliberately separate lifecycle-only routing rule.
fn unmet_prerequisites(store: &Store, intent_id: &str) -> Result<Vec<String>> {
    let mut unmet = Vec::new();
    for (kind, relation, requires_realization) in [
        (EdgeKind::Requires, "requires", true),
        (EdgeKind::Sequence, "follows", false),
    ] {
        for e in store.edges_with(Some(kind), Some(intent_id), None)? {
            if let Some(target) = store.get_node(&e.to_id)? {
                let met = if requires_realization {
                    crate::completeness::prerequisite_is_realized(store, &target)?
                } else {
                    target.status == "implemented"
                };
                if !met {
                    let state = if requires_realization && target.status == "implemented" {
                        "implemented but ungrounded"
                    } else {
                        target.status.as_str()
                    };
                    unmet.push(format!("{relation} '{}' ({state})", target.name));
                }
            }
        }
    }
    Ok(unmet)
}

/// Implemented leaf intents that carry no realizing grounding. A hierarchy
/// parent is realized through its children, so it is exempt. This is the EXACT
/// predicate the `realized` maturity rung uses for its `ungrounded` count — the
/// single source of truth shared by the ladder, the compass, `queue_counts`, and
/// the build lane, so the compass never routes `build` at work `build_item`
/// would not serve (the invariant in `maturity::ladder`).
pub(crate) fn ungrounded_implemented_intents(store: &Store) -> Result<Vec<Node>> {
    let parents: std::collections::HashSet<String> = store
        .list_edges(Some(EdgeKind::Hierarchy), usize::MAX)?
        .into_iter()
        .map(|e| e.from_id)
        .collect();
    let mut out = Vec::new();
    for n in store.nodes_by_status(NodeType::Intent, &["implemented"])? {
        if parents.contains(&n.id) {
            continue; // roll-up parent — realized via children
        }
        if store.realizing_groundings(&n.id)?.is_empty() {
            out.push(n);
        }
    }
    Ok(out)
}

/// Why this intent is build work: planned/needs_change intents are unwritten;
/// an `implemented` candidate reached the build lane only because it is
/// ungrounded — the realizing code is unlinked (or unwritten), so the move is to
/// add the `implements` edge, not to re-plan it.
fn build_reason(intent: &Node) -> String {
    if intent.status == "implemented" {
        "intent is implemented but ungrounded — add the implements edge to the code that realizes it (or reclassify it)".into()
    } else {
        format!("intent is {} and not yet realized", intent.status)
    }
}

pub(super) fn build_candidates(
    store: &Store,
    include_remainder: bool,
) -> Result<Vec<(Node, String)>> {
    let mut intents = store.nodes_by_status(NodeType::Intent, &["needs_change", "planned"])?;
    // Implemented-but-ungrounded intents block the `realized` rung exactly like
    // planned ones, and the compass routes them to `build`; serve them here (after
    // planned/needs_change work — `rank_lifecycle` sorts `implemented` last) so
    // that routing never lands on an empty queue.
    intents.extend(ungrounded_implemented_intents(store)?);
    // needs_change before planned before ungrounded-implemented; then stable by name.
    intents.sort_by(|a, b| {
        rank_lifecycle(&a.status)
            .cmp(&rank_lifecycle(&b.status))
            .then(a.name.cmp(&b.name))
    });
    // Prerequisite-ready candidates come first; blocked candidates retain their
    // lifecycle order behind them. Blocked candidates are deliberately still
    // served when nothing is ready (a requires cycle, or all deps pending):
    // the singular packet then carries its blocked reason instead of stalling
    // the lane — the driver's move is to build what it stands on or break the
    // cycle, and an empty queue would hide that work entirely.
    let mut ready = Vec::new();
    let mut blocked = Vec::new();
    for intent in intents {
        let unmet = unmet_prerequisites(store, &intent.id)?;
        if unmet.is_empty() {
            let reason = build_reason(&intent);
            ready.push((intent, reason));
            // Singular serving has always stopped at the first ready intent;
            // preserve that query boundary while the roster requests all rows.
            if !include_remainder {
                break;
            }
        } else {
            let reason = format!(
                "blocked: {} — build what it stands on first, or break the cycle",
                unmet.join(", ")
            );
            blocked.push((intent, reason));
        }
    }
    ready.extend(blocked);
    Ok(ready)
}

/// Files that plausibly realize an intent, ranked by how much of the intent's
/// language appears in the SYMBOLS loom extracted from them.
///
/// A fresh intent's packet used to say "survey registered codefiles", which on
/// a real repository means "read seventy paths and guess" — the moment a
/// sidekick is least useful. loom already knows every file's symbol names from
/// extraction, so it can propose candidates instead of a listing command.
///
/// Deliberately weak evidence: these are places to LOOK, never a grounding.
/// Matching `getChannel` to "a channel can be opened" is a hint, and the packet
/// says so — the builder still has to read the code and point the locator.
pub(crate) fn candidate_files(store: &Store, intent: &Node) -> Result<Vec<(String, Vec<String>)>> {
    let terms: Vec<String> = format!("{} {}", intent.name, intent.description)
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 3)
        .map(str::to_string)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let mut scored: Vec<(usize, String, Vec<String>)> = Vec::new();
    for cf in store.codefiles()? {
        // Already grounded to this intent? Then it is not a candidate, it is
        // the answer, and the read set carries it by another route.
        let raw = store.get_facet(&cf.id, TargetKind::Node, "symbol_fingerprints")?;
        let symbols: Vec<String> = raw
            .and_then(|r| {
                serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&r).ok()
            })
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        let mut matched: Vec<String> = Vec::new();
        for sym in &symbols {
            let lower = sym.to_lowercase();
            if terms.iter().any(|t| lower.contains(t.as_str())) {
                matched.push(sym.clone());
            }
        }
        // The path counts for less than a symbol: a directory named `channel`
        // is a weaker signal than a function named `openChannel`.
        let path_hits = terms
            .iter()
            .filter(|t| cf.name.to_lowercase().contains(t.as_str()))
            .count();
        let score = matched.len() * 2 + path_hits;
        if score > 0 {
            matched.sort();
            matched.truncate(4);
            scored.push((score, cf.name.clone(), matched));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.truncate(5);
    Ok(scored.into_iter().map(|(_, f, m)| (f, m)).collect())
}

/// Lane that can perform the named repair of a `needed` finding.
///
/// The serve path, the roster, and the ladder depth all go through this
/// predicate so a packet never names a write its `owner_role` cannot execute
/// (`analyze_serves` is the same pattern for compiler-owned proof edges).
pub(crate) fn needed_finding_repair_lane(node: &Node) -> Lane {
    match finding_kind_name(node) {
        "proof_too_shallow_for_intent" | "missing_journey_proof" | "shared_proof_command" => {
            Lane::Validate
        }
        "undeclared_coupling" => Lane::Analyze,
        _ => Lane::Fix,
    }
}

pub(super) fn needed_findings_for(
    store: &Store,
    lane: Lane,
) -> Result<Vec<crate::signal::FindingView>> {
    Ok(crate::signal::needed_findings(store)?
        .into_iter()
        .filter(|fv| needed_finding_repair_lane(&fv.node) == lane)
        .collect())
}

pub(super) fn open_research_tasks(store: &Store) -> Result<Vec<Node>> {
    let mut tasks: Vec<_> = store
        .list_nodes(Some(NodeType::TaskRecord), usize::MAX)?
        .into_iter()
        .filter(crate::research::is_open_research)
        .collect();
    tasks.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    Ok(tasks)
}

pub(super) fn unseeded_quality_pack(store: &Store) -> Result<Option<String>> {
    let rules = store.list_nodes(Some(NodeType::QualityRule), usize::MAX)?;
    if rules.iter().any(|r| r.status != "deprecated") {
        return Ok(None);
    }
    // With no active intents the measured rung is NotApplicable, not unseeded —
    // there are no boundary expectations yet, so the quality queue is empty.
    let active = store
        .list_nodes(Some(NodeType::Intent), usize::MAX)?
        .into_iter()
        .filter(|n| n.status != "deprecated")
        .count();
    if active == 0 {
        return Ok(None);
    }
    Ok(Some(
        crate::packs::recommended_packs(store.root())
            .first()
            .map(|p| p.to_string())
            .unwrap_or_else(|| "<pack>".to_string()),
    ))
}

/// Never-measured (rule × leaf implemented intent) pairs: every non-deprecated
/// `QualityRule` crossed with every leaf `implemented` intent that has no
/// `governs` edge yet. Roll-up parents (hierarchy `from_id`s) are excluded —
/// they are realized via children and have no code of their own to measure.
/// Scenario children (`scenario-of` sources, or sad/fallback/edge_case aspects)
/// are also excluded — they surround a happy path. This is the SINGLE predicate
/// shared by the work-item picker (`unmeasured_pair_item`), the queue roster
/// (`unmeasured_pair_entries`), the queue count (`queue_counts`), and the
/// maturity ladder (`hardened` rung) — so they can never disagree about what
/// "unmeasured" means.
pub(crate) fn unmeasured_quality_pairs(store: &Store) -> Result<Vec<(Node, Node)>> {
    let governs = store.edges_with(Some(EdgeKind::Governs), None, None)?;
    let measured: std::collections::BTreeSet<(String, String)> =
        governs.into_iter().map(|e| (e.from_id, e.to_id)).collect();
    // Parents in a hierarchy are roll-ups — measure their leaves instead.
    let hierarchy_parents: std::collections::BTreeSet<String> = store
        .edges_with(Some(EdgeKind::Hierarchy), None, None)?
        .into_iter()
        .map(|e| e.from_id)
        .collect();
    let scenario_children: std::collections::BTreeSet<String> = store
        .edges_with(Some(EdgeKind::ScenarioOf), None, None)?
        .into_iter()
        .map(|e| e.from_id)
        .collect();
    let rules = store.list_nodes(Some(NodeType::QualityRule), usize::MAX)?;
    let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?;
    let mut out = Vec::new();
    for rule in rules.into_iter().filter(|r| r.status != "deprecated") {
        for intent in intents.iter().filter(|i| {
            if i.status != "implemented" {
                return false;
            }
            if hierarchy_parents.contains(&i.id) || scenario_children.contains(&i.id) {
                return false;
            }
            // Aspect-only scenarios (no scenario-of edge yet) still aren't
            // independent quality surfaces.
            if let Ok(Some(aspect)) = store.get_facet(&i.id, TargetKind::Node, "aspect") {
                if matches!(aspect.as_str(), "sad" | "fallback" | "edge_case") {
                    return false;
                }
            }
            true
        }) {
            if !measured.contains(&(rule.id.clone(), intent.id.clone())) {
                out.push((rule.clone(), intent.clone()));
            }
        }
    }
    Ok(out)
}

/// One exact unit of Validate work. Compiler-created validations are keyed by
/// their Validation id and retain the profile that must be run; their several
/// `Validates` edges are evidence closure, not several queue items.
#[derive(Debug, Clone)]
pub enum ValidationWorkUnit {
    JourneyValidation {
        validation_id: String,
        journey: Node,
        profile: String,
    },
    GenericEdge(Edge),
    JourneyGap(Node),
    UnprovenIntent(Node),
}

/// The single profile-bearing Validate roster consumed by lane depth, the
pub fn validation_work_units(store: &Store) -> Result<Vec<ValidationWorkUnit>> {
    // The whole compiler-owned closure is scanned, not `validates` alone: an
    // uninspected or staled `calls`/`proves`/`exercises` edge is proof work
    // whose only door is `journey run`, and the analyze lane refuses to serve
    // it. Scanning only `validates` left those edges queued nowhere while
    // Compass still counted the graph incomplete.
    let mut closure: Vec<Edge> = store
        .live_edges_by_status(
            crate::model::TruthClass::Asserted,
            &[
                InspectionStatus::Uninspected,
                InspectionStatus::NeedsReverification,
            ],
        )?
        .into_iter()
        .filter(|edge| {
            matches!(
                edge.kind,
                EdgeKind::Validates | EdgeKind::Proves | EdgeKind::Calls | EdgeKind::Exercises
            )
        })
        .collect();
    closure.sort_by(|left, right| left.id.cmp(&right.id));

    let mut compiled = std::collections::BTreeMap::new();
    let mut generic_unrun = Vec::new();
    let mut generic_stale = Vec::new();
    // ONE readiness walk for the whole function: the compiler-owned gate
    // below and `journey_proof_gaps` read the same snapshot, so a per-edge
    // re-walk cannot stack the CPU-minutes finding 6825299d already paid for.
    let readiness_by_journey: std::collections::BTreeMap<String, _> =
        crate::completeness::all_journey_readiness(store)?
            .into_iter()
            .map(|readiness| (readiness.journey_id.clone(), readiness))
            .collect();
    for edge in closure {
        if let Some((journey, profile)) =
            crate::completeness::compiler_owned_proof_edge(store, &edge)?
        {
            // A Journey that is not compile-ready cannot run, so its packet's
            // only write_back (`journey run` → compile) refuses — the exact
            // deadlock of finding 77eaab45: served forever by this lane,
            // closable by no one. The gate mirrors ALL THREE `compile_source`
            // stage bails (derivation present-and-current, its acceptance
            // ratified, the derived intents implemented) so this lane serves a
            // packet only when `journey run` can actually close it; anything
            // earlier routes back to Derive/Build via the stage predicates.
            let ready = readiness_by_journey
                .get(&journey.id)
                .is_some_and(|readiness| {
                    readiness.derived && readiness.derivations_ratified && readiness.implemented
                });
            if !ready {
                continue;
            }
            compiled
                .entry(edge.from_id.clone())
                .or_insert((journey, profile));
        } else if edge.kind != EdgeKind::Validates {
            // A generic proof-adjacent edge keeps its own lane (analyze); only
            // `validates` is this lane's generic unit.
            continue;
        } else if edge.status == InspectionStatus::Uninspected {
            generic_unrun.push(edge);
        } else {
            generic_stale.push(edge);
        }
    }

    let routed_journeys: std::collections::BTreeSet<_> = compiled
        .values()
        .map(|(journey, _)| journey.id.clone())
        .collect();
    let mut units = compiled
        .into_iter()
        .map(
            |(validation_id, (journey, profile))| ValidationWorkUnit::JourneyValidation {
                validation_id,
                journey,
                profile,
            },
        )
        .collect::<Vec<_>>();
    units.extend(
        generic_unrun
            .into_iter()
            .map(ValidationWorkUnit::GenericEdge),
    );
    units.extend(
        generic_stale
            .into_iter()
            .map(ValidationWorkUnit::GenericEdge),
    );
    for readiness in journey_proof_gaps_with(&readiness_by_journey) {
        if routed_journeys.contains(&readiness.journey_id) {
            continue;
        }
        if let Some(journey) = store.get_node(&readiness.journey_id)? {
            units.push(ValidationWorkUnit::JourneyGap(journey));
        }
    }
    units.extend(
        unproven_implemented_intents(store)?
            .into_iter()
            .map(ValidationWorkUnit::UnprovenIntent),
    );
    Ok(units)
}

/// Journeys that are surfaced but not yet proven: the proof stage of the
/// readiness ladder, entered only after every earlier stage holds. Gating on
/// `derived && derivations_ratified && implemented` keeps this lane from
/// serving a Journey whose `journey compile` would refuse (finding 77eaab45:
/// an unaccepted derivation was served to an autonomous validate lane in a
/// loop no packet could close); those Journeys stay Derive/Build work until
/// their earlier stages close. Takes the already-gathered readiness snapshot
/// keyed by journey id so the caller never walks readiness twice.
pub(crate) fn journey_proof_gaps_with(
    readiness_by_journey: &std::collections::BTreeMap<
        String,
        crate::completeness::JourneyReadiness,
    >,
) -> Vec<crate::completeness::JourneyReadiness> {
    readiness_by_journey
        .values()
        .filter(|journey| {
            journey.surfaced
                && journey.derived
                && journey.derivations_ratified
                && journey.implemented
                && (!journey.compiled || !journey.proven)
        })
        .cloned()
        .collect()
}

/// Implemented LEAF intents with no passing `validates` edge. Hierarchy parents
/// are proven through their children, so they are exempt. Shared by the validate
/// lane and the `proven` rung — one predicate, no drift.
pub(crate) fn unproven_implemented_intents(store: &Store) -> Result<Vec<Node>> {
    let parents: std::collections::HashSet<String> = store
        .list_edges(Some(EdgeKind::Hierarchy), usize::MAX)?
        .into_iter()
        .map(|e| e.from_id)
        .collect();
    let mut out = Vec::new();
    for n in store.nodes_by_status(NodeType::Intent, &["implemented"])? {
        if parents.contains(&n.id) {
            continue;
        }
        // A PASSING proof is not enough — it must establish something. An S1
        // proof means loom ran a command and it exited zero, which is liveness,
        // not behavior. Keep the decision in proofstrength::assess so the queue
        // and completeness scorecard cannot disagree about this floor.
        let proven = crate::proofstrength::assess(store, &n.id)?.meaningful_passing;
        if !proven {
            out.push(n);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn finding_kind_name(node: &crate::model::Node) -> &str {
    node.body
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or(node.status.as_str())
}

/// Structural detectors (size/complexity) need cohesion judgment — never a
/// mechanical "length is intentional" closeout. Smells and inbox stay on the
/// generic triage contract; only these kinds get the cohesion checklist.
pub(super) fn is_structural_size_finding(node: &crate::model::Node) -> bool {
    let kind = finding_kind_name(node);
    matches!(
        kind,
        "oversized_file"
            | "complex_symbol"
            | "large_symbol"
            | "deep_nesting"
            | "excess_args"
            | "panic_marker"
            | "tangled_file"
    )
}

/// Active intents whose wantedness is unestablished: ratification claim
/// absent, `unratified`, or staled to `needs_reconfirmation`. This projects
/// missing authority; it is not itself a human queue. [`ratify_item`] separately
/// requires a concrete evidence/judgment conflict from `next_human_blocking`.
pub fn unratified_intents(store: &Store) -> Result<Vec<Node>> {
    let mut out = Vec::new();
    for n in store.list_nodes(Some(NodeType::Intent), usize::MAX)? {
        if n.status == "deprecated" {
            continue;
        }
        // `ratification` already reports "unratified" when the claim is absent,
        // so there is no missing case to substitute here.
        let state = store.ratification(&n.id)?;
        if state != "ratified" {
            out.push(n);
        }
    }
    Ok(out)
}

/// The first thing the `sound` rung is counting, whatever kind it is.
///
pub(super) fn first_audit_subject(store: &Store) -> Result<Option<crate::audit::AuditFinding>> {
    Ok(audit_subjects(store)?.into_iter().next())
}

/// Everything the `sound` rung counts, as servable findings.
///
pub(super) fn audit_subjects(store: &Store) -> Result<Vec<crate::audit::AuditFinding>> {
    crate::audit::backlog(store)
}

/// Edges the ANALYZE lane owns: everything except the two that have their own
/// measuring lanes (`governs` → quality, `validates` → validate).
/// Whether this edge kind has no measuring lane of its own and is therefore
/// not a claim the ANALYZE lane verifies: `governs`/`validates` have their own
/// lanes, `depends_on` is a federation ripple link, and `exercises` is
/// validation-specific evidence provenance rather than a relationship claim.
/// The `exercises` exclusion holds only while that provenance is intact —
/// `analyze_serves` re-admits it once sync invalidates it.
pub(crate) fn not_measured_lane(e: &Edge) -> bool {
    !matches!(
        e.kind,
        EdgeKind::Governs | EdgeKind::Validates | EdgeKind::DependsOn | EdgeKind::Exercises
    )
}

/// Whether the ANALYZE lane may serve this edge at all. Maturity's
/// relationships rung and the analyze queue both count through this one
/// predicate, so the rung can never advertise a depth the lane will not serve.
///
/// Kind alone is not enough in either direction:
///
/// - A `proves`/`calls`/`exercises` edge out of a compiler-owned Journey
///   Validation is proof topology `journey compile/run` owns, and
///   `require_generic_edge_mutable` refuses the `loom edge verdict` write an
///   analyze packet would name. Those route to the validate lane, which names
///   the run. The serve path and the write guard share one predicate so they
///   cannot drift apart again.
/// - An ordinary `exercises` edge is provenance rather than a claim — until
///   sync invalidates it. A drifted locator stales the edge, which drops the
///   validation below S3 (proof strength counts only current projections), and
///   nothing else can settle it: `validation run` writes verdicts on
///   `validates` edges alone, and `edge set-locator` repairs the facet without
///   re-inspecting. So a STALE ordinary `exercises` edge is analyze work; an
///   uninspected one still counts as current and has nothing to drain.
pub(crate) fn analyze_serves(store: &Store, edge: &Edge) -> Result<bool> {
    if crate::completeness::compiler_owned_proof_edge(store, edge)?.is_some() {
        return Ok(false);
    }
    if edge.kind == EdgeKind::Exercises {
        return Ok(edge.status == InspectionStatus::NeedsReverification);
    }
    Ok(not_measured_lane(edge))
}

/// The first edge the analyze lane may serve, evaluating ownership lazily.
pub(super) fn first_analyzable(store: &Store, edges: Vec<Edge>) -> Result<Option<Edge>> {
    for edge in edges {
        if analyze_serves(store, &edge)? {
            return Ok(Some(edge));
        }
    }
    Ok(None)
}
