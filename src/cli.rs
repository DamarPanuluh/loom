use clap::{Parser, Subcommand};

/// Crate version + git build stamp (from build.rs) — shown by `loom --version`.
/// The bare crate version tracks intentional release bumps; the build id is
/// what tells two binaries apart in local dogfood builds.
pub const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (build ",
    env!("LOOM_BUILD"),
    ")"
);

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
    version = LONG_VERSION
)]
pub struct Cli {
    /// Output machine-readable JSON (all read commands honour this).
    #[arg(long, global = true)]
    pub json: bool,

    /// Target this repo's graph instead of the current directory's — the pin
    /// for scripts/orchestrators. Precedence: --graph > $LOOM_GRAPH > cwd.
    /// Pin a whole session with `export LOOM_GRAPH=<path>`: every loom call
    /// then hits that graph no matter what `cd` does.
    #[arg(long, global = true)]
    pub graph: Option<String>,

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

        /// Skip installing the git pre-commit hook (the green-bar gate:
        /// `loom export --check` + `loom wiki --check`, plus a teach-adapt slot
        /// for the repo's own fmt/lint/test). Installed by default in a git repo
        /// when no foreign pre-commit hook is present.
        #[arg(long)]
        no_hook: bool,
    },

    /// Show graph health: intent count, edge coverage, open issues.
    Status,

    /// Manage intent nodes.
    Intent {
        #[command(subcommand)]
        subcommand: IntentCmd,
    },

    /// Capture and triage raw human/LLM language before it becomes graph truth.
    Inbox {
        #[command(subcommand)]
        subcommand: InboxCmd,
    },

    /// Manage edges between nodes (RELATES_TO, IMPLEMENTS, GOVERNS, HIERARCHY, VALIDATES).
    Edge {
        #[command(subcommand)]
        subcommand: EdgeCmd,
    },

    /// Return the single highest-priority work item with full LLM context —
    /// or, with --take N, a compact bulk read of the queue for batch draining.
    Next {
        /// Work mode — one queue per agent role: discovery (analyzer: inspect
        /// relationships) | fix (fixer: resolve failures/stale) | build
        /// (builder: realize planned/needs_change intents) | populate
        /// (builder: backfill derived graph structure) | validate
        /// (validator: run/repair proofs) | align (validator: re-affirm intent
        /// meaning against the user) | quality (quality: earn GOVERNS green)
        /// | review (re-inspect low-confidence verdicts) | prove (analyzer:
        /// prove proposed hypotheses — the pre-decision plane, optional).
        /// OMIT --mode to follow the compass phase (`loom status` shows it):
        /// bare `loom next` serves the phase's lane — fix when there are
        /// failures/staleness, build when intents need realizing, validate
        /// when proofs are pending, quality when gates are unchecked, and
        /// discovery once the vertical spine is green.
        #[arg(long)]
        mode: Option<String>,

        /// The CLOSEOUT view: every role queue at once — counts + top item per
        /// queue, vertical-completeness gaps, and doctor health, as one
        /// prioritized list. The single operational answer to "what's left?".
        #[arg(long, conflicts_with = "mode")]
        all: bool,

        /// Bulk-read: up to N COMPACT items from this mode's queue in ONE
        /// call. For discovery/fix this groups by the file that staled them
        /// with a prefilled `loom batch` template; for quality it groups rule
        /// verdicts by intent; for align it serves a user-interview agenda.
        /// Supported for discovery, fix, quality, align, and review (capped at
        /// 50). On the one-command-per-item modes (build, populate, validate,
        /// prove) --take is accepted but caps to 1 (those queues aren't
        /// bulkable); use `loom next --all` for a full queue overview. Omit for
        /// the single top item; pass N≥1 for a bulk read (`--take 0` is rejected,
        /// so a computed zero-size chunk fails loudly instead of silently
        /// switching back to the single-item shape).
        #[arg(long, conflicts_with = "all")]
        take: Option<usize>,

        /// For generated discovery pairs, choose which class to serve:
        /// suspected-coupling (default: code/vocab/domain signal), impact-map
        /// (centrality-only), or all. Only valid with --mode discovery.
        #[arg(long = "class", conflicts_with = "all")]
        discovery_class: Option<String>,

        /// Serve the single item as a PROJECTION: intent ids/names, edge id,
        /// top grounded paths, and a one-line suggested command — no
        /// validations/notes/descriptions/pulse (each names its dig command
        /// instead). For agents that only need the verdict coordinates.
        /// Supported for the RELATES_TO queues (discovery and fix).
        #[arg(long, conflicts_with_all = ["all", "take"])]
        compact: bool,
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

    /// Consumer-plane proofs: run an ordered chain of endpoint invocations the
    /// way a real consumer would (values captured from one response thread
    /// into the next request) and stamp the result into the graph — passing
    /// steps become RUNTIME evidence on the RELATES_TO path between their
    /// intents; a failing step lands as a failing edge naming exactly which
    /// boundary broke. The built-in engine is pure Rust (reqwest/rustls).
    Saga {
        #[command(subcommand)]
        subcommand: SagaCmd,
    },

    /// Inspect externally callable surfaces discovered from journeys, starting
    /// with HTTP endpoints registered by `loom saga add`.
    Interface {
        #[command(subcommand)]
        subcommand: InterfaceCmd,
    },

    /// Populate derived graph structure from existing evidence. This is the
    /// brownfield/schema-upgrade lane: it backfills graph surfaces without
    /// claiming product code is built or changed.
    Populate {
        #[command(subcommand)]
        subcommand: PopulateCmd,
    },

    /// Manage improvement hypotheses — the PRE-DECISION plane. Any lane
    /// proposes (claim + proposal + predicted outcome), an analyzer proves the
    /// claim against the code, a builder adopts (converting it into planned
    /// intents) or rejects. Invisible to coverage/completeness until adopted.
    Hypothesis {
        #[command(subcommand)]
        subcommand: HypothesisCmd,
    },

    /// Manage the bounded tag vocabulary — the registered terms intents may
    /// carry in `tags`. Bounded on purpose: two agents describing the same
    /// responsibility in open prose rarely share words, but picking from a
    /// small registry they collide — and collisions are how duplicated
    /// responsibility gets caught across unrelated files.
    Vocab {
        #[command(subcommand)]
        subcommand: VocabCmd,
    },

    /// Deprecated alias for `loom layer`. Kept for one compatibility window:
    /// old invocations still declare the architecture's layer order, but
    /// product `--domain` labels no longer arm layering.
    Domain {
        #[command(subcommand)]
        subcommand: DomainCmd,
    },

    /// Declare the architecture's layer order — which layers sit above which.
    /// Imports flowing UP the order (lower-layer code depending on a higher
    /// layer) surface as `layering_violation` in `loom smells`; a recorded
    /// relationship does not excuse direction. Intents without `--layer`, and
    /// layers not in the order, are exempt — declare only what you mean to
    /// enforce.
    Layer {
        #[command(subcommand)]
        subcommand: LayerCmd,
    },

    /// Manage user personas — named audience segments (the "as a [X]" of user
    /// stories). Personas connect to intents via inspectable SERVES edges
    /// ("does this intent actually serve this persona?") and to sagas via
    /// structural JOURNEYS edges ("this saga exercises this persona's path").
    Persona {
        #[command(subcommand)]
        subcommand: PersonaCmd,
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

    /// Run validation commands and record results: `loom validate <intent>` runs
    /// the proofs linked to one intent; `loom validate --all` runs every proof
    /// whose last_result is not_run — the one-verb drain after a sync flood
    /// invalidates many proofs at once (blocked proofs stay out either way).
    Validate {
        /// The intent whose validations should be run (omit with --all).
        intent_id: Option<String>,

        /// Run every pending (not_run) validation in the graph instead of one
        /// intent's. Skips blocked proofs (they carry a recorded reason).
        #[arg(long)]
        all: bool,

        /// Seconds to wait for each validation command before marking it failed.
        #[arg(long, default_value_t = 900)]
        timeout_secs: u64,
    },

    /// Print a full coverage and quality report.
    Report,

    /// Apply many verdicts in ONE call — JSON Lines from a file or stdin ("-").
    /// The bulk path for post-sync re-verification (dozens of stale claims =
    /// dozens of single calls otherwise). One JSON object per line:
    /// {"op":"ground","a":"<intent>","b":"<intent>","criterion":"…","confidence":0.9} ·
    /// {"op":"issue",…,"evidence":"…"} · {"op":"independent",…,"notes":"…"} ·
    /// {"op":"rule_verdict","rule":"<rule>","intent":"<intent>","status":"passing|failing|independent","criterion":"…","evidence":"…","confidence":0.9}.
    /// Every gate (lanes, substantive evidence, confidence) applies per line;
    /// continues past failures, exits non-zero if any line failed.
    #[command(
        after_help = "EXAMPLE (heredoc — no scratch file, nothing to clean up):\n  \
        loom batch - <<'EOF'\n  \
        {\"op\":\"ground\",\"a\":\"request routing\",\"b\":\"session auth\",\"confidence\":0.9}\n  \
        {\"op\":\"issue\",\"a\":\"request routing\",\"b\":\"rate limiting\",\"evidence\":\"limiter runs after dispatch\",\"confidence\":0.9}\n  \
        EOF\n  \
        (a file path instead of '-' works for very large batches)"
    )]
    Batch {
        /// JSONL file path, or "-" to read stdin.
        #[arg(default_value = "-")]
        file: String,

        /// Validate every line through all gates (resolution, substance,
        /// confidence, locator resolution) and report what WOULD apply, without
        /// writing anything. Lets a large batch be checked before it commits.
        #[arg(long)]
        dry_run: bool,
    },

    /// Verify graph integrity against the declared schema: version, required
    /// properties, valid field values, and dangling references. With
    /// --clean-orphans, instead reap dead backend relics left in `.loom/` by
    /// past storage generations (graph.grafeo, db.sqlite, graph.db + their
    /// WAL/SHM sidecars) — files loom once wrote but no longer reads. Dry-run
    /// by default (lists what would go); add --yes to actually remove.
    Doctor {
        /// Reap dead backend relics from `.loom/` (dry-run unless --yes).
        #[arg(long)]
        clean_orphans: bool,

        /// Confirm a destructive clean: actually remove the relics --clean-orphans
        /// listed. Without it, --clean-orphans only previews.
        #[arg(long)]
        yes: bool,
    },

    /// Verify the live graph's schema version against this binary. The SQLite
    /// schema is created on open and JSON imports are normalized into the active
    /// schema, so there is no in-place upgrade step: a graph stamped by an older
    /// loom is rebuilt by re-exporting from that loom, then `loom init . &&
    /// loom import loom.graph.json` here. This command only reports the version.
    Migrate,

    /// Print the driving protocol for an LLM new to loom: the mental model, the
    /// loop, the done-condition, and a mode-specific population checklist.
    Guide {
        /// greenfield (design first) | brownfield (map existing code) |
        /// refactor (change existing) | port (re-realize a mapped system in a
        /// new language/repo) | seed (interview user) | import (adopt a
        /// pattern/subsystem/contract from another repo). Auto-detected from the repo if omitted.
        #[arg(long)]
        mode: Option<String>,
        /// Adopt a role: print the charge for builder|analyzer|fixer|validator|quality
        /// — its mandate, lane (what it MAY do), queue, and setup. Takes precedence
        /// over --mode; derived from the enforced lane table so it can't drift.
        #[arg(long)]
        role: Option<String>,
    },

    /// Print loom's data model — node/edge types, properties, the inspection
    /// state machine, and the valid value vocabularies.
    Schema,

    /// The lane-skills, OPT-IN. loom serves each lane's discipline just-in-time
    /// (`loom guide --role <lane>`) with NO install — this command is only for
    /// pinning them as harness skills (model-invocable, persistent). It emits the
    /// lane-skills as SKILL.md files (a regenerable projection of the gate's lane
    /// table, like `loom wiki`); the LLM/user adds them to their own harness.
    #[command(after_help = "EXAMPLE:\n  \
        loom skill list                 # the lane-skill menu\n  \
        loom skill show analyzer        # one SKILL.md to stdout\n  \
        loom skill install              # all SKILL.md + where to write them\n  \
        loom skill install --write      # write them into ./.claude/skills/")]
    Skill {
        #[command(subcommand)]
        command: SkillCmd,
    },

    /// Ask the map: keyword search (BM25) over intent names and descriptions.
    /// Each hit carries its place in the hierarchy, code groundings with
    /// locators, and a freshness warning if claims went stale — the semantic
    /// entry point when you don't know an intent's name yet.
    Find {
        /// What you're looking for, in your own words (e.g. "where is retry handled").
        query: String,
        /// How many hits to return.
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },

    /// Intelligence on one node: what it IS, what it's FOR (groundings), what
    /// it's coupled to and BY WHAT KIND, what governs it, and what ripples if you
    /// change it. Accepts an intent (id / name / fragment) OR a file path.
    #[command(after_help = "EXAMPLE:\n  \
        loom explain \"session auth\"        # by intent name/fragment\n  \
        loom explain src/db/sqlite.rs       # by file → the intents grounded on it\n  \
        loom explain <intent-id> --json     # structured answer for an agent")]
    Explain {
        /// An intent (id, exact name, or unique name fragment) or a file path.
        target: String,
        /// Focus on the blast radius: what relationships re-open and which
        /// validations must re-run if you change this (the pre-change preview of
        /// `loom sync`'s ripple). Kind-aware — meaning-only links are excluded.
        #[arg(long)]
        impact: bool,
    },

    /// The entrance: capture a user utterance in Inbox, then return routing
    /// context and the landing menu. Door captures first; the LLM normalizes
    /// and routes the inbox card before mutating graph truth.
    #[command(after_help = "EXAMPLE:\n  \
        loom door \"users should be able to reset their password\"\n  \
        loom door \"checkout keeps breaking when the cart is empty\"\n  \
        (creates an InboxItem → read matches → normalize/route it with \
        loom inbox normalize/mark before going autonomous)")]
    Door {
        /// The user's statement, in their words.
        utterance: String,
        /// The reasoning behind it — why now, constraints, tradeoffs considered.
        /// Captured as a SEPARATE card linked to the utterance, so the "why"
        /// survives the graph boundary instead of dying in chat (conversation
        /// residue is the failure mode). Triage it like any other card.
        #[arg(long, default_value = "")]
        why: String,
        /// How many matches to return per plane.
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },

    /// Turn zero: the user said "use loom" (or "loom session" / "loom mode")
    /// without stating a goal. Loom cannot read minds — this prints the
    /// ask-the-user playbook: ONE question ("what do you want from this
    /// session?"), a state-aware offer menu (each offer backed by a live
    /// queue and its count), and exactly one recommended offer. User-gated
    /// work (align drift, hypothesis rulings, blocked proofs) outranks
    /// everything the agent can drain alone. Works before `loom init` too.
    #[command(after_help = "EXAMPLE:\n  \
        loom session\n  \
        (ask the user the ONE question, lead with the ▸ recommended offer; \
        a free-form answer is captured by `loom door \"<their words>\"`, then normalized with `loom inbox triage`)")]
    Session,

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
    /// the exact remedy command; OPEN findings gate green (phase=audit until
    /// each is resolved or refuted via its remedy).
    Smells {
        /// How many findings to show.
        #[arg(long, default_value_t = 15)]
        limit: usize,
        /// Print only counts, top summaries, and instrumentation blind spots.
        #[arg(long)]
        summary: bool,
        /// Rank stale edges by severity — turn the undifferentiated "N stale"
        /// wall of red into a triaged queue. Splits broken groundings (locator
        /// gone / file missing → re-ground) from drift (target survived →
        /// re-inspect), ranked within drift by current blast radius (symbol
        /// count of the grounding file). Honest about what it can't know: sync
        /// overwrites the prior symbol set, so retrospective drift MAGNITUDE
        /// needs a future schema field stamped at flag time; this ranks by
        /// re-inspection cost, not drift size.
        #[arg(long)]
        stale: bool,
    },

    /// Reconcile files on disk against the graph: grounded / excluded /
    /// unaccounted — so nothing is silently missed (respects .gitignore).
    Coverage {
        /// Print only counts and actionable gaps; omit full symbol/finding archives.
        #[arg(long)]
        summary: bool,
        /// Drill into adjudicated symbols — the green bought by a decision
        /// note, not a grounding locator. Lists each bought symbol with the
        /// ruling that bought it, who ruled, when (staleness), and what would
        /// re-open it, so adjudication is auditable per-symbol, not just a count.
        #[arg(long)]
        adjudicated: bool,
    },

    /// Detect the repo's stack and whether there's existing source to map
    /// (greenfield vs brownfield). Runs even before `loom init`.
    Detect,

    /// Mine CANDIDATE intents from the repo's code structure — the cold-start
    /// bootstrap so a fresh graph starts from a draft, not a blank page. Each
    /// candidate names a code unit + its public surface and emits pre-filled
    /// adopt commands; you REWRITE the description into what it's SUPPOSED to do
    /// (a falsifiable intent) before adopting. SUGGEST-only — never writes the graph.
    #[command(
        after_help = "EXAMPLE:\n  loom seed --suggest\n  loom seed --suggest --limit 0   (show all)"
    )]
    Seed {
        /// Mine candidate intents from the code (currently the only mode).
        #[arg(long)]
        suggest: bool,

        /// Max candidates to show (0 = all).
        #[arg(long, default_value_t = crate::output::LIST_LIMIT)]
        limit: usize,
    },

    /// Guided comprehension walkthrough of the intent graph in decomposition
    /// order — what each part is SUPPOSED to do, where it's realized, and (unique
    /// to loom) whether it is PROVEN. Read-only. Pass an intent to drill into one
    /// subtree.
    #[command(
        after_help = "EXAMPLE:\n  loom tour\n  loom tour \"payment flow\"\n  loom tour --limit 0   (the whole graph)"
    )]
    Tour {
        /// Intent (id / name / unique fragment) to tour the subtree of. Omit for
        /// the whole graph.
        target: Option<String>,

        /// Max stops to show (0 = all).
        #[arg(long, default_value_t = crate::output::LIST_LIMIT)]
        limit: usize,
    },

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

    /// Generate a human-readable Markdown wiki from the graph (overview +
    /// architecture tree + components-by-domain + quality bars). A deterministic
    /// PROJECTION like `loom export` — same graph, identical bytes; regenerate
    /// after changes and `loom wiki --check` guards freshness. For humans to
    /// read; the graph stays the source of truth.
    #[command(after_help = "EXAMPLE:\n  \
        loom wiki                       # write loom.wiki.md\n  \
        loom wiki docs/ARCH.md          # choose the path\n  \
        loom wiki -                     # to stdout\n  \
        loom wiki --check               # CI/pre-commit: fail if stale")]
    Wiki {
        /// Output file ("-" for stdout). Positional, mirroring `loom export`
        /// and `loom import <file>`. Defaults to loom.wiki.md.
        path: Option<String>,

        /// Output file (legacy flag form; same as the positional path).
        #[arg(long, conflicts_with = "path")]
        out: Option<String>,

        /// Don't write — verify the existing wiki matches the live graph
        /// byte-for-byte (exits non-zero on drift / missing).
        #[arg(long)]
        check: bool,
    },

    /// Rebuild a graph from a `loom export` file (into a fresh `loom init`).
    Import {
        /// The export file to restore from.
        file: String,

        /// PORTING mode: adopt the export's semantic plane as a DESIGN —
        /// intents/hierarchy/criteria/rules/notes travel; codefiles,
        /// groundings, verdicts, and proof results do NOT (they were earned
        /// against the OLD code). Every intent arrives lifecycle=planned,
        /// every proof not_run; the build queue then drives re-realization
        /// in the new repo/language. See `loom guide --mode port`.
        #[arg(long)]
        as_planned: bool,
    },

    /// Retired daemon command. SQLite-backed loom opens the embedded graph
    /// store directly; this command remains only to give old scripts a clear
    /// retirement error.
    Serve {
        /// Kept for old invocations; ignored because the daemon is retired.
        #[arg(long, default_value_t = 300)]
        idle_secs: u64,
    },

    // The universal catch-all: ANY unrecognized top-level token lands here
    // (verb-without-noun, synonym guess, typo) and `commands::teach_unknown`
    // answers with the real invocation — agents reach for `loom update` /
    // `loom rename` / `loom retire` under time pressure, and clap's stock
    // similar-name tip pointed them at the WRONG command (`update` → "did
    // you mean 'guide'?"). Errors teach, including spelling errors.
    #[command(external_subcommand)]
    Unknown(Vec<String>),
}
// ---------------------------------------------------------------------------
// Parse-error teaching — every command's EXAMPLE doubles as its error message
// ---------------------------------------------------------------------------

