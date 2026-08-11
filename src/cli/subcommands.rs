//! Subcommand argument shapes for every `loom` command family.
//!
//! Plane: surface — argument shape only. These enums declare flag names,
//! defaults, and help text; every handler lives in `crate::commands`. Nothing
//! here opens a store, resolves a graph, or contains logic beyond clap parsing.

use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum PatternCmd {
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        rationale: String,
        #[arg(long)]
        when_to_use: String,
        #[arg(long)]
        when_not_to_use: String,
        #[arg(long = "path")]
        paths: Vec<String>,
        #[arg(long = "intent-tag")]
        intent_tags: Vec<String>,
    },
    Update {
        key: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        rationale: Option<String>,
        #[arg(long)]
        when_to_use: Option<String>,
        #[arg(long)]
        when_not_to_use: Option<String>,
        #[arg(long = "path")]
        paths: Vec<String>,
        #[arg(long = "intent-tag")]
        intent_tags: Vec<String>,
        /// Intentionally remove every path selector.
        #[arg(long, conflicts_with = "paths")]
        clear_paths: bool,
        /// Intentionally remove every intent-tag selector.
        #[arg(long, conflicts_with = "intent_tags")]
        clear_intent_tags: bool,
        #[arg(long)]
        reason: String,
    },
    Show {
        key: String,
    },
    List,
    Lookup {
        #[arg(long = "path")]
        paths: Vec<String>,
        #[arg(long = "intent-tag")]
        intent_tags: Vec<String>,
        /// Skip this many matches in deterministic guidance order.
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    Ratify {
        key: String,
        #[arg(long)]
        evidence: String,
        /// Exact answer the human gave in the host conversation. Lets an LLM
        /// record the decision without acquiring authority to make it.
        #[arg(long)]
        human_decision: Option<String>,
    },
    Retire {
        key: String,
        #[arg(long)]
        reason: String,
    },
    Remove {
        key: String,
        #[arg(long)]
        reason: String,
    },
    Exemplar {
        #[command(subcommand)]
        cmd: PatternExemplarCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum PatternExemplarCmd {
    Add {
        pattern: String,
        codefile: String,
        #[arg(long)]
        locator: String,
    },
    Verdict {
        edge: String,
        verdict: String,
        #[arg(long)]
        criterion: String,
        #[arg(long)]
        evidence: String,
        #[arg(long, default_value_t = 0.9)]
        confidence: f64,
    },
    Remove {
        edge: String,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum IntentCmd {
    /// What stands on this behavior: every intent that transitively requires it,
    /// is a scenario of it, or decomposes into it — nearest first, each with
    /// whether it currently has a passing proof. The intent-graph twin of
    /// `loom impact`, which answers the same question for code.
    Dependents {
        key: String,
        /// How many edges to walk back (default 5).
        #[arg(long, default_value_t = 5)]
        depth: usize,
    },
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
    /// Declare that an Intent deliberately has no Journey ancestry. This is a
    /// human product decision, not a general completeness waiver.
    JourneyExempt {
        key: String,
        /// Stable exemption class, such as `infrastructure` or `repository_maintenance`.
        #[arg(long)]
        kind: String,
        /// Why this behavior is intentionally not rooted in a user Journey.
        #[arg(long)]
        reason: String,
        /// Exact answer the human gave in the host conversation. Required for
        /// non-interactive or llm:* execution; omit for a direct TTY challenge.
        #[arg(long)]
        human_decision: Option<String>,
    },
    /// Require Journey ancestry again by withdrawing a prior exemption.
    JourneyRequire {
        key: String,
        /// Why Journey ancestry is required again.
        #[arg(long)]
        reason: String,
        /// Exact answer the human gave in the host conversation. Required for
        /// non-interactive or llm:* execution; omit for a direct TTY challenge.
        #[arg(long)]
        human_decision: Option<String>,
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
    /// Ratify an intent (or --all unratified): the human authority's
    /// evidence-bearing "yes, this is wanted". A host LLM may record an
    /// explicit --human-decision, but may never make the decision itself.
    Ratify {
        key: Option<String>,
        /// Ratify every active unratified intent (bulk grandfathering).
        #[arg(long)]
        all: bool,
        /// Why this behavior is wanted: an utterance, source doc, or decision.
        #[arg(long)]
        evidence: Option<String>,
        /// Exact answer the human gave in the host conversation. Required for
        /// non-interactive or llm:* execution; omit for a direct TTY challenge.
        #[arg(long)]
        human_decision: Option<String>,
    },
    /// Say a behavior is NOT wanted. The cheap, high-leverage half of the
    /// authority: no typed challenge — the substantive reason IS the act —
    /// and every place the code still performs it becomes tracked work.
    Reject {
        key: String,
        /// Why this is not wanted. Recorded verbatim on the rejection.
        #[arg(long)]
        reason: String,
        /// Exact answer the human gave in the host conversation. Required for
        /// non-interactive or llm:* execution.
        #[arg(long)]
        human_decision: Option<String>,
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
        /// Rectify-lane handoff: `escalated` moves a discovered behavior to
        /// human ratify; `clear` removes the handoff marker.
        #[arg(long)]
        rectify: Option<String>,
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
    /// Record an evidence-backed semantic-impact assessment after a code change.
    /// `criterion_changed` routes wantedness back to the human ratify queue.
    Impact {
        key: String,
        /// preserved | changed_within_intent | criterion_changed
        #[arg(long)]
        classification: String,
        /// Concrete code or runtime evidence supporting the classification.
        #[arg(long)]
        evidence: String,
    },
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
    /// Issue a stable source anchor for a one-based source line without editing source or graph state.
    Anchor {
        path: String,
        #[arg(long, value_name = "LINE")]
        at_line: usize,
    },
    /// Re-expand every glob ever registered and add any newly-appeared files
    /// (e.g. an endpoint an upstream just added). Run before `loom sync`.
    Rescan,
    /// Unregister a codefile (e.g. the file was deleted/renamed/split on
    /// disk). With live asserted edges pointing at it and no --successor,
    /// refuses and lists every blocker. With --successor, each such edge is
    /// retargeted in place (keeping its verdict history) before the node is
    /// removed — one recorded graph operation for a rename/split.
    Remove {
        key: String,
        /// Successor codefile that now carries this file's behavior. Must be
        /// registered (`loom codefile add <path>` first).
        #[arg(long)]
        successor: Option<String>,
    },
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
pub enum CheckpointCmd {
    /// Inspect an exact Intent or cohesive bundle without staging or committing.
    Recommend {
        /// Intent id, name, or unique fragment; repeat for a cohesive bundle.
        #[arg(long = "intent", required = true)]
        intents: Vec<String>,
    },
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleasePhaseArg {
    /// Verify the candidate in one detached fresh-v12 workspace.
    IsolatedDogfood,
    /// Repeat the verification from independent empty workspaces and compare
    /// the semantic attestation, never build-directory bytes.
    FreshFixpoint,
    /// Run both gates and stop before every release, install, commit, or push mutation.
    GatedPreparation,
}

#[derive(Subcommand, Debug)]
pub enum ReleaseCmd {
    /// Seal one exact, current derivation batch after a human approves it.
    AuthorizeDerivations {
        /// Directory containing the reviewed loom.journey-derivation/v1 manifests.
        #[arg(long)]
        manifest_dir: PathBuf,
        /// Exact answer supplied by the human through the host conversation.
        #[arg(long)]
        human_decision: String,
    },
    /// Rehearse release gates in detached candidates without changing the caller.
    Rehearse {
        #[arg(long, value_enum)]
        phase: ReleasePhaseArg,
    },
    /// Internal typed source snapshot used by dogfood/fixpoint bootstrap.
    #[command(hide = true)]
    Snapshot {
        #[arg(long)]
        destination: PathBuf,
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
    /// Attach validation-specific code evidence used to derive the S3 call witness.
    Exercises {
        validation: String,
        codefile: String,
        /// Optional entry-point symbol in the exercised file.
        #[arg(long)]
        locator: Option<String>,
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
    /// Re-point an asserted edge's target at a successor node — the recorded
    /// operation of a file rename/split. In place: the edge keeps its id,
    /// locator/role, and verdict facts; sync's reverification re-anchors
    /// evidence that moved intact and stales what genuinely changed.
    Retarget {
        edge_id: String,
        /// Successor node (name, path, id, or fragment) — typically the
        /// renamed/split-to codefile.
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
    /// Open a product question for exactly one technical Intent or authored Journey.
    Add {
        text: String,
        #[arg(long, required_unless_present = "journey", conflicts_with = "journey")]
        intent: Option<String>,
        #[arg(long, required_unless_present = "intent", conflicts_with = "intent")]
        journey: Option<String>,
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
    /// Open a task record (spike|investigation|experiment|review|chore|research).
    Add {
        title: String,
        #[arg(long, default_value = "spike")]
        kind: String,
        /// Intent this task informs — the close/abandon outcome lands as a note on it.
        #[arg(long)]
        target: Option<String>,
        /// Why current/external knowledge is required (required for research).
        #[arg(long)]
        why_external: Option<String>,
        /// Preferred authoritative source guidance (repeatable; research only).
        #[arg(long = "preferred-source")]
        preferred_sources: Vec<String>,
    },
    /// Append one actual page read to a research task's provenance.
    SourceAdd {
        task: String,
        #[arg(long)]
        url: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        publisher: String,
        #[arg(long)]
        source_kind: String,
        #[arg(long)]
        quote: String,
        #[arg(long)]
        published_at: Option<String>,
        #[arg(long)]
        fresh_until: Option<String>,
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
        /// Where you looked: `file:line` in the flagged code, or a `journal:`
        /// ref. A settling verdict needs one — the reason says WHAT you decided,
        /// this says what you decided it FROM. Open states (`needed`,
        /// `blocked`) do not.
        #[arg(long, default_value = "")]
        evidence: String,
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
    /// Judge one pre-screen hit as not-what-the-rule-means, keyed by the
    /// matched text's content hash: judged once, it answers the same text on
    /// every future scan (any rule×intent pair, any shifted line) and expires
    /// by construction when the matched text changes.
    Suppress {
        rule: String,
        /// The matched text as shown in the hit listing (after `    `).
        #[arg(long)]
        excerpt: String,
        /// Why this hit is not what the rule means. The audit, not a formality.
        #[arg(long)]
        reason: String,
    },
    /// Withdraw a suppression; the hit re-opens on the next scan.
    Unsuppress {
        rule: String,
        /// Content hash (a prefix works) or the exact matched text.
        #[arg(long)]
        key: String,
    },
    /// The auditable ledger of hit suppressions, optionally scoped to one rule.
    Suppressions { rule: Option<String> },
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
    },
    /// Run one validation, every validation for an intent, or --all pending.
    Run {
        #[arg(default_value = "")]
        key: String,
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
    Remove {
        key: String,
        #[arg(long)]
        reason: String,
    },
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
pub enum JudgmentCmd {
    /// Stage a proposed ratify/reject/redefine for an intent, with the
    /// evidence a human will review. Staging is not gated — recommending is
    /// not deciding. One live proposal per (kind, intent).
    Propose {
        /// ratify | reject | redefine
        kind: String,
        /// The intent the proposal judges (name, id, or fragment).
        intent: String,
        /// Why the judgment holds: reject reason, ratify evidence, or
        /// redefine rationale. Substantive — this is what the human reviews.
        #[arg(long)]
        evidence: String,
        /// The replacement statement. Required for redefine; ignored
        /// otherwise.
        #[arg(long)]
        description: Option<String>,
    },
    /// The human's review surface: every staged proposal, oldest first,
    /// with the exact confirm command for each.
    Digest {
        /// Also show decided (confirmed/withdrawn) proposals.
        #[arg(long)]
        all: bool,
    },
    /// Execute a staged proposal through the SAME human gate the direct
    /// command demands: ratify/reject require the human's answer (mediated
    /// via --human-decision, or the typed challenge at a solo terminal);
    /// redefine applies the staged statement with its ripple.
    Confirm {
        key: String,
        /// Exact answer the human gave in the host conversation. Required
        /// for ratify/reject when the executor is an llm:* agent.
        #[arg(long)]
        human_decision: Option<String>,
    },
    /// Drop a staged proposal (the candidate was wrong, or the intent
    /// changed since staging). Requires a substantive reason.
    Withdraw {
        key: String,
        #[arg(long)]
        reason: String,
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
    /// Register an authored semantic Journey root from a JSON or YAML artifact.
    Add { spec: PathBuf },
    /// Show one Journey by stable id, node id, or unique fragment.
    Show { journey: String },
    /// Remove an authored Journey and its derived projections.
    Remove { journey: String },
    /// List authored Journey roots.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Show the Journey-root map and every unrooted, non-exempt Intent.
    Map,
    /// Emit a read-only packet for deriving technical Intents from a Journey.
    Derive {
        journey: String,
        /// Inspect one strict candidate manifest without accepting it. The value
        /// may be inline JSON or the path to a JSON manifest.
        #[arg(long, value_name = "MANIFEST_OR_JSON")]
        candidate_json: Option<String>,
    },
    /// Accept one exact, human-authorized technical derivation manifest.
    DeriveAccept {
        journey: String,
        #[arg(long)]
        manifest: PathBuf,
        /// Exact answer authorizing this hash-bound manifest.
        #[arg(long)]
        human_decision: String,
    },
    /// Emit a read-only contract for a real CLI projection in the target repo.
    Surface { journey: String },
    /// Accept one hash-bound reusable CLI surface manifest.
    SurfaceAccept {
        journey: String,
        #[arg(long)]
        manifest: PathBuf,
    },
    /// Compile the Journey's surfaced CLI into a runnable proof profile.
    Compile {
        journey: String,
        #[arg(long, default_value = "proof")]
        profile: String,
    },
    /// Run a compiled Journey profile and record what Loom observes.
    Run {
        journey: String,
        #[arg(long, default_value = "proof")]
        profile: String,
    },
    /// Resume one pending host-mediated Journey step with the human's exact answer.
    Resume {
        /// Opaque token returned by the pending Journey run.
        token: String,
        /// Exact stable id of one option presented by the pending gate.
        #[arg(long)]
        choice: String,
        /// Exact answer supplied by the human through the host conversation.
        #[arg(long)]
        human_decision: String,
        /// Substantive revision required only by an option marked free-form.
        #[arg(long)]
        free_form: Option<String>,
    },
    /// Diagnose a Journey with optional input overrides, without settling proof.
    Diagnose {
        journey: String,
        #[arg(long, default_value = "proof")]
        profile: String,
        /// Override one authored input as KEY=JSON (repeatable).
        #[arg(long = "input", value_name = "KEY=JSON")]
        input: Vec<String>,
    },
    /// Freeze the current observed result as the profile baseline.
    Freeze {
        journey: String,
        #[arg(long, default_value = "proof")]
        profile: String,
    },
    /// Report stale compiled Journey artifacts, optionally for one Journey.
    Drift { journey: Option<String> },
}

#[derive(Subcommand, Debug)]
pub enum DriveCmd {
    /// Compile recorded drive exchanges into a local journey YAML file.
    Freeze { name: String },
}

#[derive(Subcommand, Debug)]
pub enum HookCmd {
    /// Install idempotent post-commit and post-merge sync hooks.
    Install,
    /// Remove only hooks previously installed by Loom.
    Remove,
}

#[derive(Subcommand, Debug)]
pub enum McpCmd {
    /// Speak MCP over stdio until stdin closes. Register it with an MCP client
    /// as `loom mcp serve` (add `--graph <path>` when the client's working
    /// directory is not the repo).
    Serve,
    /// Drive one complete MCP session through the real stdio serve loop and
    /// return its ordered responses as one JSON document.
    Transcript {
        /// JSON array of JSON-RPC 2.0 request objects, in session order.
        #[arg(long, value_name = "JSON")]
        requests_json: String,
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
        /// max_nesting | max_args.
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
    ///
    /// Default keeps UpstreamIntent shadows orphaned (doctor flags them). Pass
    /// `--prune` when the upstream is permanently gone so shadows are disposed
    /// in the same step; remaining DependsOn claims refuse unless `--cascade`.
    Unlink {
        /// Alias or graph-id of the upstream to remove.
        key: String,
        /// Also delete this upstream's UpstreamIntent shadow nodes.
        #[arg(long)]
        prune: bool,
        /// With `--prune`, also cascade-delete DependsOn edges that still
        /// target those shadows (default refuses and lists the blocked ones).
        #[arg(long, requires = "prune")]
        cascade: bool,
    },
    /// Dispose orphan UpstreamIntent shadows left after `graph unlink`.
    ///
    /// Shadows whose alias is no longer in the upstream registry are removed.
    /// Orphans still targeted by local DependsOn edges are left in place
    /// unless `--cascade` is set (which removes those edges too).
    PruneOrphans {
        /// Only dispose orphans for this former alias (default: all orphans).
        #[arg(long)]
        alias: Option<String>,
        /// Also cascade-delete DependsOn edges that still target orphan shadows.
        #[arg(long)]
        cascade: bool,
    },
    /// List linked upstream graphs.
    List,
}

#[derive(Subcommand, Debug)]
pub enum AuditCmd {
    /// Inspect or accept a historical integrity incident.
    Incident {
        #[command(subcommand)]
        cmd: AuditIncidentCmd,
    },
    /// Seal a typed batch authorization over a legacy judgment burst.
    ///
    /// Requires contemporaneous evidence (journal events, apply/command
    /// records, validation runs, import tickets). A prose note written
    /// afterward is acknowledgment, not sufficient proof. Does not rewrite
    /// fact timestamps.
    ///
    /// A seal written AFTER the burst's final fact is accepted only when the
    /// authority is human and the evidence is a trusted digest-bound
    /// `batch_intent` record — the human-gated batch path's recorded
    /// HumanDecision for this exact subject set, predating the burst.
    /// The burst actor's own later seal is never accepted.
    AttestBurst {
        /// Burst key as reported by audit: `{actor}@{YYYY-MM-DDTHH:MM}`.
        subject: String,
        /// ratification | adjudication
        #[arg(long)]
        claim: String,
        /// Shared batch criterion / predicate.
        #[arg(long)]
        criterion: String,
        /// Contemporaneous evidence refs (repeatable). Prefer `journal:<id>`.
        #[arg(long = "evidence", required = true)]
        evidence: Vec<String>,
        /// Who authorized the batch (human for ratification).
        #[arg(long)]
        authority: String,
        /// Who executed the writes (often the LOOM_AGENT / llm).
        #[arg(long)]
        executor: String,
        /// The human's answer, when the seal is mediated by a host answer
        /// (same gate as `loom intent ratify --human-decision`). Without it,
        /// a retrospective seal demands the interactive typed challenge.
        #[arg(long)]
        human_decision: Option<String>,
        /// Required when the batch claims mechanical routing safety.
        #[arg(long)]
        routing_class: Option<String>,
        /// Permitted operation (default: ratify / verdict by claim).
        #[arg(long)]
        operation: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum AuditIncidentCmd {
    /// Accept an exact live burst as disclosed history, never as authorization.
    Accept {
        /// Burst key as reported by audit: `{actor}@{YYYY-MM-DDTHH:MM}`.
        subject: String,
        /// ratification | adjudication
        #[arg(long)]
        claim: String,
        /// Why this historical integrity exception is consciously accepted.
        #[arg(long)]
        reason: String,
        /// The human's exact answer from the host conversation. Without it,
        /// a direct terminal invocation demands the typed human challenge.
        #[arg(long)]
        human_decision: Option<String>,
    },
    /// List every disclosed incident, including imported history.
    List,
    /// Show the disclosure for one burst and claim.
    Show {
        /// Burst key as reported by audit: `{actor}@{YYYY-MM-DDTHH:MM}`.
        subject: String,
        /// ratification | adjudication
        #[arg(long)]
        claim: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum BootstrapCmd {
    /// Draft a Proposal of behavior clues from derived signals (registered
    /// codefiles, tests/, README H2s) to inform authored Journey roots.
    /// Never writes product meaning, Intents, edges, or verdicts.
    Suggest,
}
