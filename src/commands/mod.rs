use crate::cli::{Cli, Command};
use crate::output::Printer;
use anyhow::Result;

pub mod batch;
pub mod cluster;
pub mod codefile;
pub mod coverage;
pub mod delegate;
pub mod detect;
pub mod doctor;
pub mod domain;
pub mod door;
pub mod edge;
pub mod export;
pub mod find;
pub mod guide;
pub mod hotspots;
pub mod hypothesis;
pub mod ignore;
pub mod import;
pub mod init;
pub mod intent;
pub mod layer;
pub mod migrate;
pub mod next;
pub mod note;
pub mod persona;
pub mod report;
pub mod rule;
pub mod saga;
pub mod schema;
pub mod session;
pub mod smells;
pub mod status;
pub mod sync;
pub mod validate;
pub mod validation;
pub mod vocab;

/// The outcome of a `dispatch_with_db` attempt — the daemon's routing verdict.
/// `Ran` means the command is servable and was executed against the held
/// handle (its `Result` is the command's own outcome). `NotServable` is the
/// sentinel the daemon turns into a `fallback` reply: the client then runs the
/// command via the ordinary direct-open `dispatch`. Graph-releasing commands
/// (validate, saga) and lifecycle commands that open their own graph (init,
/// migrate's path, …) and anything lacking a `run_with_db` are NotServable.
pub enum DispatchOutcome {
    Ran(Result<()>),
    NotServable,
}

/// Daemon dispatch: run a command against an ALREADY-OPEN shared handle,
/// routing each servable command to its `*::run_with_db` variant (the body
/// that does NOT open its own graph). Mirrors `dispatch`'s match arm-for-arm
/// for every command that has a `run_with_db`; everything else returns
/// `NotServable` so the client falls back to direct dispatch.
///
/// `--graph` is honoured here too (the daemon process resolves its own root,
/// but a client may still carry the flag); `cli.json` is guaranteed true by
/// the caller (the daemon only serves JSON requests).
pub fn dispatch_with_db(
    db: &crate::db::GrafeoDb,
    root: &std::path::Path,
    cli: Cli,
    printer: &Printer,
) -> DispatchOutcome {
    if let Some(g) = &cli.graph {
        crate::db::set_explicit_graph(g);
    }
    let command = match cli.command {
        // Bare `loom` (orientation) opens no graph — safe and cheap to serve.
        Some(c) => c,
        None => return DispatchOutcome::Ran(orient(printer)),
    };
    use DispatchOutcome::{NotServable, Ran};
    match command {
        // ---- Servable: every command with a `run_with_db` body --------------
        Command::Status                     => Ran(status::run_with_db(db, root, printer)),
        Command::Intent      { subcommand } => Ran(intent::run_with_db(db, root, subcommand, printer)),
        Command::Edge        { subcommand } => Ran(edge::run_with_db(db, root, subcommand, printer)),
        Command::Next        { mode, all, take, compact } =>
            Ran(next::run_with_db(db, root, &mode, all, take, compact, printer)),
        Command::Cluster     { intent_id }  => Ran(cluster::run_with_db(db, root, &intent_id, printer)),
        Command::Rule        { subcommand } => Ran(rule::run_with_db(db, root, subcommand, printer)),
        Command::Codefile    { subcommand } => Ran(codefile::run_with_db(db, root, subcommand, printer)),
        Command::Validation  { subcommand } => Ran(validation::run_with_db(db, root, subcommand, printer)),
        Command::Hypothesis  { subcommand } => Ran(hypothesis::run_with_db(db, root, subcommand, printer)),
        Command::Note        { subcommand } => Ran(note::run_with_db(db, root, subcommand, printer)),
        Command::Vocab       { subcommand } => Ran(vocab::run_with_db(db, root, subcommand, printer)),
        Command::Layer       { subcommand } => Ran(layer::run_inner_with_db(db, root, subcommand, printer, false)),
        Command::Persona     { subcommand } => Ran(persona::run_with_db(db, root, subcommand, printer)),
        Command::Sync        { path }       => Ran(sync::run_with_db(db, root, &path, printer)),
        Command::Report                     => Ran(report::run_with_db(db, root, printer)),
        Command::Batch       { file }       => Ran(batch::run_with_db(db, root, &file, printer)),
        Command::Doctor                     => Ran(doctor::run_with_db(db, root, printer)),
        Command::Migrate                    => Ran(migrate::run_with_db(db, root, printer)),
        Command::Find        { query, limit } => Ran(find::run_with_db(db, root, &query, limit, printer)),
        Command::Door        { utterance, limit } => Ran(door::run_with_db(db, root, &utterance, limit, printer)),
        Command::Session                    => Ran(session::run_with_db(db, root, printer)),
        Command::Hotspots    { limit }      => Ran(hotspots::run_with_db(db, root, limit, printer)),
        Command::Smells      { limit }      => Ran(smells::run_with_db(db, root, limit, printer)),
        Command::Coverage                   => Ran(coverage::run_with_db(db, root, printer)),
        Command::Ignore      { subcommand } => Ran(ignore::run_with_db(db, root, subcommand, printer)),
        Command::Delegate    { subcommand } => Ran(delegate::run_with_db(db, root, subcommand, printer)),
        Command::Export      { path, out, check } => {
            let out = path.or(out).unwrap_or_else(|| "loom.graph.json".to_string());
            Ran(export::run_with_db(db, root, &out, check, printer))
        }
        Command::Import      { file, as_planned } => Ran(import::run_with_db(db, root, &file, as_planned, printer)),

        // ---- NOT servable: fall back to direct dispatch ---------------------
        // Graph-releasing (drop the handle mid-run to free the lock for an
        // external process that may itself invoke loom) — a held-open handle
        // would deadlock:
        Command::Validate { .. } => NotServable,
        Command::Saga     { .. } => NotServable,
        // Open/own their own graph lifecycle or read no held graph at all:
        Command::Init   { .. } => NotServable,
        Command::Guide  { .. } => NotServable,
        Command::Schema        => NotServable,
        Command::Domain { .. } => NotServable, // deprecated alias; runs direct
        Command::Detect        => NotServable,
        // The daemon never serves a `serve` request (the client routes `serve`
        // before the daemon check) — but be exhaustive: fall back if one arrives.
        Command::Serve { .. }  => NotServable,
        // An unrecognized token: let direct dispatch teach (it bails with help).
        Command::Unknown(_)    => NotServable,
    }
}

