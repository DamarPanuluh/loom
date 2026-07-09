//! Subcommand argument shapes for every `loom` command family.
//!
//! Plane: surface — argument shape only. These enums declare flag names,
//! defaults, and help text; every handler lives in `crate::commands`. Nothing
//! here opens a store, resolves a graph, or contains logic beyond clap parsing.

use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum IntentCmd {
    /// Add an intent.
    Add {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        description: String,
        /// system | component | feature | cross_cutting
        #[arg(long, default_value = "feature")]
        level: String,
        /// planned | implemented | needs_change
        #[arg(long, default_value = "planned")]
        lifecycle: String,
        /// user_visible | internal
        #[arg(long)]
        visibility: Option<String>,
        /// Architecture layer label (arms the layering detector once an order is declared).
        #[arg(long)]
        layer: Option<String>,
        /// Scenario aspect: happy | sad | fallback | edge_case
        #[arg(long)]
        aspect: Option<String>,
        /// Permit a symbol-looking name (requires a behavioral description).
        #[arg(long)]
        allow_symbol_name: bool,
    },
    /// Show an intent by id, name, or unique fragment.
    Show { key: String },
    /// Deliberately waive a completeness axis for this intent (recorded, re-opens
    /// when the intent's meaning changes).
    Waive {
        key: String,
        /// scenarios | prerequisites | boundary | proof | journey | questions
        axis: String,
        #[arg(long)]
        reason: String,
    },
    /// Reactivate a retired (deprecated) intent → planned.
    Reactivate {
        key: String,
        #[arg(long)]
        reason: String,
    },
    /// List intents.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// One mutation verb for an intent. --description redefines (ripples one
    /// hop; --reword: same concept, no ripple). --name relabels, --level /
    /// --visibility / --aspect correct attributes, --lifecycle moves the
    /// prescriptive state — none of those ripple. Every update records --reason.
    Update {
        key: String,
        #[arg(long)]
        description: Option<String>,
        /// New name (a label change; the description stays the criterion).
        #[arg(long)]
        name: Option<String>,
        /// system | component | feature | cross_cutting
        #[arg(long)]
        level: Option<String>,
        /// user_visible | internal
        #[arg(long)]
        visibility: Option<String>,
        /// happy | sad | fallback | edge_case
        #[arg(long)]
        aspect: Option<String>,
        /// planned | implemented | needs_change
        #[arg(long)]
        lifecycle: Option<String>,
        #[arg(long)]
        reason: String,
        /// Clearer words, same concept: no ripple.
        #[arg(long)]
        reword: bool,
    },
    /// Hard-delete a mistaken intent (typo/duplicate only). Retire superseded
    /// design instead; refuses intents that still have hierarchy children.
    Remove {
        key: String,
        #[arg(long)]
        reason: String,
    },
    /// Retire superseded design (status → deprecated).
    Retire {
        key: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        replaced_by: Option<String>,
    },
    /// Ratify the current meaning (resets the alignment clock).
    Confirm { key: String },
    /// Tag from the registered vocabulary.
    Tag {
        #[command(subcommand)]
        cmd: IntentTagCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum IntentTagCmd {
    /// Tag an intent with a registered vocabulary term.
    Add { key: String, term: String },
    /// Remove a vocabulary tag from an intent.
    Remove { key: String, term: String },
}

#[derive(Subcommand, Debug)]
pub enum CodefileCmd {
    /// Register a codefile by path.
    Add {
        path: String,
        /// Register as observed (monitored upstream code): sync scans it and
        /// surface/contract staleness still ripples, but it carries no
        /// ownership, coverage, or build obligations. Re-adding an existing
        /// file with this flag marks it observed.
        #[arg(long)]
        observed: bool,
    },
    /// Re-expand every glob ever registered and add any newly-appeared files
    /// (e.g. an endpoint an upstream just added). Run before `loom sync`.
    Rescan,
    /// Unregister a codefile (e.g. the file was deleted on disk). Removes the
    /// node and cascades its implements/exposes edges.
    Remove { key: String },
    /// Show a codefile.
    Show { key: String },
    /// List codefiles.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}

#[derive(Subcommand, Debug)]
pub enum EdgeCmd {
    /// Ground an intent in a codefile (creates an uninspected implements edge).
    Implement {
        intent: String,
        codefile: String,
        #[arg(long)]
        locator: Option<String>,
        /// Grounding role: realizes (default; owns coverage) | consumes | configures | verifies
        #[arg(long)]
        role: Option<String>,
    },
    /// Put an interface surface under contract: a validation that exercises it
    /// (creates a `calls` edge). When the code behind the surface changes, sync
    /// resets this contract — the integration-monitoring signal.
    Call { validation: String, surface: String },
    /// Remove an edge (prune a redundant grounding/relationship). Asserted edges
    /// only — derived edges are rebuilt by `loom sync`.
    Remove {
        edge_id: String,
        /// Record why (writes a decision note on the source node).
        #[arg(long)]
        reason: Option<String>,
    },
    /// Correct the locator (symbol) on an asserted edge — e.g. a moved grounding.
    SetLocator { edge_id: String, locator: String },
    /// Add a relationship edge between two intents.
    /// kind: hierarchy | requires | scenario-of | variant-of | triggers | sequence | relates
    Relate {
        kind: String,
        from: String,
        to: String,
    },
    /// Record a verdict on an existing edge (ground|issue|independent).
    Verdict {
        edge_id: String,
        /// ground | issue | independent
        verdict: String,
        #[arg(long, default_value = "")]
        criterion: String,
        #[arg(long, default_value = "")]
        evidence: String,
        #[arg(long, default_value_t = 0.9)]
        confidence: f64,
    },
    /// Inspect two intents: ensure a `relates` edge and record a verdict.
    Explore {
        a: String,
        b: String,
        /// ground | issue | independent
        verdict: String,
        #[arg(long, default_value = "")]
        criterion: String,
        #[arg(long, default_value = "")]
        evidence: String,
        #[arg(long, default_value_t = 0.9)]
        confidence: f64,
    },
    /// Show one edge.
    Show { edge_id: String },
    /// List edges.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Reclassify a grounding edge's role (realizes|consumes|configures|verifies).
    /// Keeps the edge + verdict history; a changed role re-opens the claim
    /// (stale_cause: role_changed) to be re-verdicted under the new criterion.
    SetRole {
        edge_id: String,
        /// realizes | consumes | configures | verifies
        role: String,
        #[arg(long)]
        reason: String,
    },
    /// Rehome a grounding edge to a successor intent (a true mis-attachment,
    /// not just the wrong role). Supersede-not-delete: the old edge keeps its
    /// history but stops counting; a fresh unverified edge on the successor
    /// carries stale_cause: rehomed.
    Rehome {
        edge_id: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        reason: String,
    },
    /// Declare that a local intent depends on an upstream (federated) intent.
    DependsOn {
        /// Local intent (name, id, or fragment).
        intent: String,
        /// Upstream shadow (name like upstream/<alias>/..., id, or fragment).
        upstream: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum InboxCmd {
    /// Capture raw input.
    Add {
        text: String,
        #[arg(long, default_value = "human")]
        source: String,
        /// Optional origin ref, e.g. file:src/auth.rs or a node id.
        #[arg(long)]
        link: Option<String>,
    },
    /// List inbox items.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Filter by disposition (new|routed|rejected|duplicate|deferred).
        #[arg(long)]
        status: Option<String>,
    },
    /// Show one inbox item in full.
    Show { key: String },
    /// Mark an item's disposition: routed | rejected | duplicate | deferred.
    Mark {
        key: String,
        status: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Remove an inbox item (e.g. a resolved or accidental capture).
    Remove { key: String },
}

#[derive(Subcommand, Debug)]
pub enum QuestionCmd {
    /// Open a product question for an intent.
    Add {
        text: String,
        #[arg(long)]
        intent: String,
    },
    /// List product questions.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long)]
        status: Option<String>,
    },
    /// Show one product question.
    Show { key: String },
    /// Answer a product question.
    Answer {
        key: String,
        #[arg(long)]
        answer: String,
    },
    /// Close a product question without an answer.
    Close {
        key: String,
        status: String,
        #[arg(long)]
        reason: String,
    },
    /// Remove an accidental product question.
    Remove { key: String },
}

#[derive(Subcommand, Debug)]
pub enum NoteCmd {
    /// Attach a durable note to any node or edge (adjudications, context, warnings).
    Add {
        /// The node (name, id, or unique fragment) or edge (id or prefix) the note is about.
        target: String,
        /// decision | context | warning
        #[arg(long, default_value = "decision")]
        kind: String,
        #[arg(long)]
        text: String,
    },
    /// Remove a mistaken note. Notes are history and have no edit operation;
    /// removal is only for accidental/misattached notes.
    Remove { id: String },
    /// List notes, newest first, optionally scoped to one target node or edge.
    List {
        target: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}

#[derive(Subcommand, Debug)]
pub enum TaskCmd {
    /// Open a task record (spike|investigation|experiment|review|chore).
    Add {
        title: String,
        #[arg(long, default_value = "spike")]
        kind: String,
        /// Intent this task informs — the close/abandon outcome lands as a note on it.
        #[arg(long)]
        target: Option<String>,
    },
    /// Mark a task active.
    Start { key: String },
    /// Close a task with a result summary.
    Close {
        key: String,
        #[arg(long)]
        result: String,
    },
    /// Abandon a task.
    Abandon {
        key: String,
        #[arg(long)]
        reason: String,
    },
    /// Delete an accidental task record. Use close/abandon for real work history.
    Remove { key: String },
    /// Show a task record (kind/status/result).
    Show { key: String },
    /// List task records.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}
#[derive(Subcommand, Debug)]
pub enum FindingCmd {
    /// Add an asserted evidence-backed finding for triage.
    Add {
        text: String,
        #[arg(long, default_value = "code_audit")]
        source: String,
        #[arg(long, default_value = "code_audit")]
        kind: String,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        impact: String,
        #[arg(long, default_value_t = 0.7)]
        confidence: f64,
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        link: Option<String>,
    },
    /// List findings with their adjudication state.
    List {
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        state: Option<String>,
    },
    /// Record a judgment on a finding.
    Verdict {
        id: String,
        verdict: String,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum RuleCmd {
    /// Seed a pre-authored rule pack (e.g. iso5055).
    Seed { pack: String },
    /// Record a verdict; the outcome is positional (creates the governs edge if needed).
    Verdict {
        rule: String,
        intent: String,
        /// passing | failing | independent
        outcome: String,
        #[arg(long, default_value = "")]
        criterion: String,
        #[arg(long, default_value = "")]
        evidence: String,
        #[arg(long, default_value_t = 0.9)]
        confidence: f64,
    },
    /// List quality rules.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Show one rule with its guidance fields.
    Show { key: String },
    /// Author a one-off quality rule (outside a pack).
    Add {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        category: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Edit a quality rule. Customizing a builtin/seeded rule is allowed but
    /// will intentionally surface in pack_drift until reseeded or accepted.
    Update {
        key: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        category: Option<String>,
        #[arg(long)]
        severity: Option<String>,
        #[arg(long)]
        effort: Option<String>,
        /// Replacement inspection_guide text.
        #[arg(long)]
        guide: Option<String>,
        /// Replacement detection_hints array; repeat --hint to set multiple.
        #[arg(long = "hint")]
        hint: Vec<String>,
        /// Replacement patterns array; repeat --pattern to set multiple.
        #[arg(long = "pattern")]
        pattern: Vec<String>,
        #[arg(long)]
        reason: String,
    },
    /// Remove a quality rule (and its governs edges).
    Remove { key: String },
    /// Stop a rule governing an intent (removes the governs edge by name).
    Unlink { rule: String, intent: String },
}

#[derive(Subcommand, Debug)]
pub enum ValidationCmd {
    /// Add a validation and link it to an intent.
    Add {
        #[arg(long)]
        name: String,
        /// test | assertion | benchmark | manual_check | journey | scenario | contract
        #[arg(long, default_value = "test")]
        r#type: String,
        #[arg(long, default_value = "")]
        command: String,
        #[arg(long)]
        intent: String,
        /// Proof-strength label, e.g. L0..L6. For journey proofs prefer
        /// `loom journey add <spec>`, which compiles the spec and sets this.
        #[arg(long)]
        proof_level: Option<String>,
        /// Normalized proof kind (e.g. journey). Prefer `loom journey add <spec>`;
        /// set by hand only to register a repo-native journey runner.
        #[arg(long)]
        proof_kind: Option<String>,
        /// Stable journey id/name; requires --proof-kind journey.
        #[arg(long)]
        journey_id: Option<String>,
        /// Repo-native proof artifact kind (e.g. http_contract_json); requires
        /// --proof-kind journey.
        #[arg(long)]
        repo_native_kind: Option<String>,
        /// Proof artifact path or reference; requires --proof-kind journey.
        #[arg(long)]
        artifact: Option<String>,
    },
    /// Run stored proof commands for an intent (or --all pending).
    Run {
        #[arg(default_value = "")]
        intent: String,
        /// Run every pending validation.
        #[arg(long)]
        all: bool,
    },
    /// Record a proof result by hand; the outcome is positional.
    Verdict {
        key: String,
        /// passed | failed | blocked
        outcome: String,
        #[arg(long, default_value = "")]
        evidence: String,
        #[arg(long, default_value = "")]
        reason: String,
    },
    /// Show a validation (type/command/status + the intent it validates).
    Show { key: String },
    /// Edit a validation's type and/or command.
    Update {
        key: String,
        #[arg(long)]
        r#type: Option<String>,
        #[arg(long)]
        command: Option<String>,
    },
    /// Unlink a validation from an intent (removes the validates edge by name).
    Unlink { validation: String, intent: String },
    /// Remove a validation (e.g. a stale proof). Cascades its validates/calls edges.
    Remove { key: String },
    /// List validations.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}

#[derive(Subcommand, Debug)]
pub enum HypothesisCmd {
    /// Add a hypothesis targeting an intent.
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        claim: String,
        #[arg(long, default_value = "")]
        proposal: String,
        #[arg(long, default_value = "")]
        predicted_outcome: String,
        #[arg(long)]
        target: String,
    },
    /// Prove or refute; the outcome is positional.
    Prove {
        key: String,
        /// supported | refuted
        outcome: String,
        #[arg(long, default_value = "")]
        evidence: String,
    },
    /// Adopt a supported hypothesis (spawns a planned intent).
    Adopt {
        key: String,
        #[arg(long)]
        spawned: Option<String>,
    },
    /// Reject a hypothesis.
    Reject {
        key: String,
        #[arg(long)]
        reason: String,
    },
    /// Refine a proposed hypothesis. Proven/adopted/rejected hypotheses are
    /// history; remove only mistaken hypotheses.
    Update {
        key: String,
        #[arg(long)]
        claim: Option<String>,
        #[arg(long)]
        proposal: Option<String>,
        #[arg(long)]
        predicted_outcome: Option<String>,
        #[arg(long)]
        reason: String,
    },
    /// Delete a mistaken hypothesis. Cascades its target edges.
    Remove { key: String },
    /// Show a hypothesis (claim/proposal/predicted outcome + target).
    Show { key: String },
    /// List hypotheses.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}

#[derive(Subcommand, Debug)]
pub enum SurfaceCmd {
    /// Declare an interface surface (optionally exposing a codefile).
    Add {
        #[arg(long)]
        name: String,
        /// http | cli | ui_route | message_topic | sdk_method | internal_module | storage
        #[arg(long, default_value = "http")]
        kind: String,
        #[arg(long, default_value = "")]
        identity: String,
        #[arg(long)]
        codefile: Option<String>,
    },
    /// Show a surface.
    Show { key: String },
    /// Edit a surface: change kind/identity and/or re-bind the exposed codefile.
    Update {
        key: String,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        identity: Option<String>,
        #[arg(long)]
        codefile: Option<String>,
    },
    /// Remove an interface surface. Cascades its exposes/calls edges.
    Remove { key: String },
    /// List surfaces.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Report surface-plane gaps: unexposed surfaces and surfaces never
    /// called by a validation.
    Gaps,
}

#[derive(Subcommand, Debug)]
pub enum VocabCmd {
    /// Register a vocabulary term.
    Add {
        term: String,
        #[arg(long, default_value = "")]
        why: String,
    },
    /// Remove a vocabulary term (cascade-untags any nodes carrying it).
    Remove { term: String },
    /// Rename a vocabulary term across all tags, merging into an existing term
    /// when present and deduping nodes that carried both terms.
    Rename {
        from: String,
        to: String,
        #[arg(long)]
        reason: String,
    },
    /// List vocabulary terms.
    List,
}

#[derive(Subcommand, Debug)]
pub enum LayerCmd {
    /// Declare the architecture layer order (top first).
    Order { layers: Vec<String> },
    /// Show the declared order.
    List,
    /// Clear the declared order.
    Clear,
}

#[derive(Subcommand, Debug)]
pub enum IgnoreCmd {
    /// Exclude a glob from coverage with a recorded reason.
    Add {
        glob: String,
        #[arg(long)]
        reason: String,
    },
    /// Remove a coverage-ignore rule by its glob.
    Remove { glob: String },
    /// List ignore rules.
    List,
}

#[derive(Subcommand, Debug)]
pub enum ProposalCmd {
    /// Capture a structured proposal from text or a file.
    Add {
        #[arg(long)]
        title: String,
        /// Read the proposal body from a file (use --file or --text, not both).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Inline proposal body text (use --file or --text, not both).
        #[arg(long)]
        text: Option<String>,
    },
    /// List captured proposals.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Show a proposal by id, name, or unique fragment.
    Show { key: String },
    /// Delete a mistaken proposal capture, including its embedded items. Adopted
    /// spawned work remains separate graph history.
    Remove { key: String },
    /// Proposal item subcommands.
    Item {
        #[command(subcommand)]
        cmd: ProposalItemCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProposalItemCmd {
    /// Add a numbered item to a proposal.
    Add {
        proposal: String,
        #[arg(long)]
        text: String,
        /// Optional kind label (e.g. intent, task, observation).
        #[arg(long)]
        kind: Option<String>,
    },
    /// Adopt an item, optionally spawning an intent or task.
    Adopt {
        proposal: String,
        number: usize,
        /// Spawn a planned Intent or a proposed TaskRecord.
        #[arg(long)]
        r#as: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Defer an item with a reason.
    Defer {
        proposal: String,
        number: usize,
        #[arg(long)]
        reason: String,
    },
    /// Reject an item with a reason.
    Reject {
        proposal: String,
        number: usize,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum JourneyCmd {
    /// Add a journey from a JSON or YAML spec (creates a journey Validation + validates edges to step intents).
    Add { spec: PathBuf },
    /// Remove a journey and its validation node(s) by journey id (cleans up
    /// duplicates from an older non-idempotent add).
    Remove {
        /// The journey id (the spec's `journey:` value).
        id: String,
    },
    /// List journey validations.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Joined map: every journey validation with its step intents (via
    /// Validates edges), plus every active intent no journey exercises.
    /// Deliberately unbounded — a truncated map would hide the gaps it
    /// exists to expose.
    Map,
    /// Execute a journey spec and record the result onto the graph.
    Run {
        /// Path to the journey or HTTP contract spec file (.json or .yaml/.yml).
        spec: PathBuf,
        /// Override the base URL (takes precedence over the spec's `base` field
        /// and `{{ env.BASE_URL }}`).
        #[arg(long)]
        base_url: Option<String>,
    },
    /// Execute a journey spec directly without a graph (failure diagnosis).
    Diagnose {
        /// Path to the journey or HTTP contract spec file (.json or .yaml/.yml).
        spec: PathBuf,
        /// Override the base URL (takes precedence over the spec's `base` field
        /// and `{{ env.BASE_URL }}`).
        #[arg(long)]
        base_url: Option<String>,
    },
    /// Journey coverage commands (mark flows needing a journey proof).
    Coverage {
        #[command(subcommand)]
        cmd: JourneyCoverageCmd,
    },
    /// Journey invariant point commands (mark internal domain assertions).
    Invariant {
        #[command(subcommand)]
        cmd: JourneyInvariantCmd,
    },
    /// Generate a typed journey-runner prompt context from loom's code
    /// understanding of an intent. Read-time assembly, not code generation.
    Prompt {
        /// The intent whose flow needs a typed runner (id, name, or fragment).
        intent: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum JourneyCoverageCmd {
    /// Mark a flow as needing a journey proof, linked to an intent.
    Add {
        #[arg(long)]
        name: String,
        /// Flow path, e.g. "src/facade.rs::resolve -> record -> standing".
        #[arg(long)]
        flow: String,
        /// The intent this coverage concerns (id, name, or unique fragment).
        intent: String,
        #[arg(long, default_value = "")]
        description: String,
        /// Optional typed runner reference (path or path::symbol) that must exist.
        #[arg(long)]
        runner_ref: Option<String>,
        /// Optional test reference (path or path::symbol) that must exist.
        #[arg(long)]
        test_ref: Option<String>,
        /// Optional contract/journey artifact path expected to back the proof.
        #[arg(long)]
        contract_artifact: Option<String>,
    },
    /// Fix a journey coverage declaration's proof references.
    Update {
        key: String,
        #[arg(long)]
        runner_ref: Option<String>,
        #[arg(long)]
        test_ref: Option<String>,
        #[arg(long)]
        contract_artifact: Option<String>,
        #[arg(long)]
        reason: String,
    },
    /// Withdraw a mistaken coverage declaration.
    Remove { key: String },
    /// List journey coverage nodes with their effective coverage status.
    /// effective_status is DERIVED: "covered" iff the linked intent currently
    /// has a passing L5/L6 journey validation (proof_kind=journey). Runner/test
    /// ref existence alone does not flip coverage; run the proof first.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Discover coverage gaps: user-visible implemented intents with no passing
    /// L5 journey proof and no journey_coverage node. Graph-derived (from
    /// visibility + lifecycle + validations), not static call-graph analysis.
    /// With --spawn-missing, auto-create a journey_coverage node for each gap.
    Discover {
        /// Auto-create a journey_coverage node for each discovered gap.
        #[arg(long)]
        spawn_missing: bool,
    },
    /// Enforce drift metadata around covered journey entries: configured
    /// runner/test refs must still exist, and configured contract artifacts must
    /// match the current passing L5 journey proof.
    Drift,
}

#[derive(Subcommand, Debug)]
pub enum JourneyInvariantCmd {
    /// Mark an internal domain invariant point on an intent.
    Add {
        #[arg(long)]
        name: String,
        /// The intent this invariant concerns (id, name, or unique fragment).
        intent: String,
        #[arg(long)]
        field: String,
        #[arg(long)]
        assertion: String,
        #[arg(long, default_value = "")]
        reason: String,
    },
    /// Fix an invariant point while preserving its audit trail.
    Update {
        key: String,
        #[arg(long)]
        field: Option<String>,
        #[arg(long)]
        assertion: Option<String>,
        /// Re-point the invariant at a different intent (id, name, or unique
        /// fragment). Replaces the asserts edge; the node, its history, and
        /// its notes stay intact.
        #[arg(long)]
        asserts: Option<String>,
        /// Replacement body reason for the invariant itself.
        #[arg(long = "reason-text")]
        reason_text: Option<String>,
        #[arg(long)]
        reason: String,
    },
    /// Withdraw a mistaken invariant point.
    Remove { key: String },
    /// List journey invariant points.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}

/// Output format for a scan adapter (`--format`).
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ScanFormatArg {
    /// Line-oriented text parsed by a regex map (GCC-style default).
    Lines,
    /// A JSON array/JSONL of finding objects; `--map` renames looked-up
    /// fields (`items=…,file=…,line=…,msg=…,code=…`, dotted paths allowed).
    Json,
}

impl From<ScanFormatArg> for crate::scan::ScanFormat {
    fn from(arg: ScanFormatArg) -> Self {
        match arg {
            ScanFormatArg::Lines => Self::Lines,
            ScanFormatArg::Json => Self::Json,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum ScanCmd {
    /// Register an external diagnostic tool (any language's linter/checker).
    Add {
        /// Adapter name (e.g. clippy, eslint, ruff).
        name: String,
        /// The command to run, e.g. "cargo clippy --message-format=short".
        command: String,
        /// Parser map. `lines`: regex with named groups `file` and `line`
        /// (optional `msg`, `code`); default GCC-style `file:line[:col]:
        /// message`. `json`: comma-separated `field=path` lookups
        /// (`items|file|line|msg|code`, dotted paths allowed).
        #[arg(long)]
        map: Option<String>,
        /// Output format (default: lines).
        #[arg(long, value_enum, default_value_t = ScanFormatArg::Lines)]
        format: ScanFormatArg,
    },
    /// Edit a registered adapter in place. Use this when the command or parser
    /// map changed; run scan afterwards to refresh derived findings.
    Update {
        name: String,
        #[arg(long)]
        command: Option<String>,
        #[arg(long)]
        map: Option<String>,
        /// Switch the output format (lines | json).
        #[arg(long, value_enum)]
        format: Option<ScanFormatArg>,
    },
    /// List registered adapters.
    List,
    /// Remove an adapter.
    Remove { name: String },
    /// Run one adapter (or all) and convert diagnostics into derived findings.
    Run { name: Option<String> },
}

#[derive(Subcommand, Debug)]
pub enum ThresholdCmd {
    /// Show the current gates (configured values, or shipped defaults if unset).
    List,
    /// Hand-set one gate (e.g. `max_args 8`); persists to config.thresholds.
    Set {
        /// Gate name: max_file_loc | max_symbol_complexity | max_symbol_loc |
        /// max_nesting | max_args | max_file_owners.
        gate: String,
        /// The new threshold (strict `>` bound; must be >= 1).
        value: u64,
    },
    /// Reset one gate to its shipped default, or all gates when omitted.
    Reset {
        /// Gate name; omit to reset every gate.
        gate: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum PolicyCmd {
    /// Show the current policy (configured values, or shipped defaults if unset).
    Show,
    /// Set the review-confidence floor (a fraction in [0.0, 1.0]); persists to
    /// config.evidence_policy.
    SetFloor {
        /// The new floor: verdicts strictly below it route to review.
        value: f64,
    },
    /// Add an owner lane to the human-gated set (builder | analyzer | fixer |
    /// validator | quality).
    GateAdd {
        /// The lane whose work packets should carry a human gate.
        role: String,
    },
    /// Remove an owner lane from the human-gated set.
    GateRemove {
        /// The lane to stop gating.
        role: String,
    },
    /// Reset the whole policy to the shipped defaults (drops the config).
    Reset,
}

#[derive(Subcommand, Debug)]
pub enum WikiCmd {
    /// Plan (create or re-ground) a draft wiki page and the intents it will
    /// document. Leaves it `draft` for `wiki next`; write the prose, then
    /// `wiki record`.
    Plan {
        /// Page title (its stable name).
        title: String,
        /// Output path for the authored markdown (e.g. docs/wiki/architecture.md).
        #[arg(long)]
        path: String,
        /// An intent this page documents (repeatable: --covers A --covers B).
        #[arg(long = "covers")]
        covers: Vec<String>,
    },
    /// Mark an authored page fresh — stamp the scope fingerprint of everything it
    /// documents (the prose must already be written at the page's path).
    Record {
        /// Page title.
        title: String,
    },
    /// Emit a brief for the next page that needs writing (a draft, or a stale
    /// page whose documented scope drifted).
    Next,
    /// List wiki pages and their freshness.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Remove a wiki page by title.
    Remove {
        /// Page title.
        title: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum GraphCmd {
    /// Link an upstream graph via its committed export (loom.graph.json).
    Link {
        /// Path to the upstream `loom.graph.json`.
        path: PathBuf,
        /// Human alias for this upstream (default: the upstream graph's name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Unlink an upstream graph by alias or graph-id.
    Unlink {
        /// Alias or graph-id of the upstream to remove.
        key: String,
    },
    /// List linked upstream graphs.
    List,
}
