//! Queue partition — one candidate work item per lane.
//!
//! Plane: judgment-plane routing (pure reads over the store). Each `*_item`
//! function selects the single next candidate for its lane using the SAME
//! predicates the maturity ladder and completeness scorecard use — the compass
//! must never route a lane at work its queue would not serve. Selection only:
//! nothing here writes to the graph or decides verdicts.

use super::context::{edge_context, node_context};
use super::contracts::{
    analyzer_contract, builder_contract, coverage_contract, elaborator_contract, fixer_contract,
    inbox_triage_contract, prove_contract, quality_contract, quality_contract_body,
    reviewer_contract, structural_finding_triage_contract, triage_contract, validator_contract,
};
use super::{
    axis_for_role, cause_class, effort_for, node_target, rank_lifecycle, LinkedEntity,
    SuggestedRead, Target, WorkItem,
};
use crate::model::{Edge, EdgeKind, InspectionStatus, Node, NodeType, TargetKind};
use crate::store::Store;
use crate::Result;

/// Prerequisites (`requires` targets) of `intent_id` that are not yet realized.
/// Matches the completeness prerequisites axis exactly — a target is unmet
/// unless its status is `implemented` — so the build lane and the scorecard
/// agree on when a dependent is ready to build.
fn unmet_prerequisites(store: &Store, intent_id: &str) -> Result<Vec<String>> {
    let mut unmet = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Requires), Some(intent_id), None)? {
        if let Some(target) = store.get_node(&e.to_id)? {
            if target.status != "implemented" {
                unmet.push(format!("'{}' ({})", target.name, target.status));
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
                "blocked: requires {} — build the prerequisite(s) first, or break the requires cycle",
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
        mode: "build".into(),
        owner_role: "builder".into(),
        effort: "mid".into(),
        routing_hint: super::hint_judgment(),
        reason,
        target: node_target(&intent),
        stale_causes: Vec::new(),
        prompt_contract: builder_contract(&intent),
        context: node_context(
            store,
            &intent,
            "Understand the behavior and likely implementation files before coding. Any inlined `note` entities are the prior record — adopted PoC/experiment evidence and past decisions — read them first; they say what was already tried and why.",
        )?,
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Implementation.gap(),
        next_step: "after grounding + sync, run `loom status`".into(),
    }))
}