/// Parse argv; on ANY syntax failure, append the failing command's EXAMPLE
/// block (its after_help) plus a guide pointer before exiting. Clap's stock
/// errors name the missing flag but not the why or the shape — and a stalled
/// agent leaves the loop to go doc-hunting (dogfood finding). With this,
/// writing an EXAMPLE on a command IS writing its error message.
pub fn parse_or_teach() -> Cli {
    let err = match Cli::try_parse() {
        Ok(cli) => return cli,
        Err(err) => err,
    };
    use clap::error::ErrorKind as K;
    let teachable = matches!(
        err.kind(),
        K::MissingRequiredArgument
            | K::UnknownArgument
            | K::InvalidValue
            | K::ValueValidation
            | K::WrongNumberOfValues
            | K::TooFewValues
            | K::TooManyValues
            | K::NoEquals
            | K::MissingSubcommand
            | K::InvalidSubcommand
            | K::ArgumentConflict
    );
    let _ = err.print();
    if teachable {
        if let Some(cmd) = deepest_subcommand(std::env::args().skip(1)) {
            if let Some(h) = cmd.get_after_help() {
                eprintln!();
                eprintln!("{h}");
            }
        }
        eprintln!();
        eprintln!("(`loom guide` teaches the loop; any command plus --help shows its full shape)");
    }
    std::process::exit(err.exit_code());
}

