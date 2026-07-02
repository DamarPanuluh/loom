//! WorkItem + PromptContract — the LLM-facing surface of `loom next`.
//!
//! Plane: orchestration over the store. loom computes the next correct work and
//! compiles it into a prompt contract: which role/mindset to adopt, what is
//! allowed, what evidence is required, and the exact write-back. The LLM acts
//! and reports through typed graph writes; loom validates and ripples.

use crate::model::{Edge, EdgeKind, InspectionStatus, Node, NodeType};
use crate::store::Store;
use crate::Result;
use serde::Serialize;

/// The queue a `loom next` request targets (ring 3 subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Build,
    Coverage,
    Fix,
    Analyze,
    Quality,
    Validate,
    Prove,
    Triage,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "build" => Some(Mode::Build),
            "coverage" => Some(Mode::Coverage),
            "fix" => Some(Mode::Fix),
            "analyze" | "discovery" => Some(Mode::Analyze),
            "quality" => Some(Mode::Quality),
            "validate" => Some(Mode::Validate),
            "prove" => Some(Mode::Prove),
            "triage" => Some(Mode::Triage),
            _ => None,
        }
    }
}

/// The role/mindset contract the LLM adopts for one work item.
#[derive(Debug, Clone, Serialize)]
pub struct PromptContract {
    pub role: String,
    pub mindset: String,
    pub why_now: String,
    pub allowed_actions: Vec<String>,
    pub forbidden_actions: Vec<String>,
    pub required_evidence: String,
    pub write_back: String,
    pub stop_condition: String,
    pub human_gate: Option<String>,
}

