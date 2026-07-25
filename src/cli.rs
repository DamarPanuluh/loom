//! CLI surface (ring 1 subset).
//!
//! Plane: argument parsing only. Every handler lives in `crate::commands`.
//! The surface grows ring by ring; this is init/intent/codefile/export/import.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod subcommands;
pub use subcommands::*;

#[path = "cli/debt.rs"]
mod debt;
pub use debt::DebtCmd;

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

    /// The subcommand; omit for a plain-English orientation (`loom welcome`).
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Plain-English orientation: what loom is + the one thing to do next.
    Welcome,
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
    Import {
        file: PathBuf,
        /// Drop dangling facets/tags (targets absent from the export) instead of
        /// refusing the import — the recovery path for a legacy or cross-version
        /// export. Durable adjudication verdicts are preserved regardless.
        #[arg(long)]
        repair_orphans: bool,
    },
    /// Apply one atomic batch of mutations from a JSON/YAML file — intents,
    /// groundings, relationships, verdicts, finding adjudications, vocab terms,
    /// and intent tags in a single transaction. Collapses the per-mutation call
    /// storm of a work session into one call; any rejected item rolls the whole
    /// batch back.
    Apply { file: PathBuf },
    /// Recompute the structural plane and ripple staleness.
    Sync {
        /// Suppress successful sync output (for git hooks).
        #[arg(long)]
        quiet: bool,
    },
    /// Print graph identity and counts.
    Status,
    /// Show or set the graph mode — `owned` (build + prove) or `observed`
    /// (maps code you don't own; build/fix lanes off). Omit the arg to print
    /// the current mode. This is the post-init counterpart to `init --observed`;
    /// `sync` never changes the mode.
    Mode {
        /// owned | observed. Omit to print the current mode.
        #[arg(value_enum)]
        mode: Option<GraphModeArg>,
    },
    /// The next work item (asserted residue) with its prompt contract.
    Next {
        /// The queue to serve; omit for the highest-priority item overall.
        #[arg(long, value_enum)]
        mode: Option<ModeArg>,
        /// Closeout view: the top item of every queue at once. With `--mode
        /// <m>`, instead list the FULL depth of that one queue (every item it
        /// would serve, in priority order).
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
    /// Product question commands (human-gated decisions linked to intents).
    Question {
        #[command(subcommand)]
        cmd: QuestionCmd,
    },
    /// TaskRecord commands (spikes / investigations; never certify truth).
    Task {
        #[command(subcommand)]
        cmd: TaskCmd,
    },
    /// Durable notes on any node or edge (decision/context/warning trails).
    Note {
        #[command(subcommand)]
        cmd: NoteCmd,
    },
    /// Turn-zero offer menu: how this session could be spent.
    Session,
    /// Adopt a lane's discipline (the prompt contract for a role).
    Guide {
        /// The lane to adopt; omit for the whole driving protocol.
        #[arg(long, value_enum)]
        role: Option<RoleArg>,
    },
    /// Search the graph for intents/codefiles by keyword (fuzzy), facet, or tag.
    Find {
        /// Keyword query; optional when --tag / --where alone filter the set.
        #[arg(default_value = "")]
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Whole-name (case-insensitive) matches only — a reliable existence
        /// check, with no fuzzy substring scoring.
        #[arg(long)]
        exact: bool,
        /// Restrict to nodes tagged with this vocabulary term.
        #[arg(long)]
        tag: Option<String>,
        /// Restrict to nodes with facet key=value (repeatable; AND). Allowed
        /// keys: visibility, level, aspect, origin, ratification.
        #[arg(long = "where", value_name = "KEY=VALUE")]
        where_facets: Vec<String>,
    },
    /// Read-only neighborhood brief for an intent (not a work lane).
    Explain {
        /// Intent name, id, or unique fragment.
        intent: String,
    },
    /// Pull one read-only context packet for an intent, registered file, or query.
    Context {
        /// Intent id/name/prefix, registered codefile path, or free-text query.
        target: String,
    },
    /// Detect repo languages and recommend quality packs.
    Detect,
    /// Register/run external diagnostic tools (linters, type-checkers) whose
    /// output becomes derived findings.
    Scan {
        #[command(subcommand)]
        cmd: ScanCmd,
    },
    /// Derive structural finding thresholds from this repo's code distribution
    /// (preview by default; --write persists them as portable config).
    Calibrate {
        /// Persist the proposed thresholds (they travel in the export).
        #[arg(long)]
        write: bool,
    },
    /// Hand-set the structural finding thresholds (the manual counterpart to
    /// `calibrate`; persists to portable config).
    Threshold {
        #[command(subcommand)]
        cmd: ThresholdCmd,
    },
    /// Read or set the evidence policy (review-confidence floor + human-gate
    /// placement); persists to portable config, absent = shipped defaults.
    Policy {
        #[command(subcommand)]
        cmd: PolicyCmd,
    },
    /// The Definition-of-Complete scorecard: per-intent axes met/open/waived.
    Completeness {
        /// One intent (name, id, or fragment); omit for all feature intents.
        key: Option<String>,
    },
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
    /// Structural smells (computed from graph shape, each with a remedy).
    Smells,
    /// Statistical debt feed (advisory; never required work). Promote a cluster
    /// to an asserted Finding with `loom debt promote` — the feed itself stays
    /// unstored and non-gating.
    Debt {
        #[command(subcommand)]
        cmd: Option<DebtCmd>,
    },
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
    /// Capture a structured proposal and decompose it into adopted work.
    Proposal {
        #[command(subcommand)]
        cmd: ProposalCmd,
    },
    /// Journey proof, coverage, and invariant-point commands.
    Journey {
        #[command(subcommand)]
        cmd: JourneyCmd,
    },
    /// Run an interactive, journaled human drive session, or freeze a session.
    Drive {
        #[command(subcommand)]
        cmd: Option<DriveCmd>,
    },
    /// Install or remove local git hooks that keep the structural plane fresh.
    Hook {
        #[command(subcommand)]
        cmd: HookCmd,
    },
    /// What a change here could reach: the symbols that transitively call it,
    /// the intents they belong to, and how well each proves them.
    Impact {
        /// A symbol name, or a registered codefile path.
        target: String,
        /// How many call hops to walk back (default 3).
        #[arg(long, default_value_t = 3)]
        depth: usize,
    },
    /// Read the working tree and propose the graph mutations it implies:
    /// new symbols in owned files, symbols whose callers all belong to one
    /// behavior, locators pointing at code that moved, files nothing owns.
    /// Observes only — the batch lands as a Proposal you confirm.
    Absorb {
        /// Adopt every item that needs nothing from you.
        #[arg(long)]
        confirm: bool,
    },
    /// Turn the falsifiability claim on loom's own record: fabricated
    /// ratifications, judgment bursts too fast to have been made one at a
    /// time, and settled claims standing on nothing re-checkable.
    Audit,
    /// What to strengthen next, once every floor is met. Ranks behaviors by
    /// blast radius x (1 - proof strength) x evidence age, and names the one
    /// move that would raise each. This queue re-orders; it never empties.
    Deepen {
        /// How many candidates to show (default 5).
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },
    /// Serve loom in-band over MCP (stdio JSON-RPC), so an agent pulls context
    /// as a tool call instead of shelling out. Speaks on stdin/stdout: run it
    /// from an MCP client, not interactively.
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },
    /// Reader-first wiki pages, tracked as a projection of the graph: record a
    /// page's documented intents, get the next page to write, list, or remove.
    Wiki {
        #[command(subcommand)]
        cmd: WikiCmd,
    },
    /// Cross-graph federation: link/unlink/list upstream graphs.
    Graph {
        #[command(subcommand)]
        cmd: GraphCmd,
    },
    /// Cold-start assist: draft a Proposal of planned pillar intents from
    /// derived signals (codefiles, tests, README). Never auto-verdicts.
    Bootstrap {
        #[command(subcommand)]
        cmd: BootstrapCmd,
    },
}

