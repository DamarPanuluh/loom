//! CLI surface (ring 1 subset).
//!
//! Plane: argument parsing only. Every handler lives in `crate::commands`.
//! The surface grows ring by ring; this is init/intent/codefile/export/import.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod subcommands;
pub use subcommands::*;

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
        /// build | fix | analyze/discovery | validate | quality | prove | triage (omit for highest-priority)
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
    /// Capture a structured proposal and decompose it into adopted work.
    Proposal {
        #[command(subcommand)]
        cmd: ProposalCmd,
    },
}
