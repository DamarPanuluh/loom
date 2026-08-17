//! WorkItem + PromptContract — the LLM-facing surface of `loom next`.
//!
//! Plane: orchestration over the store. loom computes the next correct work and
//! compiles it into a prompt contract: which role/mindset to adopt, what is
//! allowed, what evidence is required, and the exact write-back. The LLM acts
//! and reports through typed graph writes; loom validates and ripples.

mod context;
mod contracts;
mod queues;

use crate::lane::Lane;
use crate::model::{Edge, EdgeKind, InspectionStatus, Node, NodeType};
use crate::store::Store;
use crate::Result;
pub(crate) use queues::analyze_serves;
pub(crate) use queues::ungrounded_implemented_intents;
pub(crate) use queues::unmeasured_quality_pairs;
pub(crate) use queues::unproven_implemented_intents;
pub use queues::unratified_intents;
pub(crate) use queues::validation_work_units;
use queues::{
    analyze_item, audit_item, build_item, coverage_item, deepen_item, derive_item, elaborate_item,
    fix_item, prove_item, quality_item, ratify_item, rectify_item, review_item, surface_item,
    triage_item, validate_item,
};
pub use queues::{queue_items, QueueEntry};
use serde::Serialize;

/// The role/mindset contract the LLM adopts for one work item.
#[derive(Debug, Clone, Serialize)]
pub struct PromptContract {
    pub role: String,
    pub mindset: String,
    pub why_now: String,
    pub allowed_actions: Vec<String>,
    pub forbidden_actions: Vec<String>,
    pub required_evidence: String,
    /// The same requirement, machine-checkable.
    ///
    /// `required_evidence` is prose for the worker; these are the clauses the
    /// write boundary can actually test. Stating a requirement only in a
    /// sentence means the packet asks for one thing and the floor enforces
    /// another, and the worker discovers the gap by being refused.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub evidence_clauses: Vec<EvidenceClause>,
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
    /// What loom's own pattern scan did, when it ran. A CLEAN scan is the
    /// evidence an absence rule needs — "loom ran these patterns over these
    /// files and found nothing" — and reporting only hits made that result
    /// indistinguishable from never having looked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_screen: Option<String>,
    pub write_back: String,
    pub stop_condition: String,
    pub human_gate: Option<HumanGate>,
}

