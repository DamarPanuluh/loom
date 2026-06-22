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
            "feature" => Ok(Self::Feature),
            "component" => Ok(Self::Component),
            "system" => Ok(Self::System),
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
            Self::Feature => write!(f, "feature"),
            Self::Component => write!(f, "component"),
            Self::System => write!(f, "system"),
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
            Self::Proposed => write!(f, "proposed"),
            Self::Confirmed => write!(f, "confirmed"),
            Self::Deprecated => write!(f, "deprecated"),
        }
    }
}

impl std::str::FromStr for IntentStatus {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "proposed" => Ok(Self::Proposed),
            "confirmed" => Ok(Self::Confirmed),
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
            Self::Failing => 4.0,
            Self::NeedsReverification => 3.0,
            Self::Uninspected => 2.0,
            _ => 0.0,
        }
    }
}

impl std::fmt::Display for InspectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uninspected => write!(f, "uninspected"),
            Self::Passing => write!(f, "passing"),
            Self::Failing => write!(f, "failing"),
            Self::Independent => write!(f, "independent"),
            Self::NeedsReverification => write!(f, "needs_reverification"),
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
    /// A consumer-plane proof: an ordered chain of endpoint invocations run by
    /// the built-in saga engine (`loom saga run`). The command re-derives all
    /// detail from the spec file named in the description's `spec:` line.
    Saga,
}

impl std::fmt::Display for ValidationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Test => write!(f, "test"),
            Self::Assertion => write!(f, "assertion"),
            Self::Benchmark => write!(f, "benchmark"),
            Self::ManualCheck => write!(f, "manual_check"),
            Self::Saga => write!(f, "saga"),
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
            "saga"         => Ok(Self::Saga),
            other => anyhow::bail!(
                "Unknown validation type '{}'. Valid: test, assertion, benchmark, manual_check, saga",
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
    /// Cannot run yet — waiting on something external (a live target, an env
    /// var, a credential). Distinct from `not_run` (forgotten/pending): blocked
    /// is a *recorded decision* with a reason, so it never reads as neglect.
    Blocked,
}

impl std::fmt::Display for ValidationResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passed => write!(f, "passed"),
            Self::Failed => write!(f, "failed"),
            Self::NotRun => write!(f, "not_run"),
            Self::Blocked => write!(f, "blocked"),
        }
    }
}

impl std::str::FromStr for ValidationResult {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "passed" => Ok(Self::Passed),
            "failed" => Ok(Self::Failed),
            "not_run" => Ok(Self::NotRun),
            "blocked" => Ok(Self::Blocked),
            other => anyhow::bail!(
                "Unknown validation result '{}'. Valid: passed, failed, not_run, blocked",
                other
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// EdgeType
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
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
            Self::Governs => write!(f, "GOVERNS"),
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
            Self::Error => write!(f, "error"),
        }
    }
}

impl std::str::FromStr for Severity {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "warning" => Ok(Self::Warning),
            "error" => Ok(Self::Error),
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
    /// Auto-recorded status change (the graph's recurrence memory) —
    /// written by loom itself on every verdict transition, never by hand.
    Transition,
    /// Auto-recorded freshness stamp written by `loom intent confirm` —
    /// the alignment history `loom next --mode align` ranks by.
    Confirm,
}

impl std::fmt::Display for NoteKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Justification => "justification",
            Self::Commentary => "commentary",
            Self::Idea => "idea",
            Self::Question => "question",
            Self::Decision => "decision",
            Self::Todo => "todo",
            Self::Transition => "transition",
            Self::Confirm => "confirm",
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
            "transition"    => Ok(Self::Transition),
            "confirm"       => Ok(Self::Confirm),
            other => anyhow::bail!(
                "Unknown note kind '{}'. Valid: justification, commentary, idea, question, decision, todo, transition, confirm",
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
    /// Consciously PARKED: the design is valid and still wanted, but not being
    /// built now (e.g. premature for current scale). Out of the build queue and
    /// never blocks a parent roll-up; resume with `--lifecycle planned`. Distinct
    /// from retire (status=deprecated), which is for SUPERSEDED/out-of-scope
    /// design — deferred is alive, just not active work.
    Deferred,
    /// CLEANUP as a tracked verb: this code is SUPPOSED TO GO AWAY. Unlike
    /// needs_change (a desired WORKING end state) or retire (keeps the code and
    /// even tells coverage the file is UNREACHED), a `to_be_removed` intent's
    /// criterion is falsifiable by ABSENCE — it is "done" only when its code is
    /// verified gone (no passing IMPLEMENTS grounding remains). The third
    /// advertised verb finally has a home that gates green on deletion.
    ToBeRemoved,
}