/// Compact graph context that tells an LLM where to look next. This is a map,
/// not a verdict: coding agents use it to choose files/symbols to inspect before
/// editing; verdict agents use it to find the evidence-bearing endpoints.
#[derive(Debug, Clone, Serialize)]
pub struct TraversalContext {
    pub purpose: String,
    pub linked_entities: Vec<LinkedEntity>,
    pub suggested_reads: Vec<SuggestedRead>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LinkedEntity {
    pub role: String,
    pub kind: String,
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SuggestedRead {
    pub reason: String,
    pub command: String,
}

/// A promptable unit of work.
#[derive(Debug, Clone, Serialize)]
pub struct WorkItem {
    pub mode: String,
    pub owner_role: String,
    pub effort: String,
    pub reason: String,
    /// The primary target: an intent or an edge, described for the LLM.
    pub target: Target,
    pub prompt_contract: PromptContract,
    pub context: TraversalContext,
    /// Which form of truth this item makes true, and the authoritative write /
    /// forbidden write / refresh for that axis. Lets the LLM self-teach whether
    /// it should fill an intent, code, a proof, a saga, a verdict, or an export.
    pub truth_gap: crate::truth::TruthGap,
    pub next_step: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Target {
    pub kind: String,
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

fn node_context(store: &Store, node: &Node, purpose: &str) -> Result<TraversalContext> {
    let mut ctx = TraversalContext {
        purpose: purpose.into(),
        linked_entities: Vec::new(),
        suggested_reads: Vec::new(),
    };
    let mut seen_entities = std::collections::BTreeSet::new();
    let mut seen_reads = std::collections::BTreeSet::new();
    push_node_entity(&mut ctx, &mut seen_entities, "target", node);
    push_node_read(&mut ctx, &mut seen_reads, "show target entity", node);

    let mut edges = store.edges_with(None, Some(&node.id), None)?;
    edges.extend(store.edges_with(None, None, Some(&node.id))?);
    edges.sort_by(|a, b| {
        a.kind
            .as_str()
            .cmp(b.kind.as_str())
            .then(a.from_id.cmp(&b.from_id))
            .then(a.to_id.cmp(&b.to_id))
    });

    for edge in edges.into_iter().take(12) {
        push_edge_entity(store, &mut ctx, &mut seen_entities, "linked_edge", &edge)?;
        push_edge_read(&mut ctx, &mut seen_reads, "inspect linked edge", &edge);
        let other_id = if edge.from_id == node.id {
            &edge.to_id
        } else {
            &edge.from_id
        };
        if let Some(other) = store.get_node(other_id)? {
            let role = if edge.from_id == node.id {
                format!("outgoing_{}", edge.kind)
            } else {
                format!("incoming_{}", edge.kind)
            };
            push_node_entity(&mut ctx, &mut seen_entities, &role, &other);
            push_node_read(
                &mut ctx,
                &mut seen_reads,
                &format!("inspect {role}"),
                &other,
            );
        }
    }

    if matches!(node.node_type, NodeType::Intent)
        && !ctx
            .linked_entities
            .iter()
            .any(|e| e.kind == NodeType::CodeFile.as_str())
    {
        push_raw_read(
            &mut ctx,
            &mut seen_reads,
            "survey registered codefiles when no grounding exists yet",
            "loom codefile list",
        );
    }
    Ok(ctx)
}

fn edge_context(store: &Store, edge: &Edge, purpose: &str) -> Result<TraversalContext> {
    let mut ctx = TraversalContext {
        purpose: purpose.into(),
        linked_entities: Vec::new(),
        suggested_reads: Vec::new(),
    };
    let mut seen_entities = std::collections::BTreeSet::new();
    let mut seen_reads = std::collections::BTreeSet::new();
    push_edge_entity(store, &mut ctx, &mut seen_entities, "target_edge", edge)?;
    push_edge_read(&mut ctx, &mut seen_reads, "inspect target edge", edge);
    for (role, id) in [("from", &edge.from_id), ("to", &edge.to_id)] {
        if let Some(node) = store.get_node(id)? {
            push_node_entity(&mut ctx, &mut seen_entities, role, &node);
            push_node_read(
                &mut ctx,
                &mut seen_reads,
                &format!("inspect {role} endpoint"),
                &node,
            );
            if matches!(node.node_type, NodeType::Intent) {
                push_grounded_codefiles(
                    store,
                    &mut ctx,
                    &mut seen_entities,
                    &mut seen_reads,
                    &node,
                )?;
            }
        }
    }
    Ok(ctx)
}

fn push_grounded_codefiles(
    store: &Store,
    ctx: &mut TraversalContext,
    seen_entities: &mut std::collections::BTreeSet<String>,
    seen_reads: &mut std::collections::BTreeSet<String>,
    intent: &Node,
) -> Result<()> {
    for edge in store
        .edges_with(Some(EdgeKind::Implements), Some(&intent.id), None)?
        .into_iter()
        .take(8)
    {
        push_edge_entity(store, ctx, seen_entities, "grounding_edge", &edge)?;
        if let Some(cf) = store.get_node(&edge.to_id)? {
            push_node_entity(ctx, seen_entities, "grounded_codefile", &cf);
            push_node_read(ctx, seen_reads, "inspect grounded codefile", &cf);
        }
    }
    Ok(())
}

fn push_node_entity(
    ctx: &mut TraversalContext,
    seen: &mut std::collections::BTreeSet<String>,
    role: &str,
    node: &Node,
) {
    let key = format!("node:{}:{role}", node.id);
    if !seen.insert(key) {
        return;
    }
    ctx.linked_entities.push(LinkedEntity {
        role: role.into(),
        kind: node.node_type.as_str().into(),
        id: node.id.clone(),
        name: node.name.clone(),
        status: Some(node.status.clone()).filter(|s| !s.is_empty()),
        edge_kind: None,
        edge_status: None,
        locator: None,
    });
}

fn push_edge_entity(
    store: &Store,
    ctx: &mut TraversalContext,
    seen: &mut std::collections::BTreeSet<String>,
    role: &str,
    edge: &Edge,
) -> Result<()> {
    let key = format!("edge:{}:{role}", edge.id);
    if !seen.insert(key) {
        return Ok(());
    }
    let locator = store.get_facet(&edge.id, crate::model::TargetKind::Edge, "locator")?;
    ctx.linked_entities.push(LinkedEntity {
        role: role.into(),
        kind: "edge".into(),
        id: edge.id.clone(),
        name: format!("{} {} {}", edge.from_id, edge.kind, edge.to_id),
        status: None,
        edge_kind: Some(edge.kind.as_str().into()),
        edge_status: Some(edge.status.as_str().into()),
        locator,
    });
    Ok(())
}

fn push_node_read(
    ctx: &mut TraversalContext,
    seen: &mut std::collections::BTreeSet<String>,
    reason: &str,
    node: &Node,
) {
    let Some(command) = node_read_command(node) else {
        return;
    };
    push_raw_read(ctx, seen, reason, &command);
}

fn push_edge_read(
    ctx: &mut TraversalContext,
    seen: &mut std::collections::BTreeSet<String>,
    reason: &str,
    edge: &Edge,
) {
    push_raw_read(ctx, seen, reason, &format!("loom edge show {}", edge.id));
}

fn push_raw_read(
    ctx: &mut TraversalContext,
    seen: &mut std::collections::BTreeSet<String>,
    reason: &str,
    command: &str,
) {
    if seen.insert(command.into()) {
        ctx.suggested_reads.push(SuggestedRead {
            reason: reason.into(),
            command: command.into(),
        });
    }
}

fn node_read_command(node: &Node) -> Option<String> {
    let cmd = match node.node_type {
        NodeType::Intent => format!("loom intent show {}", node.id),
        NodeType::CodeFile => format!("loom codefile show {}", node.id),
        NodeType::QualityRule => format!("loom rule show {}", node.id),
        NodeType::Validation => format!("loom validation show {}", node.id),
        NodeType::Hypothesis => format!("loom hypothesis show {}", node.id),
        NodeType::Finding => format!("loom finding list --kind {}", node.name),
        NodeType::InboxItem => "loom inbox list".into(),
        _ => return None,
    };
    Some(cmd)
}

/// Compute the next work item for a mode, or the highest-priority item overall.
pub fn next(store: &Store, mode: Option<Mode>) -> Result<Option<WorkItem>> {
    // An observed graph maps code the driver does not own: discovery, quality,
    // and validation work only — the build and fix lanes are disabled, because a
    // monitor cannot change the upstream it watches (docs/commands.md:90).
    let observed = store.identity()?.observed;
    match mode {
        Some(Mode::Build) | Some(Mode::Fix) | Some(Mode::Coverage) if observed => Ok(None),
        Some(Mode::Build) => build_item(store),
        Some(Mode::Coverage) => coverage_item(store),
        Some(Mode::Fix) => fix_item(store),
        Some(Mode::Analyze) => analyze_item(store),
        Some(Mode::Quality) => quality_item(store),
        Some(Mode::Validate) => validate_item(store),
        Some(Mode::Prove) => prove_item(store),
        Some(Mode::Triage) => triage_item(store),
        None => {
            // Priority: repair failing/stale, then validate, then build, then
            // measure quality, inspect relationships, and finally triage derived
            // code flags after asserted graph residue is clean. On an observed
            // graph the fix/build lanes are skipped entirely.
            if !observed {
                if let Some(w) = fix_item(store)? {
                    return Ok(Some(w));
                }
            }
            if let Some(w) = validate_item(store)? {
                return Ok(Some(w));
            }
            if !observed {
                if let Some(w) = build_item(store)? {
                    return Ok(Some(w));
                }
            }
            if !observed {
                if let Some(w) = coverage_item(store)? {
                    return Ok(Some(w));
                }
            }
            if let Some(w) = quality_item(store)? {
                return Ok(Some(w));
            }
            if let Some(w) = analyze_item(store)? {
                return Ok(Some(w));
            }
            triage_item(store)
        }
    }
}

fn build_item(store: &Store) -> Result<Option<WorkItem>> {
    let mut intents = store.nodes_by_status(NodeType::Intent, &["needs_change", "planned"])?;
    // needs_change before planned; then stable by name.
    intents.sort_by(|a, b| {
        rank_lifecycle(&a.status)
            .cmp(&rank_lifecycle(&b.status))
            .then(a.name.cmp(&b.name))
    });
    let Some(intent) = intents.into_iter().next() else {
        return Ok(None);
    };
    let reason = format!("intent is {} and not yet realized", intent.status);
    Ok(Some(WorkItem {
        mode: "build".into(),
        owner_role: "builder".into(),
        effort: "mid".into(),
        reason,
        target: node_target(&intent),
        prompt_contract: builder_contract(&intent),
        context: node_context(
            store,
            &intent,
            "Understand the behavior and likely implementation files before coding.",
        )?,
        truth_gap: crate::truth::TruthAxis::Implementation.gap(),
        next_step: "after grounding + sync, run `loom status`".into(),
    }))
}

fn coverage_item(store: &Store) -> Result<Option<WorkItem>> {
    // The first unowned, non-ignored CodeFile (stable by name). Coverage is
    // grounding truth: either the file belongs to an intent (ground it) or it
    // does not belong in the graph (unregister it, or `loom ignore` it).
    let Some(cf) = crate::commands::unowned_codefiles(store)?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    Ok(Some(WorkItem {
        mode: "coverage".into(),
        owner_role: "builder".into(),
        effort: "low".into(),
        reason: format!("registered codefile '{}' has no owning intent", cf.name),
        target: node_target(&cf),
        prompt_contract: coverage_contract(&cf),
        context: node_context(
            store,
            &cf,
            "Decide which intent this file realizes, or whether it should be unregistered.",
        )?,
        truth_gap: crate::truth::TruthAxis::Implementation.gap(),
        next_step: "after grounding or unregistering + sync, run `loom status`".into(),
    }))
}

fn fix_item(store: &Store) -> Result<Option<WorkItem>> {
    // failing first, then stale (needs_reverification).
    let failing = store.edges_by_status(
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
    let stale = store.edges_by_status(
        crate::model::TruthClass::Asserted,
        &[InspectionStatus::NeedsReverification],
    )?;
    if let Some(e) = stale.into_iter().next() {
        return Ok(Some(edge_work(
            store,
            &e,
            "fix",
            "analyzer",
            "dependency changed — re-verify this claim",
        )?));
    }
    Ok(None)
}

fn analyze_item(store: &Store) -> Result<Option<WorkItem>> {
    let uninspected = store.edges_by_status(
        crate::model::TruthClass::Asserted,
        &[InspectionStatus::Uninspected],
    )?;
    // governs/validates have their own lanes (quality/validate); analyze is
    // relationships + groundings.
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

fn quality_item(store: &Store) -> Result<Option<WorkItem>> {
    let governs = store.edges_with(Some(EdgeKind::Governs), None, None)?;
    if let Some(e) = governs
        .iter()
        .find(|e| e.status == InspectionStatus::Failing)
    {
        return Ok(Some(edge_work(
            store,
            e,
            "quality",
            "quality",
            "failing quality verdict — repair the source",
        )?));
    }
    if let Some(e) = governs.iter().find(|e| {
        matches!(
            e.status,
            InspectionStatus::Uninspected | InspectionStatus::NeedsReverification
        )
    }) {
        return Ok(Some(edge_work(
            store,
            e,
            "quality",
            "quality",
            "unmeasured quality rule",
        )?));
    }
    Ok(None)
}

fn validate_item(store: &Store) -> Result<Option<WorkItem>> {
    let validates = store.edges_with(Some(EdgeKind::Validates), None, None)?;
    if let Some(e) = validates
        .iter()
        .find(|e| e.status == InspectionStatus::Failing)
    {
        return Ok(Some(edge_work(
            store,
            e,
            "validate",
            "validator",
            "failing proof — run and diagnose",
        )?));
    }
    if let Some(e) = validates.iter().find(|e| {
        matches!(
            e.status,
            InspectionStatus::Uninspected | InspectionStatus::NeedsReverification
        )
    }) {
        return Ok(Some(edge_work(
            store,
            e,
            "validate",
            "validator",
            "unrun proof",
        )?));
    }
    Ok(None)
}

fn prove_item(store: &Store) -> Result<Option<WorkItem>> {
    let hyps = store.nodes_by_status(NodeType::Hypothesis, &["proposed"])?;
    let Some(h) = hyps.into_iter().next() else {
        return Ok(None);
    };
    Ok(Some(WorkItem {
        mode: "prove".into(),
        owner_role: "analyzer".into(),
        effort: "high".into(),
        reason: "unproven hypothesis — prove or refute the claim against the code".into(),
        target: node_target(&h),
        prompt_contract: prove_contract(&h),
        context: node_context(
            store,
            &h,
            "Inspect the hypothesis target and related evidence before proving or refuting.",
        )?,
        truth_gap: crate::truth::TruthAxis::Verdict.gap(),
        next_step: "after proving/refuting, run `loom status`".into(),
    }))
}

fn triage_item(store: &Store) -> Result<Option<WorkItem>> {
    let findings = crate::signal::triage_findings(store)?;
    let Some(fv) = findings.into_iter().next() else {
        return inbox_triage_item(store);
    };
    let short = &fv.node.id[..8.min(fv.node.id.len())];
    // Cohesion evidence from the graph: which intents own the flagged file. One
    // or two cohesive owners reads as justified length; many unrelated ones (or
    // none) reads as a file that needs splitting — the judgment grep cannot make.
    let owners = store.finding_owner_intents(&fv.node.id)?;
    let cohesion = if owners.is_empty() {
        " — flagged file owns no intents (ungrounded; ground or split it)".to_string()
    } else {
        let names: Vec<&str> = owners.iter().take(4).map(|n| n.name.as_str()).collect();
        let more = if owners.len() > 4 {
            format!(", +{} more", owners.len() - 4)
        } else {
            String::new()
        };
        format!(
            " — flagged file owns {} intent(s): {}{}",
            owners.len(),
            names.join("; "),
            more
        )
    };
    let stale = if fv.stale {
        " — prior verdict is stale (file changed)"
    } else {
        ""
    };
    let reason = format!("{}{}{}", fv.node.name, stale, cohesion);
    Ok(Some(WorkItem {
        mode: "triage".into(),
        owner_role: "analyzer".into(),
        effort: "low".into(),
        reason,
        target: node_target(&fv.node),
        prompt_contract: triage_contract(short),
        context: node_context(
            store,
            &fv.node,
            "Inspect the flagged finding and owning codefile context before adjudicating.",
        )?,
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
        reason: format!("inbox item '{}' is new and needs routing", item.description),
        target: node_target(&item),
        prompt_contract: inbox_triage_contract(short),
        context: node_context(
            store,
            &item,
            "Inspect the inbox item before routing it into graph work.",
        )?,
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
        from: Some(from_name),
        to: Some(to_name),
    };
    let contract = match role {
        "fixer" => fixer_contract(edge),
        "quality" => quality_contract(store, edge)?,
        "validator" => validator_contract(store, edge)?,
        _ => analyzer_contract(edge),
    };
    Ok(WorkItem {
        mode: mode.into(),
        owner_role: role.into(),
        effort: effort_for(edge),
        reason: reason.into(),
        target,
        prompt_contract: contract,
        context: edge_context(
            store,
            edge,
            "Inspect the target edge endpoints and linked code before acting.",
        )?,
        truth_gap: axis_for_role(role).gap(),
        next_step: "after recording the verdict, run `loom status`".into(),
    })
}

fn node_target(n: &Node) -> Target {
    Target {
        kind: n.node_type.as_str().to_string(),
        id: n.id.clone(),
        name: n.name.clone(),
        from: None,
        to: None,
    }
}

fn rank_lifecycle(s: &str) -> u8 {
    match s {
        "needs_change" => 0,
        "planned" => 1,
        _ => 2,
    }
}

fn effort_for(edge: &Edge) -> String {
    match edge.status {
        InspectionStatus::Failing => "high".into(),
        _ => "mid".into(),
    }
}

/// The truth axis an edge-work role is closing. `fixer` restores implementation
/// truth (edit code at root cause); `validator` closes proof truth; everything
/// else (`analyzer`, `quality`) closes verdict truth (judge a claim by evidence).
fn axis_for_role(role: &str) -> crate::truth::TruthAxis {
    match role {
        "fixer" => crate::truth::TruthAxis::Implementation,
        "validator" => crate::truth::TruthAxis::Proof,
        _ => crate::truth::TruthAxis::Verdict,
    }
}

// ---- role contracts (see docs/llm-driver.md) -------------------------------

fn builder_contract(intent: &Node) -> PromptContract {
    PromptContract {
        role: "builder".into(),
        mindset: "Use Loom first to understand why this work exists, which entities/files are \
                  likely relevant, and what prior evidence says; then inspect the relevant code \
                  before editing. Functions/symbols are locators, not intents. Do not self-certify \
                  quality or proofs."
            .into(),
        why_now: format!("intent '{}' is {} and not yet realized", intent.name, intent.status),
        allowed_actions: vec![
            "loom status".into(),
            "loom next --all".into(),
            "loom intent show <intent>".into(),
            "loom codefile list".into(),
            "loom codefile show <file>".into(),
            "edit code".into(),
            "loom edge implement <intent> <codefile> --locator <symbol>".into(),
            "loom intent mark <intent> --lifecycle implemented".into(),
            "loom sync".into(),
            "loom inbox add (out-of-scope findings)".into(),
        ],
        forbidden_actions: vec![
            "loom rule verdict passing (quality lane)".into(),
            "loom validation mark passed (validator lane)".into(),
        ],
        required_evidence: "Loom context checked, relevant code inspected, code written, locator confirmed, sync clean".into(),
        write_back: "loom edge implement <intent> <codefile> --locator <symbol>; loom intent mark <intent> --lifecycle implemented".into(),
        stop_condition: "after grounding + sync, return to loom status".into(),
        human_gate: None,
    }
}

fn coverage_contract(codefile: &Node) -> PromptContract {
    PromptContract {
        role: "builder".into(),
        mindset: "A registered file with no owning intent is a coverage gap: either it \
                  realizes a behavior (ground it to that intent with a locator) or it is not \
                  behavior worth tracking (unregister it). Read the file before deciding; do not \
                  invent an intent just to satisfy the gate."
            .into(),
        why_now: format!("codefile '{}' is registered but unowned", codefile.name),
        allowed_actions: vec![
            "loom codefile show <file>".into(),
            "loom intent list".into(),
            "read the file to see which behavior it realizes".into(),
            "loom edge implement <intent> <codefile> --locator <symbol>".into(),
            "loom codefile remove <file> (if it should not be tracked)".into(),
            "loom ignore add '<glob>' --reason '…' (if outside the tracked surface)".into(),
            "loom sync".into(),
        ],
        forbidden_actions: vec![
            "inventing an intent with no behavioral description just to ground the file".into(),
            "loom rule verdict passing (quality lane)".into(),
        ],
        required_evidence: "file read, owning intent chosen with a locator, or a reason to unregister".into(),
        write_back: "loom edge implement <intent> <codefile> --locator <symbol>  (or)  loom codefile remove <codefile>  (or)  loom ignore add '<glob>' --reason '…'".into(),
        stop_condition: "after grounding or unregistering + sync, return to loom status".into(),
        human_gate: None,
    }
}

fn analyzer_contract(edge: &Edge) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset: "Read both sides. Form a hypothesis before inspecting code. Record exactly what \
                  the code shows. Do not fix code; do not preserve the old verdict by assumption.".into(),
        why_now: format!("{} edge is {}", edge.kind, edge.status),
        allowed_actions: vec![
            "read codefiles, notes, prior evidence".into(),
            "loom edge explore <a> <b> ground|issue|independent --criterion … --evidence … --confidence …".into(),
            "loom inbox add (out-of-scope findings)".into(),
        ],
        forbidden_actions: vec![
            "edit code".into(),
            "record a verdict from name similarity or assumption".into(),
        ],
        required_evidence: "file/line locators, validation output, or runtime evidence".into(),
        write_back: "loom edge explore <a> <b> <ground|issue|independent> --criterion '…' --evidence '…' --confidence <n>".into(),
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

fn fixer_contract(edge: &Edge) -> PromptContract {
    PromptContract {
        role: "fixer".into(),
        mindset: "Use Loom first to understand the stale/failing criterion, linked entities, \
                  likely files, and prior evidence; then inspect the relevant code before editing. \
                  Repair the actual broken behavior at its root cause, not the symptom. Code moving \
                  is not behavior changing. If the product changed, route through intent update, not \
                  a silent code change. Sync and re-route proofs after the fix."
            .into(),
        why_now: format!("{} edge is failing", edge.kind),
        allowed_actions: vec![
            "loom status".into(),
            "loom next --all".into(),
            "loom edge show <edge_id>".into(),
            "loom intent show <linked intent>".into(),
            "loom codefile show <file>".into(),
            "edit code".into(),
            "loom sync".into(),
            "loom edge implement (re-ground after fix)".into(),
            "loom edge explore <a> <b> ground (after confirmed fix)".into(),
        ],
        forbidden_actions: vec![
            "mark passing without re-verification".into(),
            "suppress the symptom without a root-cause fix".into(),
        ],
        required_evidence: "Loom context checked, relevant code inspected, code change, sync clean, the failing criterion now satisfied".into(),
        write_back: "fix code; loom sync; loom edge explore <a> <b> ground --criterion '…' --evidence '…' --confidence <n>".into(),
        stop_condition: "after the fix is grounded + synced, return to loom status".into(),
        human_gate: None,
    }
}

fn quality_contract(store: &Store, edge: &Edge) -> Result<PromptContract> {
    // The rule (edge.from) carries the inspection protocol — embed it so verdicts
    // are consistent across sessions (see docs/llm-driver.md quality contract).
    let rule = store.get_node(&edge.from_id)?;
    let body = rule.as_ref().map(|n| n.body.clone()).unwrap_or_default();
    let guide = body
        .get("inspection_guide")
        .and_then(|v| v.as_str())
        .unwrap_or("inspect the code against this rule")
        .to_string();
    let hints: Vec<String> = body
        .get("detection_hints")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let mut allowed = vec![
        "loom codefile show <file>".into(),
        "read the grounded code".into(),
        "loom rule verdict <rule> <intent> --status passing|failing|independent --criterion '…' --evidence '…' --confidence <n>".into(),
    ];
    allowed.extend(hints.into_iter().map(|h| format!("hint: {h}")));
    Ok(PromptContract {
        role: "quality".into(),
        mindset: format!(
            "Measure this rule at the highest honest altitude. Follow the rule's inspection guide; \
             do not invent your own protocol. independent requires evidence of non-applicability. \
             Guide: {guide}"
        ),
        why_now: format!("governs edge is {}", edge.status),
        allowed_actions: allowed,
        forbidden_actions: vec![
            "edit code".into(),
            "mark passing without inspecting".into(),
            "mark independent without evidence the rule does not apply".into(),
        ],
        required_evidence: "file/line locators showing compliance, violation, or non-applicability".into(),
        write_back: "loom rule verdict <rule> <intent> --status <s> --criterion '…' --evidence '…' --confidence <n>".into(),
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    })
}

fn validator_contract(store: &Store, edge: &Edge) -> Result<PromptContract> {
    let val = store.get_node(&edge.from_id)?;
    let command = val
        .as_ref()
        .and_then(|n| {
            n.body
                .get("command")
                .and_then(|c| c.as_str())
                .map(String::from)
        })
        .unwrap_or_default();
    Ok(PromptContract {
        role: "validator".into(),
        mindset: "Run it; do not guess. Record exactly what the command produced. Do not edit code \
                  to make a proof pass. A blocked proof is honest — record it with a reason.".into(),
        why_now: format!("validates edge is {}", edge.status),
        allowed_actions: vec![
            format!("run: {}", if command.is_empty() { "<no command — manual_check>".into() } else { command }),
            "loom validate <intent>".into(),
            "loom validation mark <validation> --result passed|failed|blocked --evidence '…'".into(),
        ],
        forbidden_actions: vec![
            "edit code to make the proof pass".into(),
            "mark passed without observed proof".into(),
        ],
        required_evidence: "command output, test count, failure message, or a concrete blocker reason".into(),
        write_back: "loom validate <intent>  (or)  loom validation mark <validation> --result <r> --evidence '…'".into(),
        stop_condition: "after recording the result, return to loom status".into(),
        human_gate: None,
    })
}

fn prove_contract(hyp: &Node) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset:
            "An idea is not work until its claim survives contact with the code. Form your own \
                  reading first, then prove or refute the claim. Unproven ideas die honestly."
                .into(),
        why_now: format!("hypothesis '{}' is unproven", hyp.name),
        allowed_actions: vec![
            "read the targeted code".into(),
            "loom hypothesis prove <hypothesis> --verdict supported|refuted --evidence '…'".into(),
        ],
        forbidden_actions: vec![
            "adopt the hypothesis before proving it".into(),
            "edit code".into(),
        ],
        required_evidence: "code evidence that the claim holds or fails".into(),
        write_back:
            "loom hypothesis prove <hypothesis> --verdict <supported|refuted> --evidence '…'".into(),
        stop_condition: "after the verdict, return to loom status".into(),
        human_gate: None,
    }
}

fn triage_contract(id: &str) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset: "Look and decide; do not fix here. Justified, needed, or blocked — record why."
            .into(),
        why_now: "a programmatic flag is unjudged (or its prior judgment went stale when the file changed)".into(),
        allowed_actions: vec![
            format!("loom finding verdict {id} justified --reason <why it is acceptable>"),
            format!("loom finding verdict {id} needed --reason <what to do>"),
            format!("loom finding verdict {id} blocked --reason <what it waits on>"),
        ],
        forbidden_actions: vec![
            "edit code here (mark it needed, then fix in build/fix)".into(),
            "justified without a concrete reason".into(),
        ],
        required_evidence: "a concrete reason: why it is fine, what to do, or what it blocks on"
            .into(),
        write_back: format!("loom finding verdict {id} <justified|needed|blocked> --reason '…'"),
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

fn inbox_triage_contract(id: &str) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset: "Normalize the raw lead. Route it to durable graph work or reject it with a reason; do not let it sit as free text.".into(),
        why_now: "a raw inbox item is still new".into(),
        allowed_actions: vec![
            format!("loom inbox mark {id} triaged --reason <where it was routed>"),
            format!("loom inbox mark {id} rejected --reason <why it is not actionable>"),
            "implemented routing commands: loom intent add / loom hypothesis add / loom rule seed / loom task add".into(),
        ],
        forbidden_actions: vec![
            "leave the item new after using it".into(),
            "drop context without recording the disposition".into(),
        ],
        required_evidence: "the durable destination or concrete rejection reason".into(),
        write_back: format!("loom inbox mark {id} <triaged|rejected|deferred> --reason '…'"),
        stop_condition: "after disposition, return to loom status".into(),
        human_gate: None,
    }
}

/// Resolve a relationship edge kind from a CLI verb (for `loom edge <kind>`).
pub fn relationship_kind(verb: &str) -> Option<EdgeKind> {
    match verb {
        "hierarchy" => Some(EdgeKind::Hierarchy),
        "requires" => Some(EdgeKind::Requires),
        "scenario-of" => Some(EdgeKind::ScenarioOf),
        "variant-of" => Some(EdgeKind::VariantOf),
        "triggers" => Some(EdgeKind::Triggers),
        "sequence" => Some(EdgeKind::Sequence),
        "relates" => Some(EdgeKind::Relates),
        _ => None,
    }
}

/// A one-line graph pulse: where the driver stands. Emitted alongside every
/// work item so an LLM recovering from a compacted context can re-orient.
#[derive(Debug, Clone, Serialize)]
pub struct GraphState {
    pub planned: usize,
    pub stale: usize,
    pub uninspected: usize,
    pub findings: usize,
    pub untriaged: usize,
    pub stale_findings: usize,
    pub needed: usize,
    /// Findings still demanding attention: untriaged OR stale OR verdict=needed.
    pub open_findings: usize,
    /// Findings adjudicated and current (justified/blocked, not stale).
    pub resolved_findings: usize,
    pub inbox: usize,
}

/// The full `loom next --json` envelope: the work item plus the graph pulse.
#[derive(Debug, Clone, Serialize)]
pub struct NextOutput {
    pub work_item: Option<WorkItem>,
    pub graph_state: GraphState,
}

pub fn graph_state(store: &Store) -> Result<GraphState> {
    use crate::model::TruthClass;
    let findings = crate::signal::findings_view(store)?;
    let untriaged = findings.iter().filter(|fv| fv.state == "untriaged").count();
    let stale_findings = findings.iter().filter(|fv| fv.stale).count();
    let needed = findings.iter().filter(|fv| fv.state == "needed").count();
    // One finding can satisfy several open predicates (e.g. needed AND stale);
    // count it once so open + resolved == total with no double-count.
    let open_findings = findings
        .iter()
        .filter(|fv| fv.state == "untriaged" || fv.stale || fv.state == "needed")
        .count();
    let resolved_findings = findings.len() - open_findings;
    Ok(GraphState {
        planned: store
            .nodes_by_status(NodeType::Intent, &["planned", "needs_change"])?
            .len(),
        stale: store
            .edges_by_status(
                TruthClass::Asserted,
                &[
                    InspectionStatus::NeedsReverification,
                    InspectionStatus::Failing,
                ],
            )?
            .len(),
        uninspected: store
            .edges_by_status(TruthClass::Asserted, &[InspectionStatus::Uninspected])?
            .len(),
        findings: findings.len(),
        untriaged,
        stale_findings,
        needed,
        open_findings,
        resolved_findings,
        inbox: store
            .list_nodes(Some(NodeType::InboxItem), usize::MAX)?
            .into_iter()
            .filter(|n| n.status == "new")
            .count(),
    })
}