/// A host-facing decision request. It is structured so an LLM can map it to
/// an ask-user tool without inventing choices or hiding consequences.
#[derive(Debug, Clone, Serialize)]
pub struct HumanGate {
    pub question: String,
    pub options: Vec<HumanGateOption>,
    /// How the presenting LLM should form its recommendation. The human still
    /// chooses; this field prevents a content-free menu with no useful advice.
    pub recommendation: String,
    pub after_answer: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HumanGateOption {
    pub id: String,
    pub label: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_back: Option<String>,
}

/// The canonical host-facing decision packet for accepting one exact Journey
/// derivation manifest. Direct `journey derive` callers use the same shape as
/// the derive lane so a missing authority decision is never flattened into a
/// terminal, unactionable blocker.
pub(crate) fn derivation_human_gate(journey: &Node) -> HumanGate {
    let id = crate::model::short(&journey.id);
    HumanGate {
        question: format!("Accept the proposed technical derivation for Journey '{}'?", journey.name),
        options: vec![
            HumanGateOption {
                id: "accept".into(),
                label: "Accept derivation".into(),
                description: "Adopt the manifest's exact hash-table: technical intents, create/reuse operations, stable-step mappings, and relationships for the current Journey hash.".into(),
                write_back: Some(format!(
                    "loom journey derive-accept {id} --manifest <file> --human-decision '<exact human answer>'"
                )),
            },
            HumanGateOption {
                id: "revise".into(),
                label: "Revise manifest".into(),
                description: "Correct intent meaning or step coverage before accepting anything.".into(),
                write_back: None,
            },
            HumanGateOption {
                id: "defer".into(),
                label: "Defer".into(),
                description: "Leave every proposed mapping unaccepted and keep the gap queued.".into(),
                write_back: None,
            },
        ],
        recommendation: "Recommend acceptance only when every authored step is covered by the smallest falsifiable technical intents and no product choice was inferred.".into(),
        after_answer: "Present these options to the human and wait. Accept records their exact answer; revise/defer writes no derivation. Missing human authority is a pause, not a terminal handoff.".into(),
    }
}

/// One machine-checkable requirement on the evidence a packet asks for.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "clause", rename_all = "snake_case")]
pub enum EvidenceClause {
    /// At least `n` `file:line` citations that resolve.
    CitesSpans { n: usize },
    /// A citation into each of these files specifically.
    CitesFiles { files: Vec<String> },
    /// A run loom itself performed.
    CitesRun,
    /// The proof must grade at least this high once loom has run it.
    ProofStrengthAtLeast { grade: String },
    /// The resulting fact must reach at least this verification level.
    VerificationAtLeast { level: String },
    /// Something must exist afterwards that did not before.
    Produces { what: String },
    /// A substantive sentence — the weakest clause, and never the only one on
    /// anything that counts.
    Prose,
}

impl EvidenceClause {
    /// One line a worker can act on.
    pub fn describe(&self) -> String {
        match self {
            EvidenceClause::CitesSpans { n } => {
                format!("cite at least {n} file:line location(s) that exist")
            }
            EvidenceClause::CitesFiles { files } => {
                format!("cite a location in each of: {}", files.join(", "))
            }
            EvidenceClause::CitesRun => {
                "let loom run it — a reported outcome does not count".into()
            }
            EvidenceClause::ProofStrengthAtLeast { grade } => {
                format!("the proof must grade {grade} or better once loom has run it")
            }
            EvidenceClause::VerificationAtLeast { level } => {
                format!("the resulting fact must reach '{level}'")
            }
            EvidenceClause::Produces { what } => format!("produce: {what}"),
            EvidenceClause::Prose => "say what you found, substantively".into(),
        }
    }
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
    /// Canonical facets needed to interpret this linked entity. Work packets
    /// normally omit these; `loom context` fills them for intent provenance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets: Option<std::collections::BTreeMap<String, String>>,
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
    /// Identifies this serving of the packet. Minted at the boundary where the
    /// packet leaves the process (CLI or MCP), never during assembly — see
    /// [`crate::packet`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packet_id: Option<String>,
    /// Live, fail-closed repository guidance for coding packets only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern_guidance: Option<crate::pattern::PatternGuidance>,
    pub mode: String,
    pub owner_role: String,
    pub effort: String,
    /// Orchestrator hint: `mechanical` (cheap re-confirm / fully prefilled
    /// write-back) vs `judgment` (needs fresh inspection). Harnesses map this
    /// to model tiers; loom never names vendors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_hint: Option<String>,
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
    /// it should fill an intent, code, a proof, a journey, a verdict, or an export.
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

/// Compute the next work item, then stamp the policy's human gate on it. Gate
/// placement is portable config ([`crate::policy`]): a repo can require human
/// sign-off for a lane's writes without a code change (default: no lane gated).
pub fn next(store: &Store, lane: Option<Lane>) -> Result<Option<WorkItem>> {
    let mut item = next_inner(store, lane)?;
    if let Some(w) = item.as_mut() {
        enrich_patterns(store, w)?;
        let policy = crate::policy::load(store)?;
        if policy.gates_role(&w.owner_role) {
            w.prompt_contract.human_gate = Some(HumanGate {
                question: format!(
                    "May the {} lane record the proposed write for this packet?",
                    w.owner_role
                ),
                options: vec![
                    HumanGateOption {
                        id: "approve".into(),
                        label: "Approve write".into(),
                        description: "Allow the lane to record this packet's proposed write.".into(),
                        write_back: None,
                    },
                    HumanGateOption {
                        id: "defer".into(),
                        label: "Defer".into(),
                        description: "Leave the packet unchanged for later review.".into(),
                        write_back: None,
                    },
                ],
                recommendation: "Recommend approve only when the packet's required evidence is satisfied; otherwise recommend defer.".into(),
                after_answer: "Wait for the human answer before recording the gated write.".into(),
            });
        }
    }
    Ok(item)
}

fn enrich_patterns(store: &Store, item: &mut WorkItem) -> Result<()> {
    if item.mode != "build" && item.mode != "fix" && item.mode != "surface" {
        return Ok(());
    }
    let paths: Vec<String> = item
        .context
        .read_set
        .iter()
        .map(|read| read.path.clone())
        .collect();
    let mut intent_ids = Vec::new();
    if item.mode == "build" && item.target.kind == NodeType::Intent.as_str() {
        intent_ids.push(item.target.id.clone());
    } else if item.mode == "surface" {
        for edge in store.edges_with(Some(EdgeKind::Derives), Some(&item.target.id), None)? {
            if edge.status != InspectionStatus::Failing
                && edge.status != InspectionStatus::NeedsReverification
            {
                intent_ids.push(edge.to_id);
            }
        }
    } else if item.mode == "fix" {
        if let Some(edge) = store.get_edge(&item.target.id)? {
            for id in [&edge.from_id, &edge.to_id] {
                if store
                    .get_node(id)?
                    .is_some_and(|node| node.node_type == NodeType::Intent)
                {
                    intent_ids.push(id.clone());
                }
            }
        }
    }
    let mut tags = Vec::new();
    for id in intent_ids {
        tags.extend(store.tags_of(&id, crate::model::TargetKind::Node)?);
    }
    tags.sort();
    tags.dedup();
    let guidance = crate::pattern::guidance(store, &paths, &tags)?;
    if guidance.matched != 0 {
        item.pattern_guidance = Some(guidance);
    }
    Ok(())
}

/// Compile the work packet for one lane. The single dispatch point: adding a
/// lane means adding an arm here and an entry in `Lane::LADDER`, never a new
/// priority list.
pub(crate) fn lane_item(store: &Store, lane: Lane) -> Result<Option<WorkItem>> {
    // An observed graph maps code the driver does not own: discovery, quality,
    // and validation work only — the build and fix lanes are disabled, because a
    // monitor cannot change the upstream it watches (docs/commands.md:90).
    if lane.observed_disabled() && store.identity()?.observed {
        return Ok(None);
    }
    match lane {
        Lane::Derive => derive_item(store),
        Lane::Build => build_item(store),
        Lane::Surface => surface_item(store),
        Lane::Coverage => coverage_item(store),
        Lane::Fix => fix_item(store),
        Lane::Analyze => analyze_item(store),
        Lane::Quality => quality_item(store),
        Lane::Validate => validate_item(store),
        Lane::Prove => prove_item(store),
        Lane::Triage => triage_item(store),
        Lane::Review => review_item(store),
        Lane::Divergence => ratify_item(store),
        Lane::Rectify => rectify_item(store),
        Lane::Elaborate => elaborate_item(store),
        // Lanes that route to a whole-graph command rather than a per-item
        // packet (`loom door`, `loom doctor`, `loom export`).
        Lane::Audit => audit_item(store),
        Lane::Deepen => deepen_item(store),
        Lane::Seed | Lane::Export => Ok(None),
    }
}

/// Compute the next work item for a lane, or the highest-priority item overall.
///
/// The default (no lane) walks `Lane::LADDER` in order — the SAME order the
/// maturity rungs and the compass use. There is no second priority list to keep
/// in step.
///
/// Contract invariant (uniform adjudicability): every served packet's
/// write_back names the runnable loom command(s) that close it, and — for
/// every lane whose closure is a graph write — that command accepts the
/// packet's own target (id, name, or edge endpoints). An item whose closure
/// command cannot be named is a loom defect, not work: it is journaled as
/// `unservable_packet` and never handed to a worker. The default walk skips
/// it; an explicit `--mode` refuses with the defect named.
fn next_inner(store: &Store, lane: Option<Lane>) -> Result<Option<WorkItem>> {
    match lane {
        Some(l) => {
            let item = lane_item(store, l)?;
            if let Some(w) = &item {
                if let Some(problem) = closure_problem(w) {
                    journal_unservable(store, w, &problem)?;
                    anyhow::bail!(
                        "unservable packet (a loom defect, not work): {problem} — target '{}' \
                         stays queued and the defect is journaled as unservable_packet",
                        w.target.id
                    );
                }
            }
            Ok(item)
        }
        None => {
            for &l in Lane::LADDER {
                // Human-decision lanes are served ONLY on explicit request so
                // an autonomous loop never blocks waiting for conversation.
                if l.requires_human_decision() || !l.serves_items() {
                    continue;
                }
                let Some(w) = lane_item(store, l)? else {
                    continue;
                };
                match closure_problem(&w) {
                    Some(problem) => journal_unservable(store, &w, &problem)?,
                    None => return Ok(Some(w)),
                }
            }
            Ok(None)
        }
    }
}

/// Why this item may not be served, or None. Pure in the item: the same
/// packet always gets the same answer, so a journaled defect does not flap
/// with graph state.
pub(crate) fn closure_problem(item: &WorkItem) -> Option<String> {
    let commands = named_commands(&item.prompt_contract.write_back);
    if commands.is_empty() {
        return Some(format!(
            "mode '{}' names no runnable loom command in its write_back",
            item.mode
        ));
    }
    // Fix and audit packets close through STATE re-reads (sync re-checks the
    // code; audit re-reads the record), not a write keyed by the target id —
    // their uniform closeout names the runnable command without a target
    // argument. Graph-wide subjects have no target id to accept.
    if STATE_CLOSED.contains(&item.mode.as_str()) || item.target.kind == "graph" {
        return None;
    }
    // The closure command must accept the packet's own target. Commands carry
    // the full id, the short id prefix, the name, or — for edge pairs — the
    // endpoint names; name-resolving commands make all of them equivalent.
    let short: String = item.target.id.chars().take(8).collect();
    let handles: Vec<&str> = [
        Some(item.target.id.as_str()),
        Some(item.target.name.as_str()),
        item.target.from.as_deref(),
        item.target.to.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|h| !h.is_empty())
    .collect();
    if commands
        .iter()
        .any(|c| handles.iter().any(|h| c.contains(h)) || c.contains(short.as_str()))
    {
        None
    } else {
        Some(format!(
            "mode '{}' write_back names no closure command accepting its target ('{}')",
            item.mode, item.target.id
        ))
    }
}

/// Modes whose closure is a state change loom re-reads, not a graph write
/// naming the target: the runnable closeout (`loom sync`, `loom audit`) does
/// not take the target id.
const STATE_CLOSED: &[&str] = &["fix", "audit"];

/// The loom commands a write_back names: `;`- and newline-separated segments
/// containing `loom `. Multi-command write_backs separate commands that way.
fn named_commands(write_back: &str) -> Vec<&str> {
    write_back
        .split([';', '\n'])
        .map(str::trim)
        .filter(|s| s.contains("loom "))
        .collect()
}

/// Record the defect that made a packet unservable. Propagates: a defect we
/// cannot journal is a defect we must not hide.
fn journal_unservable(store: &Store, item: &WorkItem, problem: &str) -> Result<()> {
    store.append_journal(
        "unservable_packet",
        &item.target.id,
        serde_json::json!({
            "mode": item.mode,
            "problem": problem,
            "write_back": item.prompt_contract.write_back,
        }),
    )?;
    Ok(())
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

pub(crate) fn effort_for(edge: &Edge) -> String {
    match edge.status {
        InspectionStatus::Failing => "high".into(),
        _ => "mid".into(),
    }
}

/// Classify a stale cause for roster filtering: `cheap` | `full` | `other`.
///
/// Reads the TYPED cause loom recorded — `StaleCause::rework()` is the cost
/// class, and it is a method on the enum rather than a substring recovered from
/// a sentence. The prose grades this used to grep for ("cheap re-confirm",
/// "full re-inspection") were written by one module and parsed back out by
/// another, which meant rewording a message silently rerouted work.
pub(crate) fn cause_class(stale_causes: &[String]) -> &'static str {
    let worst = stale_causes
        .iter()
        .filter_map(|c| {
            c.rsplit_once('(')
                .and_then(|(_, tail)| tail.strip_suffix(')'))
                .and_then(|tok| tok.parse::<crate::model::StaleCause>().ok())
        })
        .map(|c| c.rework())
        .fold(None, |acc: Option<crate::model::Rework>, r| match acc {
            // Reinspect dominates: a batch is only cheap if all of it is.
            Some(crate::model::Rework::Reinspect) => acc,
            _ if r == crate::model::Rework::Reinspect => Some(r),
            None => Some(r),
            _ => acc,
        });
    match worst {
        Some(crate::model::Rework::Reconfirm) => "cheap",
        Some(crate::model::Rework::Reinspect) => "full",
        _ => "other",
    }
}

/// Refine effort + routing_hint from sync grading and packet shape.
///
/// - `cheap re-confirm` → effort `low`, hint `mechanical`
/// - `full re-inspection` / rewritten evidence → hint `judgment`, keep base effort
/// - else: fully prefilled write_back + non-empty prior criterion + small read_set
///   → `mechanical`; otherwise `judgment`
pub(crate) fn refine_effort_and_hint(
    base_effort: String,
    stale_causes: &[String],
    write_back: &str,
    prior_criterion: &str,
    read_set_len: usize,
) -> (String, Option<String>) {
    match cause_class(stale_causes) {
        "cheap" => ("low".into(), Some("mechanical".into())),
        "full" => (base_effort, Some("judgment".into())),
        _ => {
            let templated = write_back.contains('<') && write_back.contains('>');
            let mechanical = !templated && !prior_criterion.trim().is_empty() && read_set_len <= 3;
            if mechanical {
                (base_effort, Some("mechanical".into()))
            } else {
                (base_effort, Some("judgment".into()))
            }
        }
    }
}

pub(crate) fn hint_judgment() -> Option<String> {
    Some("judgment".into())
}

pub(crate) fn hint_mechanical() -> Option<String> {
    Some("mechanical".into())
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
    /// Findings still demanding attention: untriaged OR stale OR verdict=needed
    /// OR verdict=blocked.
    pub open_findings: usize,
    /// Findings adjudicated and current with resolving verdicts.
    pub resolved_findings: usize,
    pub inbox: usize,
    /// Recorded verdicts standing below the review confidence floor: honest
    /// uncertainty awaiting independent re-inspection (`loom next --mode review`).
    pub low_confidence: usize,
    /// Open product questions raised for the human. These are the human-gated
    /// remainder: batch them into one conversation window instead of interrupting
    /// per question.
    pub open_questions: usize,
}

/// The full `loom next --json` envelope: the work item plus the graph pulse.
#[derive(Debug, Clone, Serialize)]
pub struct NextOutput {
    pub work_item: Option<WorkItem>,
    pub graph_state: GraphState,
    /// Advisory collision notice: the served packet's owning role is freshly
    /// leased to a different profile (see `rolelease::conflict_warning`).
    /// Absent when there is no conflict, so single-driver output is unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_conflict: Option<String>,
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
        .filter(|fv| {
            fv.state == "untriaged" || fv.stale || fv.state == "needed" || fv.state == "blocked"
        })
        .count();
    let resolved_findings = findings.len() - open_findings;
    let floor = crate::policy::load(store)?.review_confidence_floor;
    Ok(GraphState {
        planned: store
            .nodes_by_status(NodeType::Intent, &["planned", "needs_change"])?
            .len(),
        stale: store
            .live_edges_by_status(
                TruthClass::Asserted,
                &[
                    InspectionStatus::NeedsReverification,
                    InspectionStatus::Failing,
                ],
            )?
            .len(),
        uninspected: store
            .live_edges_by_status(TruthClass::Asserted, &[InspectionStatus::Uninspected])?
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
            .list_nodes(Some(NodeType::Question), usize::MAX)?
            .into_iter()
            .filter(|n| n.status == "open")
            .count(),
        low_confidence: store
            .live_edges_by_status(
                TruthClass::Asserted,
                &[InspectionStatus::Passing, InspectionStatus::Independent],
            )?
            .into_iter()
            .filter(|e| e.confidence > 0.0 && e.confidence < floor)
            .count(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(mode: &str, kind: &str, id: &str, name: &str, write_back: &str) -> WorkItem {
        WorkItem {
            packet_id: None,
            pattern_guidance: None,
            mode: mode.into(),
            owner_role: "analyzer".into(),
            effort: "mid".into(),
            routing_hint: None,
            reason: String::new(),
            target: Target {
                kind: kind.into(),
                id: id.into(),
                name: name.into(),
                from: None,
                to: None,
            },
            stale_causes: Vec::new(),
            prompt_contract: PromptContract {
                role: "analyzer".into(),
                mindset: String::new(),
                why_now: String::new(),
                allowed_actions: Vec::new(),
                forbidden_actions: Vec::new(),
                evidence_clauses: Vec::new(),
                required_evidence: String::new(),
                evidence_template: None,
                examples: None,
                pre_screen: None,
                pre_screened_hits: Vec::new(),
                write_back: write_back.into(),
                stop_condition: String::new(),
                human_gate: None,
            },
            context: TraversalContext {
                purpose: String::new(),
                linked_entities: Vec::new(),
                suggested_reads: Vec::new(),
                read_set: Vec::new(),
            },
            scorecard: None,
            truth_gap: crate::truth::TruthAxis::Intent.gap(),
            next_step: String::new(),
        }
    }

    #[test]
    fn prose_write_back_is_unservable() {
        let it = item(
            "triage",
            "finding",
            "abc123def456",
            "f",
            "look and decide; do not fix here",
        );
        assert!(closure_problem(&it)
            .expect("prose cannot close anything")
            .contains("no runnable loom command"));
    }

    #[test]
    fn a_command_that_never_names_the_target_is_unservable() {
        let it = item("triage", "finding", "abc123def456", "f", "loom status");
        assert!(closure_problem(&it)
            .expect("loom status accepts no target")
            .contains("accepting its target"));
    }

    #[test]
    fn the_short_id_prefix_counts_as_accepting_the_target() {
        let it = item(
            "triage",
            "finding",
            "abc123def4567890",
            "f",
            "loom finding verdict abc123de justified --reason '…'",
        );
        assert_eq!(closure_problem(&it), None);
    }

    #[test]
    fn name_resolving_commands_accept_the_target_by_endpoint_name() {
        let mut it = item(
            "quality",
            "edge",
            "edgeid",
            "no-prints —governs→ users can check out",
            "loom rule verdict no-prints 'users can check out' passing --criterion '…'",
        );
        it.target.from = Some("no-prints".into());
        it.target.to = Some("users can check out".into());
        assert_eq!(closure_problem(&it), None);
    }

    #[test]
    fn state_closed_lanes_need_the_closeout_command_not_a_target_argument() {
        let it = item(
            "fix",
            "edge",
            "edgeid",
            "x",
            "fix the source at root cause, then loom sync — sync re-opens this claim",
        );
        assert_eq!(closure_problem(&it), None);
        // A graph subject has no target id, but a runnable closeout is still owed.
        let it = item("audit", "graph", "g", "g", "inspect the record");
        assert!(closure_problem(&it).is_some());
        let it = item(
            "audit",
            "graph",
            "g",
            "g",
            "fix per the remedy, then loom audit --json",
        );
        assert_eq!(closure_problem(&it), None);
    }

    #[test]
    fn an_intent_proof_contract_never_invents_a_legacy_journey() {
        let intent = crate::model::Node {
            id: "abc123def4567890".into(),
            node_type: crate::model::NodeType::Intent,
            name: "users can check out".into(),
            description: "d".into(),
            status: "implemented".into(),
            truth_class: crate::model::TruthClass::Asserted,
            body: serde_json::json!({}),
            created_at: String::new(),
            updated_at: String::new(),
        };
        let proof = crate::proofstrength::ProofAssessment {
            any_registered: true,
            any_passing: true,
            best_passing_strength: Some(crate::proofstrength::Strength::S2),
            meaningful_passing: true,
        };
        let mut it = item("validate", "intent", &intent.id, &intent.name, "");
        it.prompt_contract = super::contracts::unproven_contract(&intent, proof);
        it.target = node_target(&intent);
        assert!(!it.prompt_contract.write_back.contains("loom journey"));
        assert!(
            it.prompt_contract.evidence_clauses.iter().any(|c| matches!(
                c, EvidenceClause::ProofStrengthAtLeast { grade } if grade == "S2"
            )),
            "Intent proof remains the S2 floor: {:?}",
            it.prompt_contract.evidence_clauses
        );
    }
}