pub fn dispatch(cli: Cli) -> Result<()> {
    let printer = Printer::new(cli.json);
    if let Some(g) = &cli.graph {
        crate::db::set_explicit_graph(g);
    }
    let command = match cli.command {
        Some(c) => c,
        None => return orient(&printer),
    };
    match command {
        Command::Init        { path, name, observed } =>
            init::run(&path, name.as_deref(), observed, &printer),
        Command::Status                     => status::run(&printer),
        Command::Intent      { subcommand } => intent::run(subcommand, &printer),
        Command::Edge        { subcommand } => edge::run(subcommand, &printer),
        Command::Next        { mode, all, take, compact } => next::run(&mode, all, take, compact, &printer),
        Command::Cluster     { intent_id }  => cluster::run(&intent_id, &printer),
        Command::Rule        { subcommand } => rule::run(subcommand, &printer),
        Command::Codefile    { subcommand } => codefile::run(subcommand, &printer),
        Command::Validation  { subcommand } => validation::run(subcommand, &printer),
        Command::Saga        { subcommand } => saga::run(subcommand, &printer),
        Command::Hypothesis  { subcommand } => hypothesis::run(subcommand, &printer),
        Command::Note        { subcommand } => note::run(subcommand, &printer),
        Command::Vocab       { subcommand } => vocab::run(subcommand, &printer),
        Command::Domain      { subcommand } => domain::run(subcommand, &printer),
        Command::Layer       { subcommand } => layer::run(subcommand, &printer),
        Command::Persona     { subcommand } => persona::run(subcommand, &printer),
        Command::Sync        { path }       => sync::run(&path, &printer),
        Command::Validate    { intent_id, all, timeout_secs } => match (intent_id, all) {
            (Some(i), false) => validate::run(&i, timeout_secs, &printer),
            (None,    true)  => validate::run_all(timeout_secs, &printer),
            (Some(_), true)  => anyhow::bail!("Pass an intent OR --all, not both."),
            (None,    false) => anyhow::bail!("Pass the intent whose proofs should run, or --all for every pending (not_run) validation."),
        },
        Command::Report                     => report::run(&printer),
        Command::Batch       { file }       => batch::run(&file, &printer),
        Command::Doctor                     => doctor::run(&printer),
        Command::Migrate                    => migrate::run(&printer),
        Command::Guide       { mode }       => guide::run(mode.as_deref(), &printer),
        Command::Schema                     => schema::run(&printer),
        Command::Find        { query, limit } => find::run(&query, limit, &printer),
        Command::Door        { utterance, limit } => door::run(&utterance, limit, &printer),
        Command::Session                    => session::run(&printer),
        Command::Hotspots    { limit }      => hotspots::run(limit, &printer),
        Command::Smells      { limit }      => smells::run(limit, &printer),
        Command::Coverage                   => coverage::run(&printer),
        Command::Detect                     => detect::run(&printer),
        Command::Ignore      { subcommand } => ignore::run(subcommand, &printer),
        Command::Delegate    { subcommand } => delegate::run(subcommand, &printer),
        Command::Export      { path, out, check } => {
            let out = path.or(out).unwrap_or_else(|| "loom.graph.json".to_string());
            export::run(&out, check, &printer)
        }
        Command::Import      { file, as_planned } => import::run(&file, as_planned, &printer),
        Command::Serve       { idle_secs }  => crate::serve::serve(&crate::db::resolve_root()?, idle_secs),
        Command::Unknown(tokens) => teach_unknown(&tokens),
    }
}