/// Walk the clap tree along argv to the deepest matching subcommand — the
/// command whose shape the failing invocation was reaching for. Non-matching
/// tokens (flags, values) are skipped; a flag VALUE that happens to spell a
/// subcommand name could mis-route the walk, which costs at worst an
/// unrelated example under an otherwise-correct error. Returns None when no
/// subcommand matched (the error is top-level; nothing to add).
fn deepest_subcommand(argv: impl Iterator<Item = String>) -> Option<clap::Command> {
    use clap::CommandFactory;
    let mut cur = Cli::command();
    let mut matched = false;
    for tok in argv {
        if tok.starts_with('-') {
            continue;
        }
        let hit = cur
            .get_subcommands()
            .find(|s| s.get_name() == tok || s.get_all_aliases().any(|a| a == tok))
            .cloned();
        if let Some(sc) = hit {
            cur = sc;
            matched = true;
        }
    }
    matched.then_some(cur)
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

    /// Remove a delegation by its exact glob pattern.
    #[command(after_help = "EXAMPLE:\n  \
        loom delegate remove 'services/grid/**'")]
    Remove {
        /// Glob pattern to remove (must exactly match `loom delegate list`).
        pattern: String,
    },

    /// Link a parent SEAM intent to a delegation — "this intent depends on the
    /// child service's contract." When the child's committed export changes,
    /// `loom sync` re-opens the seam intents' claims (cross-service ripple).
    #[command(after_help = "EXAMPLE (monorepo root):\n  \
        loom delegate seam 'services/grid/**' \"grid query gateway\"")]
    Seam {
        /// Delegation id or exact glob pattern (`loom delegate list`).
        delegation: String,

        /// Parent intent id, name, or unique fragment that consumes the child.
        intent: String,
    },

    /// List all delegations (and whether each child export exists).
    List,
}

// ---------------------------------------------------------------------------
// Skill subcommands
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum SkillCmd {
    /// List the lane-skills (name + the JIT-trigger description) — the menu.
    List,
    /// Print ONE lane-skill as a complete SKILL.md to stdout.
    Show {
        /// The lane: builder | analyzer | fixer | validator | quality.
        role: String,
    },
    /// Emit every lane-skill as a SKILL.md plus where to write it. Without
    /// `--write`, prints the materialization plan for the LLM/user to apply;
    /// with `--write`, writes them into the skills dir (default ./.claude/skills).
    Install {
        /// Skills directory to write into (default `./.claude/skills`).
        #[arg(long)]
        dir: Option<String>,
        /// Actually write the files (otherwise just print the plan).
        #[arg(long)]
        write: bool,
    },
}

