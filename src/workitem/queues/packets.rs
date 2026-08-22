use super::super::context::{edge_context, node_context};
use super::super::contracts::{
    adversarial_reviewer_contract, analyzer_contract, builder_contract, coverage_contract,
    derive_contract, elaborator_contract, exemplar_contract, fixer_contract, inbox_triage_contract,
    journey_proof_contract, journey_proof_contract_for_profile, needed_finding_analyze_contract,
    needed_finding_fix_contract, needed_finding_validate_contract, prove_contract,
    quality_contract, quality_contract_body, ratify_contract, rectify_contract, research_contract,
    reviewer_contract, structural_finding_triage_contract, surface_contract, triage_contract,
    unproven_contract, validator_contract,
};
use super::super::{
    axis_for_role, effort_for, node_target, LinkedEntity, SuggestedRead, Target, TraversalContext,
    WorkItem,
};
use super::predicates::{
    build_candidates, candidate_files, first_analyzable, first_audit_subject,
    is_structural_size_finding, needed_finding_repair_lane, needed_findings_for,
    open_research_tasks, unmeasured_quality_pairs, unseeded_quality_pack, validation_work_units,
    ValidationWorkUnit,
};
use super::prescreen::prescreen_for;
use crate::lane::Lane;
use crate::model::{Edge, EdgeKind, InspectionStatus, Node, NodeType};
use crate::store::Store;
use crate::Result;

