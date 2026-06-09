use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AbstractionLevel {
    Feature,
    Component,
    System,
    CrossCutting,
}

impl std::str::FromStr for AbstractionLevel {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "feature"                    => Ok(Self::Feature),
            "component"                  => Ok(Self::Component),
            "system"                     => Ok(Self::System),
            "cross_cutting" | "cross-cutting" => Ok(Self::CrossCutting),
            other => anyhow::bail!(
                "Unknown abstraction level '{}'. Valid: feature, component, system, cross_cutting",
                other
            ),
        }
    }
}

impl std::fmt::Display for AbstractionLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Feature      => write!(f, "feature"),
            Self::Component    => write!(f, "component"),
            Self::System       => write!(f, "system"),
            Self::CrossCutting => write!(f, "cross_cutting"),
        }
    }
}

// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntentStatus {
    Proposed,
    Confirmed,
    Deprecated,
}

impl std::fmt::Display for IntentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Proposed   => write!(f, "proposed"),
            Self::Confirmed  => write!(f, "confirmed"),
            Self::Deprecated => write!(f, "deprecated"),
        }
    }
}

impl std::str::FromStr for IntentStatus {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "proposed"   => Ok(Self::Proposed),
            "confirmed"  => Ok(Self::Confirmed),
            "deprecated" => Ok(Self::Deprecated),
            other => anyhow::bail!(
                "Unknown intent status '{}'. Valid: proposed, confirmed, deprecated",
                other
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// InspectionStatus — replaces the old EdgeStatus
// Applies to ALL edge types as `inspection_status`.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InspectionStatus {
    /// Edge has not yet been examined (default for new edges).
    Uninspected,
    /// Inspection confirmed no problem — coexistence criterion is met.
    Passing,
    /// Inspection found a problem — criterion is violated.
    Failing,
    /// RELATES_TO specific: the two intents are confirmed unrelated.
    Independent,
    /// A code change has invalidated a previous passing/independent result.
    NeedsReverification,
}

impl InspectionStatus {
    /// Higher = more urgent. Used in priority scoring for `loom next`.
    pub fn urgency(&self) -> f64 {
        match self {
            Self::Failing              => 4.0,
            Self::NeedsReverification  => 3.0,
            Self::Uninspected          => 2.0,
            _ => 0.0,
        }
    }
}

impl std::fmt::Display for InspectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uninspected          => write!(f, "uninspected"),
            Self::Passing              => write!(f, "passing"),
            Self::Failing              => write!(f, "failing"),
            Self::Independent          => write!(f, "independent"),
            Self::NeedsReverification  => write!(f, "needs_reverification"),
        }
    }
}

impl std::str::FromStr for InspectionStatus {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "uninspected"         => Ok(Self::Uninspected),
            "passing"             => Ok(Self::Passing),
            "failing"             => Ok(Self::Failing),
            "independent"         => Ok(Self::Independent),
            "needs_reverification" => Ok(Self::NeedsReverification),
            other => anyhow::bail!(
                "Unknown inspection status '{}'. Valid: uninspected, passing, failing, independent, needs_reverification",
                other
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// ValidationType
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValidationType {
    Test,
    Assertion,
    Benchmark,
    ManualCheck,
}

impl std::fmt::Display for ValidationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Test        => write!(f, "test"),
            Self::Assertion   => write!(f, "assertion"),
            Self::Benchmark   => write!(f, "benchmark"),
            Self::ManualCheck => write!(f, "manual_check"),
        }
    }
}

impl std::str::FromStr for ValidationType {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "test"         => Ok(Self::Test),
            "assertion"    => Ok(Self::Assertion),
            "benchmark"    => Ok(Self::Benchmark),
            "manual_check" => Ok(Self::ManualCheck),
            other => anyhow::bail!(
                "Unknown validation type '{}'. Valid: test, assertion, benchmark, manual_check",
                other
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// ValidationResult
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValidationResult {
    Passed,
    Failed,
    NotRun,
}

impl std::fmt::Display for ValidationResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passed  => write!(f, "passed"),
            Self::Failed  => write!(f, "failed"),
            Self::NotRun  => write!(f, "not_run"),
        }
    }
}

