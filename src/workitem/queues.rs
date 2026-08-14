//! Queue partition — one candidate work item per lane.
//!
//! Plane: judgment-plane routing (pure reads over the store). Each `*_item`
//! function selects the single next candidate for its lane using the SAME
//! predicates the maturity ladder and completeness scorecard use — the compass
//! must never route a lane at work its queue would not serve. Selection only:
//! nothing here writes to the graph or decides verdicts.

use super::context::{edge_context, node_context};
use super::contracts::{
    analyzer_contract, builder_contract, coverage_contract, derive_contract, elaborator_contract,
    exemplar_contract, fixer_contract, inbox_triage_contract, journey_proof_contract,
    journey_proof_contract_for_profile, prove_contract, quality_contract, quality_contract_body,
    ratify_contract, rectify_contract, research_contract, reviewer_contract,
    structural_finding_triage_contract, surface_contract, triage_contract, unproven_contract,
    validator_contract,
};
use super::{
    axis_for_role, cause_class, effort_for, node_target, rank_lifecycle, LinkedEntity,
    SuggestedRead, Target, TraversalContext, WorkItem,
};
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

pub(super) fn build_item(store: &Store) -> Result<Option<WorkItem>> {
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
    // Serve a prerequisite before the intent that `requires` it: prefer the
    // highest-ranked candidate whose prerequisites are all implemented. If every
    // candidate is blocked (a requires cycle, or all deps still pending), fall
    // back to the top-ranked one carrying a blocked reason — never stall the lane.
    let mut blocked: Option<(Node, String)> = None;
    let mut ready: Option<Node> = None;
    for intent in intents {
        let unmet = unmet_prerequisites(store, &intent.id)?;
        if unmet.is_empty() {
            ready = Some(intent);
            break;
        } else if blocked.is_none() {
            let reason = format!(
                "blocked: {} — build what it stands on first, or break the cycle",
                unmet.join(", ")
            );
            blocked = Some((intent, reason));
        }
    }
    let (intent, reason) = match ready {
        Some(i) => {
            let reason = build_reason(&i);
            (i, reason)
        }
        None => match blocked {
            Some(pair) => pair,
            None => return Ok(None),
        },
    };
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        mode: "build".into(),
        owner_role: "builder".into(),
        effort: "mid".into(),
        routing_hint: super::hint_judgment(),
        reason,
        target: node_target(&intent),
        stale_causes: Vec::new(),
        prompt_contract: builder_contract(&intent),
        context: {
            let mut ctx = node_context(
                store,
                &intent,
                "Understand the behavior and likely implementation files before coding. Any inlined `note` entities are the prior record — adopted PoC/experiment evidence and past decisions — read them first; they say what was already tried and why.",
            )?;
            // Propose where to look. On a repository of any size, "survey the
            // registered codefiles" is not help.
            for (path, symbols) in candidate_files(store, &intent)? {
                let why = if symbols.is_empty() {
                    "candidate — the path echoes this behavior's language; confirm by reading"
                        .into()
                } else {
                    format!(
                        "candidate — defines {}; confirm it PERFORMS the behavior before grounding",
                        symbols.join(", ")
                    )
                };
                ctx.read_set.push(crate::workitem::FileRead {
                    path,
                    locator: None,
                    why,
                });
            }
            ctx
        },
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Implementation.gap(),
        next_step: "after grounding + sync, run `loom status`".into(),
    }))
}

pub(super) fn derive_item(store: &Store) -> Result<Option<WorkItem>> {
    let Some(gap) = crate::completeness::journey_derive_gaps(store)?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let Some(journey) = store.get_node(&gap.journey_id)? else {
        return Ok(None);
    };
    let readiness = crate::completeness::journey_readiness(store, &journey)?;
    let mut context = node_context(
        store,
        &journey,
        "Read the authored Journey, its accepted projections, and the exact gap before proposing a hash-bound derivation manifest.",
    )?;
    if gap.subject_id != journey.id {
        if let Some(subject) = store.get_node(&gap.subject_id)? {
            context.linked_entities.push(LinkedEntity {
                role: gap.kind.clone(),
                kind: subject.node_type.as_str().into(),
                id: subject.id,
                name: subject.name,
                description: Some(subject.description).filter(|d| !d.is_empty()),
                status: Some(subject.status),
                edge_kind: None,
                edge_status: None,
                locator: None,
                facets: None,
            });
        }
    }
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        mode: "derive".into(),
        owner_role: "builder".into(),
        effort: "high".into(),
        routing_hint: super::hint_judgment(),
        reason: gap.detail,
        target: node_target(&journey),
        stale_causes: Vec::new(),
        prompt_contract: derive_contract(&journey, &readiness),
        context,
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Intent.gap(),
        next_step:
            "after the human accepts the current hash-bound derivation manifest, run `loom status`"
                .into(),
    }))
}

pub(super) fn surface_item(store: &Store) -> Result<Option<WorkItem>> {
    let Some(gap) = crate::completeness::journey_surface_gaps(store)?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let Some(journey) = store.get_node(&gap.journey_id)? else {
        return Ok(None);
    };
    let readiness = crate::completeness::journey_readiness(store, &journey)?;
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        mode: "surface".into(),
        owner_role: "builder".into(),
        effort: "high".into(),
        routing_hint: super::hint_judgment(),
        reason: gap.detail,
        target: node_target(&journey),
        stale_causes: Vec::new(),
        prompt_contract: surface_contract(&journey, &readiness),
        context: node_context(
            store,
            &journey,
            "Read the accepted derivations and their realizing code before implementing the structured CLI contract in the target repository.",
        )?,
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Implementation.gap(),
        next_step: "after accepting the current hash-bound surface manifest + sync, run `loom status`; Validate owns proof".into(),
    }))
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

pub(super) fn coverage_item(store: &Store) -> Result<Option<WorkItem>> {
    // The first unowned, non-ignored CodeFile (stable by name). Coverage is
    // grounding truth: either the file belongs to an intent (ground it) or it
    // does not belong in the graph (unregister it, or `loom ignore` it).
    let Some(cf) = crate::coverage::unowned_codefiles(store)?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    // A registered file that no longer exists on disk cannot be read or
    // grounded — do not send a worker to read a ghost. The only honest moves
    // are unregistering it or re-pointing the registration.
    let missing = !store.root().join(&cf.name).exists();
    let (reason, contract) = if missing {
        (
            format!(
                "registered codefile '{}' no longer exists on disk — unregister it (or re-register its successor)",
                cf.name
            ),
            super::contracts::missing_codefile_contract(&cf),
        )
    } else {
        (
            format!("registered codefile '{}' has no owning intent", cf.name),
            coverage_contract(store, &cf)?,
        )
    };
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        mode: "coverage".into(),
        owner_role: "builder".into(),
        effort: "low".into(),
        routing_hint: super::hint_mechanical(),
        reason,
        target: node_target(&cf),
        stale_causes: Vec::new(),
        prompt_contract: contract,
        context: node_context(
            store,
            &cf,
            if missing {
                "The file is gone; decide what its registration should become."
            } else {
                "Decide which intent this file realizes, or whether it should be unregistered."
            },
        )?,
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Implementation.gap(),
        next_step: "after grounding or unregistering + sync, run `loom status`".into(),
    }))
}

