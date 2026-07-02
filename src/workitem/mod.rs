//! WorkItem + PromptContract — the LLM-facing surface of `loom next`.
//!
//! Plane: orchestration over the store. loom computes the next correct work and
//! compiles it into a prompt contract: which role/mindset to adopt, what is
//! allowed, what evidence is required, and the exact write-back. The LLM acts
//! and reports through typed graph writes; loom validates and ripples.

mod context;
mod contracts;
mod queues;

use crate::model::{Edge, EdgeKind, InspectionStatus, Node, NodeType};
use crate::store::Store;
use crate::Result;
use queues::{
    analyze_item, build_item, coverage_item, elaborate_item, fix_item, prove_item, quality_item,
    review_item, triage_item, validate_item,
};
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
    Review,
    Elaborate,
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
            "review" => Some(Mode::Review),
            "elaborate" => Some(Mode::Elaborate),
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
    /// Rule-authored phrasing templates for passing/failing evidence (quality
    /// items only). Using them keeps verdicts comparable across sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_template: Option<serde_json::Value>,
    /// Rule-authored few-shot verdict examples (quality items only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<serde_json::Value>,
    /// Machine pre-screened pattern hits for this rule against the intent's
    /// grounded files (quality items only). Computed on read, never stored:
    /// candidates for the LLM to CONFIRM or REFUTE, never verdicts.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pre_screened_hits: Vec<crate::prescan::PreScreenHit>,
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
    /// Concrete files to open, with the locator that points inside each. This
    /// is the packet's read set: a small-context worker starts here instead of
    /// issuing follow-up show commands.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub read_set: Vec<FileRead>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LinkedEntity {
    pub role: String,
    pub kind: String,
    pub id: String,
    pub name: String,
    /// The node's own description (its behavioral criterion, claim, or body
    /// text). Inlined for the target and edge endpoints so the packet is
    /// actionable without a second lookup; omitted for peripheral entities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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

/// One entry of a work item's read set: a real file path plus the locator that
/// narrows where to look inside it.
#[derive(Debug, Clone, Serialize)]
pub struct FileRead {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    pub why: String,
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
    /// Typed refs naming what change triggered this item's staleness, so the
    /// worker can trace root cause without exploratory lookups.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stale_causes: Vec<String>,
    pub prompt_contract: PromptContract,
    pub context: TraversalContext,
    /// The completeness scorecard (elaborate items only): which axes around
    /// this idea are met, open, or waived.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scorecard: Option<serde_json::Value>,
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
        Some(Mode::Review) => review_item(store),
        Some(Mode::Elaborate) if observed => Ok(None),
        Some(Mode::Elaborate) => elaborate_item(store),
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
            if let Some(w) = triage_item(store)? {
                return Ok(Some(w));
            }
            if let Some(w) = review_item(store)? {
                return Ok(Some(w));
            }
            // Last: grow the surroundings of user-visible ideas. Elaboration
            // creates NEW work, so it only surfaces once existing debts are
            // drained.
            if !observed {
                return elaborate_item(store);
            }
            Ok(None)
        }
    }
}

/// Verdicts below this confidence are not settled truth; they route to review.
pub const REVIEW_CONFIDENCE_FLOOR: f64 = 0.7;

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

/// Shell-quote a name for a prefilled command: single quotes, close/reopen for
/// embedded quotes. Workers copy these commands verbatim — never make a
/// small-context model do its own substitution.
pub(crate) fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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
    /// Recorded verdicts standing below the review confidence floor: honest
    /// uncertainty awaiting independent re-inspection (`loom next --mode review`).
    pub low_confidence: usize,
    /// Unanswered questions raised for the human (inbox items with source
    /// `question`). These are the human-gated remainder: batch them into one
    /// conversation window instead of interrupting per question.
    pub open_questions: usize,
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
        open_questions: store
            .list_nodes(Some(NodeType::InboxItem), usize::MAX)?
            .into_iter()
            .filter(|n| {
                n.status == "new"
                    && n.body.get("source").and_then(|v| v.as_str()) == Some("question")
            })
            .count(),
        low_confidence: store
            .edges_by_status(
                TruthClass::Asserted,
                &[InspectionStatus::Passing, InspectionStatus::Independent],
            )?
            .into_iter()
            .filter(|e| e.confidence > 0.0 && e.confidence < REVIEW_CONFIDENCE_FLOOR)
            .count(),
    })
}
