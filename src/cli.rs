use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "loom",
    about = "Intent graph CLI — externalized, falsifiable memory for understanding and cleaning up a codebase.",
    long_about = "loom builds a living graph of *intents* (what code is supposed to do), grounded in real \
files, where every relationship carries a verification status + evidence. An LLM drives it one relationship \
at a time: the graph is the durable memory, the context window is just the working set.\n\
\n\
New here? Run `loom guide` for the full driving protocol, `loom schema` for the data model, and \
`loom status` to see where you are. Drive the work with `loom next`. Add `--json` to any command for \
machine-readable output (including a `graph_state` pulse).",
    after_help = "QUICK START:\n  \
        loom init .                                  # create .loom/ in this repo\n  \
        loom guide                                   # learn the loop (read this first)\n  \
        loom intent add --name \"…\" --level system    # seed intents\n  \
        loom next                                    # get the next thing to inspect\n  \
        loom status                                  # where am I? what next?",
    version
)]
pub struct Cli {
    /// Output machine-readable JSON (all read commands honour this).
    #[arg(long, global = true)]
    pub json: bool,

    /// A subcommand. Omit it to print a short orientation (then try `loom guide`).
    #[command(subcommand)]
    pub command: Option<Command>,
}

// ---------------------------------------------------------------------------
// Top-level commands
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum Command {
    /// Initialise a .loom/ directory and its embedded graph database. Stamps
    /// the graph's identity (graph_id + name) — re-running is safe and
    /// backfills identity on older graphs.
    Init {
        /// Directory to initialise (default: current directory).
        #[arg(default_value = ".")]
        path: String,

        /// Human name for this graph (default: the directory name). Other
        /// looms reference this graph by name/id in a federation.
        #[arg(long)]
        name: Option<String>,

        /// This graph OBSERVES a repo its drivers don't own (vendor SDK,
        /// upstream dep, another team's service): mapping, measuring, and
        /// proving all work; build/fix lanes are disabled — findings, not
        /// fixes. Verdicts export as observer testimony.
        #[arg(long)]
        observed: bool,
    },

    /// Show graph health: intent count, edge coverage, open issues.
    Status,

    /// Manage intent nodes.
    Intent {
        #[command(subcommand)]
        subcommand: IntentCmd,
    },

    /// Manage edges between nodes (RELATES_TO, IMPLEMENTS, GOVERNS, HIERARCHY, VALIDATES).
    Edge {
        #[command(subcommand)]
        subcommand: EdgeCmd,
    },

    /// Return the single highest-priority work item with full LLM context.
    Next {
        /// Work mode — one queue per agent role: discovery (analyzer: inspect
        /// relationships) | fix (fixer: resolve failures/stale) | build
        /// (builder: realize planned/needs_change intents) | validate
        /// (validator: run/repair proofs) | quality (quality: earn GOVERNS green).
        #[arg(long, default_value = "discovery")]
        mode: String,

        /// The CLOSEOUT view: every role queue at once — counts + top item per
        /// queue, vertical-completeness gaps, and doctor health, as one
        /// prioritized list. The single operational answer to "what's left?".
        #[arg(long, conflicts_with = "mode")]
        all: bool,
    },

    /// Return all unresolved edges touching a given intent — batch a
    /// neighborhood while you have its context loaded (locality is free).
    Cluster {
        /// Intent ID to cluster around.
        intent_id: String,
    },

    /// Manage quality rules.
    Rule {
        #[command(subcommand)]
        subcommand: RuleCmd,
    },

    /// Manage code files registered in the graph.
    Codefile {
        #[command(subcommand)]
        subcommand: CodefileCmd,
    },

    /// Manage validation nodes (proof objects for intents).
    Validation {
        #[command(subcommand)]
        subcommand: ValidationCmd,
    },

    /// Append free-text memory — justification, commentary, idea, question, etc.
    Note {
        #[command(subcommand)]
        subcommand: NoteCmd,
    },

    /// The flag engine — run after ANY code change. Detects CONTENT changes on
    /// registered files (content-hash; checkout-only mtime churn never false-
    /// flags) and propagates one hop: stale RELATES_TO edges and passing
    /// GOVERNS verdicts → needs_reverification (each flip noted with the file
    /// that caused it), linked validations → not_run; files missing on disk
    /// are reported (drop with `codefile remove`).
    Sync {
        /// Project root (default: current directory, where .loom/ lives).
        #[arg(default_value = ".")]
        path: String,
    },

    /// Run all validation commands linked to an intent and record results.
    Validate {
        /// The intent whose validations should be run.
        intent_id: String,
    },

    /// Print a full coverage and quality report.
    Report,

    /// Verify graph integrity against the declared schema: version, required
    /// properties, valid field values, and dangling references.
    Doctor,

    /// Print the driving protocol for an LLM new to loom: the mental model, the
    /// loop, the done-condition, and a mode-specific population checklist.
    Guide {
        /// greenfield (design first) | brownfield (map existing code) |
        /// refactor (change existing). Auto-detected from the repo if omitted.
        #[arg(long)]
        mode: Option<String>,
    },

    /// Print loom's data model — node/edge types, properties, the inspection
    /// state machine, and the valid value vocabularies.
    Schema,

    /// Structural hot spots: most-central intents and most-tangled files
    /// (importance by graph centrality, not runtime profiling).
    Hotspots {
        /// How many to show.
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },

    /// Derived problem signals computed from the graph — split-brain twins,
    /// overlapping ownership, scattered intents, tangled files, and quality
    /// rules never held against intents that have code. Each finding carries
    /// the exact remedy command.
    Smells {
        /// How many findings to show.
        #[arg(long, default_value_t = 15)]
        limit: usize,
    },

    /// Reconcile files on disk against the graph: grounded / excluded /
    /// unaccounted — so nothing is silently missed (respects .gitignore).
    Coverage,

    /// Detect the repo's stack and whether there's existing source to map
    /// (greenfield vs brownfield). Runs even before `loom init`.
    Detect,

    /// Manage coverage exclusion patterns (the escape hatch), stored in the graph.
    Ignore {
        #[command(subcommand)]
        subcommand: IgnoreCmd,
    },

    /// Delegate a subtree to ANOTHER loom graph (monorepo/federation):
    /// `loom coverage` treats matching files as covered-by-child — a verified
    /// boundary (the child's committed export must exist), not a blanket ignore.
    Delegate {
        #[command(subcommand)]
        subcommand: DelegateCmd,
    },

    /// Export the graph as deterministic JSON — commit it so the graph travels
    /// with the repo (and graph changes become diffable in PRs).
    #[command(after_help = "EXAMPLES:\n  \
        loom export                      # writes loom.graph.json\n  \
        loom export graph-backup.json    # positional path (mirrors `loom import <file>`)\n  \
        loom export -                    # stdout\n  \
        loom export --check              # verify the committed export is fresh (CI/pre-commit)")]
    Export {
        /// Output file ("-" for stdout). Positional, mirroring `loom import
        /// <file>`. Defaults to loom.graph.json.
        path: Option<String>,

        /// Output file (legacy flag form; same as the positional path).
        #[arg(long, conflicts_with = "path")]
        out: Option<String>,

        /// Don't write — verify the existing export file matches the live
        /// graph byte-for-byte. Exits non-zero on drift (or a missing file),
        /// so a pre-commit hook / CI can stop a stale export from shipping.
        #[arg(long)]
        check: bool,
    },

    /// Rebuild a graph from a `loom export` file (into a fresh `loom init`).
    Import {
        /// The export file to restore from.
        file: String,
    },
}