impl std::fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Planned => "planned",
            Self::Implemented => "implemented",
            Self::NeedsChange => "needs_change",
            Self::Deferred => "deferred",
            Self::ToBeRemoved => "to_be_removed",
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
            "deferred" => Ok(Self::Deferred),
            "to_be_removed" => Ok(Self::ToBeRemoved),
            other => anyhow::bail!(
                "Unknown lifecycle '{}'. Valid: planned, implemented, needs_change, deferred, to_be_removed",
                other
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// HypothesisStatus — the state machine on the pre-decision plane
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisStatus {
    /// Declared, not yet proven against the code (the only state `add` creates).
    Proposed,
    /// An analyzer confirmed the claimed problem exists in the code as it is now.
    Supported,
    /// An analyzer looked and the claimed problem is not real.
    Refuted,
    /// Decision: converted into planned intents / needs_change marks. Terminal —
    /// from here the ordinary intent machinery owns the work.
    Adopted,
    /// Adopted work's predicted outcome was verified after implementation.
    Confirmed,
    /// Decision: not pursuing it (with a recorded why). Terminal.
    Rejected,
}

impl std::fmt::Display for HypothesisStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Proposed => "proposed",
            Self::Supported => "supported",
            Self::Refuted => "refuted",
            Self::Adopted => "adopted",
            Self::Confirmed => "confirmed",
            Self::Rejected => "rejected",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for HypothesisStatus {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "proposed"  => Ok(Self::Proposed),
            "supported" => Ok(Self::Supported),
            "refuted"   => Ok(Self::Refuted),
            "adopted"   => Ok(Self::Adopted),
            "confirmed" => Ok(Self::Confirmed),
            "rejected"  => Ok(Self::Rejected),
            other => anyhow::bail!(
                "Unknown hypothesis status '{}'. Valid: proposed, supported, refuted, adopted, confirmed, rejected",
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
    /// The ONE falsifiable criterion this intent is done/correct by — first
    /// class as of v10 (previously overloaded onto `description`). Set/edited via
    /// `loom intent add|update --criterion`; gated by `require_substantive` at
    /// write and `is_vacuous` in `doctor`, symmetric with edge criteria. Empty
    /// on intents from older graphs (additive; backfill is separate work).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub criterion: String,
    pub abstraction_level: String,
    /// Product/business facet for discovery and scoring (auth, billing,
    /// onboarding, etc.). Not an architecture layer.
    pub domain: String,
    /// Architecture layer used by `layering_violation` (presentation,
    /// application, storage, etc.). Empty means undeclared/exempt.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub layer: String,
    /// File path strings (native list in the store as of schema v5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<String>,
    pub status: String,
    /// Behavioural facet for completeness: happy | sad | fallback | … (open
    /// vocabulary; "" = unspecified).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub aspect: String,
    /// Registered vocabulary terms (≤3, sorted, deduped; native list in the
    /// store as of schema v5). The bounded facet duplicate-responsibility
    /// detection collides on; empty = untagged (honest absence — never
    /// counted as evidence).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Who the behavior is for: user_visible | internal | "" (untriaged).
    /// The align interview's framing label — internal machinery is never
    /// presented to the user as a product capability, and internal intents
    /// leave the align queue until redefined.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub visibility: String,
    /// Relationship to the system boundary: inbound (this intent EXPOSES a
    /// surface the outside world calls — an HTTP handler, public API, webhook
    /// receiver: a provider contract others depend on) | outbound (this intent
    /// CALLS an external system — an HTTP client, third-party SDK, publish: a
    /// consumer dependency on someone else's contract) | "" (internal — does
    /// not cross the boundary). Traversal context: when the driver pulls this
    /// intent it sees that these files cross into the outside world, so a
    /// change here is contract-affecting, not local. Additive — absent on
    /// intents from older graphs (reads as "").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub boundary: String,
    /// Implementation lifecycle: planned | implemented | needs_change | deferred.
    pub lifecycle: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Physical file on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeFile {
    pub id: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_modified: String,
    /// Repo-relative paths this file statically imports (extracted by
    /// `loom sync`; native list in the store as of schema v5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<String>,
    /// Canonical top-level symbols this file declares (extracted by
    /// `loom sync`; diagnostic physical evidence as of schema v7).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<String>,
    /// Rich top-level symbol facts used by symbol-accountability diagnostics
    /// (stored as a list of JSON strings in the graph as of schema v8).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symbol_facts: Vec<SymbolFact>,
    /// Content hash (FNV-1a 64, hex) of the file's bytes — `loom sync`'s change
    /// detector. mtime alone false-flags on checkout/rebase (mtime churns,
    /// content doesn't); the hash makes "changed" mean the bytes changed.
    /// Empty on never-synced/pre-upgrade graphs (sync falls back to mtime once).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_hash: String,
    /// How this file's symbol facts were produced — `high` (tree-sitter),
    /// `low` (heuristic line-scanner), `none` (no extractor for this language),
    /// or empty (legacy/never re-synced under a grade-aware loom). Lets a
    /// consumer weight how much to trust the facts. Set by `loom sync`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub extractor_grade: String,
}

/// A top-level syntax symbol extracted from a CodeFile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymbolFact {
    pub label: String,
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub visibility: String,
    pub line_start: usize,
    pub line_end: usize,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub is_test: bool,
    /// Source string literals found inside this symbol. These are physical
    /// facts, not ownership claims; smell detectors use them to catch repeated
    /// user-facing/contract strings that can drift.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub string_literals: Vec<StringLiteralFact>,
    /// Count of call/macro markers that can abort a path or leave it unfinished
    /// (`panic`, `unwrap`, `expect`, `todo`, `unimplemented`). Extracted from
    /// source tokens, excluding comments and string literals.
    #[serde(default, skip_serializing_if = "usize_is_zero")]
    pub panic_marker_count: usize,
    /// Distinct uncontrolled-panic marker names seen in this symbol.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panic_markers: Vec<String>,
    /// FNV-1a hash of this symbol's source lines — `loom sync`'s per-symbol
    /// change detector. Lets sync flip only the IMPLEMENTS edges whose locator
    /// targets a symbol whose BODY actually changed, instead of every edge on a
    /// changed file (the transition-churn source). Empty on pre-upgrade graphs
    /// and feature-light builds; sync falls back to whole-file flipping when
    /// any compared fact lacks it. Self-heals — sync re-extracts symbol_facts
    /// every run, so it populates on the first sync after upgrade.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body_hash: String,
    /// FNV-1a hash of this symbol's normalized tree-sitter token shape. Unlike
    /// `body_hash`, this is NOT a sync invalidation signal: it intentionally
    /// ignores comments/formatting and canonicalizes identifiers/literals so
    /// clone advisories can catch renamed or formatted copies without weakening
    /// exact change detection.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shape_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StringLiteralFact {
    pub value: String,
    pub line: usize,
}

/// Named anti-pattern rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityRule {
    pub id: String,
    pub name: String,
    pub description: String,
    pub detection_logic: String,
    pub severity: String,
    /// Norm category (the GOVERNS taxonomy): security | correctness |
    /// performance | architecture | resource_safety. Set at `rule add`/seed;
    /// "" reads as uncategorized. Each kind carries a default inspection_effort
    /// (see `GovernsKind::default_effort`) so effort derives from kind+override
    /// instead of a hardcoded rule-name match.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// How much capability inspecting this rule needs: "low" (near-mechanical,
    /// e.g. a secrets scan) | "mid" (read-and-judge) | "high" (deep semantic
    /// reading, e.g. atomicity). Optional — "" reads as mid. Loom names the
    /// WORK; which model answers is the harness's business.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub inspection_effort: String,
}

/// The RELATES_TO relationship taxonomy. The single home for each kind's
/// strategy: whether it's mechanically derivable, how much it should be trusted,
/// and whether a code change stales it. Stored as validated strings in the edge
/// `kinds` multiset; this enum is the closed vocabulary + behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    Imports,
    SharesFile,
    SharesVocab,
    SameDomain,
    Calls,
    Inheritance,
    SharesState,
    DocReference,
    Manual,
}