pub(super) fn fix_item(store: &Store) -> Result<Option<WorkItem>> {
    // Repair lane, strictly: failing verdicts of every kind (root cause lives
    // in the source). Stale (needs_reverification) claims are remeasurement
    // work and belong to their measuring lanes — governs/validates to
    // quality/validate, everything else to analyze — so a fix packet never
    // carries verdict authority and no edge is ever served by two queues.
    let failing = store.live_edges_by_status(
        crate::model::TruthClass::Asserted,
        &[InspectionStatus::Failing],
    )?;
    if let Some(e) = failing
        .into_iter()
        .find(|edge| edge.kind != EdgeKind::Exemplar)
    {
        return Ok(Some(edge_work(
            store,
            &e,
            "fix",
            "fixer",
            "failing verdict — repair at root cause",
        )?));
    }
    Ok(None)
}

pub(super) fn analyze_item(store: &Store) -> Result<Option<WorkItem>> {
    // Verdict lane for relationship/grounding claims; governs/validates have
    // their own measuring lanes (quality/validate). Stale claims outrank
    // never-inspected ones: a settled truth that broke misleads readers,
    // an uninspected claim only waits.
    let stale = store.live_edges_by_status(
        crate::model::TruthClass::Asserted,
        &[InspectionStatus::NeedsReverification],
    )?;
    // Analyze packets run AS the edge's owning lane (implements/hierarchy/
    // requires → builder, relates → analyzer, …): the re-verdict write is
    // gated by the registry owner, so a packet naming any other role would
    // promise work its lane cannot record — the exact INV-7 rejection a
    // drain worker hit on 2026-07-19. Same rule review_item already applies.
    if let Some(e) = first_analyzable(store, stale)? {
        let owner = crate::registry::spec(e.kind).owner.as_str();
        return Ok(Some(edge_work(
            store,
            &e,
            "analyze",
            owner,
            "dependency changed — re-verify this claim",
        )?));
    }
    if let Some(task) = open_research_tasks(store)?.into_iter().next() {
        return Ok(Some(research_work(store, &task)?));
    }
    let failing_exemplars = store.live_edges_by_status(
        crate::model::TruthClass::Asserted,
        &[InspectionStatus::Failing],
    )?;
    if let Some(e) = failing_exemplars
        .into_iter()
        .find(|edge| edge.kind == EdgeKind::Exemplar)
    {
        return Ok(Some(edge_work(
            store,
            &e,
            "analyze",
            "analyzer",
            "exemplar review failed — inspect or replace this claimed example",
        )?));
    }
    let uninspected = store.live_edges_by_status(
        crate::model::TruthClass::Asserted,
        &[InspectionStatus::Uninspected],
    )?;
    if let Some(e) = first_analyzable(store, uninspected)? {
        let owner = crate::registry::spec(e.kind).owner.as_str();
        return Ok(Some(edge_work(
            store,
            &e,
            "analyze",
            owner,
            "uninspected claim — inspect the code and record a verdict",
        )?));
    }
    Ok(None)
}

fn open_research_tasks(store: &Store) -> Result<Vec<Node>> {
    let mut tasks: Vec<_> = store
        .list_nodes(Some(NodeType::TaskRecord), usize::MAX)?
        .into_iter()
        .filter(crate::research::is_open_research)
        .collect();
    tasks.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    Ok(tasks)
}

fn research_work(store: &Store, task: &Node) -> Result<WorkItem> {
    let body = crate::research::ResearchBody::parse(&task.body)?;
    let target = body
        .target_id
        .as_deref()
        .map(|id| store.get_node(id))
        .transpose()?
        .flatten();
    let target_text = target
        .as_ref()
        .map(|n| format!("{} [{}]", n.name, n.id))
        .unwrap_or_else(|| "none".into());
    Ok(WorkItem {
        packet_id: None, pattern_guidance: None, mode: "analyze".into(), owner_role: "analyzer".into(),
        effort: "mid".into(), routing_hint: super::hint_judgment(),
        reason: "current external knowledge is missing — research before relying on assumptions".into(),
        target: node_target(task), stale_causes: Vec::new(), prompt_contract: research_contract(task),
        context: node_context(store, task, &format!("Answer the bounded external question and preserve actual-page provenance; this context remains advisory. why_external: {} preferred_sources: {} resolved_target_intent: {}", body.why_external, body.preferred_sources.join(", "), target_text))?,
        scorecard: None, truth_gap: crate::truth::TruthAxis::Verdict.gap(),
        next_step: "record every actual page with source-add, then close with an advisory synthesis; do not edit code".into(),
    })
}

pub(super) fn quality_item(store: &Store) -> Result<Option<WorkItem>> {
    // Measurement lane only: uninspected rules are measured, stale verdicts are
    // re-measured. A FAILING quality verdict is repair work and is served by the
    // fix queue — measuring it again would not make the source better.
    // `live_edges_by_status` is the SAME set the `hardened` rung counts: it drops
    // superseded groundings and claims about retired behaviors, so the item the
    // picker serves is always one the depth admitted.
    let governs: Vec<Edge> = store
        .live_edges_by_status(
            crate::model::TruthClass::Asserted,
            &[
                InspectionStatus::Uninspected,
                InspectionStatus::NeedsReverification,
            ],
        )?
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Governs)
        .collect();
    if let Some(e) = governs
        .iter()
        .find(|e| e.status == InspectionStatus::Uninspected)
    {
        return Ok(Some(edge_work(
            store,
            e,
            "quality",
            "quality",
            "unmeasured quality rule",
        )?));
    }
    if let Some(e) = governs
        .iter()
        .find(|e| e.status == InspectionStatus::NeedsReverification)
    {
        return Ok(Some(edge_work(
            store,
            e,
            "quality",
            "quality",
            "quality verdict went stale — a dependency changed; re-measure",
        )?));
    }
    // Fallback: propose the first never-measured (rule × leaf intent) pair.
    // Seeding a pack must create actionable work — a rule nobody is asked to
    // measure is a dead end. Leaves only: roll-up parents have no code of their
    // own to inspect, while scenario children are surroundings rather than
    // independent quality surfaces.
    unmeasured_pair_item(store)
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

fn unmeasured_pair_item(store: &Store) -> Result<Option<WorkItem>> {
    let pairs = unmeasured_quality_pairs(store)?;
    let Some((rule, intent)) = pairs.into_iter().next() else {
        return Ok(None);
    };
    let effort = rule
        .body
        .get("effort")
        .and_then(|v| v.as_str())
        .unwrap_or("mid")
        .to_string();
    let mut context = node_context(
        store,
        &intent,
        "Read the rule's inspection guide, then measure the intent's grounded code against it.",
    )?;
    context.linked_entities.push(LinkedEntity {
        role: "measuring_rule".into(),
        kind: NodeType::QualityRule.as_str().into(),
        id: rule.id.clone(),
        name: rule.name.clone(),
        description: Some(rule.description.clone()).filter(|d| !d.is_empty()),
        status: None,
        edge_kind: None,
        edge_status: None,
        locator: None,
        facets: None,
    });
    context.suggested_reads.push(SuggestedRead {
        reason: "the measuring stick — its inspection guide and examples".into(),
        command: format!("loom rule show {}", rule.id),
    });
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        mode: "quality".into(),
        owner_role: "quality".into(),
        effort,
        routing_hint: super::hint_judgment(),
        reason: format!(
            "rule '{}' has never been measured against '{}' — the verdict creates the governs edge",
            rule.name, intent.name
        ),
        target: Target {
            kind: "rule_intent_pair".into(),
            id: intent.id.clone(),
            name: format!("{} —governs?→ {}", rule.name, intent.name),
            from: Some(rule.name.clone()),
            to: Some(intent.name.clone()),
        },
        stale_causes: Vec::new(),
        prompt_contract: quality_contract_body(
            Some(&rule),
            &format!(
                "'{}' is seeded but unmeasured against this intent",
                rule.name
            ),
            &rule.name,
            &intent.name,
            prescreen_for(store, Some(&rule), &intent.id)?,
        ),
        context,
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Verdict.gap(),
        next_step: "after recording the verdict, run `loom status`".into(),
    }))
}