// ---------------------------------------------------------------------------
// Ignore subcommands
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum IgnoreCmd {
    /// Exclude files matching a glob from coverage, with a recorded reason.
    #[command(after_help = "EXAMPLE:\n  \
        loom ignore add 'fixtures/**' --reason \"test fixtures, not product code\"")]
    Add {
        /// Glob pattern (e.g. 'fixtures/**', '*.generated.rs').
        pattern: String,

        /// Why these files are out of scope (required — keeps exclusions honest).
        #[arg(long)]
        reason: String,

        /// Who decided — role-aware (e.g. llm:analyzer, human). Defaults to
        /// $LOOM_AGENT, else "llm".
        #[arg(long)]
        author: Option<String>,
    },

    /// List all coverage exclusion patterns and their reasons.
    List,
}

// ---------------------------------------------------------------------------
// Delegate subcommands (federation: a subtree owned by another graph)
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum DelegateCmd {
    /// Delegate files matching a glob to a child graph's committed export.
    #[command(after_help = "EXAMPLE (monorepo root):\n  \
        loom delegate add 'services/grid/**' --to services/grid/loom.graph.json")]
    Add {
        /// Glob pattern for the delegated subtree (quote it).
        pattern: String,

        /// Path to the child graph's committed export (loom.graph.json) —
        /// the verifiable boundary artifact.
        #[arg(long = "to")]
        target: String,

        /// Who decided — role-aware (e.g. llm:builder, human). Defaults to
        /// $LOOM_AGENT, else "llm".
        #[arg(long)]
        author: Option<String>,
    },

    /// List all delegations (and whether each child export exists).
    List,
}