impl RelationKind {
    pub const ALL: &'static [RelationKind] = &[
        Self::Imports,
        Self::SharesFile,
        Self::SharesVocab,
        Self::SameDomain,
        Self::Calls,
        Self::Inheritance,
        Self::SharesState,
        Self::DocReference,
        Self::Manual,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Imports => "imports",
            Self::SharesFile => "shares_file",
            Self::SharesVocab => "shares_vocab",
            Self::SameDomain => "same_domain",
            Self::Calls => "calls",
            Self::Inheritance => "inheritance",
            Self::SharesState => "shares_state",
            Self::DocReference => "doc_reference",
            Self::Manual => "manual",
        }
    }

    /// How much a passing edge grounded ONLY by this kind should be trusted —
    /// the epistemic doctor flags a verdict whose every kind is weak (concept
    /// similarity, not proven coupling). (stales_on_code_change lands in C5.)
    pub fn trust_weight(self) -> &'static str {
        match self {
            Self::Imports | Self::Calls | Self::Inheritance => "strong",
            Self::SharesFile | Self::SharesState => "medium",
            Self::SharesVocab | Self::SameDomain | Self::DocReference | Self::Manual => "weak",
        }
    }

    /// Mechanical kinds are derivable from extraction with no judgment — the
    /// `populate` lane backfills them from discovery signals. The rest are
    /// analyzer judgments asserted with a locator.
    pub fn is_mechanical(self) -> bool {
        matches!(
            self,
            Self::Imports | Self::SharesFile | Self::SharesVocab | Self::SameDomain
        )
    }

    /// Whether a code-content change should stale an edge of this kind. Meaning
    /// kinds (shares_vocab/same_domain) track concept overlap and doc_reference
    /// tracks docs — a code edit must NOT re-open those (kind-aware sync). An
    /// edge with NO kinds is conservatively staled (unknown coupling).
    pub fn stales_on_code_change(self) -> bool {
        !matches!(
            self,
            Self::SharesVocab | Self::SameDomain | Self::DocReference
        )
    }
}