pub(super) fn validate_item(store: &Store) -> Result<Option<WorkItem>> {
    let Some(unit) = validation_work_units(store)?.into_iter().next() else {
        return Ok(None);
    };
    match unit {
        ValidationWorkUnit::JourneyValidation {
            journey, profile, ..
        } => Ok(Some(WorkItem {
            packet_id: None,
            pattern_guidance: None,
            mode: "validate".into(),
            owner_role: "validator".into(),
            effort: "high".into(),
            routing_hint: super::hint_judgment(),
            reason: format!(
                "compiled Journey '{}' requires its dedicated proof run",
                journey.name
            ),
            target: node_target(&journey),
            stale_causes: Vec::new(),
            prompt_contract: journey_proof_contract_for_profile(&journey, &profile),
            context: node_context(
                store,
                &journey,
                "Run the compiler-owned Journey validation through its exact profile.",
            )?,
            scorecard: None,
            truth_gap: crate::truth::TruthAxis::Proof.gap(),
            next_step: format!(
                "after `loom journey run {} --profile {}`, run `loom status --json`",
                journey.id,
                super::q(&profile)
            ),
        })),
        ValidationWorkUnit::GenericEdge(e) => Ok(Some(edge_work(
            store,
            &e,
            "validate",
            "validator",
            if e.status == InspectionStatus::Uninspected {
                "unrun proof"
            } else {
                "proof went stale — a dependency changed; re-run it"
            },
        )?)),
        ValidationWorkUnit::JourneyGap(journey) => Ok(Some(WorkItem {
            packet_id: None,
            pattern_guidance: None,
            mode: "validate".into(),
            owner_role: "validator".into(),
            effort: "high".into(),
            routing_hint: super::hint_judgment(),
            reason: format!(
                "compiled Journey '{}' lacks a current passing S3 proof through its surfaced CLI",
                journey.name
            ),
            target: node_target(&journey),
            stale_causes: Vec::new(),
            prompt_contract: journey_proof_contract(&journey),
            context: node_context(
                store,
                &journey,
                "Inspect the compiled proof profile and its surfaced CLI call witness before running it.",
            )?,
            scorecard: None,
            truth_gap: crate::truth::TruthAxis::Proof.gap(),
            next_step: "after `loom journey compile <journey> --profile proof` and `loom journey run <journey> --profile proof`, run `loom status`".into(),
        })),
        ValidationWorkUnit::UnprovenIntent(intent) => {
        let proof = crate::proofstrength::assess(store, &intent.id)?;
        let reason = if proof.any_passing && !proof.meaningful_passing {
            let best = proof
                .best_passing_strength
                .unwrap_or(crate::proofstrength::Strength::S0)
                .as_str();
            format!(
                "'{}' has a proof that ran and passed, but it is {best}: liveness only — strengthen it to S2 with an output/content assertion and rerun",
                intent.name
            )
        } else if proof.any_registered {
            format!(
                "'{}' is implemented and has registered proof(s), but none is passing — run them",
                intent.name
            )
        } else {
            format!(
                "'{}' is implemented with no registered proof — an unproven claim is not truth",
                intent.name
            )
        };
        Ok(Some(WorkItem {
            packet_id: None,
            pattern_guidance: None,
            mode: "validate".into(),
            owner_role: "validator".into(),
            effort: "mid".into(),
            routing_hint: super::hint_judgment(),
            reason,
            target: node_target(&intent),
            stale_causes: Vec::new(),
            prompt_contract: unproven_contract(&intent, proof),
            context: node_context(
                store,
                &intent,
                "Read the behavior and its grounded code, then register a proof that would FAIL if the behavior broke.",
            )?,
            scorecard: None,
            truth_gap: crate::truth::TruthAxis::Proof.gap(),
            next_step: "after the proof runs, run `loom status`".into(),
            }))
        }
    }
}

/// One exact unit of Validate work. Compiler-created validations are keyed by
/// their Validation id and retain the profile that must be run; their several
/// `Validates` edges are evidence closure, not several queue items.
#[derive(Debug, Clone)]
pub(crate) enum ValidationWorkUnit {
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
/// lightweight roster, and singular packet selection.
pub(crate) fn validation_work_units(store: &Store) -> Result<Vec<ValidationWorkUnit>> {
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
    for edge in closure {
        if let Some((journey, profile)) =
            crate::completeness::compiler_owned_proof_edge(store, &edge)?
        {
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
    for readiness in journey_proof_gaps(store)? {
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

/// Serve the lowest-confidence recorded verdict for independent re-inspection.
/// Confidence is the coordination channel between capability tiers: a worker
/// records an honest low-confidence verdict, and this queue routes it to a
/// stronger reviewer instead of letting it stand as settled truth. Failing
/// verdicts are excluded — they already route to the fix queue.
pub(super) fn review_item(store: &Store) -> Result<Option<WorkItem>> {
    let floor = crate::policy::load(store)?.review_confidence_floor;
    let mut candidates: Vec<Edge> = store
        .live_edges_by_status(
            crate::model::TruthClass::Asserted,
            &[InspectionStatus::Passing, InspectionStatus::Independent],
        )?
        .into_iter()
        .filter(|e| e.confidence > 0.0 && e.confidence < floor)
        .collect();
    candidates.sort_by(|a, b| {
        a.confidence
            .partial_cmp(&b.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    let Some(edge) = candidates.into_iter().next() else {
        return Ok(None);
    };
    let reason = format!(
        "verdict recorded with confidence {:.2} (< {}) — re-inspect independently",
        edge.confidence, floor
    );
    // Review runs AS the owning lane: the re-record command is gated by the
    // edge's owner (governs → quality, validates → validator, …), so the
    // packet's owner_role must match or the write-back would be rejected
    // under enforced roles. The reviewer mindset is what makes it a review.
    let owner = crate::registry::spec(edge.kind).owner.as_str();
    edge_work(store, &edge, "review", owner, &reason).map(Some)
}

/// Serve the most-incomplete user-visible feature intent for elaboration.
/// Humans hand loom a core idea and systematically forget the surroundings —
/// failure scenarios, prerequisites, boundary expectations, proofs, open
/// product questions. This queue routes exactly that gap: each open axis is
/// closed by an artifact, a recorded waiver, or a question to the human.
pub(super) fn elaborate_item(store: &Store) -> Result<Option<WorkItem>> {
    let cards = crate::completeness::all_scorecards(store)?;
    let Some(card) = cards
        .into_iter()
        .find(|c| c.open > 0 && c.visibility.as_deref() == Some("user_visible"))
    else {
        return Ok(None);
    };
    let Some(intent) = store.get_node(&card.intent_id)? else {
        return Ok(None);
    };
    let open_names: Vec<&str> = card.open_axes().map(|a| a.axis.as_str()).collect();
    let reason = format!(
        "user-visible idea with {} open completeness axis(es): {}",
        card.open,
        open_names.join(", ")
    );
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        mode: "elaborate".into(),
        owner_role: "builder".into(),
        effort: "high".into(),
        routing_hint: super::hint_judgment(),
        reason,
        target: node_target(&intent),
        stale_causes: Vec::new(),
        prompt_contract: elaborator_contract(&intent, &card),
        context: node_context(
            store,
            &intent,
            "Understand the idea and its existing family before growing its surroundings.",
        )?,
        scorecard: Some(serde_json::to_value(&card)?),
        truth_gap: crate::truth::TruthAxis::Intent.gap(),
        next_step: "after every open axis is addressed, run `loom status`".into(),
    }))
}

pub(super) fn prove_item(store: &Store) -> Result<Option<WorkItem>> {
    let hyps = store.nodes_by_status(NodeType::Hypothesis, &["proposed"])?;
    let Some(h) = hyps.into_iter().next() else {
        return Ok(None);
    };
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        mode: "prove".into(),
        owner_role: "analyzer".into(),
        effort: "high".into(),
        routing_hint: super::hint_judgment(),
        reason: "unproven hypothesis — prove or refute the claim against the code".into(),
        target: node_target(&h),
        stale_causes: Vec::new(),
        prompt_contract: prove_contract(&h),
        context: node_context(
            store,
            &h,
            "Inspect the hypothesis target and related evidence before proving or refuting.",
        )?,
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Verdict.gap(),
        next_step: format!(
            "supported → loom hypothesis adopt {} (spawns a planned build intent); refuted → the record stands; then loom status",
            super::q(&h.name)
        ),
    }))
}

/// Structural detectors (size/complexity) need cohesion judgment — never a
/// mechanical "length is intentional" closeout. Smells and inbox stay on the
/// generic triage contract; only these kinds get the cohesion checklist.
fn is_structural_size_finding(node: &crate::model::Node) -> bool {
    let kind = node
        .body
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or(node.status.as_str());
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

/// The ratify queue: human-decision work. An LLM presents this packet, makes an
/// evidence-backed recommendation, waits for the human, then may record the
/// exact answer through the mediated decision path. It never owns the choice.
///
/// Skips rectifiable friction (duplicates / un-escalated discoveries) — those
/// belong to [`rectify_item`]. Served one at a time, ranked kind-first. Plain
/// `loom next` does not interrupt an autonomous loop with a product question;
/// a host requests `--mode ratify` when it has a conversation channel to the human.
pub(super) fn ratify_item(store: &Store) -> Result<Option<WorkItem>> {
    let Some(d) = crate::divergence::next_human_blocking(store)? else {
        return Ok(None);
    };
    let Some(n) = store.get_node(&d.intent_id)? else {
        return Ok(None);
    };
    let reason = format!(
        "{}: '{}' — {}",
        d.kind.as_str().replace('_', " "),
        n.name,
        d.evidence
    );
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        mode: "ratify".into(),
        owner_role: "human".into(),
        effort: "low".into(),
        routing_hint: super::hint_judgment(),
        reason,
        target: node_target(&n),
        stale_causes: Vec::new(),
        prompt_contract: ratify_contract(&n),
        context: node_context(
            store,
            &n,
            "Present this divergence to the human with the evidence already gathered. \
             Recommend one of the structured options, ask through the host, wait, then \
             record the human's answer with the prefilled command.",
        )?,
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Intent.gap(),
        next_step: format!("{} — or — {}", d.ratify_command, d.reject_command),
    }))
}

