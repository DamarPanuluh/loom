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
    /// Human-ratified, prescriptive repository pattern library.
    Pattern {
        #[command(subcommand)]
        cmd: PatternCmd,
    },
    /// CodeFile commands.
    Codefile {
        #[command(subcommand)]
        cmd: CodefileCmd,
    },
    /// Inspect whether an implemented Intent or cohesive bundle is ready for a semantic Git checkpoint.
    Checkpoint {
        #[command(subcommand)]
        cmd: CheckpointCmd,
    },
    /// Rehearse the fresh Journey-root release gates without releasing anything.
    Release {
        #[command(subcommand)]
        cmd: ReleaseCmd,
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
        /// Discard the derived plane and rebuild it from scratch.
        ///
        /// Sync only re-derives files whose CONTENT changed, so upgrading loom
        /// leaves facts computed by the old binary in place — a call graph, a
        /// symbol map or a finding set that no longer matches what this version
        /// would produce. Run this after an upgrade. Asserted truth is
        /// untouched; only what loom computes for itself is rebuilt.
        #[arg(long)]
        rebuild: bool,
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
        /// With unscoped `--all --json`, emit full work packets (prompt
        /// contract + context) and mint packet ids. Default closeout JSON is
        /// compact: depth, target, and reason per lane — no packet minting.
        /// Does not apply to `--mode <m> --all` (lightweight roster).
        #[arg(long)]
        full: bool,
    },
    /// Edge commands.
    Edge {
        #[command(subcommand)]
        cmd: EdgeCmd,
    },
    /// Durable adversarial review attempts against exact edge-verdict revisions.
    Challenge {
        #[command(subcommand)]
        cmd: ChallengeCmd,
    },
    /// Capture a raw product utterance and route it toward an authored Journey root.
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
    /// Pull one read-only context packet for a Journey, Intent, registered file, or query.
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
    /// Read or set the evidence policy (confidence floor, adversarial frontier,
    /// and human-gate placement); persists to portable config.
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
    /// Every enforced resource limit, named with its value, scope, and remedy.
    /// Violations name the same limits at failure time.
    Limits,
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
    /// Advisory role leases: several LLM drivers coordinate on one graph by
    /// each claiming a free role (heartbeat lease; a lease grants no write
    /// authority — the lane gate does).
    Role {
        #[command(subcommand)]
        cmd: RoleCmd,
    },
    /// Capture a structured proposal and decompose it into adopted work.
    Proposal {
        #[command(subcommand)]
        cmd: ProposalCmd,
    },
    /// The judgment inbox: an LLM stages a proposed ratify/reject/redefine
    /// with evidence; the human reviews a digest and confirms each through
    /// the same typed challenge the direct command demands.
    Judgment {
        #[command(subcommand)]
        cmd: JudgmentCmd,
    },
    /// Author Journey roots, project technical Intents and CLI surfaces, then prove them.
    Journey {
        #[command(subcommand)]
        cmd: JourneyCmd,
    },
    /// Run an interactive, journaled human drive session, or freeze a session.
    Drive {
        #[command(subcommand)]
        cmd: Option<DriveCmd>,
    },
    /// Install or remove local git hooks for structural sync and opt-in local CI.
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
    /// Record a decision as a REVERSAL: what was chosen, what was rejected,
    /// and why. Surfaced to whoever next touches the code it concerns, so the
    /// reasoning arrives before the rediscovery instead of after it.
    Decide {
        /// What was chosen.
        chose: String,
        /// What was rejected. A decision with no alternative is a description.
        #[arg(long = "instead-of")]
        instead_of: String,
        /// Why — the reason that would change if the tradeoff changed.
        #[arg(long)]
        because: String,
        /// Where this shows: a `file:line`, an intent, or a journal ref.
        #[arg(long, default_value = "")]
        evidence: String,
        /// The behavior or file this decision concerns.
        #[arg(long)]
        about: Option<String>,
    },
    /// Run a command loom watches, and keep what it saw.
    ///
    /// This is the cheap way in: prefix the test command you were going to run
    /// anyway and the run becomes evidence — a RunRecord over the files it
    /// covered, plus a journal entry. With `--for` it binds to that behavior's
    /// proof directly; without one it is recorded and offered.
    Observe {
        /// The behavior (or validation) this run is evidence about.
        #[arg(long = "for")]
        target: Option<String>,
        /// Seconds to wait before giving up. A timeout is recorded as blocked,
        /// never as a failure — loom refuses to guess which it was.
        #[arg(long, default_value_t = 900)]
        timeout: u64,
        /// The command, after `--`.
        #[arg(last = true, required = true, num_args = 1..)]
        command: Vec<String>,
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
    /// time, and settled claims standing on nothing re-checkable. Subcommands
    /// seal legacy bursts; bare `loom audit` still runs the fabrication checks.
    Audit {
        #[command(subcommand)]
        cmd: Option<AuditCmd>,
        /// Instead of the fabrication checks, report how often a served packet
        /// was followed by work that established something re-checkable about
        /// its target. Statistical — reported, never gating. Ignored when a
        /// subcommand is given.
        #[arg(long)]
        efficacy: bool,
    },
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
    /// Cold-start assist: recover candidate behavior from code as input to
    /// authored Journeys. Never treats inferred Intents as product roots.
    Bootstrap {
        #[command(subcommand)]
        cmd: BootstrapCmd,
    },
}