// ---------------------------------------------------------------------------
// Intent subcommands
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum IntentCmd {
    /// Add an Intent node (status = proposed).
    #[command(after_help = "EXAMPLE:\n  \
        loom intent add --name \"loom init is idempotent\" --level feature \\\n    \
          --description \"re-running init with an existing graph is a no-op\" --aspect fallback")]
    Add {
        #[arg(long)]
        name: String,

        #[arg(long)]
        description: String,

        /// Abstraction level. system = 1–3 per repo (the product's purpose) |
        /// component = 5–15 (cohesive subsystems) | feature = many, ATOMIC —
        /// independently verifiable | cross_cutting = spans everything.
        /// Granularity test: can you write ONE falsifiable criterion for it?
        /// If the description needs an "and", split it into several intents.
        #[arg(long)]
        level: String,

        #[arg(long, default_value = "unknown")]
        domain: String,

        /// Behavioural facet for completeness: happy | sad | fallback | edge_case
        /// | … (open vocabulary; omit for an unspecified/whole-feature intent).
        #[arg(long, default_value = "")]
        aspect: String,

        /// Lifecycle: implemented (default, brownfield) | planned (greenfield,
        /// not built yet) | needs_change (refactor / known issue).
        #[arg(long, default_value = "implemented")]
        lifecycle: String,

        /// Source file paths (may be repeated).
        #[arg(long = "source", num_args = 0..)]
        sources: Vec<String>,
    },

    /// Mark an intent as confirmed.
    Confirm {
        id: String,
    },

    /// Set an intent's lifecycle (planned | implemented | needs_change). Use
    /// needs_change to flag a known issue/refactor without faking a verdict.
    Mark {
        id: String,

        /// New lifecycle: planned | implemented | needs_change
        #[arg(long)]
        lifecycle: String,

        /// Optional rationale, recorded as a note on the intent.
        #[arg(long)]
        reason: Option<String>,
    },

    /// Delete an intent and everything attached to it (edges + notes). For
    /// removing mistakes. Irreversible.
    Delete {
        id: String,
    },

    /// List intents, optionally filtered by status or level.
    List {
        #[arg(long)]
        status: Option<String>,

        #[arg(long)]
        level: Option<String>,
    },

    /// Show full detail of one intent including all its edges.
    Show {
        id: String,
    },

    /// Manage an intent's source_refs — the canonical-source list (code AND
    /// docs: contracts, ADRs, design notes). Set at `intent add --source`;
    /// these subcommands edit it afterwards.
    Source {
        #[command(subcommand)]
        subcommand: SourceCmd,
    },
}