// Intent subcommands
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum IntentCmd {
    /// Add an Intent node (status = proposed).
    #[command(after_help = "EXAMPLE:\n  \
        loom intent add --name \"loom init is idempotent\" --level feature \\\n    \
          --description \"re-running init with an existing graph is a no-op\" \\\n    \
          --domain developer-experience --layer cli --aspect fallback")]
    Add {
        #[arg(long)]
        name: String,

        #[arg(long)]
        description: String,

        /// The ONE falsifiable criterion this intent is done/correct by — what
        /// "passing" means for it, first-class as of v10. Optional but
        /// recommended; when given it is held to the same substantive-evidence
        /// gate as edge criteria (no placeholders, ≥10 chars).
        #[arg(long, default_value = "")]
        criterion: String,

        /// Abstraction level. system = 1–3 per repo (the product's purpose) |
        /// component = 5–15 (cohesive subsystems) | feature = many, ATOMIC —
        /// independently verifiable | cross_cutting = spans everything.
        /// Granularity test: can you write ONE falsifiable criterion for it?
        /// If the description needs an "and", split it into several intents.
        #[arg(long)]
        level: String,

        /// Product/business facet for discovery and scoring (auth, billing,
        /// onboarding). Not an architecture layer.
        #[arg(long, default_value = "unknown")]
        domain: String,

        /// Architecture layer for layering_violation, separate from product
        /// domain. Empty/omitted = undeclared and exempt from layer checks.
        #[arg(long, default_value = "")]
        layer: String,

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

        /// Registered vocabulary terms (repeatable, max 3). Optional — an
        /// untagged intent is honest; a wrong tag lies. `loom vocab list`
        /// shows the registry; unknown terms error with the full list inline.
        #[arg(long = "tag", num_args = 0..)]
        tags: Vec<String>,

        /// Who the behavior is for: user_visible (a capability the user can
        /// see/feel) | internal (machinery serving other intents). Omit when
        /// untriaged — the align interview triages it. Internal intents are
        /// excluded from the user interview until redefined.
        #[arg(long, default_value = "")]
        visibility: String,

        /// Relationship to the system boundary: inbound (exposes a surface the
        /// outside world calls — a provider contract) | outbound (calls an
        /// external system — a consumer dependency). Omit for internal intents
        /// that don't cross the boundary.
        #[arg(long, default_value = "")]
        boundary: String,
    },

    /// Confirm an intent (status = confirmed) AND stamp a freshness note —
    /// "the user re-affirmed this meaning as of now". Re-confirming is the
    /// align loop's cheap outcome: it resets the drift-suspicion clock that
    /// `loom next --mode align` ranks by.
    Confirm {
        id: String,

        /// Record the audience ruling alongside the confirmation:
        /// `--visibility internal` is the "this is machinery, stop asking the
        /// user about it" interview outcome (leaves the align queue until the
        /// meaning is redefined); `user_visible` pins the opposite.
        #[arg(long)]
        visibility: Option<String>,
    },

    /// UPDATE an intent's meaning in place — design EVOLUTION (same node,
    /// same id, full history), distinct from `retire` (supersession by a
    /// different intent). A --description change is a REDEFINITION and
    /// ripples one hop, like `loom sync` but for meaning: earned verdicts
    /// touching the intent → needs_reverification, linked proofs → not_run —
    /// every green claim was earned against the old wording. A --name-only
    /// change is cosmetic and ripples nothing.
    #[command(after_help = "EXAMPLE:\n  \
        loom intent update \"request routing\" \\\n    \
          --description \"route by host AND path; unknown hosts get 421\" \\\n    \
          --reason \"multi-tenant pivot: host-based routing is now in scope\"")]
    Update {
        /// Intent id, name, or unique name fragment.
        id: String,

        /// New name (cosmetic — no ripple).
        #[arg(long)]
        name: Option<String>,

        /// New architecture layer (metadata — no ripple).
        #[arg(long)]
        layer: Option<String>,

        /// Set/clear the boundary facet (inbound | outbound | "" to clear).
        /// Metadata — no ripple, like --layer. Records a decision note.
        #[arg(long)]
        boundary: Option<String>,

        /// New meaning statement (REDEFINITION — ripples staleness one hop,
        /// and clears the visibility ruling: the new meaning's audience is
        /// unknown again). With --reword: same concept in clearer words —
        /// no ripple, no visibility clear, but the align clock still resets.
        #[arg(long)]
        description: Option<String>,

        /// The description change is a REWORDING, not a redefinition: the
        /// concept the user confirmed stays; only the words get clearer
        /// (the "terminology confusing, keep concept" interview outcome).
        /// Skips the semantic ripple — use ONLY when no claim's meaning
        /// moved; if behavior changed, that is a redefinition.
        #[arg(long, requires = "description")]
        reword: bool,

        /// New falsifiable criterion ("done means …"). The previous criterion is
        /// preserved in the decision note (the version chain), gated by the same
        /// substantive-evidence check as edge criteria.
        #[arg(long)]
        criterion: Option<String>,

        /// Why the meaning moved (recorded as a decision note, with the
        /// previous wording preserved alongside). Required in effect — the
        /// handler teaches the full shape when it's missing (a clap "required
        /// argument" line names the flag but not the why or the variants).
        #[arg(long, default_value = "")]
        reason: String,

        /// Catch-all so positional wording teaches instead of clap-babbling
        /// ("unexpected argument found"): new wording travels through flags.
        #[arg(hide = true, num_args = 0.., value_name = "UNEXPECTED")]
        extra: Vec<String>,
    },

    /// Set an intent's lifecycle (planned | implemented | needs_change |
    /// deferred | to_be_removed). Use needs_change to flag a known issue/refactor
    /// without faking a verdict; deferred PARKS valid-but-not-now work (never
    /// blocks a roll-up); to_be_removed marks code that should GO AWAY (cleanup
    /// as a tracked verb — done by absence, gates green only once it's gone) —
    /// all distinct from retire (superseded design).
    #[command(after_help = "EXAMPLE:\n  \
        loom intent mark \"request routing\" --lifecycle needs_change \\\n    \
          --reason \"routing table rebuilt on every call — known hotspot\"")]
    Mark {
        id: String,

        /// New lifecycle: planned | implemented | needs_change | deferred | to_be_removed
        #[arg(long)]
        lifecycle: String,

        /// Optional rationale, recorded as a note on the intent.
        #[arg(long)]
        reason: Option<String>,
    },

    /// Delete an intent and everything attached to it (edges + notes). For
    /// removing mistakes. Irreversible.
    Delete { id: String },

    /// RETIRE an intent: design that was real and got superseded (delete is
    /// for mistakes). Status → deprecated; the node, edges, and notes remain
    /// as history, but the intent becomes INVISIBLE TO COMPUTATION — queues,
    /// coverage, centrality, the grid, and sync ripple stop counting it.
    /// Reports the triggered fallout: orphaned children to re-parent or
    /// retire, files that lost their only owner (they surface as vertical
    /// gaps), and proofs left dangling.
    #[command(after_help = "EXAMPLE:\n  \
        loom intent retire \"legacy draft store\" \\\n    \
          --reason \"superseded by the unified content store\" --replaced-by \"unified content store\"")]
    Retire {
        /// Intent id, name, or unique name fragment.
        id: String,

        /// Why this design was superseded (recorded as a decision note).
        #[arg(long)]
        reason: String,

        /// The successor intent (id/name/fragment), recorded in the decision
        /// note so the lineage is traceable.
        #[arg(long)]
        replaced_by: Option<String>,
    },

    /// List intents, optionally filtered by status or level.
    List {
        #[arg(long)]
        status: Option<String>,

        #[arg(long)]
        level: Option<String>,

        /// Max rows (0 = all). Newest-irrelevant inventories stay bounded so
        /// recovery commands never become the token spike.
        #[arg(long, default_value_t = crate::output::LIST_LIMIT)]
        limit: usize,
    },

    /// Show full detail of one intent including all its edges.
    Show { id: String },

    /// Manage an intent's source_refs — the canonical-source list (code AND
    /// docs: contracts, ADRs, design notes). Set at `intent add --source`;
    /// these subcommands edit it afterwards.
    Source {
        #[command(subcommand)]
        subcommand: SourceCmd,
    },

    /// Manage an intent's vocabulary tags (max 3, from the registered
    /// vocabulary — `loom vocab list`). Tags are the bounded facet that lets
    /// duplicate-responsibility detection see across unrelated files.
    Tag {
        #[command(subcommand)]
        subcommand: TagCmd,
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

#[derive(Subcommand)]
pub enum TagCmd {
    /// Add a registered term to an intent's tags (idempotent).
    #[command(after_help = "EXAMPLE:\n  \
        loom intent tag add gate-authority enforcement")]
    Add {
        /// Intent id, exact name, or unique name fragment.
        id: String,

        /// A registered vocab term (see `loom vocab list`).
        term: String,
    },

    /// Remove a term from an intent's tags.
    Remove {
        /// Intent id, exact name, or unique name fragment.
        id: String,

        /// The exact term to remove.
        term: String,
    },
}

// ---------------------------------------------------------------------------
// Vocab subcommands (the bounded tag vocabulary)
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum VocabCmd {
    /// Register a new term. Keep the registry SMALL — its value is forcing
    /// collisions, and a list an agent can't hold in context stops forcing
    /// anything. Before adding, check `loom vocab list` for a term that
    /// already covers this.
    #[command(after_help = "EXAMPLE:\n  \
        loom vocab add authz --why \"permission checks, role gates, ACLs — NOT login/session (that's authn)\"")]
    Add {
        /// The term: lowercase, digits, '-' or '_' (an exact-match key, not prose).
        term: String,

        /// Contrastive definition: what it covers AND what it does not —
        /// name the neighbouring term so the next agent can disambiguate.
        #[arg(long)]
        why: String,

        /// Acting agent (defaults to $LOOM_AGENT or "llm").
        #[arg(long)]
        author: Option<String>,
    },

    /// List the registry: every term with its usage count and definition.
    List,

    /// Mine THIS graph's own intents for candidate vocabulary terms — tokens
    /// that recur across ≥2 intents and aren't registered yet, ranked by how
    /// many share each (collision potential). The low-friction way to ARM
    /// duplicate-responsibility detection on an untagged graph: loom can't know
    /// your codebase's vocabulary, so it surfaces what already recurs in it.
    /// Register the ones that name a real shared responsibility, then tag —
    /// loom suggests the KEY; the contrastive `--why` stays your judgment.
    #[command(after_help = "EXAMPLE:\n  \
        loom vocab suggest --limit 20")]
    Suggest {
        /// Max candidates to show (0 = all).
        #[arg(long, default_value_t = crate::output::LIST_LIMIT)]
        limit: usize,
    },

    /// Merge term <from> into term <to>: every intent carrying <from> is
    /// retagged to <to>, then <from> is deleted. One sweep, nothing to
    /// re-inspect — this is how vocab drift converges.
    #[command(after_help = "EXAMPLE:\n  \
        loom vocab merge authentication authn")]
    Merge {
        /// The term to dissolve.
        from: String,

        /// The term that absorbs it.
        to: String,
    },
}

// ---------------------------------------------------------------------------
// Domain subcommands (deprecated alias for the declared layer order)
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum DomainCmd {
    /// Deprecated alias for `loom layer order`.
    #[command(after_help = "EXAMPLE:\n  \
        loom layer order cli commands queries storage")]
    Order {
        /// Layers, highest layer first (≥2; each holds exactly one rank).
        #[arg(num_args = 2..)]
        domains: Vec<String>,

        /// Acting agent (defaults to $LOOM_AGENT or "llm").
        #[arg(long)]
        author: Option<String>,
    },

    /// Deprecated alias for `loom layer list`.
    List,

    /// Deprecated alias for `loom layer clear`.
    Clear,
}