/// The rectify queue: LLM prep that clears needless ratify friction without
/// deciding wantedness. Plain `loom next` serves this lane so an autonomous
/// loop can shrink the human queue before asking.
pub(super) fn rectify_item(store: &Store) -> Result<Option<WorkItem>> {
    let Some(d) = crate::divergence::next_rectifiable(store)? else {
        return Ok(None);
    };
    let Some(n) = store.get_node(&d.intent_id)? else {
        return Ok(None);
    };
    let kind = d.kind.as_str().replace('_', " ");
    let reason = format!("{kind}: '{}' — {}", n.name, d.evidence);
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        mode: "rectify".into(),
        owner_role: "rectify".into(),
        effort: "low".into(),
        routing_hint: super::hint_judgment(),
        reason,
        target: node_target(&n),
        stale_causes: Vec::new(),
        prompt_contract: rectify_contract(&n, &kind),
        context: node_context(
            store,
            &n,
            "Inspect whether this blocking divergence is false friction. Prefer \
             structural fixes (visibility, scenario_of, retire duplicate). Escalate \
             to human ratify only when wantedness is a real product call.",
        )?,
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Intent.gap(),
        next_step: "after the structural write or escalation, loom status".into(),
    }))
}

pub(super) fn triage_item(store: &Store) -> Result<Option<WorkItem>> {
    let findings = crate::signal::triage_findings(store)?;
    let Some(fv) = findings.into_iter().next() else {
        return inbox_triage_item(store);
    };
    let short = crate::model::short(&fv.node.id);
    // Cohesion evidence from the graph: which intents own the flagged file.
    // Owner-count is a hint for the LLM, not a verdict — a catch-all file can
    // still "own" intents. Graph-shape smells flag no file: remedy is context.
    let is_smell = fv.node.body.get("category").and_then(|v| v.as_str()) == Some("smell");
    let structural = is_structural_size_finding(&fv.node);
    let cohesion = if is_smell {
        format!(" — structural smell; remedy: {}", fv.node.description)
    } else {
        let owners = store.finding_owner_intents(&fv.node.id)?;
        if owners.is_empty() {
            " — flagged file owns no intents (ungrounded; ground or split it)".to_string()
        } else {
            let names: Vec<&str> = owners.iter().take(4).map(|n| n.name.as_str()).collect();
            let more = if owners.len() > 4 {
                format!(", +{} more", owners.len() - 4)
            } else {
                String::new()
            };
            format!(
                " — flagged file owns {} intent(s): {}{}{}",
                owners.len(),
                names.join("; "),
                more,
                if structural {
                    " (owner-count is a hint — read the file; catch-all bags are needed, not justified)"
                } else {
                    ""
                }
            )
        }
    };
    let stale = if fv.stale {
        " — prior verdict is stale (metric worsened or open work's file changed)"
    } else {
        ""
    };
    let reason = format!(
        "adjudicate evidence-backed finding: {}{}{}",
        fv.node.name, stale, cohesion
    );
    let (effort, routing_hint, prompt_contract) = if structural {
        (
            "mid".into(),
            super::hint_judgment(),
            structural_finding_triage_contract(short),
        )
    } else {
        (
            "low".into(),
            super::hint_mechanical(),
            triage_contract(short),
        )
    };
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        mode: "triage".into(),
        owner_role: "analyzer".into(),
        effort,
        routing_hint,
        reason,
        target: node_target(&fv.node),
        stale_causes: Vec::new(),
        prompt_contract,
        context: node_context(
            store,
            &fv.node,
            if structural {
                "Read the flagged file's structure (modules/handlers). Judge one concern vs catch-all before adjudicating."
            } else {
                "Inspect the flagged finding and owning codefile context before adjudicating."
            },
        )?,
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Signal.gap(),
        next_step: "after recording the verdict, run `loom status`".into(),
    }))
}

fn inbox_triage_item(store: &Store) -> Result<Option<WorkItem>> {
    let Some(item) = store
        .list_nodes(Some(NodeType::InboxItem), usize::MAX)?
        .into_iter()
        .find(|n| n.status == "new")
    else {
        return Ok(None);
    };
    let short = crate::model::short(&item.id);
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        mode: "triage".into(),
        owner_role: "analyzer".into(),
        effort: "low".into(),
        routing_hint: super::hint_mechanical(),
        reason: format!("route raw human/external input: '{}'", item.description),
        target: node_target(&item),
        stale_causes: Vec::new(),
        prompt_contract: inbox_triage_contract(short),
        context: node_context(
            store,
            &item,
            "Inspect the inbox item before routing it into graph work.",
        )?,
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Intent.gap(),
        next_step: "after marking or routing the inbox item, run `loom status`".into(),
    }))
}

