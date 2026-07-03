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
        /// The queue to serve; omit for the highest-priority item overall.
        #[arg(long, value_enum)]
        mode: Option<ModeArg>,
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
    /// Search the graph for intents/codefiles by keyword.
    Find {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Detect repo languages and recommend quality packs.
    Detect,
    /// Register/run external diagnostic tools (linters, type-checkers) whose
    /// output becomes derived findings.
    Scan {
        #[command(subcommand)]
        cmd: ScanCmd,
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
    /// Journey proof, coverage, and invariant-point commands.
    Journey {
        #[command(subcommand)]
        cmd: JourneyCmd,
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
        }
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
