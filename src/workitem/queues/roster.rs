use super::super::{cause_class, effort_for, node_target, Target};
use super::predicates::{
    analyze_serves, audit_subjects, build_candidates, is_structural_size_finding,
    needed_findings_for, open_research_tasks, unmeasured_quality_pairs, unseeded_quality_pack,
    validation_work_units, ValidationWorkUnit,
};
use crate::lane::Lane;
use crate::model::{Edge, EdgeKind, InspectionStatus, Node, NodeType};
use crate::store::Store;
use crate::Result;

fn push_needed_finding_entries(store: &Store, lane: Lane, out: &mut Vec<QueueEntry>) -> Result<()> {
    let effort = if lane == Lane::Validate {
        "high"
    } else {
        "mid"
    };
    for fv in needed_findings_for(store, lane)? {
        out.push(node_entry(
            lane.as_str(),
            effort,
            &fv.node,
            format!(
                "adjudicated needed — repair at root cause: {}",
                fv.reason.trim()
            ),
        ));
    }
    Ok(())
}

fn unseeded_quality_entry(store: &Store) -> Result<Option<QueueEntry>> {
    let Some(pack) = unseeded_quality_pack(store)? else {
        return Ok(None);
    };
    Ok(Some(QueueEntry {
        mode: "quality".into(),
        effort: "low".into(),
        routing_hint: Some("seeding".into()),
        cause_class: None,
        owner_role: Some("quality".into()),
        review: None,
        reason: "quality rung is unseeded — no quality rules exist; seed a pack so boundary expectations can be measured".into(),
        target: Target {
            kind: "graph".into(),
            id: "quality-seed".into(),
            name: format!("unseeded — no quality rules seeded (loom rule seed {pack})"),
            from: None,
            to: None,
        },
    }))
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<super::super::ReviewDirective>,
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
        review: None,
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
        "derive" | "build" | "surface" | "coverage" | "elaborate" => Some("builder".into()),
        "triage" | "prove" | "analyze" => Some("analyzer".into()),
        "fix" => Some("fixer".into()),
        "validate" => Some("validator".into()),
        "quality" => Some("quality".into()),
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
        review: None,
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
            review: None,
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
    push_needed_finding_entries(store, Lane::Fix, out)?;
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
    push_needed_finding_entries(store, Lane::Analyze, out)?;
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
    push_needed_finding_entries(store, Lane::Validate, out)?;
    Ok(())
}

/// The `quality` lane's roster. One arm per lane, each its own function:
/// `queue_items` was a 297-line match that only dispatched, so every lane's
/// enumeration lived inside one symbol loom scored at complexity 32.
fn roster_quality(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    use crate::model::TruthClass;
    if let Some(entry) = unseeded_quality_entry(store)? {
        out.push(entry);
        return Ok(());
    }
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
    for (intent, reason) in build_candidates(store, true)? {
        out.push(node_entry("build", "mid", &intent, reason));
    }
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
    for candidate in crate::review::pending(store)? {
        let mut entry = edge_entry(store, &candidate.edge, "review", &candidate.reason)?;
        entry.owner_role = Some(candidate.owner_role);
        entry.review = Some(super::super::ReviewDirective {
            variant: candidate.variant.as_str().into(),
            target_verdict_fact_id: candidate.target_verdict_fact_id,
            risk_score: candidate.risk_score,
            prefer_profile_not: candidate.prefer_profile_not,
        });
        if candidate.variant == crate::review::ReviewVariant::Adversarial {
            entry.target.kind = "edge_challenge".into();
        }
        out.push(entry);
    }
    Ok(())
}

/// The `elaborate` lane's roster. One arm per lane, each its own function:
/// `queue_items` was a 297-line match that only dispatched, so every lane's
/// enumeration lived inside one symbol loom scored at complexity 32.
fn roster_elaborate(store: &Store, out: &mut Vec<QueueEntry>) -> Result<()> {
    let readiness = crate::completeness::all_journey_readiness(store)?;
    for (intent, card) in crate::completeness::elaboration_queue(store, &readiness)? {
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
                review: None,
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