/// Queue names for `loom next --mode`; `--help` is the enumeration.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ModeArg {
    Fix,
    /// Project authored Journey steps into human-approved technical Intents.
    Derive,
    Validate,
    Build,
    /// Build a reusable CLI projection for a fully derived Journey.
    Surface,
    Coverage,
    Quality,
    /// Alias: discovery.
    #[value(alias = "discovery")]
    Analyze,
    Prove,
    Triage,
    Review,
    Elaborate,
    /// LLM prep: clear needless ratify friction without deciding wantedness.
    Rectify,
    /// Human-decision queue: an LLM may present/recommend/record, but the human
    /// selects the outcome (never served by plain `loom next`).
    Ratify,
    /// Self-fabrication and risk signals worth acting on.
    Audit,
    /// Post-floor risk work: strengthening the behaviors most worth deepening.
    Deepen,
}

impl ModeArg {
    pub fn as_str(self) -> &'static str {
        match self {
            ModeArg::Fix => "fix",
            ModeArg::Derive => "derive",
            ModeArg::Validate => "validate",
            ModeArg::Build => "build",
            ModeArg::Surface => "surface",
            ModeArg::Coverage => "coverage",
            ModeArg::Quality => "quality",
            ModeArg::Analyze => "analyze",
            ModeArg::Prove => "prove",
            ModeArg::Triage => "triage",
            ModeArg::Review => "review",
            ModeArg::Elaborate => "elaborate",
            ModeArg::Rectify => "rectify",
            ModeArg::Ratify => "ratify",
            ModeArg::Audit => "audit",
            ModeArg::Deepen => "deepen",
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
    Rectify,
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
            RoleArg::Rectify => "rectify",
            RoleArg::Monitor => "monitor",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lane::Lane;
    use clap::{CommandFactory, ValueEnum};

    #[test]
    fn policy_delegation_stays_absent_but_mediated_human_decisions_are_typed() {
        assert!(Cli::try_parse_from([
            "loom",
            "intent",
            "ratify",
            "some-intent",
            "--evidence",
            "approved",
            "--by-policy",
            "legacy"
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "loom",
            "intent",
            "ratify",
            "some-intent",
            "--evidence",
            "approved after review",
            "--human-decision",
            "Keep behavior"
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "loom",
            "policy",
            "ratify-add",
            "legacy",
            "--description",
            "delegated",
            "--source",
            "journal:old"
        ])
        .is_err());

        let mut help = Vec::new();
        Cli::command().write_long_help(&mut help).unwrap();
        let help = String::from_utf8(help).unwrap();
        assert!(!help.contains("by-policy"));
        assert!(!help.contains("ratify-add"));
    }

    /// `loom next --mode <m>` (this clap enum) and the MCP `loom_next` lane enum
    /// (built from `Lane::serves_items`) must offer the SAME lanes — a mode the
    /// partner can request in-band but a human cannot type is a divergence.
    #[test]
    fn mode_arg_matches_the_lanes_that_serve_items() {
        let modes: std::collections::BTreeSet<&str> = ModeArg::value_variants()
            .iter()
            .map(|m| m.as_str())
            .collect();
        let lanes: std::collections::BTreeSet<&str> = Lane::LADDER
            .iter()
            .filter(|l| l.serves_items())
            .map(|l| l.as_str())
            .collect();
        assert_eq!(
            modes, lanes,
            "the CLI --mode enum and the MCP lane enum must serve the same lanes"
        );
    }
}