// ---------------------------------------------------------------------------
// Layer subcommands (the declared architecture layer order)
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum LayerCmd {
    /// Declare the layer order, top layer first — REPLACES any previous
    /// order. `loom layer order presentation app storage` means presentation
    /// may depend on app and storage, app on storage, and any import pointing
    /// the other way is a layering_violation.
    #[command(after_help = "EXAMPLE:\n  \
        loom layer order presentation commands queries storage")]
    Order {
        /// Layers, highest layer first (≥2; each holds exactly one rank).
        #[arg(num_args = 2..)]
        layers: Vec<String>,

        /// Acting agent (defaults to $LOOM_AGENT or "llm").
        #[arg(long)]
        author: Option<String>,
    },

    /// Show the declared order with per-layer intent counts, plus the
    /// layers in use that the order does not cover (exempt from the smell).
    List,

    /// Remove the declared order — layering_violation goes silent.
    Clear,
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
        intent_id: String,
        codefile_id: String,

        /// Finer-than-file anchor inside the file — a symbol or region, e.g.
        /// "fn run" or "impl GraphReadRepository". Ignored for glob (bulk) grounding.
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
        intent_id: String,
        codefile_id: String,
    },

    /// Create a GOVERNS edge: QualityRule → Intent.
    Govern {
        rule_id: String,
        intent_id: String,

        /// Optional criterion describing what "passing" looks like.
        #[arg(long)]
        criterion: Option<String>,
    },

    /// Create a HIERARCHY edge: parent Intent → child Intent.
    Hierarchy {
        parent_id: String,
        child_id: String,

        #[arg(long)]
        notes: Option<String>,
    },

    /// Create a VALIDATES edge: Validation → Intent.
    Validates {
        validation_id: String,
        intent_id: String,

        #[arg(long)]
        notes: Option<String>,
    },

    /// List RELATES_TO edges, optionally filtered by inspection_status.
    List {
        #[arg(long)]
        status: Option<String>,

        /// Max rows (0 = all).
        #[arg(long, default_value_t = crate::output::LIST_LIMIT)]
        limit: usize,
    },

    /// Show full detail of one RELATES_TO edge including both intent nodes.
    Show { edge_id: String },

    /// Mark a grounded RELATES_TO edge `stable` (a settled coupling) so `loom
    /// sync` stops re-opening it every time either endpoint's file changes. Use
    /// `--off` to clear the flag. The horizontal grid is the dominant
    /// re-verification cost; stable is the lever to retire that churn for
    /// relationships you've decided are settled.
    #[command(after_help = "EXAMPLE:\n  \
        loom edge stable \"request routing\" \"session auth\"   (settled; sync won't re-open it)\n  \
        loom edge stable \"request routing\" \"session auth\" --off")]
    Stable {
        intent_a_id: String,
        intent_b_id: String,

        /// Clear the stable flag (re-arm sync reverification for this edge).
        #[arg(long)]
        off: bool,
    },

    /// Mark a failing RELATES_TO edge as passing and propagate reverification.
    #[command(after_help = "EXAMPLE:\n  \
        loom edge fix rt:a1b2:c3d4 --description \"moved auth middleware registration ahead of route dispatch\"\n  \
        (fix AFTER the code change and `loom sync`; the edge id comes from `loom next --mode fix`)")]
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
    #[command(after_help = "EXAMPLE:\n  \
        loom edge explore \"request routing\" \"session auth\" ground \\\n    \
          --criterion \"routing never reads session state; auth middleware runs before route dispatch\" \\\n    \
          --evidence \"verified the middleware order\" --evidence-locator src/server/mod.rs:40-80 --confidence 0.9")]
    Ground {
        #[arg(long)]
        criterion: String,

        /// What the inspection actually FOUND (optional — the criterion may
        /// say it all). Stored on the edge; replaces any previous verdict's
        /// evidence.
        #[arg(long, default_value = "")]
        evidence: String,

        /// File/line anchor(s) the evidence points at, e.g.
        /// `src/db/queries/stats.rs:299-340` (repeatable). Folded into the
        /// stored evidence as `@<locator>` so a later review lands on the
        /// exact lines instead of re-deriving them from prose.
        #[arg(long)]
        evidence_locator: Vec<String>,

        #[arg(long, default_value_t = 0.9)]
        confidence: f64,

        /// Relationship kind(s) this grounding asserts (repeatable): calls |
        /// inheritance | shares_state | doc_reference | manual (the judgment
        /// tier — mechanical kinds like imports/shares_file are derived by
        /// `loom populate kinds`). Replaces the judgment kinds on the edge.
        #[arg(long = "kind")]
        kinds: Vec<String>,

        /// Who performed the inspection: "human" or "llm".
        #[arg(long)]
        inspected_by: Option<String>,
    },

    /// Record that a problem was found between these two intents.
    #[command(after_help = "EXAMPLE:\n  \
        loom edge explore \"request routing\" \"session auth\" issue \\\n    \
          --criterion \"auth must run before route dispatch\" \\\n    \
          --evidence \"routes register before the auth layer\" --evidence-locator src/server/mod.rs:52 --confidence 0.9")]
    Issue {
        #[arg(long)]
        criterion: String,

        #[arg(long)]
        evidence: String,

        /// File/line anchor(s) the evidence points at, e.g.
        /// `src/db/queries/stats.rs:299-340` (repeatable). Folded into the
        /// stored evidence as `@<locator>`.
        #[arg(long)]
        evidence_locator: Vec<String>,

        /// Confidence the problem is real (0.0–1.0). Same slot as `ground`.
        #[arg(long, default_value_t = 0.9)]
        confidence: f64,

        /// Relationship kind(s) this verdict asserts (repeatable; same vocab as
        /// `ground --kind`).
        #[arg(long = "kind")]
        kinds: Vec<String>,

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
    #[command(after_help = "EXAMPLE:\n  \
        loom rule add --name no_sql_in_handlers \\\n    \
          --description \"HTTP handlers never embed SQL; data access goes through the repository layer\" \\\n    \
          --severity error --effort low")]
    Add {
        #[arg(long)]
        name: String,

        #[arg(long)]
        description: String,

        /// Severity: warning | error
        #[arg(long)]
        severity: String,

        /// Norm category: security | correctness | performance | architecture |
        /// resource_safety. Optional. When set and --effort is omitted, the
        /// kind's default effort applies (security/correctness/resource_safety →
        /// high; architecture/performance → mid).
        #[arg(long)]
        kind: Option<String>,

        /// How much capability INSPECTING this rule needs: low (near-mechanical
        /// scan) | mid (read-and-judge, the default) | high (deep semantic
        /// reading). Travels into quality work items as `effort` so tiered
        /// agents route correctly — a statement about the work, never a model.
        /// Overrides the kind's default effort when both are given.
        #[arg(long)]
        effort: Option<String>,
    },

    /// List all quality rules.
    List {
        /// Max rows (0 = all).
        #[arg(long, default_value_t = crate::output::LIST_LIMIT)]
        limit: usize,
    },

    /// Show one quality rule's full record (description, detection_logic,
    /// severity, kind, inspection_effort) — the detail a quality-lane agent
    /// needs to hold the rule against an intent without listing all 22 rules
    /// and grepping. `<identifier>` matches by NAME first (the handle `loom
    /// rule list` prints), then by id — either works.
    #[command(after_help = "EXAMPLE:\n  \
        loom rule show endpoint-matched-edges")]
    Show { identifier: String },

    /// Show all GOVERNS edges for an intent (violations and passing checks).
    Check { intent_id: String },

    /// Seed a built-in measuring-stick pack — the repo-kind vantage points for
    /// 360° normative coverage. `loom detect` recommends which packs fit this
    /// repo; after seeding, `loom next --mode quality` serves every coded
    /// intent the rules were never held against. Already-present rule names
    /// are skipped (idempotent).
    Seed {
        /// Pack name. Available: iso5055 (baseline, any code), mobile
        /// (lifecycle/offline/permissions/touch targets), web-ui (view states/
        /// a11y/contrast/touch targets/XSS), service
        /// (contracts/idempotency/timeouts/sagas), data
        /// (migrations/ingest/PII/lineage), concurrency (sync discipline/
        /// lock hygiene/atomicity/proven perf budgets).
        pack: String,
    },

    /// Apply a rule to an intent — creates a GOVERNS edge (uninspected).
    Apply {
        rule_id: String,
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
        rule_id: String,
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

        /// File/line anchor(s) the evidence points at, e.g.
        /// `src/db/queries/stats.rs:299-340` (repeatable). Folded into the
        /// stored evidence as `@<locator>`.
        #[arg(long)]
        evidence_locator: Vec<String>,

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
// Hypothesis subcommands (the pre-decision plane)
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum HypothesisCmd {
    /// Propose an improvement hypothesis (status = proposed). Any lane may
    /// propose — the structured upgrade of `loom note add --kind idea`.
    #[command(after_help = "EXAMPLE:\n  \
        loom hypothesis add --name \"split the scoring module\" \\\n    \
          --claim \"scoring.rs serves 4 unrelated intents (660 lines) — every queue change rebuilds all of them\" \\\n    \
          --proposal \"extract discovery-candidate ranking into its own module\" \\\n    \
          --predicted-outcome \"scoring.rs under 300 lines and the tangled-file smell on it disappears\" \\\n    \
          --target \"priority-scored work queues\"")]
    Add {
        /// Short handle (addressable later by name or fragment).
        #[arg(long)]
        name: String,

        /// What is wrong/suboptimal NOW — falsifiable, provable against the
        /// current code (substantive; the prover will check exactly this).
        #[arg(long)]
        claim: String,

        /// The proposed change.
        #[arg(long)]
        proposal: String,

        /// The measurable result if adopted — the acceptance contract a
        /// post-implementation validation will be written from (substantive).
        #[arg(long = "predicted-outcome")]
        predicted_outcome: String,

        /// Intent(s) this hypothesis would touch (id/name/fragment; may be
        /// repeated). Creates TARGETS edges; add more later with
        /// `loom hypothesis target`.
        #[arg(long = "target", num_args = 0..)]
        targets: Vec<String>,

        /// Who proposes — role-aware (e.g. llm:quality, human). Defaults to
        /// $LOOM_AGENT, else "llm". The prover must be someone else.
        #[arg(long)]
        author: Option<String>,
    },

    /// Link an existing hypothesis to another intent it would touch (TARGETS).
    Target {
        /// Hypothesis id, name, or unique fragment.
        hypothesis: String,

        /// Intent id, name, or unique fragment.
        intent: String,
    },

    /// Record the proof verdict: did the claimed problem turn out to be real?
    /// Analyzer lane; the prover must differ from the proposer (when both
    /// declare roles). Only a not-yet-decided hypothesis can be proven.
    #[command(after_help = "EXAMPLE:\n  \
        loom hypothesis prove split-the-scoring \\\n    \
          --verdict supported \\\n    \
          --evidence \"read scoring.rs: discovery ranking (L312-660) shares no types with priority scoring; 4 intents ground here\" \\\n    \
          --confidence 0.9")]
    Prove {
        /// Hypothesis id, name, or unique fragment.
        id: String,

        /// supported (the claimed problem is real in the code as it is now) |
        /// refuted (looked — it is not).
        #[arg(long)]
        verdict: String,

        /// What you actually found while checking the claim (substantive).
        #[arg(long)]
        evidence: String,

        /// Confidence in the proof verdict. Written to the TARGETS verdicts
        /// stamped by this proof so doctor can distinguish earned green from
        /// the uninspected default.
        #[arg(long)]
        confidence: f64,

        /// Who proved it — role-aware (e.g. llm:analyzer). Defaults to
        /// $LOOM_AGENT, else "llm".
        #[arg(long)]
        inspected_by: Option<String>,
    },

    /// ADOPT a supported hypothesis: the conversion point. Link the planned
    /// intents you spawned from it (lineage travels as decision notes, and the
    /// predicted outcome lands on each spawned intent as its acceptance
    /// contract). Builder lane; the hypothesis itself never enters any queue.
    #[command(after_help = "EXAMPLE:\n  \
        loom intent add --name \"discovery ranking module\" --level feature --lifecycle planned …\n  \
        loom hypothesis adopt split-the-scoring --spawned \"discovery ranking module\" \\\n    \
          --reason \"verified split point at the type boundary; one new module\"")]
    Adopt {
        /// Hypothesis id, name, or unique fragment (must be `supported`).
        id: String,

        /// Planned intent(s) spawned from this hypothesis (id/name/fragment;
        /// may be repeated). Create them first with `loom intent add
        /// --lifecycle planned` (or mark targets needs_change instead).
        #[arg(long = "spawned", num_args = 0..)]
        spawned: Vec<String>,

        /// How the adoption converts into work (required when no --spawned
        /// intent is given — e.g. "targets marked needs_change instead").
        #[arg(long)]
        reason: Option<String>,
    },

    /// REJECT a hypothesis with a recorded why (not pursuing it). Valid from
    /// any state except adopted. Refuted hypotheses usually end here.
    #[command(after_help = "EXAMPLE:\n  \
        loom hypothesis reject \"split the scoring module\" \\\n    \
          --reason \"the ranking passes share one snapshot; splitting doubles the load path for no cohesion win\"")]
    Reject {
        /// Hypothesis id, name, or unique fragment.
        id: String,

        /// Why this is not being pursued (substantive; recorded as a decision note).
        #[arg(long)]
        reason: String,
    },

    /// List hypotheses, optionally filtered by status.
    List {
        /// proposed | supported | refuted | adopted | confirmed | rejected
        #[arg(long)]
        status: Option<String>,

        /// Max rows (0 = all).
        #[arg(long, default_value_t = crate::output::LIST_LIMIT)]
        limit: usize,
    },

    /// Show one hypothesis in full: fields, TARGETS edges, and notes.
    Show {
        /// Hypothesis id, name, or unique fragment.
        id: String,
    },
}