fn edge_work(store: &Store, edge: &Edge, mode: &str, role: &str, reason: &str) -> Result<WorkItem> {
    let from = store.get_node(&edge.from_id)?;
    let to = store.get_node(&edge.to_id)?;
    let from_name = from.as_ref().map(|n| n.name.clone()).unwrap_or_default();
    let to_name = to.as_ref().map(|n| n.name.clone()).unwrap_or_default();
    let target = Target {
        kind: "edge".into(),
        id: edge.id.clone(),
        name: format!("{} —{}→ {}", from_name, edge.kind, to_name),
        from: Some(from_name.clone()),
        to: Some(to_name.clone()),
    };
    // Sync records why it staled a claim; surface it so the worker can trace
    // root cause without exploratory lookups.
    let stale_causes = store
        .get_facet(&edge.id, crate::model::TargetKind::Edge, "stale_cause")?
        .map(|c| vec![c])
        .unwrap_or_default();
    let contract = match (mode, role) {
        ("review", _) => reviewer_contract(
            edge,
            role,
            &from_name,
            &to_name,
            crate::policy::load(store)?.review_confidence_floor,
        ),
        (_, "fixer") => fixer_contract(
            edge,
            &from_name,
            &to_name,
            crate::completeness::compiler_owned_proof_edge(store, edge)?,
        ),
        (_, "quality") => quality_contract(store, edge, &from_name, &to_name)?,
        (_, "validator") if edge.kind == EdgeKind::Validates => {
            validator_contract(store, edge, &from_name, &to_name)?
        }
        // Every other validator-owned kind (`calls`, `exercises`) is an
        // inspection claim closed by `loom edge verdict`, not by re-running the
        // proof: a validation run settles its `validates` edges and nothing
        // else, so naming it here was a door that never opened. The packet
        // keeps the registry owner, because that is what gates the write.
        (_, "validator") => analyzer_contract(edge, role, &from_name, &to_name),
        _ if edge.kind == EdgeKind::Exemplar => exemplar_contract(edge, &from_name, &to_name),
        _ => analyzer_contract(edge, role, &from_name, &to_name),
    };
    // A rule's authored inspection effort beats the generic status mapping.
    let base_effort = if edge.kind == EdgeKind::Governs {
        from.as_ref()
            .and_then(|r| r.body.get("effort"))
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| effort_for(edge))
    } else {
        effort_for(edge)
    };
    let context = edge_context(
        store,
        edge,
        "Inspect the target edge endpoints and linked code before acting.",
    )?;
    let (effort, routing_hint) = super::refine_effort_and_hint(
        base_effort,
        &stale_causes,
        &contract.write_back,
        &edge.criterion,
        context.read_set.len(),
    );
    Ok(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        mode: mode.into(),
        owner_role: role.into(),
        effort,
        routing_hint,
        reason: reason.into(),
        target,
        stale_causes,
        prompt_contract: contract,
        context,
        scorecard: None,
        truth_gap: axis_for_role(role).gap(),
        // The fixer lane never records verdicts — its loop ends at sync, and
        // the owning lane re-measures. Every other edge role writes a verdict.
        next_step: if role == "fixer" {
            "after the fix + `loom sync`, run `loom status`".into()
        } else {
            "after recording the verdict, run `loom status`".into()
        },
    })
}

/// Machine pre-screen: run the rule's authored regex patterns over the
/// intent's grounded files. Computed on read at packet-build time, never
/// stored — hits are candidates for the LLM to confirm or refute, mirroring
/// how debt clusters are computed rather than persisted.
/// What a pattern pre-screen actually did.
///
/// An empty hit list is ambiguous — it means both "no patterns, nothing ran"
/// and "loom scanned and found nothing". Only the second is evidence, and it is
/// the evidence that answers an ABSENCE rule ("no hardcoded secrets here"), so
/// it has to be distinguishable.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct PreScreen {
    pub ran: bool,
    pub patterns: usize,
    pub files: usize,
    pub hits: Vec<crate::prescan::PreScreenHit>,
    /// Hits a recorded adjudication already answered (`loom rule suppress`).
    /// Filtered out of `hits` so a packet never re-litigates a judged false
    /// positive; counted so the suppression is visible, not silent.
    #[serde(default)]
    pub suppressed: usize,
}

pub(super) fn prescreen_for(
    store: &Store,
    rule: Option<&Node>,
    intent_id: &str,
) -> Result<PreScreen> {
    let Some(rule) = rule else {
        return Ok(PreScreen::default());
    };
    let patterns: Vec<String> = rule
        .body
        .get("patterns")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if patterns.is_empty() {
        return Ok(PreScreen::default());
    }
    let mut files = Vec::new();
    // Pre-screen every realizing file: the grounding-file set is intentionally
    // unbounded, while the retained hit set is bounded by PRESCREEN_HIT_CAP.
    for e in store.realizing_groundings(intent_id)? {
        if let Some(cf) = store.get_node(&e.to_id)? {
            files.push(cf.name);
        }
    }
    if files.is_empty() {
        return Ok(PreScreen::default());
    }
    let hits = crate::prescan::prescreen(
        store.root(),
        &files,
        &patterns,
        crate::runner::PRESCREEN_HIT_CAP,
    )?;
    // Drop hits a recorded adjudication already answered: judged once by
    // content hash, they are never re-served for the same matched text.
    let mut open = Vec::with_capacity(hits.len());
    let mut suppressed = 0usize;
    for h in hits {
        if store.is_hit_suppressed(&rule.name, &h.excerpt)? {
            suppressed += 1;
        } else {
            open.push(h);
        }
    }
    Ok(PreScreen {
        ran: true,
        patterns: patterns.len(),
        files: files.len(),
        hits: open,
        suppressed,
    })
}

// NOTE: `QueueCounts` + `queue_counts` lived here and recomputed, with a second
// set of predicates, what the maturity rungs already counted. They are replaced
// by `crate::lane::QueueDepths`, projected from the single `LadderInputs::gather`
// — one gather, one predicate per lane, no drift.

/// One lightweight row in a queue roster: what/why/effort, without the full
/// prompt contract or traversal context a served work item carries. This is the
/// depth view behind `loom next --mode <m> --all` — enough to page and pick,
/// cheap enough to list a queue that is hundreds deep. To actually WORK an item,
/// `loom next --mode <m>` still compiles the full packet for the top of the
/// queue.
#[derive(Debug, Clone, serde::Serialize)]
pub struct QueueEntry {
    pub mode: String,
    pub effort: String,
    /// `mechanical` | `judgment` — same axis as `WorkItem.routing_hint`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_hint: Option<String>,
    /// `cheap` | `full` | `other` — derived from the edge's stale_cause facet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause_class: Option<String>,
    /// The lane whose write boundary owns this item's write-back (edge rows:
    /// the registry owner). Batch orchestrators MUST partition by this — a
    /// batch recorded under any other lane is rejected by INV-7.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_role: Option<String>,
    pub reason: String,
    pub target: Target,
}