impl std::str::FromStr for ValidationResult {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "passed"  => Ok(Self::Passed),
            "failed"  => Ok(Self::Failed),
            "not_run" => Ok(Self::NotRun),
            other => anyhow::bail!(
                "Unknown validation result '{}'. Valid: passed, failed, not_run",
                other
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// EdgeType
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EdgeType {
    RelatesTo,
    Hierarchy,
    Implements,
    Governs,
    Validates,
}

impl std::fmt::Display for EdgeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RelatesTo => write!(f, "RELATES_TO"),
            Self::Hierarchy => write!(f, "HIERARCHY"),
            Self::Implements => write!(f, "IMPLEMENTS"),
            Self::Governs   => write!(f, "GOVERNS"),
            Self::Validates => write!(f, "VALIDATES"),
        }
    }
}

// ---------------------------------------------------------------------------
// Severity (kept for QualityRule)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    Error,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Warning => write!(f, "warning"),
            Self::Error   => write!(f, "error"),
        }
    }
}

impl std::str::FromStr for Severity {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "warning" => Ok(Self::Warning),
            "error"   => Ok(Self::Error),
            other => anyhow::bail!("Unknown severity '{}'. Valid: warning, error", other),
        }
    }
}

// ---------------------------------------------------------------------------
// NoteKind — the flavour of a free-text annotation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    /// Why a verdict was reached.
    Justification,
    /// General remark.
    Commentary,
    /// A proposal or improvement to consider.
    Idea,
    /// Something unresolved to revisit.
    Question,
    /// A choice made and its rationale.
    Decision,
    /// A follow-up action.
    Todo,
}

impl std::fmt::Display for NoteKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Justification => "justification",
            Self::Commentary    => "commentary",
            Self::Idea          => "idea",
            Self::Question      => "question",
            Self::Decision      => "decision",
            Self::Todo          => "todo",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for NoteKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "justification" => Ok(Self::Justification),
            "commentary"    => Ok(Self::Commentary),
            "idea"          => Ok(Self::Idea),
            "question"      => Ok(Self::Question),
            "decision"      => Ok(Self::Decision),
            "todo"          => Ok(Self::Todo),
            other => anyhow::bail!(
                "Unknown note kind '{}'. Valid: justification, commentary, idea, question, decision, todo",
                other
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// LifecycleState — the prescriptive axis on an Intent (does code need work?)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    /// Designed but not built yet (greenfield). The intent's criteria are a spec.
    Planned,
    /// Code exists for this intent (the brownfield default).
    Implemented,
    /// Code exists but must change (refactor / known issue). The criterion
    /// describes the desired end state; notes hold the rationale.
    NeedsChange,
}

impl std::fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Planned => "planned",
            Self::Implemented => "implemented",
            Self::NeedsChange => "needs_change",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for LifecycleState {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "planned" => Ok(Self::Planned),
            "implemented" => Ok(Self::Implemented),
            "needs_change" => Ok(Self::NeedsChange),
            other => anyhow::bail!(
                "Unknown lifecycle '{}'. Valid: planned, implemented, needs_change",
                other
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Node types (4 total)
// ---------------------------------------------------------------------------

/// What code is supposed to do.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    pub id: String,
    pub name: String,
    pub description: String,
    pub abstraction_level: String,
    pub domain: String,
    /// JSON-encoded array of file path strings.
    pub source_refs: String,
    pub status: String,
    /// Behavioural facet for completeness: happy | sad | fallback | … (open
    /// vocabulary; "" = unspecified).
    pub aspect: String,
    /// Implementation lifecycle: planned | implemented | needs_change.
    pub lifecycle: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Physical file on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeFile {
    pub id: String,
    pub path: String,
    pub language: String,
    pub last_modified: String,
}

/// Named anti-pattern rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityRule {
    pub id: String,
    pub name: String,
    pub description: String,
    pub detection_logic: String,
    pub severity: String,
}

/// Explicit proof object that an intent is fulfilled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Validation {
    pub id: String,
    pub name: String,
    pub description: String,
    /// test | assertion | benchmark | manual_check
    pub validation_type: String,
    /// Shell command to run, e.g. "cargo test --test foo"
    pub command: String,
    /// RFC3339 timestamp of last run (empty = never run)
    pub last_run: String,
    /// passed | failed | not_run
    pub last_result: String,
}