pub(in super::super) fn build_item(store: &Store) -> Result<Option<WorkItem>> {
    let Some((intent, reason)) = build_candidates(store, false)?.into_iter().next() else {
        return Ok(None);
    };
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        review: None,
        mode: "build".into(),
        owner_role: "builder".into(),
        effort: "mid".into(),
        routing_hint: super::super::hint_judgment(),
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

pub(in super::super) fn derive_item(store: &Store) -> Result<Option<WorkItem>> {
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
        review: None,
        mode: "derive".into(),
        owner_role: "builder".into(),
        effort: "high".into(),
        routing_hint: super::super::hint_judgment(),
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

pub(in super::super) fn surface_item(store: &Store) -> Result<Option<WorkItem>> {
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
        review: None,
        mode: "surface".into(),
        owner_role: "builder".into(),
        effort: "high".into(),
        routing_hint: super::super::hint_judgment(),
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

pub(in super::super) fn coverage_item(store: &Store) -> Result<Option<WorkItem>> {
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
            super::super::contracts::missing_codefile_contract(&cf),
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
        review: None,
        mode: "coverage".into(),
        owner_role: "builder".into(),
        effort: "low".into(),
        routing_hint: super::super::hint_mechanical(),
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

pub(in super::super) fn fix_item(store: &Store) -> Result<Option<WorkItem>> {
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
    // Second intake: findings a triager adjudicated `needed` whose named
    // repair is a code edit. Proof-rerun and undeclared-coupling findings
    // write other lanes' facts, so they are served there — a packet that
    // names a write its owner_role cannot execute is the INV-7 rejection
    // a drain worker hit on 2026-08-21.
    if let Some(fv) = needed_findings_for(store, Lane::Fix)?.into_iter().next() {
        return Ok(Some(needed_finding_work(store, &fv)?));
    }
    Ok(None)
}

/// A `needed` finding packet on the lane that owns the named repair.
/// The repair loop still closes through existing machinery: the write (code
/// edit, proof run, or relates edge) plus sync; the detector drops or stales
/// the finding; triage records `resolved` if it remains.
fn needed_finding_work(store: &Store, fv: &crate::signal::FindingView) -> Result<WorkItem> {
    let id = crate::model::short(&fv.node.id);
    let lane = needed_finding_repair_lane(&fv.node);
    let reason = format!(
        "adjudicated needed — repair at root cause: {}",
        fv.reason.trim()
    );
    let (mode, owner_role, effort, contract, truth_gap, context_note, next_step) = match lane {
        Lane::Validate => (
            "validate",
            "validator",
            "high",
            needed_finding_validate_contract(id),
            crate::truth::TruthAxis::Proof.gap(),
            "Compile and run the current Journey proof profile (or register and run a validation). Do not edit code to make the proof pass.",
            "after the proof run, return to loom status — the detector drops this finding when an S3-or-stronger proof holds",
        ),
        Lane::Analyze => (
            "analyze",
            "analyzer",
            "mid",
            needed_finding_analyze_contract(id),
            crate::truth::TruthAxis::Verdict.gap(),
            "Record the missing relationship between the owning intents; do not edit production code.",
            "after recording the relates edge and syncing, return to loom status",
        ),
        _ => (
            "fix",
            "fixer",
            "mid",
            needed_finding_fix_contract(id, fv.node.body.get("file").and_then(|v| v.as_str())),
            crate::truth::TruthAxis::Implementation.gap(),
            "Read the finding's evidence and the cited code before repairing; the triager's reason says what to do, the evidence says where.",
            "after the repair + sync, return to loom status — the reopened finding routes to triage for its resolved verdict",
        ),
    };
    Ok(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        review: None,
        mode: mode.into(),
        owner_role: owner_role.into(),
        effort: effort.into(),
        routing_hint: super::super::hint_judgment(),
        reason,
        target: node_target(&fv.node),
        stale_causes: Vec::new(),
        prompt_contract: contract,
        context: node_context(store, &fv.node, context_note)?,
        scorecard: None,
        truth_gap,
        next_step: next_step.into(),
    })
}

pub(in super::super) fn analyze_item(store: &Store) -> Result<Option<WorkItem>> {
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
    if let Some(fv) = needed_findings_for(store, Lane::Analyze)?
        .into_iter()
        .next()
    {
        return Ok(Some(needed_finding_work(store, &fv)?));
    }
    Ok(None)
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
        packet_id: None, pattern_guidance: None, review: None, mode: "analyze".into(), owner_role: "analyzer".into(),
        effort: "mid".into(), routing_hint: super::super::hint_judgment(),
        reason: "current external knowledge is missing — research before relying on assumptions".into(),
        target: node_target(task), stale_causes: Vec::new(), prompt_contract: research_contract(task),
        context: node_context(store, task, &format!("Answer the bounded external question and preserve actual-page provenance; this context remains advisory. why_external: {} preferred_sources: {} resolved_target_intent: {}", body.why_external, body.preferred_sources.join(", "), target_text))?,
        scorecard: None, truth_gap: crate::truth::TruthAxis::Verdict.gap(),
        next_step: "record every actual page with source-add, then close with an advisory synthesis; do not edit code".into(),
    })
}

/// With zero non-deprecated quality rules, the measured rung is vacuous. The
/// lane's first packet is the seed step itself, so `loom next --mode quality`
/// and the default walk never point at an empty queue while the rung is Unmet.
fn unseeded_quality_item(store: &Store) -> Result<Option<WorkItem>> {
    let Some(pack) = unseeded_quality_pack(store)? else {
        return Ok(None);
    };
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        review: None,
        mode: "quality".into(),
        owner_role: "quality".into(),
        effort: "low".into(),
        routing_hint: Some("seeding".into()),
        reason: "quality rung is unseeded — no quality rules exist; seed a pack so boundary expectations can be measured"
            .into(),
        target: Target {
            kind: "graph".into(),
            id: "quality-seed".into(),
            name: format!("unseeded — no quality rules seeded (loom rule seed {pack})"),
            from: None,
            to: None,
        },
        stale_causes: Vec::new(),
        prompt_contract: super::super::PromptContract {
            role: "quality".into(),
            mindset: "Seeding authority. Seed a recommended quality pack so the measured rung has non-vacuous rules to measure against implemented intents.".into(),
            why_now: "The measured rung is unseeded: zero non-deprecated quality rules exist, so every boundary axis reads not_applicable and the rung cannot honestly report met.".into(),
            allowed_actions: vec![
                "Run `loom detect` to review languages and project markers.".into(),
                format!("Run `loom rule seed {pack}` (or another available pack)."),
                "Run `loom rule list` and `loom rule show <rule>` to verify the seeded rules.".into(),
            ],
            forbidden_actions: vec![
                "Do not record a quality verdict before a rule exists.".into(),
                "Do not edit code as part of seeding.".into(),
            ],
            required_evidence: "The pack name and seeded-rule count from `loom rule seed` output; `loom status` then shows the measured rung with real pair counts instead of unseeded.".into(),
            evidence_clauses: Vec::new(),
            evidence_template: None,
            examples: None,
            pre_screened_hits: Vec::new(),
            pre_screen: None,
            write_back: format!("loom rule seed {pack}; loom status"),
            stop_condition: "`loom status` shows the measured rung as met or counting unmeasured pairs, never unseeded.".into(),
            human_gate: None,
        },
        context: TraversalContext {
            purpose: "Choose and seed a quality pack; then re-run status so the measured rung has rules to measure.".into(),
            linked_entities: Vec::new(),
            suggested_reads: vec![
                SuggestedRead {
                    reason: "the recommended pack detection is the same signal the compass used".into(),
                    command: "loom detect".into(),
                },
                SuggestedRead {
                    reason: "seeded rules appear here after the write".into(),
                    command: "loom rule list".into(),
                },
            ],
            read_set: Vec::new(),
        },
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Verdict.gap(),
        next_step: format!("run `loom rule seed {pack}`, then `loom status`"),
    }))
}