/// Queue names for `loom next --mode`; `--help` is the enumeration.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ModeArg {
    Fix,
    Validate,
    Build,
    Coverage,
    Quality,
    /// Alias: discovery.
    #[value(alias = "discovery")]
    Analyze,
    Prove,
    Triage,
    Review,
    Elaborate,
    /// Human-presence queue: intents awaiting the human authority's
    /// ratification (never served by plain `loom next`).
    Ratify,
}

impl ModeArg {
    pub fn as_str(self) -> &'static str {
        match self {
            ModeArg::Fix => "fix",
            ModeArg::Validate => "validate",
            ModeArg::Build => "build",
            ModeArg::Coverage => "coverage",
            ModeArg::Quality => "quality",
            ModeArg::Analyze => "analyze",
            ModeArg::Prove => "prove",
            ModeArg::Triage => "triage",
            ModeArg::Review => "review",
            ModeArg::Elaborate => "elaborate",
            ModeArg::Ratify => "ratify",
        }
    }
}

/// The graph mode for `loom mode`.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum GraphModeArg {
    /// Normal mode: build + prove; all lanes active.
    Owned,
    /// Monitoring mode: maps code the driver does not own; build/fix/coverage/
    /// elaborate lanes disabled (discovery/quality/validation only).
    Observed,
}

impl GraphModeArg {
    pub fn is_observed(self) -> bool {
        matches!(self, GraphModeArg::Observed)
    }
}

/// Lane names for `loom guide --role`.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum RoleArg {
    Builder,
    Analyzer,
    Fixer,
    Validator,
    Quality,
    Monitor,
}

impl RoleArg {
    pub fn as_str(self) -> &'static str {
        match self {
            RoleArg::Builder => "builder",
            RoleArg::Analyzer => "analyzer",
            RoleArg::Fixer => "fixer",
            RoleArg::Validator => "validator",
            RoleArg::Quality => "quality",
            RoleArg::Monitor => "monitor",
        }
    }
}