fn edge_entry(store: &Store, edge: &Edge, mode: &str, reason: &str) -> Result<QueueEntry> {
    let from_name = store
        .get_node(&edge.from_id)?
        .map(|n| n.name)
        .unwrap_or_default();
    let to_name = store
        .get_node(&edge.to_id)?
        .map(|n| n.name)
        .unwrap_or_default();
    let stale_causes = store
        .get_facet(&edge.id, crate::model::TargetKind::Edge, "stale_cause")?
        .map(|c| vec![c])
        .unwrap_or_default();
    let class = cause_class(&stale_causes);
    let base = effort_for(edge);
    let (effort, routing_hint) = match class {
        "cheap" => ("low".into(), Some("mechanical".into())),
        "full" => (base, Some("judgment".into())),
        _ => (base, Some("judgment".into())),
    };
    Ok(QueueEntry {
        mode: mode.into(),
        effort,
        routing_hint,
        cause_class: Some(class.into()),
        owner_role: Some(crate::registry::spec(edge.kind).owner.as_str().into()),
        reason: reason.into(),
        target: Target {
            kind: "edge".into(),
            id: edge.id.clone(),
            name: format!("{} —{}→ {}", from_name, edge.kind, to_name),
            from: Some(from_name),
            to: Some(to_name),
        },
    })
}

fn node_entry(mode: &str, effort: &str, node: &Node, reason: String) -> QueueEntry {
    let routing_hint = match mode {
        "elaborate" | "prove" | "derive" | "build" | "surface" => Some("judgment".into()),
        "coverage" => Some("mechanical".into()),
        // Structural size/complexity findings need cohesion judgment; inbox and
        // generic findings stay mechanical.
        "triage" if is_structural_size_finding(node) => Some("judgment".into()),
        "triage" => Some("mechanical".into()),
        _ => Some("judgment".into()),
    };
    // Node rows: the mode's serving lane (mirrors each *_item's owner_role).
    let owner_role = match mode {
        "derive" | "build" | "surface" | "coverage" | "prove" | "elaborate" => {
            Some("builder".into())
        }
        "triage" => Some("analyzer".into()),
        "rectify" => Some("rectify".into()),
        "ratify" => Some("human".into()),
        _ => None,
    };
    QueueEntry {
        mode: mode.into(),
        effort: effort.into(),
        routing_hint,
        cause_class: None,
        owner_role,
        reason,
        target: node_target(node),
    }
}

/// The FULL roster of one queue: every item the lane would serve, in the exact
/// order it serves them — entry 0 is what `loom next --mode <m>` returns. Entries
/// are lightweight (see `QueueEntry`). Observed graphs disable the
/// build/fix/coverage/elaborate lanes, so those roster empty here too, matching
/// `next` and `queue_counts`.
pub fn queue_items(store: &Store, lane: crate::lane::Lane) -> Result<Vec<QueueEntry>> {
    use crate::lane::Lane;

    if lane.observed_disabled() && store.identity()?.observed {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    match lane {
        Lane::Fix => roster_fix(store, &mut out)?,
        Lane::Derive => roster_derive(store, &mut out)?,
        Lane::Analyze => roster_analyze(store, &mut out)?,
        Lane::Validate => roster_validate(store, &mut out)?,
        Lane::Quality => roster_quality(store, &mut out)?,
        Lane::Build => roster_build(store, &mut out)?,
        Lane::Surface => roster_surface(store, &mut out)?,
        Lane::Coverage => roster_coverage(store, &mut out)?,
        Lane::Prove => roster_prove(store, &mut out)?,
        Lane::Triage => roster_triage(store, &mut out)?,
        Lane::Review => roster_review(store, &mut out)?,
        Lane::Elaborate => roster_elaborate(store, &mut out)?,
        Lane::Rectify => roster_rectify(store, &mut out)?,
        Lane::Divergence => roster_divergence(store, &mut out)?,
        Lane::Audit => roster_audit(store, &mut out)?,
        // Lanes that route to a whole-graph command instead of a per-item
        // roster (`loom door`, `loom export`).
        Lane::Seed | Lane::Export | Lane::Deepen => {}
    }
    Ok(out)
}

/// Every never-measured (rule × leaf implemented intent) pair as a roster row —
/// the enumerated form of `unmeasured_pair_item`'s single pick.
fn unmeasured_pair_entries(store: &Store) -> Result<Vec<QueueEntry>> {
    let pairs = unmeasured_quality_pairs(store)?;
    let mut out = Vec::new();
    for (rule, intent) in &pairs {
        let effort = rule
            .body
            .get("effort")
            .and_then(|v| v.as_str())
            .unwrap_or("mid")
            .to_string();
        out.push(QueueEntry {
            mode: "quality".into(),
            effort,
            routing_hint: Some("judgment".into()),
            cause_class: None,
            owner_role: Some("quality".into()),
            reason: format!(
                "rule '{}' has never been measured against '{}' — the verdict creates the governs edge",
                rule.name, intent.name
            ),
            target: Target {
                kind: "rule_intent_pair".into(),
                id: intent.id.clone(),
                name: format!("{} —governs?→ {}", rule.name, intent.name),
                from: Some(rule.name.clone()),
                to: Some(intent.name.clone()),
            },
        });
    }
    Ok(out)
}

/// The deepen queue: what to strengthen next, one move at a time.
///
/// Serves the top-ranked candidate only. The ranking re-orders after every
/// change — including the change this packet asks for, which lowers its own
/// candidate's score — so handing out a batch would hand out a list that is
/// stale by the second item.
pub(super) fn deepen_item(store: &Store) -> Result<Option<WorkItem>> {
    let Some(c) = crate::risk::rank(store)?.into_iter().next() else {
        return Ok(None);
    };
    let Some(n) = store.get_node(&c.intent_id)? else {
        return Ok(None);
    };
    let short = crate::model::short(&n.id);
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        mode: "deepen".into(),
        owner_role: "validator".into(),
        effort: "mid".into(),
        routing_hint: super::hint_judgment(),
        reason: format!("'{}' is at {} — {}", n.name, c.proof_strength, c.why),
        target: node_target(&n),
        stale_causes: Vec::new(),
        prompt_contract: super::contracts::deepen_contract(short, &n.name, c.next_move.as_str()),
        context: node_context(
            store,
            &n,
            "This behavior is already green. Find the weakest thing holding it up.",
        )?,
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Risk.gap(),
        next_step: "make ONE move, then re-run `loom deepen` — the ranking will have changed"
            .into(),
    }))
}

/// The audit queue: this graph's own record, where it does not look earned.
/// The audit queue.
///
/// Status and this queue read the same actionable self-audit backlog.
pub(super) fn audit_item(store: &Store) -> Result<Option<WorkItem>> {
    let Some(f) = first_audit_subject(store)? else {
        return Ok(None);
    };
    let (target, context) = match &f.subject {
        crate::audit::AuditSubject::Node(id) => {
            let n = store
                .get_node(id)?
                .ok_or_else(|| anyhow::anyhow!("audit node subject '{id}' is absent"))?;
            (
                node_target(&n),
                node_context(
                    store,
                    &n,
                    "Establish what actually happened before changing anything.",
                )?,
            )
        }
        crate::audit::AuditSubject::Edge(id) => {
            let e = store
                .get_edge(id)?
                .ok_or_else(|| anyhow::anyhow!("audit edge subject '{id}' is absent"))?;
            (
                Target {
                    kind: "edge".into(),
                    id: e.id.clone(),
                    name: e.kind.to_string(),
                    from: Some(e.from_id.clone()),
                    to: Some(e.to_id.clone()),
                },
                edge_context(
                    store,
                    &e,
                    "Establish what actually happened before changing anything.",
                )?,
            )
        }
        crate::audit::AuditSubject::Graph(id) => (
            Target {
                kind: "graph".into(),
                id: id.clone(),
                name: f.kind.to_string(),
                from: None,
                to: None,
            },
            TraversalContext {
                purpose: "Establish what actually happened before changing anything.".into(),
                linked_entities: Vec::new(),
                suggested_reads: Vec::new(),
                read_set: Vec::new(),
            },
        ),
    };
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        mode: "audit".into(),
        owner_role: "analyzer".into(),
        effort: "mid".into(),
        routing_hint: super::hint_judgment(),
        reason: format!("{}: {}", f.kind, f.detail),
        target,
        stale_causes: Vec::new(),
        prompt_contract: super::contracts::audit_contract(&f.remedy),
        context,
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Signal.gap(),
        next_step: f.remedy.clone(),
    }))
}

