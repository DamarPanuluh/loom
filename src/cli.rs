//! CLI surface (ring 1 subset).
//!
//! Plane: argument parsing only. Every handler lives in `crate::commands`.
//! The surface grows ring by ring; this is init/intent/codefile/export/import.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "loom",
    version,
    about = "A falsifiable graph of what a codebase should do."
)]
pub struct Cli {
    /// Pin the target graph root explicitly (overrides cwd discovery).
    #[arg(long, global = true)]
    pub graph: Option<PathBuf>,

    /// Machine-readable output.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Initialize a graph store at the given path (default: cwd).
    Init {
        path: Option<PathBuf>,
        /// Human name for this graph (default: directory name).
        #[arg(long)]
        name: Option<String>,
        /// This graph maps code the driver does not own (discovery-only).
        #[arg(long)]
        observed: bool,
    },
    /// Intent commands.
    Intent {
        #[command(subcommand)]
        cmd: IntentCmd,
    },
    /// CodeFile commands.
    Codefile {
        #[command(subcommand)]
        cmd: CodefileCmd,
    },
    /// Write the deterministic export (loom.graph.json).
    Export {
        /// Exit non-zero if the committed export drifts from the live graph.
        #[arg(long)]
        check: bool,
    },
    /// Restore a graph from an export into a fresh store.
    Import { file: PathBuf },
    /// Recompute the structural plane and ripple staleness.
    Sync,
    /// Print graph identity and counts.
    Status,
    /// The next work item (asserted residue) with its prompt contract.
    Next {
        /// build | fix | analyze (omit for highest-priority)
        #[arg(long)]
        mode: Option<String>,
        /// Closeout view: every queue at once.
        #[arg(long)]
        all: bool,
    },
    /// Edge commands.
    Edge {
        #[command(subcommand)]
        cmd: EdgeCmd,
    },
    /// Capture a raw utterance as an inbox item (the entrance).
    Door { utterance: String },
    /// Inbox commands (the single free-form input boundary).
    Inbox {
        #[command(subcommand)]
        cmd: InboxCmd,
    },
    /// TaskRecord commands (spikes / investigations; never certify truth).
    Task {
        #[command(subcommand)]
        cmd: TaskCmd,
    },
    /// Turn-zero offer menu: how this session could be spent.
    Session,
    /// Adopt a lane's discipline (the prompt contract for a role).
    Guide {
        /// builder | analyzer | fixer | validator | quality | monitor
        #[arg(long)]
        role: Option<String>,
    },
    /// Search the graph for intents/codefiles by keyword.
    Find {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Detect repo languages and recommend quality packs.
    Detect,
    /// Print the data model: node/edge kinds, statuses, vocabularies.
    Schema,
    /// Quality rule commands.
    Rule {
        #[command(subcommand)]
        cmd: RuleCmd,
    },
    /// Validation (proof) commands.
    Validation {
        #[command(subcommand)]
        cmd: ValidationCmd,
    },
    /// Run proofs for an intent (or --all pending).
    Validate {
        #[arg(default_value = "")]
        intent: String,
        #[arg(long)]
        all: bool,
    },
    /// Hypothesis commands (ideas proven before they become work).
    Hypothesis {
        #[command(subcommand)]
        cmd: HypothesisCmd,
    },
    /// Interface surface commands.
    Surface {
        #[command(subcommand)]
        cmd: SurfaceCmd,
    },
    /// Saga (composition proof) commands.
    Saga {
        #[command(subcommand)]
        cmd: SagaCmd,
    },
    /// Vocabulary commands.
    Vocab {
        #[command(subcommand)]
        cmd: VocabCmd,
    },
    /// Architecture layer-order commands.
    Layer {
        #[command(subcommand)]
        cmd: LayerCmd,
    },
    /// Interface-plane gaps (uncalled surfaces, unbound boundaries).
    Interface {
        #[command(subcommand)]
        cmd: InterfaceCmd,
    },
    /// Structural smells (computed from graph shape, each with a remedy).
    Smells,
    /// Statistical debt feed (advisory; never required work).
    Debt,
    /// Derived code findings and durable adjudication verdicts.
    Finding {
        #[command(subcommand)]
        cmd: FindingCmd,
    },
    /// Integrity audit (exits non-zero on any violation).
    Doctor,
    /// Vertical-spine coverage: grounding, ownership, unaccounted files.
    Coverage,
    /// Coverage exclusion commands.
    Ignore {
        #[command(subcommand)]
        cmd: IgnoreCmd,
    },
    /// Report the acting agent identity and lane enforcement.
    Whoami,
}

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