/// Any unrecognized top-level token lands here (clap `external_subcommand`).
/// Three populations, three answers — all of them teach:
/// 1. noun-less verbs and synonym guesses (`update`, `rename`, `retire`) →
///    the real invocation, with the agent's own argument spliced in;
/// 2. typos of real commands (`statsu`) → the nearest command by edit
///    distance (clap's stock tip once mapped `update` → 'guide');
/// 3. anything else → the noun list and the guide.
fn teach_unknown(tokens: &[String]) -> Result<()> {
    let verb = tokens.first().map(String::as_str).unwrap_or("");
    let arg = tokens
        .iter()
        .skip(1)
        .find(|t| !t.starts_with('-'))
        .map(|s| format!("\"{s}\""))
        .unwrap_or_else(|| "<id>".into());
    match verb {
        "update" | "edit" | "set" | "describe" | "redefine" | "reword" => anyhow::bail!(
            "`{verb}` lives under its noun:\n  \
             loom intent update {arg} --description \"<new meaning>\" --reason \"<why it moved>\"   (meaning evolution; --reword when only the words change)\n  \
             loom validation update <id> --command \"<cmd>\"   (repair a proof's command)"
        ),
        "rename" => anyhow::bail!(
            "renaming is `update` under the noun:\n  \
             loom intent update {arg} --name \"<new name>\" --reason \"<why>\"   (cosmetic — no ripple)"
        ),
        "confirm" => anyhow::bail!(
            "`confirm` lives under its noun:\n  \
             loom intent confirm {arg}   (ratify the meaning; resets the align clock)\n  \
             loom intent confirm {arg} --visibility internal   (machinery — stop interviewing it)"
        ),
        "ground" | "issue" | "independent" => anyhow::bail!(
            "`{verb}` is a verdict on an edge:\n  \
             loom edge explore <intent-a> <intent-b> {verb}{flags}\n  \
             (`loom next` serves the pair to inspect, with the verdict commands prefilled)",
            flags = match verb {
                "ground" => " --criterion \"<falsifiable coexistence test>\" --confidence 0.9",
                "issue" => " --criterion \"<test>\" --evidence \"<what is wrong>\" --confidence 0.9",
                _ => " --notes \"<why these never interact>\"",
            },
        ),
        "add" | "create" | "new" => anyhow::bail!(
            "`{verb}` needs a noun — what is being added?\n  \
             loom intent add --name \"<behavior>\" --description \"<falsifiable meaning>\" --level feature\n  \
             loom note add --text \"<text>\" --kind decision [--intent <id>]\n  \
             loom validation add --name \"<proof>\" --type test --command \"<cmd>\" --intent <id>\n  \
             (also: codefile add · rule add · vocab add · hypothesis add)"
        ),
        "retire" | "remove" | "delete" | "deprecate" => anyhow::bail!(
            "removal lives under the noun, and the WHY picks the verb:\n  \
             loom intent retire {arg} --reason \"<why>\" [--replaced-by <successor>]   (superseded design — permanent history, ripples like a redefinition)\n  \
             loom intent delete {arg}   (a MISTAKE, not history — irreversible, takes edges and notes with it)"
        ),
        "explore" => anyhow::bail!(
            "`explore` lives under `edge`:\n  \
             loom edge explore <intent-a> <intent-b> ground|issue|independent …   (`loom next` serves the pair)"
        ),
        "implement" | "map" => anyhow::bail!(
            "grounding is `implement` under `edge`:\n  \
             loom edge implement <intent> <file> [--locator \"<symbol>\"]   (the structural claim `loom sync` watches)"
        ),
        "mark" => anyhow::bail!(
            "`mark` lives under its noun:\n  \
             loom intent mark {arg} --lifecycle planned|implemented|needs_change [--reason \"<why>\"]\n  \
             loom validation mark <id> --result passed|failed|blocked [--reason \"<why>\"]"
        ),
        "verdict" => anyhow::bail!(
            "`verdict` lives under `rule` (GOVERNS); edge verdicts go through `edge explore`:\n  \
             loom rule verdict <rule> <intent> --status passing|failing|independent --criterion \"…\" --evidence \"…\"\n  \
             loom edge explore <a> <b> ground|issue|independent …"
        ),
        "prove" | "run" | "test" => anyhow::bail!(
            "proofs run through `validate`:\n  \
             loom validate {arg}   (run one intent's proofs)\n  \
             loom validate --all   (every pending proof)"
        ),
        "start" | "begin" | "hello" | "hi" | "mode" | "talk" | "chat" | "interview" => anyhow::bail!(
            "opening a session is its own command:\n  \
             loom session   (turn zero: the ask-the-user playbook — one question, a state-aware offer menu)\n  \
             loom door \"<their words>\"   (the user already said something? route it to its landing)"
        ),
        "show" | "list" => anyhow::bail!(
            "`{verb}` needs a noun:\n  \
             loom intent {verb} · loom edge list · loom note list · loom rule list · loom validation list\n  \
             (or the read surfaces: loom status · loom next · loom find \"<query>\")"
        ),
        _ => {
            // A typo of a real command? Suggest the nearest by edit distance —
            // computed against the live command list, so it never drifts.
            use clap::CommandFactory;
            let cli = crate::cli::Cli::command();
            let nearest = cli
                .get_subcommands()
                .map(|s| s.get_name())
                .filter(|n| *n != "help")
                .map(|n| (edit_distance(verb, n), n))
                .min()
                .filter(|(d, _)| *d <= 2);
            match nearest {
                Some((_, n)) => anyhow::bail!(
                    "Unknown command '{verb}' — did you mean `loom {n}`? (`loom --help` lists everything)"
                ),
                None => anyhow::bail!(
                    "Unknown command '{verb}'. The nouns: intent · edge · note · rule · validation · \
                     codefile · vocab · hypothesis — each with add/list/show verbs. The loop: \
                     `loom status` → `loom next` → record what you found. `loom guide` teaches it."
                ),
            }
        }
    }
}