// ---------------------------------------------------------------------------
// Saga subcommands (the consumer plane)
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum SagaCmd {
    /// Register a saga spec (YAML): creates the Validation node (type=saga),
    /// VALIDATES edges to every step's intent, the RELATES_TO path edges
    /// between consecutive step intents (uninspected until a run earns them),
    /// and registers the spec file itself as a CodeFile. Idempotent —
    /// re-running after editing the spec reconciles the links.
    ///
    /// JOURNEY-FIRST: with --spawn-missing, a step may name an intent that
    /// does not exist yet — it is spawned as a planned, user_visible feature
    /// (the narrated journey IS the design; the build queue realizes it).
    #[command(after_help = "SPEC FORMAT (YAML):\n  \
        saga: checkout-flow\n  \
        base: \"{{ env.BASE_URL }}\"\n  \
        steps:\n    \
          - name: create cart\n      \
            intent: cart-creation          # id, exact name, or unique fragment\n      \
            request: { method: POST, url: /carts, json: { items: [] } }\n      \
            expect:  { status: 201 }\n      \
            capture: { cart_id: \"$.id\" }\n    \
          - name: capture payment\n      \
            intent: payment-capture\n      \
            request: { method: POST, url: \"/carts/{{ cart_id }}/payment\" }\n      \
            auth:     { requires_scopes: [payments.write] }  # optional diagnosis hint\n      \
            expect:  { status: 200, body: { \"$.state\": paid } }\n\n  \
        JOURNEY-FIRST (story → intents):\n  \
        loom saga add journeys/checkout.yaml --spawn-missing --under \"checkout component\"")]
    Add {
        /// Path to the saga spec file.
        file: String,

        /// Spawn a planned, user_visible FEATURE intent for every step whose
        /// `intent:` binding resolves to nothing — the journey-first entrance:
        /// the user narrates the story, the steps become the design, the build
        /// queue realizes them. Builder lane; refuses on observed graphs.
        #[arg(long)]
        spawn_missing: bool,

        /// Parent intent (id, name, or fragment) for spawned steps — keeps the
        /// hierarchy a tree instead of minting roots. Requires --spawn-missing.
        #[arg(long, requires = "spawn_missing")]
        under: Option<String>,
    },

    /// Execute a saga and stamp the run into the graph: the Validation's
    /// result, every linked intent's VALIDATES verdict, and the RELATES_TO
    /// path — steps before a failure are runtime-passing evidence, the failing
    /// boundary goes failing with the broken expectation, steps after it are
    /// untouched (never reached ≠ failing). Exits non-zero on failure, so it
    /// also works as the command behind `loom validate`.
    ///
    /// `{{ env.X }}` values (the LIVE target's url, tokens, …) are passed at
    /// invocation: `BASE_URL=http://localhost:3000 loom saga run <name>` —
    /// never stored in the graph. Missing values refuse to run (nothing is
    /// stamped) and the error names every var plus the exact invocation;
    /// `loom saga list` shows each saga's `run with:` line.
    Run {
        /// Saga name (the registered validation) or a spec file path.
        saga: String,
    },

    /// Run a saga without stamping graph verdicts and explain the first failed
    /// boundary as a structured, repo-agnostic diagnosis. This is for triage:
    /// missing env/template problems, auth-like status failures, 404/resource
    /// misses, body/status mismatches, request failures, and skipped dependent
    /// steps. When a step declares `auth.requires_scopes`, diagnosis decodes
    /// bearer JWT `scope`/`scp`/`scopes` claims and names missing scopes.
    /// Repo-specific state probes can layer on top later.
    Diagnose {
        /// Saga name (the registered validation) or a spec file path.
        saga: String,
    },

    /// List registered sagas (validations of type=saga) with their last result.
    List,
}

// ---------------------------------------------------------------------------
// Interface subcommands
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum InterfaceCmd {
    /// List interface surfaces and the number of saga steps that call each one.
    List,

    /// Show gaps in the already-populated InterfaceSurface/CALLS plane.
    Gaps,

    /// Show one interface surface with its saga callers.
    Show {
        /// Surface id, exact name, or unique name/target fragment.
        surface: String,
    },

    /// Remove an interface surface (its CALLS edges go with it) — the reachable
    /// remedy for a `surface_without_calls` gap (a stale surface a renamed/
    /// removed endpoint left behind).
    Remove {
        /// Surface id, exact name, or unique name/target fragment.
        surface: String,
    },
}