#[derive(Subcommand)]
pub enum SourceCmd {
    /// Append a path to an intent's source_refs (idempotent).
    #[command(after_help = "EXAMPLE:\n  \
        loom intent source add gate-authority docs/AUTHORING-CONTRACT.md")]
    Add {
        /// Intent id, exact name, or unique name fragment.
        id: String,

        /// File path (code or doc) — anchors like `docs/x.md#section` are fine.
        path: String,
    },

    /// Remove a path from an intent's source_refs.
    Remove {
        /// Intent id, exact name, or unique name fragment.
        id: String,

        /// The exact path to remove.
        path: String,
    },
}

// ---------------------------------------------------------------------------
// Edge subcommands
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum EdgeCmd {
    /// Explore (or create) a RELATES_TO edge between two intents.
    #[command(after_help = "EXAMPLES:\n  \
        loom edge explore <A> <B>                                   # create + print context\n  \
        loom edge explore <A> <B> ground --criterion \"…\" --confidence 0.9\n  \
        loom edge explore <A> <B> issue  --criterion \"…\" --evidence \"…\"\n  \
        loom edge explore <A> <B> independent --notes \"no real relationship\"")]
    Explore {
        intent_a_id: String,
        intent_b_id: String,

        #[command(subcommand)]
        subcommand: Option<ExploreSubCmd>,
    },

    /// Create IMPLEMENTS edge(s): Intent → CodeFile. The codefile may be an
    /// id, a registered path, or a glob over REGISTERED paths (quote it:
    /// 'src/db/**') for bulk grounding.
    Implement {
        intent_id:   String,
        codefile_id: String,

        /// Finer-than-file anchor inside the file — a symbol or region, e.g.
        /// "fn run" or "impl LoomDb". Ignored for glob (bulk) grounding.
        #[arg(long, default_value = "")]
        locator: String,

        /// Optional notes about what part of the intent lives in this file.
        #[arg(long, default_value = "")]
        notes: String,
    },

    /// Remove IMPLEMENTS edge(s) — the ungrounding half of `implement`, for
    /// moving groundings down to children during decomposition. Accepts an
    /// id, a registered path, or a glob over registered paths.
    Unimplement {
        intent_id:   String,
        codefile_id: String,
    },

    /// Create a GOVERNS edge: QualityRule → Intent.
    Govern {
        rule_id:   String,
        intent_id: String,

        /// Optional criterion describing what "passing" looks like.
        #[arg(long)]
        criterion: Option<String>,
    },

    /// Create a HIERARCHY edge: parent Intent → child Intent.
    Hierarchy {
        parent_id: String,
        child_id:  String,

        #[arg(long)]
        notes: Option<String>,
    },

    /// Create a VALIDATES edge: Validation → Intent.
    Validates {
        validation_id: String,
        intent_id:     String,

        #[arg(long)]
        notes: Option<String>,
    },

    /// List RELATES_TO edges, optionally filtered by inspection_status.
    List {
        #[arg(long)]
        status: Option<String>,
    },

    /// Show full detail of one RELATES_TO edge including both intent nodes.
    Show {
        edge_id: String,
    },

    /// Mark a failing RELATES_TO edge as passing and propagate reverification.
    Fix {
        edge_id: String,

        /// Human-readable description of what was changed.
        #[arg(long)]
        description: String,
    },
}

#[derive(Subcommand)]
pub enum ExploreSubCmd {
    /// Record that this edge is passing (coexistence criterion defined).
    Ground {
        #[arg(long)]
        criterion: String,

        #[arg(long, default_value_t = 0.9)]
        confidence: f64,

        /// Who performed the inspection: "human" or "llm".
        #[arg(long)]
        inspected_by: Option<String>,
    },

    /// Record that a problem was found between these two intents.
    Issue {
        #[arg(long)]
        criterion: String,

        #[arg(long)]
        evidence: String,

        /// Confidence the problem is real (0.0–1.0). Same slot as `ground`.
        #[arg(long, default_value_t = 0.9)]
        confidence: f64,

        #[arg(long)]
        inspected_by: Option<String>,
    },