// ---------------------------------------------------------------------------
// Note — append-only free-text memory (justification, idea, question, …)
// ---------------------------------------------------------------------------

/// A timestamped annotation. Optionally scoped to a target (an intent or an
/// edge) via `target_kind` + `target_id`; a note with `target_kind = "none"`
/// floats free (e.g. a standalone idea not yet tied to anything). Append-only:
/// notes accumulate, they are never overwritten — that is the richer memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    /// justification | commentary | idea | question | decision | todo
    pub kind: String,
    pub text: String,
    /// "human" | "llm"
    pub author: String,
    /// "intent" | "edge" | "none"
    pub target_kind: String,
    /// id of the targeted intent/edge, or "" when floating
    pub target_id: String,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Ignore — a coverage exclusion pattern (the escape hatch), recorded with a why
// ---------------------------------------------------------------------------

/// A glob pattern marking files as intentionally out-of-scope for coverage —
/// the physical-plane analogue of `independent`: a recorded decision ("we looked;
/// these don't need an intent, because <reason>"), not a silent gap. Lives in the
/// graph (not a .loomignore file) so it's queryable and `doctor`-checkable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ignore {
    pub id: String,
    pub pattern: String,
    pub reason: String,
    pub author: String,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Edge types (5 total)
// ---------------------------------------------------------------------------

/// RELATES_TO: Intent ↔ Intent — any tracked relationship worth inspecting.
/// `inspection_status = independent` replaces the old CONFIRMED_INDEPENDENT edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatesTo {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    pub from_name: String,
    pub to_name: String,
    // --- State (workflow-critical) ---
    pub inspection_status: String,
    // --- Meta (evidence layer) ---
    pub criterion: String,
    pub confidence: f64,
    pub evidence: String,
    pub last_inspected: String,
    /// Who inspected — role-aware provenance, e.g. "llm:analyzer", "human".
    /// Resolved from --inspected-by / $LOOM_AGENT (see `crate::agent`).
    pub inspected_by: String,
    pub priority_score: f64,
    pub notes: String,
}

/// HIERARCHY: Intent → Intent — parent/child zoom relationship.
/// A structural tree edge, enforced at insert (unique parent, no cycles); it is
/// never "inspected", so it carries no inspection_status (dropped in schema v3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hierarchy {
    pub id: String,
    pub parent_id: String,
    pub child_id: String,
    pub parent_name: String,
    pub child_name: String,
    pub notes: String,
}

/// IMPLEMENTS: Intent → CodeFile — where in code this intent lives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Implements {
    pub id: String,
    pub intent_id: String,
    pub codefile_id: String,
    pub intent_name: String,
    pub codefile_path: String,
    // --- State ---
    pub inspection_status: String,
    // --- Meta ---
    pub criterion: String,
    pub confidence: f64,
    pub evidence: String,
    pub last_inspected: String,
    pub inspected_by: String,
    /// Finer-than-file anchor inside the CodeFile (symbol/region), or "".
    pub locator: String,
    pub notes: String,
}