/// True when a code-content change should stale this RELATES_TO edge: it has no
/// kinds (unknown — conservatively stale) OR at least one kind that tracks code.
/// Meaning-only edges (every kind is concept/docs) are left alone on code change.
pub fn relates_stales_on_code_change(kinds: &[String]) -> bool {
    kinds.is_empty()
        || kinds.iter().any(|k| {
            k.parse::<RelationKind>()
                .map(|rk| rk.stales_on_code_change())
                .unwrap_or(true)
        })
}

impl std::str::FromStr for RelationKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Unknown relation kind '{s}'. Valid: {}",
                    Self::ALL
                        .iter()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }
}

/// The GOVERNS norm taxonomy — the closed category vocabulary for QualityRule,
/// with each kind's default inspection effort (overridable per rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernsKind {
    Security,
    Correctness,
    Performance,
    Architecture,
    ResourceSafety,
}

impl GovernsKind {
    pub const ALL: &'static [GovernsKind] = &[
        Self::Security,
        Self::Correctness,
        Self::Performance,
        Self::Architecture,
        Self::ResourceSafety,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Security => "security",
            Self::Correctness => "correctness",
            Self::Performance => "performance",
            Self::Architecture => "architecture",
            Self::ResourceSafety => "resource_safety",
        }
    }

    /// Default inspection effort for a rule of this kind (a rule may override
    /// with --effort). Replaces hardcoded rule-name → effort guessing.
    pub fn default_effort(self) -> &'static str {
        match self {
            Self::Security | Self::Correctness | Self::ResourceSafety => "high",
            Self::Architecture | Self::Performance => "mid",
        }
    }
}

impl std::str::FromStr for GovernsKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Unknown governs kind '{s}'. Valid: {}",
                    Self::ALL
                        .iter()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }
}

/// Explicit proof object that an intent is fulfilled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Validation {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// test | assertion | benchmark | manual_check
    pub validation_type: String,
    /// Shell command to run, e.g. "cargo test --test foo"
    pub command: String,
    /// RFC3339 timestamp of last run (empty = never run). Set by BOTH the
    /// executor (`loom validate`) and a hand-mark (`loom validation mark`) —
    /// so it alone cannot tell machine-run from hand-marked.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_run: String,
    /// passed | failed | not_run
    pub last_result: String,
    /// RFC3339 timestamp the EXECUTOR last ran the command (empty = never
    /// machine-run). Set ONLY by `loom validate`, never by `loom validation
    /// mark`. The proven axis uses this to split EXECUTED (machine-verified)
    /// from ASSERTED (hand-marked) — a command-bearing validation marked
    /// passed by hand has `last_run` set but this empty, so it reads ASSERTED.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_executed_run: String,
    /// What the EXECUTOR observed the runner DO (G2 falsification-witness):
    /// `discriminating` = captured output showed a recognized test runner that
    /// actually asserted >=1 thing (cargo `test result: ok. N passed`,
    /// pytest/jest `N passed`, go `--- PASS:`); `ran_inert` = exited 0 but no
    /// assertion signal (0 tests, unknown runner, a bare `echo`/`true`); empty =
    /// never machine-run under G2. ONLY `discriminating` feeds the EXECUTED proof
    /// tier — exit-0 alone can no longer mint EXECUTED. Set ONLY by the executor;
    /// a hand-mark never touches it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub discrimination_status: String,
}

// ---------------------------------------------------------------------------
// Note — append-only free-text memory (justification, idea, question, …)
// ---------------------------------------------------------------------------

/// A timestamped annotation. Optionally scoped to a target (an intent, an
/// edge, or a code file) via `target_kind` + `target_id`; a note with
/// `target_kind = "none"` floats free (e.g. a standalone idea not yet tied to
/// anything). Append-only:
/// notes accumulate, they are never overwritten — that is the richer memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    /// justification | commentary | idea | question | decision | todo |
    /// transition (auto) | confirm (auto)
    pub kind: String,
    pub text: String,
    /// "human" | "llm"
    pub author: String,
    /// "intent" | "edge" | "codefile" | "none"
    pub target_kind: String,
    /// id of the targeted intent/edge/codefile, or "" when floating
    pub target_id: String,
    /// OPTIONAL lane this note is addressed to ("" = everyone): builder |
    /// analyzer | fixer | validator | quality. The directed-handoff channel —
    /// an out-of-lane finding becomes a message the owning lane will see
    /// first in its work items.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub audience: String,
    pub created_at: String,
    /// Resolution lifecycle for `kind=todo` notes (the only obligation-shaped
    /// kind): "" = OPEN — keeps surfacing in `loom next` until consciously
    /// closed; non-empty = the close reason set by `loom note resolve`. loom
    /// can't parse a free-form todo's prose, so it can't auto-clear it — but it
    /// won't forget it either: open todos persist in the queue so a compacted
    /// LLM can't silently drop them. Advisory only (gates nothing), so a
    /// dishonest close can't launder green — the truth stays in computed signals.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resolution: String,
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
// VocabTerm — the bounded tag vocabulary (a registry of keys, not a plane)
// ---------------------------------------------------------------------------