pub(super) fn coverage_item(store: &Store) -> Result<Option<WorkItem>> {
    // The first unowned, non-ignored CodeFile (stable by name). Coverage is
    // grounding truth: either the file belongs to an intent (ground it) or it
    // does not belong in the graph (unregister it, or `loom ignore` it).
    let Some(cf) = crate::commands::unowned_codefiles(store)?
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
            coverage_contract(&cf),
        )
    };
    Ok(Some(WorkItem {
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
    if let Some(e) = failing.into_iter().next() {
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
    if let Some(e) = stale
        .into_iter()
        .find(|e| !matches!(e.kind, EdgeKind::Governs | EdgeKind::Validates))
    {
        return Ok(Some(edge_work(
            store,
            &e,
            "analyze",
            "analyzer",
            "dependency changed — re-verify this claim",
        )?));
    }
    let uninspected = store.live_edges_by_status(
        crate::model::TruthClass::Asserted,
        &[InspectionStatus::Uninspected],
    )?;
    if let Some(e) = uninspected
        .into_iter()
        .find(|e| !matches!(e.kind, EdgeKind::Governs | EdgeKind::Validates))
    {
        return Ok(Some(edge_work(
            store,
            &e,
            "analyze",
            "analyzer",
            "uninspected claim — inspect the code and record a verdict",
        )?));
    }
    Ok(None)
}

pub(super) fn quality_item(store: &Store) -> Result<Option<WorkItem>> {
    // Measurement lane only: uninspected rules are measured, stale verdicts are
    // re-measured. A FAILING quality verdict is repair work and is served by the
    // fix queue — measuring it again would not make the source better.
    let governs = store.edges_with(Some(EdgeKind::Governs), None, None)?;
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
    });
    context.suggested_reads.push(SuggestedRead {
        reason: "the measuring stick — its inspection guide and examples".into(),
        command: format!("loom rule show {}", rule.id),
    });
    Ok(Some(WorkItem {
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
    // Proof lane only: unrun and stale proofs are (re-)run here. A FAILING
    // proof means the code is broken — that is fix-queue repair work.
    let validates = store.edges_with(Some(EdgeKind::Validates), None, None)?;
    if let Some(e) = validates
        .iter()
        .find(|e| e.status == InspectionStatus::Uninspected)
    {
        return Ok(Some(edge_work(
            store,
            e,
            "validate",
            "validator",
            "unrun proof",
        )?));
    }
    if let Some(e) = validates
        .iter()
        .find(|e| e.status == InspectionStatus::NeedsReverification)
    {
        return Ok(Some(edge_work(
            store,
            e,
            "validate",
            "validator",
            "proof went stale — a dependency changed; re-run it",
        )?));
    }
    Ok(None)
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

pub(super) fn triage_item(store: &Store) -> Result<Option<WorkItem>> {
    let findings = crate::signal::triage_findings(store)?;
    let Some(fv) = findings.into_iter().next() else {
        return inbox_triage_item(store);
    };
    let short = &fv.node.id[..8.min(fv.node.id.len())];
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
    let short = &item.id[..8.min(item.id.len())];
    Ok(Some(WorkItem {
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
        (_, "fixer") => fixer_contract(edge, &from_name, &to_name),
        (_, "quality") => quality_contract(store, edge, &from_name, &to_name)?,
        (_, "validator") => validator_contract(store, edge, &from_name, &to_name)?,
        _ => analyzer_contract(edge, &from_name, &to_name),
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
pub(super) fn prescreen_for(
    store: &Store,
    rule: Option<&Node>,
    intent_id: &str,
) -> Result<Vec<crate::prescan::PreScreenHit>> {
    let Some(rule) = rule else {
        return Ok(Vec::new());
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
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    // Pre-screen every realizing file. A cap here would let the quality packet
    // omit a violation solely because its file sorted after the eighth edge.
    for e in store.realizing_groundings(intent_id)? {
        if let Some(cf) = store.get_node(&e.to_id)? {
            files.push(cf.name);
        }
    }
    if files.is_empty() {
        return Ok(Vec::new());
    }
    crate::prescan::prescreen(store.root(), &files, &patterns, 20)
}

/// Per-queue backlog counts mirroring the EXACT serving partition of each
/// queue above — the status surface reads this so it can never disagree with
/// what `loom next --mode <m>` would actually serve.
#[derive(Debug, Clone, serde::Serialize)]
pub struct QueueCounts {
    pub build: usize,
    pub coverage: usize,
    /// failing (any kind) + stale relationship/grounding claims.
    pub fix: usize,
    /// uninspected relationship/grounding claims.
    pub analyze: usize,
    /// open governs edges (uninspected + stale) plus never-measured
    /// rule×root-intent pairs.
    pub quality: usize,
    /// open validates edges (unrun + stale).
    pub validate: usize,
    /// recorded verdicts below the review confidence floor.
    pub review: usize,
    /// unjudged/stale findings + new inbox items.
    pub triage: usize,
    /// proposed hypotheses awaiting proof.
    pub prove: usize,
    /// user-visible feature intents with open completeness axes.
    pub elaborate: usize,
}

pub fn queue_counts(store: &Store) -> Result<QueueCounts> {
    use crate::model::TruthClass;
    // An observed graph disables the build/fix/coverage/elaborate lanes in
    // `next`; the counts MUST mirror that or `status` disagrees with what the
    // queues actually serve (H-12).
    let observed = store.identity()?.observed;
    let failing = store
        .live_edges_by_status(TruthClass::Asserted, &[InspectionStatus::Failing])?
        .len();
    let stale = store.live_edges_by_status(
        TruthClass::Asserted,
        &[InspectionStatus::NeedsReverification],
    )?;
    let uninspected =
        store.live_edges_by_status(TruthClass::Asserted, &[InspectionStatus::Uninspected])?;
    let split = |edges: &[Edge]| -> (usize, usize, usize) {
        let governs = edges.iter().filter(|e| e.kind == EdgeKind::Governs).count();
        let validates = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Validates)
            .count();
        (edges.len() - governs - validates, governs, validates)
    };
    let (stale_rel, stale_gov, stale_val) = split(&stale);
    let (unin_rel, unin_gov, unin_val) = split(&uninspected);

    // Never-measured rule × root implemented intent pairs — shared predicate.
    let pairs = unmeasured_quality_pairs(store)?.len();

    let floor = crate::policy::load(store)?.review_confidence_floor;
    let review = store
        .live_edges_by_status(
            TruthClass::Asserted,
            &[InspectionStatus::Passing, InspectionStatus::Independent],
        )?
        .into_iter()
        .filter(|e| e.confidence > 0.0 && e.confidence < floor)
        .count();
    let findings = crate::signal::triage_findings(store)?.len();
    let inbox_new = store
        .list_nodes(Some(NodeType::InboxItem), usize::MAX)?
        .into_iter()
        .filter(|n| n.status == "new")
        .count();
    Ok(QueueCounts {
        build: if observed {
            0
        } else {
            store
                .nodes_by_status(NodeType::Intent, &["planned", "needs_change"])?
                .len()
                + ungrounded_implemented_intents(store)?.len()
        },
        coverage: if observed {
            0
        } else {
            crate::commands::unowned_codefiles(store)?.len()
        },
        fix: if observed { 0 } else { failing },
        analyze: unin_rel + stale_rel,
        quality: stale_gov + unin_gov + pairs,
        validate: stale_val + unin_val,
        review,
        triage: findings + inbox_new,
        prove: store
            .nodes_by_status(NodeType::Hypothesis, &["proposed"])?
            .len(),
        elaborate: if observed {
            0
        } else {
            crate::completeness::all_scorecards(store)?
                .iter()
                .filter(|c| c.open > 0 && c.visibility.as_deref() == Some("user_visible"))
                .count()
        },
    })
}

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
        "elaborate" | "prove" | "build" => Some("judgment".into()),
        "coverage" => Some("mechanical".into()),
        // Structural size/complexity findings need cohesion judgment; inbox and
        // generic findings stay mechanical.
        "triage" if is_structural_size_finding(node) => Some("judgment".into()),
        "triage" => Some("mechanical".into()),
        _ => Some("judgment".into()),
    };
    QueueEntry {
        mode: mode.into(),
        effort: effort.into(),
        routing_hint,
        cause_class: None,
        reason,
        target: node_target(node),
    }
}

/// The FULL roster of one queue: every item the lane would serve, in the exact
/// order it serves them — entry 0 is what `loom next --mode <m>` returns. Entries
/// are lightweight (see `QueueEntry`). Observed graphs disable the
/// build/fix/coverage/elaborate lanes, so those roster empty here too, matching
/// `next` and `queue_counts`.
pub fn queue_items(store: &Store, mode: super::Mode) -> Result<Vec<QueueEntry>> {
    use super::Mode;
    use crate::model::TruthClass;
    let observed = store.identity()?.observed;
    if observed
        && matches!(
            mode,
            Mode::Build | Mode::Fix | Mode::Coverage | Mode::Elaborate
        )
    {
        return Ok(Vec::new());
    }
    let not_measured_lane = |e: &Edge| !matches!(e.kind, EdgeKind::Governs | EdgeKind::Validates);
    let mut out = Vec::new();
    match mode {
        Mode::Fix => {
            for e in
                store.live_edges_by_status(TruthClass::Asserted, &[InspectionStatus::Failing])?
            {
                out.push(edge_entry(
                    store,
                    &e,
                    "fix",
                    "failing verdict — repair at root cause",
                )?);
            }
        }
        Mode::Analyze => {
            for e in store
                .live_edges_by_status(
                    TruthClass::Asserted,
                    &[InspectionStatus::NeedsReverification],
                )?
                .iter()
                .filter(|e| not_measured_lane(e))
            {
                out.push(edge_entry(
                    store,
                    e,
                    "analyze",
                    "dependency changed — re-verify this claim",
                )?);
            }
            for e in store
                .live_edges_by_status(TruthClass::Asserted, &[InspectionStatus::Uninspected])?
                .iter()
                .filter(|e| not_measured_lane(e))
            {
                out.push(edge_entry(
                    store,
                    e,
                    "analyze",
                    "uninspected claim — inspect the code and record a verdict",
                )?);
            }
        }
        Mode::Validate => {
            let validates = store.edges_with(Some(EdgeKind::Validates), None, None)?;
            for e in validates
                .iter()
                .filter(|e| e.status == InspectionStatus::Uninspected)
            {
                out.push(edge_entry(store, e, "validate", "unrun proof")?);
            }
            for e in validates
                .iter()
                .filter(|e| e.status == InspectionStatus::NeedsReverification)
            {
                out.push(edge_entry(
                    store,
                    e,
                    "validate",
                    "proof went stale — a dependency changed; re-run it",
                )?);
            }
        }
        Mode::Quality => {
            let governs = store.edges_with(Some(EdgeKind::Governs), None, None)?;
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
        }
        Mode::Build => {
            let mut intents =
                store.nodes_by_status(NodeType::Intent, &["needs_change", "planned"])?;
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
                            "blocked: requires {} — build the prerequisite(s) first, or break the requires cycle",
                            unmet.join(", ")
                        ),
                    ));
                }
            }
            out.extend(blocked);
        }
        Mode::Coverage => {
            for cf in crate::commands::unowned_codefiles(store)? {
                let missing = !store.root().join(&cf.name).exists();
                let reason = if missing {
                    "no longer exists on disk — unregister it (or re-register its successor)"
                } else {
                    "no owning intent — ground it, or unregister the file"
                };
                out.push(node_entry("coverage", "low", &cf, reason.into()));
            }
        }
        Mode::Prove => {
            for h in store.nodes_by_status(NodeType::Hypothesis, &["proposed"])? {
                out.push(node_entry(
                    "prove",
                    "high",
                    &h,
                    "unproven hypothesis — prove or refute the claim against the code".into(),
                ));
            }
        }
        Mode::Triage => {
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
        }
        Mode::Review => {
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
        }
        Mode::Elaborate => {
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
        }
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