/// GOVERNS: QualityRule → Intent — this rule applies to this intent.
/// Replaces both MUST_COMPLY_WITH and VIOLATES.
/// `inspection_status = failing` means a violation was found.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Governs {
    pub id: String,
    pub rule_id: String,
    pub intent_id: String,
    pub rule_name: String,
    pub intent_name: String,
    // --- State ---
    pub inspection_status: String,
    // --- Meta ---
    pub criterion: String,
    pub confidence: f64,
    pub evidence: String,
    pub last_inspected: String,
    pub inspected_by: String,
    pub notes: String,
}

/// VALIDATES: Validation → Intent — this validation proves intent is fulfilled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatesEdge {
    pub id: String,
    pub validation_id: String,
    pub intent_id: String,
    pub validation_name: String,
    pub intent_name: String,
    // --- State ---
    pub inspection_status: String,
    pub notes: String,
}

// ---------------------------------------------------------------------------
// Composite output types
// ---------------------------------------------------------------------------

/// Rich work item returned by `loom next` — includes all context an LLM needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItem {
    pub edge_type: String,
    pub edge_id: String,
    pub inspection_status: String,
    pub criterion: String,
    pub evidence: String,
    pub priority_score: f64,
    /// The subject intent (always present).
    pub intent_a: Intent,
    /// The related intent (present for RELATES_TO edges).
    pub intent_b: Option<Intent>,
    /// Code files touched by these intents (from IMPLEMENTS edges).
    pub code_files: Vec<CodeFile>,
    /// IMPLEMENTS edges for intent_a, carrying the finer-grained `locator`
    /// (symbol/region) where the intent is grounded.
    pub implements: Vec<Implements>,
    /// Validations linked to intent_a (from VALIDATES edges).
    pub validations: Vec<Validation>,
    /// Accumulated free-text memory relevant to this work item — notes on the
    /// edge and on both intents. Lets prior reasoning travel with the task.
    pub notes: Vec<Note>,
    /// Recommended action string for the LLM.
    pub suggested_action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReport {
    pub total_intents: i64,
    pub total_codefiles: i64,
    pub total_validations: i64,
    /// Sum of all edges across all types.
    pub total_edges: i64,
    // Breakdown by inspection_status (across all edge types)
    pub uninspected_edges: i64,
    pub passing_edges: i64,
    pub failing_edges: i64,
    pub independent_edges: i64,
    pub needs_reverification: i64,
    /// Intents with zero VALIDATES edges — no proof of fulfillment.
    pub intents_without_validations: i64,
    /// Fraction of Validation.last_result == "passed".
    pub validation_pass_rate: f64,
    /// Convenience alias: failing_edges (RELATES_TO + GOVERNS).
    pub open_issues: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullReport {
    pub status: StatusReport,
    pub top_intents_by_centrality: Vec<IntentCentrality>,
    /// Intents that have no VALIDATES edge — risky, no proof.
    pub intents_without_validations: Vec<Intent>,
    /// GOVERNS edges with inspection_status = failing.
    pub failing_governs: Vec<Governs>,
    /// Recently updated passing RELATES_TO edges.
    pub recent_passing: Vec<RelatesTo>,
    /// Raw counts by (edge_type, inspection_status) pair.
    pub edge_counts_by_status: std::collections::HashMap<String, i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentCentrality {
    pub intent: Intent,
    pub degree: i64,
}

/// Summary produced by `loom sync`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReport {
    pub files_checked: usize,
    pub files_changed: usize,
    pub relates_to_edges_flagged: usize,
    /// Passing GOVERNS edges flipped to needs_reverification — quality green
    /// must be re-earned after the code it judged changes.
    pub governs_edges_flagged: usize,
    pub validations_invalidated: usize,
    /// Registered CodeFiles that no longer exist on disk (deleted/renamed).
    /// Phantom files distort coverage — remove them (`loom codefile remove`)
    /// or restore them.
    pub missing_files: Vec<String>,
    pub changes: Vec<SyncChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncChange {
    pub path: String,
    pub codefile_id: String,
    pub new_mtime: String,
}