pub(crate) fn journey_proof_gaps(
    store: &Store,
) -> Result<Vec<crate::completeness::JourneyReadiness>> {
    Ok(crate::completeness::all_journey_readiness(store)?
        .into_iter()
        .filter(|journey| journey.surfaced && (!journey.compiled || !journey.proven))
        .collect())
}

/// The first thing the `sound` rung is counting, whatever kind it is.
///
fn first_audit_subject(store: &Store) -> Result<Option<crate::audit::AuditFinding>> {
    Ok(audit_subjects(store)?.into_iter().next())
}

/// Everything the `sound` rung counts, as servable findings.
///
fn audit_subjects(store: &Store) -> Result<Vec<crate::audit::AuditFinding>> {
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
fn first_analyzable(store: &Store, edges: Vec<Edge>) -> Result<Option<Edge>> {
    for edge in edges {
        if analyze_serves(store, &edge)? {
            return Ok(Some(edge));
        }
    }
    Ok(None)
}

/// The `fix` lane's roster. One arm per lane, each its own function:
/// `queue_items` was a 297-line match that only dispatched, so every lane's
/// enumeration lived inside one symbol loom scored at complexity 32.
fn roster_fix(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    use crate::model::TruthClass;
    for e in store
        .live_edges_by_status(TruthClass::Asserted, &[InspectionStatus::Failing])?
        .into_iter()
        .filter(|edge| edge.kind != EdgeKind::Exemplar)
    {
        out.push(edge_entry(
            store,
            &e,
            "fix",
            "failing verdict — repair at root cause",
        )?);
    }
    Ok(())
}

/// The `analyze` lane's roster. One arm per lane, each its own function:
/// `queue_items` was a 297-line match that only dispatched, so every lane's
/// enumeration lived inside one symbol loom scored at complexity 32.
fn roster_analyze(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    use crate::model::TruthClass;
    for e in store.live_edges_by_status(
        TruthClass::Asserted,
        &[InspectionStatus::NeedsReverification],
    )? {
        if !analyze_serves(store, &e)? {
            continue;
        }
        out.push(edge_entry(
            store,
            &e,
            "analyze",
            "dependency changed — re-verify this claim",
        )?);
    }
    for task in open_research_tasks(store)? {
        out.push(node_entry(
            "analyze",
            "mid",
            &task,
            "current external knowledge is missing — research before relying on assumptions".into(),
        ));
    }
    for e in store
        .live_edges_by_status(TruthClass::Asserted, &[InspectionStatus::Failing])?
        .into_iter()
        .filter(|edge| edge.kind == EdgeKind::Exemplar)
    {
        out.push(edge_entry(
            store,
            &e,
            "analyze",
            "exemplar review failed — inspect or replace this claimed example",
        )?);
    }
    for e in store.live_edges_by_status(TruthClass::Asserted, &[InspectionStatus::Uninspected])? {
        if !analyze_serves(store, &e)? {
            continue;
        }
        out.push(edge_entry(
            store,
            &e,
            "analyze",
            "uninspected claim — inspect the code and record a verdict",
        )?);
    }
    Ok(())
}

/// The `validate` lane's roster. One arm per lane, each its own function:
/// `queue_items` was a 297-line match that only dispatched, so every lane's
/// enumeration lived inside one symbol loom scored at complexity 32.
fn roster_validate(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    for unit in validation_work_units(store)? {
        match unit {
            ValidationWorkUnit::JourneyValidation {
                validation_id,
                journey,
                profile,
            } => out.push(node_entry(
                "validate",
                "high",
                &journey,
                format!(
                    "compiler-owned Validation '{}' requires `loom journey run '{}' --profile '{}'`",
                    validation_id, journey.id, profile
                ),
            )),
            ValidationWorkUnit::GenericEdge(edge) => {
                let reason = if edge.status == InspectionStatus::Uninspected {
                    "unrun proof"
                } else {
                    "proof went stale — a dependency changed; re-run it"
                };
                out.push(edge_entry(store, &edge, "validate", reason)?);
            }
            ValidationWorkUnit::JourneyGap(journey) => out.push(node_entry(
                "validate",
                "high",
                &journey,
                "compiled Journey lacks a current passing S3 proof through its surfaced CLI".into(),
            )),
            ValidationWorkUnit::UnprovenIntent(intent) => {
                let proof = crate::proofstrength::assess(store, &intent.id)?;
                let reason = if proof.any_passing && !proof.meaningful_passing {
                    let best = proof
                        .best_passing_strength
                        .unwrap_or(crate::proofstrength::Strength::S0)
                        .as_str();
                    format!(
                        "implemented; passing proof is {best} (liveness only) — strengthen to S2 with an output/content assertion and rerun"
                    )
                } else {
                    "implemented, with no proof that establishes the behavior".into()
                };
                out.push(node_entry("validate", "mid", &intent, reason));
            }
        }
    }
    Ok(())
}

/// The `quality` lane's roster. One arm per lane, each its own function:
/// `queue_items` was a 297-line match that only dispatched, so every lane's
/// enumeration lived inside one symbol loom scored at complexity 32.
fn roster_quality(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    use crate::model::TruthClass;
    // `live_edges_by_status` is what the DEPTH counts: it drops
    // superseded groundings and claims about retired behaviors. Reading
    // raw edges here re-admitted exactly what the rung had excluded, so
    // the roster ran longer than the number beside it.
    let governs: Vec<Edge> = store
        .live_edges_by_status(
            TruthClass::Asserted,
            &[
                InspectionStatus::Uninspected,
                InspectionStatus::NeedsReverification,
            ],
        )?
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Governs)
        .collect();
    for e in governs
        .iter()
        .filter(|e| e.status == InspectionStatus::Uninspected)
    {
        out.push(edge_entry(store, e, "quality", "unmeasured quality rule")?);
    }
    for e in governs
        .iter()
        .filter(|e| e.status == InspectionStatus::NeedsReverification)
    {
        out.push(edge_entry(
            store,
            e,
            "quality",
            "quality verdict went stale — a dependency changed; re-measure",
        )?);
    }
    out.extend(unmeasured_pair_entries(store)?);
    Ok(())
}