/// A registered tag term. Deliberately a KEY, not a knowledge node: it carries
/// no lifecycle, no edges, no inspection state. Its value is forced collision —
/// two agents describing the same responsibility in open prose rarely share
/// words, but picking from a small registry they collide, and collisions are
/// what duplicate-responsibility detection consumes. Kept honest by detection
/// (`vocab_drift` smell + `loom vocab merge`), never by a closed list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VocabTerm {
    pub id: String,
    /// The term itself — lowercase, the unique key intents reference in `tags`.
    pub name: String,
    /// Contrastive one-liner: what it covers AND what it does not (names the
    /// neighbouring term), so an agent picking from the list can disambiguate.
    pub description: String,
    pub author: String,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Persona — a named audience segment (the user story "as a [X]" node)
// ---------------------------------------------------------------------------

/// A user persona: a named audience segment (e.g. "admin", "end_user").
/// Connects to intents via inspectable SERVES edges ("does this intent actually
/// serve this persona?") and to saga Validations via structural JOURNEYS edges
/// ("this saga exercises this persona's path end-to-end").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Persona {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub created_at: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// InterfaceSurface — an externally callable boundary surface
// ---------------------------------------------------------------------------

/// A named boundary surface the outside world can call, or that a journey calls
/// as a consumer. HTTP endpoints are the first concrete kind; the shape remains
/// generic enough for CLI commands, RPC methods, and event topics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceSurface {
    pub id: String,
    /// Human-readable identity, e.g. `POST /carts/{id}/payment`.
    pub name: String,
    pub description: String,
    /// http_endpoint | cli_command | rpc_method | event_topic | …
    pub surface_kind: String,
    /// HTTP verb for http_endpoint; empty for surfaces that do not use verbs.
    pub method: String,
    /// The route/path/topic/command identity, without environment-specific host.
    pub target: String,
    pub created_at: String,
    pub updated_at: String,
}

pub fn interface_surface_name(surface_kind: &str, method: &str, target: &str) -> String {
    if surface_kind == "http_endpoint" && !method.trim().is_empty() {
        format!("{} {}", method.trim().to_uppercase(), target.trim())
    } else {
        target.trim().to_string()
    }
}

// ---------------------------------------------------------------------------
// InboxItem — raw language intake before graph truth
// ---------------------------------------------------------------------------

/// A durable intake card for free-form human/LLM language. Inbox items are
/// candidates, not graph truth: normalization proposes a route, and existing
/// graph commands still perform the actual mutation in v1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxItem {
    pub id: String,
    pub raw_text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub normalized_claim: String,
    /// observation | user_request | feature_proposal | bug_suspicion |
    /// refactor_suspicion | missing_intent | missing_validation |
    /// missing_story | terminology | rough_edge | external_blocker | question |
    /// decision_capture | constraint | acceptance_criterion | interface_gap |
    /// evidence | risk | follow_up | duplicate_candidate | docs_gap |
    /// migration_need
    pub kind: String,
    /// new | triaged | routed | rejected | deferred | duplicate
    pub status: String,
    /// chat | user | llm | code_audit | validation | import | unknown
    pub source: String,
    pub author: String,
    /// Existing VocabTerm names. Empty is honest absence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Lightweight backlinks such as `intent:<id>`, `file:src/main.rs`, or
    /// `inbox:<id>`. These make the card OKF/wiki-ready without creating a
    /// separate knowledge-base subsystem.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
    /// intent | hypothesis | validation | quality_rule | vocab | note |
    /// ignore | answer | none
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub route_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub route_command: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub route_target_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub route_target_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resolution: String,
    pub created_at: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Delegation — a subtree owned by ANOTHER loom graph (monorepo/federation)
// ---------------------------------------------------------------------------