    /// Record that these two intents are confirmed independent (no relationship).
    Independent {
        #[arg(long, default_value = "")]
        notes: String,

        #[arg(long)]
        inspected_by: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Rule subcommands
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum RuleCmd {
    /// Add a quality rule.
    Add {
        #[arg(long)]
        name: String,

        #[arg(long)]
        description: String,

        /// Severity: warning | error
        #[arg(long)]
        severity: String,
    },

    /// List all quality rules.
    List,

    /// Show all GOVERNS edges for an intent (violations and passing checks).
    Check {
        intent_id: String,
    },

    /// Seed a built-in measuring-stick pack — the repo-kind vantage points for
    /// 360° normative coverage. `loom detect` recommends which packs fit this
    /// repo; after seeding, `loom next --mode quality` serves every coded
    /// intent the rules were never held against. Already-present rule names
    /// are skipped (idempotent).
    Seed {
        /// Pack name. Available: iso5055 (baseline, any code), mobile
        /// (lifecycle/offline/permissions), web-ui (view states/a11y/XSS),
        /// service (contracts/idempotency/timeouts/sagas), data
        /// (migrations/ingest/PII/lineage), concurrency (sync discipline/
        /// lock hygiene/atomicity/proven perf budgets).
        pack: String,
    },

    /// Apply a rule to an intent — creates a GOVERNS edge (uninspected).
    Apply {
        rule_id:   String,
        intent_id: String,

        #[arg(long)]
        criterion: Option<String>,
    },

    /// Record the quality verdict on a GOVERNS edge — this is how GOVERNS green
    /// is earned (`apply` only asserts the rule applies; `verdict` says whether
    /// the intent complies).
    #[command(after_help = "EXAMPLE:\n  \
        loom rule verdict <rule-id> <intent-id> --status passing \\\n    \
          --criterion \"no query matches a relationship by its own property\" \\\n    \
          --evidence \"grep over src/db/queries: all updates are endpoint-matched\"")]
    Verdict {
        rule_id:   String,
        intent_id: String,

        /// The verdict: passing (complies) | failing (violates) | independent
        /// (measured — the rule does not apply to this intent; silences the
        /// `unmeasured_intents` smell without faking compliance).
        #[arg(long)]
        status: String,

        /// What compliance looks like for this rule on this intent (falsifiable).
        #[arg(long)]
        criterion: String,

        /// What was actually found during inspection.
        #[arg(long)]
        evidence: String,

        /// Confidence the verdict is correct (0.0–1.0).
        #[arg(long, default_value_t = 0.9)]
        confidence: f64,

        /// Who inspected — role-aware (e.g. llm:quality). Defaults to
        /// $LOOM_AGENT, else "llm".
        #[arg(long)]
        inspected_by: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Note subcommands
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum NoteCmd {
    /// Add a note. Attach it to an intent or an edge, or leave it free-floating.
    Add {
        /// The note text.
        #[arg(long)]
        text: String,

        /// Kind: justification | commentary | idea | question | decision | todo
        #[arg(long, default_value = "commentary")]
        kind: String,

        /// Attach to this intent id (mutually exclusive with --edge).
        #[arg(long)]
        intent: Option<String>,

        /// Attach to this edge id (mutually exclusive with --intent).
        #[arg(long)]
        edge: Option<String>,

        /// Who wrote it — role-aware (e.g. llm:analyzer, human:reviewer).
        /// Defaults to $LOOM_AGENT, else "llm".
        #[arg(long)]
        author: Option<String>,
    },

    /// List notes, optionally filtered by target or kind.
    List {
        /// Only notes attached to this intent id.
        #[arg(long)]
        intent: Option<String>,

        /// Only notes attached to this edge id.
        #[arg(long)]
        edge: Option<String>,

        /// Only notes of this kind.
        #[arg(long)]
        kind: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Codefile subcommands
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum CodefileCmd {
    /// Register a file, or many at once via a glob (e.g. 'src/**/*.rs').
    /// Already-registered paths are skipped, so re-running is safe.
    #[command(after_help = "EXAMPLES:\n  \
        loom codefile add src/main.rs\n  \
        loom codefile add 'src/**/*.rs'        # quote globs so the shell doesn't expand them")]
    Add {
        /// A file path, or a glob pattern (quote it to avoid shell expansion).
        path: String,

        /// Language override (auto-detected from extension if omitted).
        #[arg(long)]
        language: Option<String>,
    },

    /// List all registered code files.
    List,

    /// The ownership view of ONE file: which intents claim it (with locators),
    /// which quality rules reach it through them, and whether it is tangled
    /// (serving too many intents). The per-file answer hotspots only hint at.
    Show {
        /// The CodeFile id or its exact registered path.
        path_or_id: String,
    },

    /// Remove a code file from the graph (with its IMPLEMENTS edges). For
    /// phantoms after a delete/rename on disk — `loom sync` reports those.
    Remove {
        /// The CodeFile id or its exact registered path.
        path_or_id: String,
    },
}

// ---------------------------------------------------------------------------
// Validation subcommands
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum ValidationCmd {
    /// Add a Validation node. Pass --intent to also link it (VALIDATES) in one step.
    Add {
        #[arg(long)]
        name: String,

        #[arg(long)]
        description: Option<String>,

        /// Type: test | assertion | benchmark | manual_check
        #[arg(long = "type")]
        validation_type: String,

        /// Shell command to run (e.g. "cargo test --test integration").
        #[arg(long)]
        command: Option<String>,

        /// Optionally link the new validation to this intent (creates a VALIDATES
        /// edge in the same step). Omit to link later with `loom edge validates`.
        #[arg(long)]
        intent: Option<String>,
    },

    /// Record a validation's result by hand (for manual_check / async proofs that
    /// have no runnable --command). Updates the node's last_result and the
    /// per-intent VALIDATES verdict. Validator-lane work.
    #[command(after_help = "EXAMPLES:\n  \
        loom validation mark smoke-bundle --result passed --evidence \"ran the bundle against staging; 200s on all 4 routes\"\n  \
        loom validation mark smoke-bundle --result blocked --reason \"needs a live TARGET_URL — staging is down until the infra migration lands\"")]
    Mark {
        /// Validation id, name, or unique name fragment.
        id: String,

        /// The verdict: passed | failed | blocked. `blocked` = cannot run yet
        /// for a recorded external reason (distinct from not_run = forgotten);
        /// blocked proofs leave the validator queue until you mark them again.
        #[arg(long)]
        result: String,

        /// What you checked to reach a passed/failed verdict (substantive —
        /// recorded as the VALIDATES edge evidence). Required for passed/failed.
        #[arg(long)]
        evidence: Option<String>,

        /// Why this proof cannot run yet (substantive — recorded on the
        /// VALIDATES edge). Required for --result blocked.
        #[arg(long)]
        reason: Option<String>,
    },

    /// Fix a validation's definition — its shell command and/or description
    /// (e.g. a wrong cargo package in --command). Changing the command resets
    /// the proof: last_result → not_run and the VALIDATES edges → uninspected,
    /// because the old result was about a different command.
    #[command(after_help = "EXAMPLE:\n  \
        loom validation update s3-ledger-write-path --command \"cargo test -p grid-infra --test ledger\"")]
    Update {
        /// Validation id, name, or unique name fragment.
        id: String,

        /// The corrected shell command.
        #[arg(long)]
        command: Option<String>,

        /// The corrected description.
        #[arg(long)]
        description: Option<String>,
    },

    /// Delete a validation node and its VALIDATES edges (the validation
    /// analogue of `intent delete` — for removing mistakes). Intents whose
    /// only proof dies become provably unproven again. Irreversible.
    Delete {
        /// Validation id, name, or unique name fragment.
        id: String,
    },

    /// List all validation nodes.
    List,

    /// Show full detail of one validation node.
    Show {
        id: String,
    },
}