// ---------------------------------------------------------------------------
// Populate subcommands
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum PopulateCmd {
    /// Show computed population work without changing the graph.
    Plan,

    /// Populate/repopulate interface surfaces and CALLS edges.
    Interfaces {
        /// Rebuild HTTP endpoint surfaces and CALLS edges from registered saga
        /// specs. Existing calls for each saga validation are replaced.
        #[arg(long)]
        from_sagas: bool,

        /// Report what would change without writing to the graph.
        #[arg(long)]
        dry_run: bool,

        /// After repopulating, delete interface surfaces left with NO CALLS — the
        /// stale surfaces a renamed/removed endpoint orphaned (the reachable form
        /// of the `surface_without_calls` gap remedy, in bulk).
        #[arg(long)]
        prune: bool,
    },

    /// Backfill mechanical relationship kinds (imports | shares_file |
    /// shares_vocab | same_domain) onto grounded RELATES_TO edges from existing
    /// evidence. Judgment kinds (calls/inheritance/…) are preserved; only the
    /// mechanical tier is recomputed.
    Kinds {
        /// Report what would change without writing to the graph.
        #[arg(long)]
        dry_run: bool,
    },
}

// ---------------------------------------------------------------------------
// Note subcommands
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum NoteCmd {
    /// Remove notes whose target no longer exists (deleted intent/hypothesis/
    /// edge) — the remedy `loom doctor` names for dangling note targets.
    /// Reports what was removed; floating and file notes are never touched.
    /// With --transitions, ALSO compacts low-signal transition history: keeps
    /// the newest per target plus every regression marker, drops the bulk
    /// passing↔needs_reverification sync churn (smells + align unchanged).
    #[command(after_help = "EXAMPLE:\n  \
        loom note prune --transitions --keep-per-target 3 --dry-run")]
    Prune {
        /// Also compact transition history: keep, per target, the newest
        /// ROUTINE transitions plus every `→ failing` / `→ needs_change`
        /// marker; drop the rest. `loom smells` findings and the align
        /// candidate set are unchanged — only the sync flip-flop is removed.
        /// (`loom sync` does this automatically up to the graph's cap; this is
        /// the manual / retroactive sweep.)
        #[arg(long)]
        transitions: bool,

        /// Newest routine transitions to keep per target for THIS sweep. Omit
        /// to use the graph's transition_cap (the ceiling sync enforces).
        #[arg(long)]
        keep_per_target: Option<usize>,

        /// Persist the graph's per-target transition cap — the ceiling `loom
        /// sync` trims to going forward. 0 = off (strict append-only). Setting
        /// it also compacts now to that ceiling.
        #[arg(long)]
        set_cap: Option<usize>,

        /// Report what would be removed without deleting anything.
        #[arg(long)]
        dry_run: bool,
    },

    /// Add a note. Attach it to an intent, an edge, or a code file, or leave
    /// it free-floating.
    #[command(after_help = "EXAMPLE:\n  \
        loom note add --kind decision --intent \"request routing\" \\\n    \
          --text \"host-only routing is deliberate: path routing was descoped in the multi-tenant review\"")]
    Add {
        /// The note text.
        #[arg(long)]
        text: String,

        /// Kind: justification | commentary | idea | question | decision | todo
        /// (transition and confirm are auto-recorded by loom itself).
        #[arg(long, default_value = "commentary")]
        kind: String,

        /// Attach to this intent id (mutually exclusive with --edge/--file).
        #[arg(long)]
        intent: Option<String>,

        /// Attach to this edge id (mutually exclusive with --intent/--file).
        #[arg(long)]
        edge: Option<String>,

        /// Attach to this CodeFile (id or registered path).
        #[arg(long)]
        file: Option<String>,

        /// Adjudicate a SPECIFIC smell finding by its identity, e.g.
        /// `--smell "tangled_file:src/db/sqlite.rs"` or
        /// `--smell "large_behavioral_symbol:src/x.rs:fn foo"` (the exact string
        /// `loom smells` prints in the remedy). A decision note scoped this way
        /// clears ONLY that finding — a per-symbol ruling can no longer launder a
        /// file-level finding, and one ruling can't silence a whole file. The
        /// ruling must be a real inspection of THIS finding: name the
        /// decomposition you considered and the concrete reason it is wrong here,
        /// in terms true only of it. loom REJECTS a vacuous ruling or one that
        /// reuses the wording of a ruling you recorded on another finding —
        /// batch-stamping the audit gate green is exactly what this guards.
        #[arg(long)]
        smell: Option<String>,

        /// Who wrote it — role-aware (e.g. llm:analyzer, human:reviewer).
        /// Defaults to $LOOM_AGENT, else "llm".
        #[arg(long)]
        author: Option<String>,

        /// Address this note to a lane: builder | analyzer | fixer |
        /// validator | quality. The directed-handoff channel — an out-of-lane
        /// finding becomes a message the owning lane sees FIRST in its work
        /// items (`loom next` sorts addressed notes to the top). Omit for
        /// everyone.
        #[arg(long = "for", value_name = "ROLE")]
        for_role: Option<String>,
    },

    /// List notes, optionally filtered by target or kind.
    List {
        /// Only notes attached to this intent id.
        #[arg(long)]
        intent: Option<String>,

        /// Only notes attached to this edge id.
        #[arg(long)]
        edge: Option<String>,

        /// Only notes attached to this CodeFile (id or registered path).
        #[arg(long)]
        file: Option<String>,

        /// Only notes of this kind.
        #[arg(long)]
        kind: Option<String>,

        /// Only notes addressed to this lane (the lane's inbox).
        #[arg(long = "for", value_name = "ROLE")]
        for_role: Option<String>,

        /// Max rows, NEWEST kept (0 = all) — note memory is append-only and
        /// grows forever; the tail is the live context.
        #[arg(long, default_value_t = crate::output::LIST_LIMIT)]
        limit: usize,
    },
}

// ---------------------------------------------------------------------------
// Inbox subcommands
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum InboxCmd {
    /// Capture raw language as an intake card. This is the single boundary for
    /// free-form human/LLM input before it becomes graph truth.
    #[command(after_help = "EXAMPLE:\n  \
        loom inbox add \"status debt feels scarier than reality\" --source chat --tag status")]
    Add {
        raw_text: String,

        /// Source: chat | user | llm | code_audit | validation | import | unknown
        #[arg(long, default_value = "unknown")]
        source: String,

        /// Existing VocabTerm name. Repeatable.
        #[arg(long = "tag")]
        tags: Vec<String>,

        /// Link to an existing graph object: intent:<id>, file:<path>,
        /// validation:<id>, hypothesis:<id>, rule:<id>, vocab:<term>, inbox:<id>.
        #[arg(long = "link")]
        links: Vec<String>,

        /// Who captured it. Defaults to $LOOM_AGENT, else "llm".
        #[arg(long)]
        author: Option<String>,
    },

    /// List inbox cards.
    List {
        /// Filter by status: new | triaged | routed | rejected | deferred | duplicate
        #[arg(long)]
        status: Option<String>,

        /// Filter by kind.
        #[arg(long)]
        kind: Option<String>,

        /// Max rows, newest first. 0 = all.
        #[arg(long, default_value_t = crate::output::LIST_LIMIT)]
        limit: usize,
    },

    /// Show one inbox card.
    Show { id: String },

    /// Return triage context for new/triaged cards: matches, vocab, route menu,
    /// and exact normalize templates.
    Triage {
        /// Number of cards to serve.
        #[arg(long, default_value_t = 20)]
        take: usize,
    },

    /// Store the LLM's normalized reading and route proposal. This does not
    /// mutate graph truth; run the proposed command separately, then mark routed.
    #[command(after_help = "EXAMPLE:\n  \
        loom inbox normalize <id> --kind missing_intent \\\n    \
          --claim \"password reset is a planned user-visible capability\" \\\n    \
          --route intent \\\n    \
          --command \"loom intent add --name 'password reset' --description 'users can request a reset link' --level feature --lifecycle planned\"")]
    Normalize {
        id: String,

        /// Inbox kind.
        #[arg(long)]
        kind: String,

        /// Normalized claim, in loom vocabulary.
        #[arg(long)]
        claim: String,

        /// Route kind: intent | hypothesis | validation | quality_rule | vocab | note | ignore | answer | none
        #[arg(long = "route")]
        route_kind: String,

        /// Exact command or answer the operator should execute/review.
        #[arg(long)]
        command: String,

        /// Existing VocabTerm name. Repeatable; replaces current tags.
        #[arg(long = "tag")]
        tags: Vec<String>,

        /// Link to an existing graph object. Repeatable; replaces current links.
        #[arg(long = "link")]
        links: Vec<String>,

        /// Optional expected target kind for the route.
        #[arg(long = "target-kind")]
        route_target_kind: Option<String>,

        /// Optional expected target id/name for the route.
        #[arg(long = "target-id")]
        route_target_id: Option<String>,
    },

    /// Mark an inbox card after a decision or after executing its route command.
    #[command(after_help = "EXAMPLE:\n  \
        loom inbox mark <id> --status routed \\\n    \
          --reason \"created planned intent password reset with loom intent add\" \\\n    \
          --target-kind intent --target-id <intent-id>")]
    Mark {
        id: String,

        /// Status: routed | rejected | duplicate | deferred
        #[arg(long)]
        status: String,

        /// Why this status is correct, or what command/result routed it.
        #[arg(long)]
        reason: String,

        /// Graph object kind produced/used by the route.
        #[arg(long = "target-kind")]
        route_target_kind: Option<String>,

        /// Graph object id/name produced/used by the route.
        #[arg(long = "target-id")]
        route_target_id: Option<String>,
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
    List {
        /// Max rows (0 = all).
        #[arg(long, default_value_t = crate::output::LIST_LIMIT)]
        limit: usize,
    },

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
    #[command(after_help = "EXAMPLE:\n  \
        loom validation add --name \"routing smoke\" --type test \\\n    \
          --command \"cargo test -p server --test routing\" --intent \"request routing\"")]
    Add {
        #[arg(long)]
        name: String,

        #[arg(long)]
        description: Option<String>,

        /// Type: test | assertion | benchmark | manual_check | saga
        /// (saga is normally created via `loom saga add`, not here).
        #[arg(long = "type")]
        validation_type: String,

        /// Shell command to run (e.g. "cargo test --test integration").
        #[arg(long)]
        command: Option<String>,

        /// Link the new validation to intent(s) in one step (repeatable —
        /// one VALIDATES edge each). Omit to link later with `loom edge validates`.
        #[arg(long)]
        intent: Vec<String>,
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
    List {
        /// Filter by last result: passed | failed | blocked | not_run.
        #[arg(long)]
        result: Option<String>,

        /// Max rows (0 = all).
        #[arg(long, default_value_t = crate::output::LIST_LIMIT)]
        limit: usize,
    },

    /// Show full detail of one validation node.
    Show { id: String },
}