pub(in super::super) fn quality_item(store: &Store) -> Result<Option<WorkItem>> {
    if let Some(item) = unseeded_quality_item(store)? {
        return Ok(Some(item));
    }
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
        review: None,
        mode: "quality".into(),
        owner_role: "quality".into(),
        effort,
        routing_hint: super::super::hint_judgment(),
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

pub(in super::super) fn validate_item(store: &Store) -> Result<Option<WorkItem>> {
    if let Some(unit) = validation_work_units(store)?.into_iter().next() {
        return match unit {
        ValidationWorkUnit::JourneyValidation {
            journey, profile, ..
        } => Ok(Some(WorkItem {
            packet_id: None,
            pattern_guidance: None,
            review: None,
            mode: "validate".into(),
            owner_role: "validator".into(),
            effort: "high".into(),
            routing_hint: super::super::hint_judgment(),
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
                super::super::q(&profile)
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
            review: None,
            mode: "validate".into(),
            owner_role: "validator".into(),
            effort: "high".into(),
            routing_hint: super::super::hint_judgment(),
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
            review: None,
            mode: "validate".into(),
            owner_role: "validator".into(),
            effort: "mid".into(),
            routing_hint: super::super::hint_judgment(),
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
        };
    }
    if let Some(fv) = needed_findings_for(store, Lane::Validate)?
        .into_iter()
        .next()
    {
        return Ok(Some(needed_finding_work(store, &fv)?));
    }
    Ok(None)
}

pub(in super::super) fn review_item(store: &Store) -> Result<Option<WorkItem>> {
    let Some(candidate) = crate::review::pending(store)?.into_iter().next() else {
        return Ok(None);
    };
    let mut item = edge_work(
        store,
        &candidate.edge,
        "review",
        &candidate.owner_role,
        &candidate.reason,
    )?;
    item.review = Some(super::super::ReviewDirective {
        variant: candidate.variant.as_str().into(),
        target_verdict_fact_id: candidate.target_verdict_fact_id.clone(),
        risk_score: candidate.risk_score,
        prefer_profile_not: candidate.prefer_profile_not.clone(),
    });
    if candidate.variant == crate::review::ReviewVariant::Adversarial {
        item.target.kind = "edge_challenge".into();
        item.routing_hint = super::super::hint_judgment();
        item.prompt_contract = adversarial_reviewer_contract(
            &candidate.edge,
            item.target.from.as_deref().unwrap_or(""),
            item.target.to.as_deref().unwrap_or(""),
            candidate.prefer_profile_not.as_deref(),
        );
        item.next_step = "after recording the challenge, run `loom status`".into();
    }
    Ok(Some(item))
}

/// Serve the most-incomplete user-visible feature intent for elaboration.
/// Humans hand loom a core idea and systematically forget the surroundings —
/// failure scenarios, prerequisites, boundary expectations, proofs, open
/// product questions. This queue routes exactly that gap: each open axis is
/// closed by an artifact, a recorded waiver, or a question to the human.
pub(in super::super) fn elaborate_item(store: &Store) -> Result<Option<WorkItem>> {
    let readiness = crate::completeness::all_journey_readiness(store)?;
    let Some((intent, card)) = crate::completeness::elaboration_queue(store, &readiness)?
        .into_iter()
        .next()
    else {
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
        review: None,
        mode: "elaborate".into(),
        owner_role: "builder".into(),
        effort: "high".into(),
        routing_hint: super::super::hint_judgment(),
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

pub(in super::super) fn prove_item(store: &Store) -> Result<Option<WorkItem>> {
    let hyps = store.nodes_by_status(NodeType::Hypothesis, &["proposed"])?;
    let Some(h) = hyps.into_iter().next() else {
        return Ok(None);
    };
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        review: None,
        mode: "prove".into(),
        owner_role: "analyzer".into(),
        effort: "high".into(),
        routing_hint: super::super::hint_judgment(),
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
            super::super::q(&h.name)
        ),
    }))
}

/// The ratify queue: human-decision work. An LLM presents this packet, makes an
/// evidence-backed recommendation, waits for the human, then may record the
/// exact answer through the mediated decision path. It never owns the choice.
///
/// Skips rectifiable friction (duplicates / un-escalated discoveries) — those
/// belong to [`rectify_item`]. Served one at a time, ranked kind-first. Plain
/// `loom next` does not interrupt an autonomous loop with a product question;
/// a host requests `--mode ratify` when it has a conversation channel to the human.
pub(in super::super) fn ratify_item(store: &Store) -> Result<Option<WorkItem>> {
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
        review: None,
        mode: "ratify".into(),
        owner_role: "human".into(),
        effort: "low".into(),
        routing_hint: super::super::hint_judgment(),
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
pub(in super::super) fn rectify_item(store: &Store) -> Result<Option<WorkItem>> {
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
        review: None,
        mode: "rectify".into(),
        owner_role: "rectify".into(),
        effort: "low".into(),
        routing_hint: super::super::hint_judgment(),
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

pub(in super::super) fn triage_item(store: &Store) -> Result<Option<WorkItem>> {
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
            super::super::hint_judgment(),
            structural_finding_triage_contract(short),
        )
    } else {
        (
            "low".into(),
            super::super::hint_mechanical(),
            triage_contract(short),
        )
    };
    Ok(Some(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        review: None,
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
        review: None,
        mode: "triage".into(),
        owner_role: "analyzer".into(),
        effort: "low".into(),
        routing_hint: super::super::hint_mechanical(),
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
    let (effort, routing_hint) = super::super::refine_effort_and_hint(
        base_effort,
        &stale_causes,
        &contract.write_back,
        &edge.criterion,
        context.read_set.len(),
    );
    Ok(WorkItem {
        packet_id: None,
        pattern_guidance: None,
        review: None,
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

/// The deepen queue: what to strengthen next, one move at a time.
///
/// Serves the top-ranked candidate only. The ranking re-orders after every
/// change — including the change this packet asks for, which lowers its own
/// candidate's score — so handing out a batch would hand out a list that is
/// stale by the second item.
pub(in super::super) fn deepen_item(store: &Store) -> Result<Option<WorkItem>> {
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
        review: None,
        mode: "deepen".into(),
        owner_role: "validator".into(),
        effort: "mid".into(),
        routing_hint: super::super::hint_judgment(),
        reason: format!("'{}' is at {} — {}", n.name, c.proof_strength, c.why),
        target: node_target(&n),
        stale_causes: Vec::new(),
        prompt_contract: super::super::contracts::deepen_contract(short, &n.name, c.next_move),
        context: node_context(
            store,
            &n,
            "This behavior is already green. Find the weakest thing holding it up.",
        )?,
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Risk.gap(),
        // Only true while the move can still change the grade. `rank` orders by
        // proof strength and S3 is the highest grade assigned today, so the
        // baseline move leaves the ranking exactly as it was and this item
        // returns — which reads as failure to anyone who just did the work
        // correctly.
        next_step: if c.next_move == crate::risk::Move::FreezeBaseline {
            "freeze the baseline for the shape guarantee, then move on: S3 is the highest \
             grade currently assigned, so this item will rank first again and that is not \
             a sign you did it wrong"
                .into()
        } else {
            "make ONE move, then re-run `loom deepen` — the ranking will have changed".into()
        },
    }))
}

/// The audit queue: this graph's own record, where it does not look earned.
/// The audit queue.
///
/// Status and this queue read the same actionable self-audit backlog.
pub(in super::super) fn audit_item(store: &Store) -> Result<Option<WorkItem>> {
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
        review: None,
        mode: "audit".into(),
        owner_role: "analyzer".into(),
        effort: "mid".into(),
        routing_hint: super::super::hint_judgment(),
        reason: format!("{}: {}", f.kind, f.detail),
        target,
        stale_causes: Vec::new(),
        prompt_contract: super::super::contracts::audit_contract(&f.remedy),
        context,
        scorecard: None,
        truth_gap: crate::truth::TruthAxis::Signal.gap(),
        next_step: f.remedy.clone(),
    }))
}