/// A glob pattern marking files as covered by a CHILD graph rather than this
/// one — the federation primitive for monorepos. `loom coverage` buckets
/// matching files as `delegated` (covered, not gaps), and the `target` names
/// the child's committed export so the boundary is a verifiable artifact, not
/// a blanket ignore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delegation {
    pub id: String,
    pub pattern: String,
    /// Path to the child graph's committed export (e.g. services/grid/loom.graph.json).
    pub target: String,
    /// Content hash of the child export at the last sync — the watched-artifact
    /// baseline. When it changes, sync re-opens the seam intents. Empty until the
    /// first sync observes the export (v10).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub export_hash: String,
    /// Parent intent ids that depend on this child's contract (the seams a child
    /// export change ripples to). Set via `loom delegate seam` (v10).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seam_intents: Vec<String>,
    pub author: String,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Hypothesis — the pre-decision plane (improvement proposals, proven twice)
// ---------------------------------------------------------------------------

/// A falsifiable improvement proposal: claim (what's wrong NOW), proposal
/// (what to change), predicted_outcome (the measurable result if adopted —
/// the post-implementation acceptance contract). State machine:
/// proposed → supported | refuted → adopted | rejected. Invisible to coverage,
/// completeness, and every queue until adoption converts it into planned
/// intents — speculation never counts as state of the world.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hypothesis {
    pub id: String,
    pub name: String,
    /// What is wrong/suboptimal — provable against the code as it is now.
    pub claim: String,
    /// The proposed change.
    pub proposal: String,
    /// The measurable result if adopted (falsifiable).
    pub predicted_outcome: String,
    /// proposed | supported | refuted | adopted | rejected
    pub status: String,
    /// Who proposed it — role-aware provenance. The prover must differ
    /// (proposer ≠ prover when both declare roles).
    pub author: String,
    /// What the prover found ("" until proven).
    pub evidence: String,
    /// Who proved it ("" until proven).
    pub inspected_by: String,
    /// When it was proven ("" until proven).
    pub last_inspected: String,
    pub created_at: String,
    pub updated_at: String,
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
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub criterion: String,
    pub confidence: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evidence: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_inspected: String,
    /// Who inspected — role-aware provenance, e.g. "llm:analyzer", "human".
    /// Resolved from --inspected-by / $LOOM_AGENT (see `crate::agent`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub inspected_by: String,
    pub priority_score: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// The relationship taxonomy (a MULTISET — a pair is often coupled several
    /// ways at once, mirroring `discovery_signals`). Mechanical kinds
    /// (imports/shares_file/shares_vocab/same_domain) are carried from the
    /// discovery signals by `populate`; judgment kinds (calls/inheritance/
    /// shares_state/doc_reference/manual) are asserted by an analyzer with a
    /// locator. Validated against `RelationKind`; empty = un-kinded (legacy).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
    /// Low-churn flag: a settled grounded relationship that `loom sync` must NOT
    /// re-open on every endpoint code change (set via `loom edge stable <a> <b>`).
    /// Owner: analyzer. Stored as TEXT ("true") in SQLite; a bool here.
    #[serde(default)]
    pub stable: bool,
    /// Synthetic discovery queue metadata. Empty for stored RELATES_TO edges.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub discovery_class: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discovery_signals: Vec<DiscoverySignal>,
    #[serde(default, skip_serializing_if = "DiscoveryCentrality::is_empty")]
    pub discovery_centrality: DiscoveryCentrality,
}

/// A mechanical reason an unexplored pair was offered to the analyzer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoverySignal {
    pub kind: String,
    pub detail: String,
    pub weight: f64,
}

/// Structural importance for the generated discovery pair.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveryCentrality {
    pub a_degree: i64,
    pub b_degree: i64,
}

impl DiscoveryCentrality {
    pub fn is_empty(&self) -> bool {
        self.a_degree == 0 && self.b_degree == 0
    }
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
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub criterion: String,
    #[serde(default, skip_serializing_if = "f64_is_zero")]
    pub confidence: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evidence: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_inspected: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub inspected_by: String,
    /// Finer-than-file anchor inside the CodeFile (symbol/region), or "".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub locator: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// When the grounding was made — the staleness anchor for smell
    /// adjudication (a decision note older than the newest grounding no
    /// longer speaks for the structure). "" on edges from pre-v3 graphs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_at: String,
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
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub criterion: String,
    pub confidence: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evidence: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_inspected: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub inspected_by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

/// TARGETS: Hypothesis → Intent — which intents an improvement hypothesis
/// would touch. Mirrors GOVERNS: full inspectable meta, so per-target
/// grounding and sync staleness work like every other claim about code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetsEdge {
    pub id: String,
    pub hypothesis_id: String,
    pub intent_id: String,
    pub hypothesis_name: String,
    pub intent_name: String,
    // --- State ---
    pub inspection_status: String,
    // --- Meta ---
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub criterion: String,
    pub confidence: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evidence: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_inspected: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub inspected_by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

/// SERVES: Persona → Intent — this intent serves this persona.
/// Inspectable: the claim "this intent serves persona X" must be verified
/// against actual behavior, not assumed from the declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServesEdge {
    pub id: String,
    pub persona_id: String,
    pub intent_id: String,
    pub persona_name: String,
    pub intent_name: String,
    // --- State ---
    pub inspection_status: String,
    // --- Meta ---
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub criterion: String,
    pub confidence: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evidence: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_inspected: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub inspected_by: String,
    pub priority_score: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_at: String,
}