// ---------------------------------------------------------------------------
// Persona subcommands (the audience-segment plane)
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum PersonaCmd {
    /// Register a new persona — a named audience segment.
    #[command(after_help = "EXAMPLE:\n  \
        loom persona add --name admin \\\n    \
          --description \"system administrator who configures accounts and manages billing — NOT an end-user (that's viewer)\"")]
    Add {
        /// The persona name (short, lowercase, e.g. "admin", "end_user").
        #[arg(long)]
        name: String,

        /// Who this persona is and what distinguishes them from other segments.
        /// Make it CONTRASTIVE: what they can do AND what they cannot (name the
        /// neighbouring persona), so an agent can disambiguate at tagging time.
        #[arg(long)]
        description: String,

        /// Who added this — role-aware (e.g. llm:builder, human).
        /// Defaults to $LOOM_AGENT, else "llm".
        #[arg(long)]
        author: Option<String>,
    },

    /// List all registered personas.
    List {
        /// Max rows (0 = all).
        #[arg(long, default_value_t = crate::output::LIST_LIMIT)]
        limit: usize,
    },

    /// Show one persona: its SERVES edges (with inspection status) and JOURNEYS.
    Show {
        /// Persona id, exact name, or unique name fragment.
        id: String,
    },

    /// Inspect or create a SERVES edge — "does this intent serve this persona?"
    /// Without a subcommand, creates the edge (uninspected) and prints context.
    /// With ground/issue/independent, records the verdict.
    #[command(after_help = "EXAMPLES:\n  \
        loom persona serve admin \"user management\"                      # create + print context\n  \
        loom persona serve admin \"user management\" ground \\\n    \
          --criterion \"admin can create/suspend/delete accounts; end_user cannot\" --confidence 0.9\n  \
        loom persona serve admin \"shopping cart\" independent \\\n    \
          --notes \"cart is end_user only; admin uses bulk order API instead\"")]
    Serve {
        /// Persona id, exact name, or unique name fragment.
        persona_id: String,

        /// Intent id, exact name, or unique name fragment.
        intent_id: String,

        #[command(subcommand)]
        subcommand: Option<ExploreSubCmd>,
    },

    /// Bind a saga to a persona — "this journey exercises this persona's path."
    /// Creates a structural JOURNEYS edge (Persona → Validation of type=saga).
    #[command(after_help = "EXAMPLE:\n  \
        loom persona journey admin checkout-admin-flow")]
    Journey {
        /// Persona id, exact name, or unique name fragment.
        persona_id: String,

        /// Saga validation id, name, or unique name fragment.
        saga_id: String,
    },

    /// Remove a persona (its SERVES + JOURNEYS edges go with it) — for a stale
    /// audience segment that no longer exists.
    Remove {
        /// Persona id, exact name, or unique name fragment.
        id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Validates the entire clap tree config (conflicts, trailing var-args,
    /// hidden stubs) — a mis-declared arg panics here instead of at runtime.
    #[test]
    fn clap_tree_is_well_formed() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn next_discovery_class_parses_as_explicit_selector() {
        let cli = Cli::parse_from([
            "loom",
            "next",
            "--mode",
            "discovery",
            "--class",
            "impact-map",
        ]);
        match cli.command {
            Some(Command::Next {
                mode,
                discovery_class,
                ..
            }) => {
                assert_eq!(mode.as_deref(), Some("discovery"));
                assert_eq!(discovery_class.as_deref(), Some("impact-map"));
            }
            _ => panic!("expected next command"),
        }
    }

    #[test]
    fn saga_diagnose_parses_as_consumer_plane_command() {
        let cli = Cli::parse_from(["loom", "saga", "diagnose", "journeys/checkout.yaml"]);
        match cli.command {
            Some(Command::Saga {
                subcommand: SagaCmd::Diagnose { saga },
            }) => assert_eq!(saga, "journeys/checkout.yaml"),
            _ => panic!("expected saga diagnose command"),
        }
    }

    /// Unrecognized top-level tokens must CAPTURE into the catch-all (not
    /// clap-error) so `teach_unknown` answers with the real invocation —
    /// verbs, synonyms, and typos alike, flags included.
    #[test]
    fn unknown_tokens_parse_into_the_teaching_catchall() {
        for argv in [
            vec!["loom", "update", "request routing", "--description", "x"],
            vec!["loom", "rename", "x"],
            vec!["loom", "retire"],
            vec!["loom", "statsu"],
        ] {
            let verb = argv[1].to_string();
            match Cli::parse_from(&argv).command {
                Some(Command::Unknown(rest)) => assert_eq!(rest[0], verb),
                other => panic!("expected catch-all for {verb}, got {:?}", other.is_some()),
            }
        }
        // Real commands must NOT be swallowed by the catch-all.
        assert!(matches!(
            Cli::parse_from(["loom", "status"]).command,
            Some(Command::Status)
        ));
    }

    /// The parse-error footer walks argv to the failing command — its EXAMPLE
    /// is what gets appended under the clap error.
    #[test]
    fn deepest_subcommand_walk_finds_the_failing_command() {
        let walk = |argv: &[&str]| {
            super::deepest_subcommand(argv.iter().map(|s| s.to_string()))
                .map(|c| c.get_name().to_string())
        };
        assert_eq!(
            walk(&["intent", "update", "routing"]).as_deref(),
            Some("update")
        );
        assert_eq!(
            walk(&["--json", "intent", "update"]).as_deref(),
            Some("update")
        );
        assert_eq!(walk(&["export", "--check"]).as_deref(), Some("export"));
        assert_eq!(walk(&["nonsense"]), None);
    }

    /// THE RATCHET: every leaf command that REQUIRES a flag must ship an
    /// EXAMPLE after_help — `parse_or_teach` prints it under every syntax
    /// error, so a command without one fails with bare clap-babble and the
    /// agent leaves the loop to go doc-hunting (the dogfood finding that
    /// started this). Adding a required flag obligates adding the example.
    #[test]
    fn every_flag_requiring_command_ships_an_example() {
        use clap::CommandFactory;
        fn walk(cmd: &clap::Command, path: &str, missing: &mut Vec<String>) {
            let subs: Vec<&clap::Command> = cmd
                .get_subcommands()
                .filter(|s| s.get_name() != "help")
                .collect();
            if subs.is_empty() {
                let requires_flag = cmd
                    .get_arguments()
                    .any(|a| a.is_required_set() && a.get_long().is_some());
                let has_example = cmd
                    .get_after_help()
                    .map(|h| h.to_string().contains("loom "))
                    .unwrap_or(false);
                if requires_flag && !has_example {
                    missing.push(path.to_string());
                }
            }
            for s in subs {
                walk(s, &format!("{path} {}", s.get_name()), missing);
            }
        }
        let cmd = Cli::command();
        let mut missing = Vec::new();
        walk(&cmd, "loom", &mut missing);
        assert!(
            missing.is_empty(),
            "commands with required flags but no EXAMPLE after_help (their errors can't teach): {missing:#?}"
        );
    }

    /// Positional wording on `intent update` lands in the hidden catch-all
    /// (so the handler teaches) instead of a clap "unexpected argument".
    #[test]
    fn intent_update_positional_text_is_caught_for_teaching() {
        let cli = Cli::parse_from(["loom", "intent", "update", "routing", "new words here"]);
        let Some(Command::Intent {
            subcommand: IntentCmd::Update { id, extra, .. },
        }) = cli.command
        else {
            panic!("expected intent update");
        };
        assert_eq!(id, "routing");
        assert_eq!(extra, vec!["new words here".to_string()]);
    }
}
