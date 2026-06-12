use anyhow::Result;
use crate::cli::{Cli, Command};
use crate::output::Printer;

pub mod batch;
pub mod cluster;
pub mod codefile;
pub mod coverage;
pub mod delegate;
pub mod detect;
pub mod doctor;
pub mod export;
pub mod find;
pub mod guide;
pub mod hypothesis;
pub mod import;
pub mod migrate;
pub mod hotspots;
pub mod ignore;
pub mod edge;
pub mod init;
pub mod intent;
pub mod next;
pub mod note;
pub mod report;
pub mod rule;
pub mod saga;
pub mod schema;
pub mod smells;
pub mod status;
pub mod sync;
pub mod validate;
pub mod vocab;
pub mod validation;

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
        // Noun-less verb stubs: teach the real invocation instead of letting
        // clap's similar-name tip mislead (`loom update` suggested 'guide').
        Command::Update { rest } => anyhow::bail!(
            "`update` lives under its noun:\n  \
             loom intent update {target} --description \"<new meaning>\" --reason \"<why it moved>\"   (meaning evolution; --reword when only the words change)\n  \
             loom validation update <id> --command \"<cmd>\"   (repair a proof's command)",
            target = rest.first().map(|s| format!("\"{s}\"")).unwrap_or_else(|| "<id>".into()),
        ),
        Command::Confirm { rest } => anyhow::bail!(
            "`confirm` lives under its noun:\n  \
             loom intent confirm {target}   (ratify the meaning; resets the align clock)\n  \
             loom intent confirm {target} --visibility internal   (machinery — stop interviewing it)",
            target = rest.first().map(|s| format!("\"{s}\"")).unwrap_or_else(|| "<id>".into()),
        ),
        Command::Ground { rest } => anyhow::bail!(
            "`ground` is a verdict on an edge:\n  \
             loom edge explore <intent-a> <intent-b> ground --criterion \"<falsifiable coexistence test>\" --confidence 0.9\n  \
             (issue / independent are the other verdicts; `loom next` serves the pair to inspect{hint})",
            hint = if rest.is_empty() { "" } else { " — pass BOTH intents, then the verdict" },
        ),
    }
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
        println!("  loom sync        run after ANY code change");
        println!();
        println!("Every command has --help; add --json for machine-readable output.");
    }
    Ok(())
}