/// JOURNEYS: Persona → Validation (type=saga) — this saga exercises this
/// persona's end-to-end path. Structural (like HIERARCHY): no inspection state;
/// the saga run itself is the proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JourneysEdge {
    pub id: String,
    pub persona_id: String,
    pub validation_id: String,
    pub persona_name: String,
    pub validation_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_at: String,
}

/// CALLS: Validation → InterfaceSurface — one ordered saga step invokes one
/// externally callable surface. The semantic proof remains on VALIDATES
/// (Validation → Intent); CALLS makes the interface inventory queryable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsEdge {
    pub id: String,
    pub validation_id: String,
    pub interface_id: String,
    pub validation_name: String,
    pub interface_name: String,
    pub step_index: usize,
    pub step_name: String,
    pub intent_id: String,
    pub intent_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_at: String,
}

/// VALIDATES: Validation → Intent — this validation proves intent is fulfilled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatesEdge {
    pub id: String,
    pub validation_id: String,
    pub intent_id: String,
    pub validation_name: String,
    pub intent_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_at: String,
    // --- State ---
    pub inspection_status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

// ---------------------------------------------------------------------------
// Serde skip predicates — an empty/zero field means "not recorded"; omitting
// it from JSON says the same thing in zero bytes. Round-trip safe: every
// skipped field pairs with #[serde(default)].
// ---------------------------------------------------------------------------

fn f64_is_zero(v: &f64) -> bool {
    *v == 0.0
}

fn bool_is_false(v: &bool) -> bool {
    !*v
}

fn usize_is_zero(v: &usize) -> bool {
    *v == 0
}

/// "unknown" is the historical placeholder default — as empty as "".
fn domain_is_unknown(s: &String) -> bool {
    s.is_empty() || s == "unknown"
}

// ---------------------------------------------------------------------------
// Composite output types
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Tier-1 surfaces — the projection of an entity that travels INSIDE a work
// item: exactly the fields the verdict needs, nothing it doesn't. The full
// record stays one dig away (`loom intent show`, `loom edge show`,
// `loom note list`, `loom validation list`); these never replace those views.
// ---------------------------------------------------------------------------

/// An intent as embedded in a work item. Timestamps and empty facets stay in
/// `loom intent show <id>`.
#[derive(Debug, Clone, Serialize)]
pub struct IntentSurface {
    pub id: String,
    pub name: String,
    pub description: String,
    /// The falsifiable acceptance check — THE test for this intent. Surfaced into
    /// work items so a builder/validator proves the actual criterion, not a
    /// paraphrase of the description. Empty for intents that never declared one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub criterion: String,
    pub level: String,
    #[serde(skip_serializing_if = "domain_is_unknown")]
    pub domain: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub layer: String,
    pub status: String,
    pub lifecycle: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub aspect: String,
    /// inbound | outbound | "" — surfaced so the driver sees a boundary crossing
    /// while traversing, without re-deriving it from the code.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub boundary: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
}

impl From<&Intent> for IntentSurface {
    fn from(i: &Intent) -> Self {
        Self {
            id: i.id.clone(),
            name: i.name.clone(),
            description: i.description.clone(),
            criterion: i.criterion.clone(),
            level: i.abstraction_level.clone(),
            domain: i.domain.clone(),
            layer: i.layer.clone(),
            status: i.status.clone(),
            // "" reads as implemented everywhere (pre-lifecycle graphs).
            lifecycle: if i.lifecycle.is_empty() {
                "implemented".into()
            } else {
                i.lifecycle.clone()
            },
            aspect: i.aspect.clone(),
            boundary: i.boundary.clone(),
            tags: i.tags.clone(),
            sources: i.source_refs.clone(),
        }
    }
}

/// An IMPLEMENTS grounding as embedded in a work item: where the code lives.
/// The full edge (criterion, evidence, provenance): `loom intent show <id>`.
#[derive(Debug, Clone, Serialize)]
pub struct GroundingSurface {
    pub path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub locator: String,
    pub status: String,
}

impl From<&Implements> for GroundingSurface {
    fn from(im: &Implements) -> Self {
        Self {
            path: im.codefile_path.clone(),
            locator: im.locator.clone(),
            status: im.inspection_status.clone(),
        }
    }
}

/// A validation as embedded in a work item: enough to run it or mark it.
/// Full record: `loom validation list`.
#[derive(Debug, Clone, Serialize)]
pub struct ValidationSurface {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub command: String,
    pub result: String,
}