/// Levenshtein distance, two-row DP — small inputs (command names), no
/// dependency worth taking.
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = if ca == cb {
                prev[j]
            } else {
                1 + prev[j].min(prev[j + 1]).min(cur[j])
            };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Bare `loom` (no subcommand): a short orientation pointing at the self-teaching
/// commands, so an LLM that knows nothing can find its footing.
fn orient(printer: &Printer) -> Result<()> {
    if printer.json {
        printer.print_json(&serde_json::json!({
            "tool": "loom",
            "what": "Externalized, falsifiable memory for understanding and cleaning up a codebase.",
            "start_here": [
                "loom guide      — the full driving protocol (read first)",
                "loom schema     — the data model (node/edge types, states, vocabularies)",
                "loom status     — where the graph is now + the recommended next action",
                "loom next       — get the next thing to inspect",
                "loom next --all — the closeout view: every role queue + gaps in one list",
                "loom find <q>   — ask the map: keyword search over intents (with groundings)",
                "loom door \"<utterance>\" — route a user statement to its landing (matches + landing menu)",
                "loom session   — turn zero: user opened a session with no stated goal? ask them (offer menu)",
                "loom sync       — run after ANY code change (flags stale edges/verdicts/proofs)",
                "loom export --check — fail if the committed graph export went stale",
            ],
            "note": "Add --json to any command for machine-readable output. Every command has --help.",
        }));
    } else {
        println!("loom — externalized, falsifiable memory for understanding/cleaning a codebase.");
        println!();
        println!("Start here:");
        println!("  loom guide       learn the loop (read this first)");
        println!("  loom schema      the data model");
        println!("  loom status      where am I? what next?");
        println!("  loom next        get the next thing to inspect");
        println!("  loom next --all  closeout: every role queue + gaps in one list");
        println!("  loom find <q>    ask the map: keyword search over intents");
        println!("  loom door \"…\"   user said something? route it to its landing");
        println!("  loom session     user said \"use loom\" and nothing else? ask them");
        println!("  loom sync        run after ANY code change");
        println!();
        println!("Every command has --help; add --json for machine-readable output.");
    }
    Ok(())
}