/// The `build` lane's roster. One arm per lane, each its own function:
/// `queue_items` was a 297-line match that only dispatched, so every lane's
/// enumeration lived inside one symbol loom scored at complexity 32.
fn roster_build(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    let mut intents = store.nodes_by_status(NodeType::Intent, &["needs_change", "planned"])?;
    intents.extend(ungrounded_implemented_intents(store)?);
    intents.sort_by(|a, b| {
        rank_lifecycle(&a.status)
            .cmp(&rank_lifecycle(&b.status))
            .then(a.name.cmp(&b.name))
    });
    // Prerequisite-ready candidates first (entry 0 is what the lane
    // serves), then blocked ones carrying their blocked reason.
    let mut blocked = Vec::new();
    for intent in &intents {
        let unmet = unmet_prerequisites(store, &intent.id)?;
        if unmet.is_empty() {
            out.push(node_entry("build", "mid", intent, build_reason(intent)));
        } else {
            blocked.push(node_entry(
                "build",
                "mid",
                intent,
                format!(
                    "blocked: {} — build what it stands on first, or break the cycle",
                    unmet.join(", ")
                ),
            ));
        }
    }
    out.extend(blocked);
    Ok(())
}

fn roster_derive(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    for gap in crate::completeness::journey_derive_gaps(store)? {
        let Some(journey) = store.get_node(&gap.journey_id)? else {
            continue;
        };
        out.push(node_entry("derive", "high", &journey, gap.detail));
    }
    Ok(())
}

fn roster_surface(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    for gap in crate::completeness::journey_surface_gaps(store)? {
        let Some(journey) = store.get_node(&gap.journey_id)? else {
            continue;
        };
        out.push(node_entry("surface", "high", &journey, gap.detail));
    }
    Ok(())
}

/// The `coverage` lane's roster. One arm per lane, each its own function:
/// `queue_items` was a 297-line match that only dispatched, so every lane's
/// enumeration lived inside one symbol loom scored at complexity 32.
fn roster_coverage(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    for cf in crate::coverage::unowned_codefiles(store)? {
        let missing = !store.root().join(&cf.name).exists();
        let reason = if missing {
            "no longer exists on disk — unregister it (or re-register its successor)"
        } else {
            "no owning intent — ground it, or unregister the file"
        };
        out.push(node_entry("coverage", "low", &cf, reason.into()));
    }
    Ok(())
}

/// The `prove` lane's roster. One arm per lane, each its own function:
/// `queue_items` was a 297-line match that only dispatched, so every lane's
/// enumeration lived inside one symbol loom scored at complexity 32.
fn roster_prove(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    for h in store.nodes_by_status(NodeType::Hypothesis, &["proposed"])? {
        out.push(node_entry(
            "prove",
            "high",
            &h,
            "unproven hypothesis — prove or refute the claim against the code".into(),
        ));
    }
    Ok(())
}

/// The `triage` lane's roster. One arm per lane, each its own function:
/// `queue_items` was a 297-line match that only dispatched, so every lane's
/// enumeration lived inside one symbol loom scored at complexity 32.
fn roster_triage(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    for fv in crate::signal::triage_findings(store)? {
        let structural = is_structural_size_finding(&fv.node);
        let reason = if fv.stale {
            "stale evidence-backed finding — re-adjudicate it"
        } else if structural {
            "adjudicate structural finding — judge cohesion, not length"
        } else {
            "adjudicate evidence-backed finding"
        };
        let effort = if structural { "mid" } else { "low" };
        out.push(node_entry("triage", effort, &fv.node, reason.into()));
    }
    for item in store
        .list_nodes(Some(NodeType::InboxItem), usize::MAX)?
        .into_iter()
        .filter(|n| n.status == "new")
    {
        out.push(node_entry(
            "triage",
            "low",
            &item,
            "route raw human/external input".into(),
        ));
    }
    Ok(())
}

/// The `review` lane's roster. One arm per lane, each its own function:
/// `queue_items` was a 297-line match that only dispatched, so every lane's
/// enumeration lived inside one symbol loom scored at complexity 32.
fn roster_review(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    use crate::model::TruthClass;
    let floor = crate::policy::load(store)?.review_confidence_floor;
    let mut candidates: Vec<Edge> = store
        .live_edges_by_status(
            TruthClass::Asserted,
            &[InspectionStatus::Passing, InspectionStatus::Independent],
        )?
        .into_iter()
        .filter(|e| e.confidence > 0.0 && e.confidence < floor)
        .collect();
    candidates.sort_by(|a, b| {
        a.confidence
            .partial_cmp(&b.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    for e in &candidates {
        let reason = format!(
            "verdict recorded with confidence {:.2} (< {}) — re-inspect independently",
            e.confidence, floor
        );
        out.push(edge_entry(store, e, "review", &reason)?);
    }
    Ok(())
}

/// The `elaborate` lane's roster. One arm per lane, each its own function:
/// `queue_items` was a 297-line match that only dispatched, so every lane's
/// enumeration lived inside one symbol loom scored at complexity 32.
fn roster_elaborate(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    for card in crate::completeness::all_scorecards(store)?
        .into_iter()
        .filter(|c| c.open > 0 && c.visibility.as_deref() == Some("user_visible"))
    {
        let Some(intent) = store.get_node(&card.intent_id)? else {
            continue;
        };
        let open_names: Vec<&str> = card.open_axes().map(|a| a.axis.as_str()).collect();
        let reason = format!(
            "user-visible idea with {} open completeness axis(es): {}",
            card.open,
            open_names.join(", ")
        );
        out.push(node_entry("elaborate", "high", &intent, reason));
    }
    Ok(())
}

/// The `divergence` lane's roster. One arm per lane, each its own function:
/// `queue_items` was a 297-line match that only dispatched, so every lane's
/// enumeration lived inside one symbol loom scored at complexity 32.
fn roster_divergence(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    for d in crate::divergence::all(store)? {
        if !d.blocking || crate::divergence::is_rectifiable(store, &d)? {
            continue;
        }
        let Some(n) = store.get_node(&d.intent_id)? else {
            continue;
        };
        out.push(node_entry(
            "ratify",
            "low",
            &n,
            format!("{}: {}", d.kind.as_str().replace('_', " "), d.evidence),
        ));
    }
    Ok(())
}

fn roster_rectify(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    for d in crate::divergence::all(store)? {
        if !crate::divergence::is_rectifiable(store, &d)? {
            continue;
        }
        let Some(n) = store.get_node(&d.intent_id)? else {
            continue;
        };
        out.push(node_entry(
            "rectify",
            "low",
            &n,
            format!("{}: {}", d.kind.as_str().replace('_', " "), d.evidence),
        ));
    }
    Ok(())
}

/// The `audit` lane's roster. One arm per lane, each its own function:
/// `queue_items` was a 297-line match that only dispatched, so every lane's
/// enumeration lived inside one symbol loom scored at complexity 32.
fn roster_audit(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    for f in audit_subjects(store)? {
        match &f.subject {
            crate::audit::AuditSubject::Node(id) => {
                let n = store
                    .get_node(id)?
                    .ok_or_else(|| anyhow::anyhow!("audit node subject '{id}' is absent"))?;
                out.push(node_entry("audit", "mid", &n, f.detail.clone()));
            }
            crate::audit::AuditSubject::Edge(id) => {
                let e = store
                    .get_edge(id)?
                    .ok_or_else(|| anyhow::anyhow!("audit edge subject '{id}' is absent"))?;
                out.push(edge_entry(store, &e, "audit", &f.detail)?);
            }
            crate::audit::AuditSubject::Graph(id) => out.push(QueueEntry {
                mode: "audit".into(),
                effort: "mid".into(),
                routing_hint: Some("judgment".into()),
                cause_class: None,
                owner_role: Some("analyzer".into()),
                reason: f.detail.clone(),
                target: Target {
                    kind: "graph".into(),
                    id: id.clone(),
                    name: f.kind.to_string(),
                    from: None,
                    to: None,
                },
            }),
        }
    }
    Ok(())
}