impl From<&Validation> for ValidationSurface {
    fn from(v: &Validation) -> Self {
        Self {
            id: v.id.clone(),
            name: v.name.clone(),
            command: v.command.clone(),
            result: v.last_result.clone(),
        }
    }
}

/// A note as embedded in a work item. Repeated identical notes (sync re-flips
/// spam the same transition text) collapse into one surface with `times` —
/// the count IS the information; the copies are not. Full records with ids
/// and timestamps: `loom note list`.
#[derive(Debug, Clone, Serialize)]
pub struct NoteSurface {
    pub kind: String,
    pub text: String,
    pub author: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub audience: String,
    #[serde(skip_serializing_if = "times_is_one")]
    pub times: u32,
}

fn times_is_one(n: &u32) -> bool {
    *n == 1
}

/// Rich work item returned by `loom next` — the tier-1 actionable surface.
/// Every embedded entity is a *Surface projection; the dig commands above
/// retrieve what the projection elides.
#[derive(Debug, Clone, Serialize)]
pub struct WorkItem {
    pub edge_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub edge_id: String,
    pub inspection_status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub criterion: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub evidence: String,
    pub priority_score: f64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub discovery_class: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub discovery_signals: Vec<DiscoverySignal>,
    #[serde(skip_serializing_if = "DiscoveryCentrality::is_empty")]
    pub discovery_centrality: DiscoveryCentrality,
    /// The subject intent (always present).
    pub intent_a: IntentSurface,
    /// The related intent (present for RELATES_TO edges).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_b: Option<IntentSurface>,
    /// IMPLEMENTS groundings for intent_a — path + locator + status.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub implements: Vec<GroundingSurface>,
    /// Validations linked to intent_a (from VALIDATES edges).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub validations: Vec<ValidationSurface>,
    /// Accumulated free-text memory relevant to this work item — notes on the
    /// edge and on both intents, deduplicated. Lets prior reasoning travel.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<NoteSurface>,
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
    /// Fraction of Validation.last_result == "passed" over ALL validations
    /// (blocked included in the denominator). Kept for compatibility.
    pub validation_pass_rate: f64,
    /// Validations marked `blocked` — they CANNOT run yet (environment not
    /// ready) and are out of the validator queue. Surfaced so a wall of
    /// environmentally-blocked proofs cannot silently drag the headline rate.
    pub blocked_validations: i64,
    /// passed / (total − blocked): the pass rate over proofs that can actually
    /// run, undiluted by blocked ones — the "real health" number. Equals
    /// `validation_pass_rate` when nothing is blocked.
    pub validation_pass_rate_runnable: f64,
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
    /// Intents two/three hops from the current `needs_reverification` frontier
    /// that receive a decaying priority bump WITHOUT any status change — the
    /// graded ripple beyond the surgical one-hop blast radius (one hop flips,
    /// farther hops only nudge `loom next` ranking). Derived from the post-sync
    /// stale frontier, so on an already-stale graph it reflects the whole
    /// elevated region, not just this sync's own contribution.
    #[serde(default)]
    pub intents_priority_bumped: usize,
    /// Passing GOVERNS edges flipped to needs_reverification — quality green
    /// must be re-earned after the code it judged changes.
    pub governs_edges_flagged: usize,
    /// Passing TARGETS edges flipped to needs_reverification — hypothesis
    /// support must be re-earned after target code changes.
    pub targets_edges_flagged: usize,
    /// Passing SERVES edges flipped to needs_reverification — persona serving
    /// claims must be re-earned after the intent's code changes.
    pub serves_edges_flagged: usize,
    pub validations_invalidated: usize,
    /// Registered CodeFiles that no longer exist on disk (deleted/renamed).
    /// Phantom files distort coverage — remove them (`loom codefile remove`)
    /// or restore them.
    pub missing_files: Vec<String>,
    /// Registered CodeFile paths that resolve OUTSIDE the graph root
    /// (hostile/corrupt graph data) — sync refuses to read them; remove the
    /// registration.
    pub escaped_files: Vec<String>,
    /// IMPLEMENTS locators that no longer occur in their file (renamed
    /// symbol?) — the edge was flipped to needs_reverification if it was
    /// passing. Re-ground with a fresh locator.
    pub locators_stale: Vec<String>,
    /// Paths whose CONTENT changed this sync (the ripple's causes). The path
    /// IS the identity an agent acts on; ids/mtimes stay in the store.
    pub changes: Vec<String>,
    /// Low-signal ROUTINE transition notes compacted away this sync to hold the
    /// per-target `transition_cap` (regression markers + authored notes are
    /// never touched). `0` when the cap is disabled or nothing exceeded it.
    pub transitions_compacted: usize,
}
