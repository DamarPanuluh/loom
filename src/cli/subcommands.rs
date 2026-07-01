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
        /// Permit a symbol-looking name (requires a behavioral description).
        #[arg(long)]
        allow_symbol_name: bool,
    },
    /// Show an intent by id, name, or unique fragment.
    Show { key: String },
    /// Correct the intent's attributes (level/visibility) without redefining it.
    Set {
        key: String,
        /// system | component | feature | cross_cutting
        #[arg(long)]
        level: Option<String>,
        /// user_visible | internal
        #[arg(long)]
        visibility: Option<String>,
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
    },
    /// Set the prescriptive lifecycle (planned|implemented|needs_change).
    Mark {
        key: String,
        #[arg(long)]
        lifecycle: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Redefine the description (ripples one hop) or rename (--reword: no ripple).
    Update {
        key: String,
        #[arg(long)]
        description: String,
        #[arg(long)]
        reason: String,
        /// Clearer words, same concept: no ripple.
        #[arg(long)]
        reword: bool,
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
        /// add | remove
        action: String,
        key: String,
        term: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum CodefileCmd {
    /// Register a codefile by path.
    Add { path: String },
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
    },
}

#[derive(Subcommand, Debug)]
pub enum InboxCmd {
    /// Capture raw input.
    Add {
        text: String,
        #[arg(long, default_value = "human")]
        source: String,
    },
    /// List inbox items.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Mark an item's disposition (routed|rejected|duplicate|deferred).
    Mark {
        key: String,
        #[arg(long)]
        status: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Remove an inbox item (e.g. a resolved or accidental capture).
    Remove { key: String },
}

#[derive(Subcommand, Debug)]
pub enum TaskCmd {
    /// Open a task record (spike|investigation|experiment|review|chore).
    Add {
        title: String,
        #[arg(long, default_value = "spike")]
        kind: String,
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
    /// Show a task record (kind/status/result).
    Show { key: String },
    /// List task records.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}
#[derive(Subcommand, Debug)]
pub enum FindingCmd {
    /// List derived findings with their adjudication state.
    List {
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        state: Option<String>,
    },
    /// Record a judgment on a finding: justified | needed | blocked.
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
    /// Record a verdict (creates the governs edge if needed).
    Verdict {
        rule: String,
        intent: String,
        /// passing | failing | independent
        #[arg(long)]
        status: String,
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
    /// Remove a quality rule (and its governs edges).
    Remove { key: String },
    /// Stop a rule governing an intent (removes the governs edge by name).
    Ungovern { rule: String, intent: String },
}

#[derive(Subcommand, Debug)]
pub enum ValidationCmd {
    /// Add a validation and link it to an intent.
    Add {
        #[arg(long)]
        name: String,
        /// test | assertion | benchmark | manual_check | saga | scenario | contract
        #[arg(long, default_value = "test")]
        r#type: String,
        #[arg(long, default_value = "")]
        command: String,
        #[arg(long)]
        intent: String,
    },
    /// Mark a result by hand (passed|failed|blocked).
    Mark {
        key: String,
        #[arg(long)]
        result: String,
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
    /// Delete a validation (e.g. a stale proof). Cascades its validates/calls edges.
    Delete { key: String },
    /// List validations.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
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
    /// Prove or refute.
    Prove {
        key: String,
        /// supported | refuted
        #[arg(long)]
        verdict: String,
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
    /// Show a hypothesis (claim/proposal/predicted outcome + target).
    Show { key: String },
    /// List hypotheses.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
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
    /// Delete an interface surface. Cascades its exposes/calls edges.
    Delete { key: String },
    /// List surfaces.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}

#[derive(Subcommand, Debug)]
pub enum SagaCmd {
    /// Add a saga from a JSON spec (creates a saga Validation + step edges).
    Add { spec: PathBuf },
    /// List saga validations.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Execute a saga spec and stamp the result onto the graph.
    Run { spec: PathBuf },
    /// Execute a saga spec without writing the graph (failure diagnosis).
    Diagnose { spec: PathBuf },
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
pub enum InterfaceCmd {
    /// Report interface-plane gaps.
    Gaps,
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
    },
    /// Show a proposal by id, name, or unique fragment.
    Show { key: String },
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
