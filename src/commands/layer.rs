//! `loom layer` — the declared architecture layer order.
//!
//! Product domain and architecture layer are intentionally separate. Domains
//! help discovery/scoring ("auth", "billing"); layers express dependency
//! direction ("presentation", "application", "storage"). Only layers arm the
//! `layering_violation` smell.

use anyhow::Result;
use std::collections::{HashMap, HashSet};

use crate::cli::LayerCmd;
use crate::db::{ensure_initialized, GraphReadHandle, GraphReadRepository};
use crate::gate;
use crate::output::Printer;

/// One ranked "<n>. <layer>  <count> intent(s)" line of the declared layer
/// order — shared by `run_list_with_db` and `print_order_result` so the column
/// widths and wording stay identical between the two surfaces.
fn layer_usage_line(rank: usize, layer: &str, n: usize) -> String {
    format!("  {rank}. {layer:<24} {n:>3} intent(s)")
}

pub fn run(cmd: LayerCmd, printer: &Printer) -> Result<()> {
    run_inner(cmd, printer, false)
}

pub fn run_deprecated_domain_alias(cmd: crate::cli::DomainCmd, printer: &Printer) -> Result<()> {
    let cmd = match cmd {
        crate::cli::DomainCmd::Order { domains, author } => LayerCmd::Order {
            layers: domains,
            author,
        },
        crate::cli::DomainCmd::List => LayerCmd::List,
        crate::cli::DomainCmd::Clear => LayerCmd::Clear,
    };
    run_inner(cmd, printer, true)
}

fn run_inner(cmd: LayerCmd, printer: &Printer, deprecated_alias: bool) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    match cmd {
        LayerCmd::List => {
            let db = GraphReadHandle::open(&cwd)?;
            run_list_with_db(&db, printer, deprecated_alias)
        }
        LayerCmd::Order { layers, author } => {
            run_order_with_sqlite(&cwd, layers, author, printer, deprecated_alias)
        }
        LayerCmd::Clear => run_clear_with_sqlite(&cwd, printer, deprecated_alias),
    }
}

fn validate_layer_order(layers: &[String]) -> Result<()> {
    let mut seen: HashSet<&str> = HashSet::new();
    for layer in layers {
        if layer.trim().is_empty() {
            anyhow::bail!("Empty layer name in the order.");
        }
        if !seen.insert(layer.as_str()) {
            anyhow::bail!("Layer '{layer}' appears twice — each layer holds exactly one rank.");
        }
    }
    Ok(())
}

fn layer_usage(db: &dyn GraphReadRepository) -> Result<HashMap<String, usize>> {
    let mut usage: HashMap<String, usize> = HashMap::new();
    for i in db
        .list_intents(None, None)?
        .into_iter()
        .filter(|intent| intent.status != "deprecated")
    {
        if !i.layer.is_empty() {
            *usage.entry(i.layer).or_insert(0) += 1;
        }
    }
    Ok(usage)
}

fn alias_note(deprecated_alias: bool) -> Option<&'static str> {
    deprecated_alias.then_some(
        "`loom domain` is deprecated for architecture ordering; use `loom layer` and intent `--layer`. Product `--domain` no longer arms layering.",
    )
}

fn run_list_with_db(
    db: &dyn GraphReadRepository,
    printer: &Printer,
    deprecated_alias: bool,
) -> Result<()> {
    let usage = layer_usage(db)?;
    let alias_note = alias_note(deprecated_alias);
    let order = db.layer_order()?;
    let covered: HashSet<&str> = order.iter().map(String::as_str).collect();
    let mut uncovered: Vec<(&str, usize)> = usage
        .iter()
        .filter(|(layer, _)| !covered.contains(layer.as_str()))
        .map(|(layer, n)| (layer.as_str(), *n))
        .collect();
    uncovered.sort();
    if printer.json {
        printer.print_json(&serde_json::json!({
            "order": order,
            "uncovered": uncovered
                .iter()
                .map(|(layer, n)| serde_json::json!({"layer": layer, "intents": n}))
                .collect::<Vec<_>>(),
            "deprecated_alias": alias_note,
        }));
    } else if order.is_empty() {
        if let Some(note) = alias_note {
            println!("⚠ {note}");
        }
        println!("(no layer order declared — layering_violation is silent)");
        // Show the layers intents ALREADY carry, so you can declare an order over
        // them without grepping the json — the names below are the candidates.
        if uncovered.is_empty() {
            println!("  No intents carry a `--layer` label yet — set one with `loom intent add/update --layer <name>`.");
        } else {
            println!("  Layers already in use by intents (declare an order over these):");
            for (layer, n) in &uncovered {
                println!("  -  {layer:<24} {n:>3} intent(s)");
            }
        }
        println!("  → loom layer order <top> … <bottom>   (top layer first)");
    } else {
        if let Some(note) = alias_note {
            println!("⚠ {note}");
        }
        println!("Declared layer order (top first):");
        for (rank, layer) in order.iter().enumerate() {
            let n = usage.get(layer.as_str()).copied().unwrap_or(0);
            println!("{}", layer_usage_line(rank, layer, n));
        }
        if !uncovered.is_empty() {
            println!("\n  Exempt (in use, not in the order):");
            for (layer, n) in &uncovered {
                println!("  -  {layer:<24} {n:>3} intent(s)");
            }
        }
    }
    Ok(())
}

fn print_order_result(
    layers: &[String],
    previous: &[String],
    usage: &HashMap<String, usize>,
    alias_note: Option<&str>,
    printer: &Printer,
) {
    let unused: Vec<&str> = layers
        .iter()
        .filter(|layer| !usage.contains_key(layer.as_str()))
        .map(String::as_str)
        .collect();
    let next_step =
        "loom smells — layering_violation now judges every registered import against this order";
    if printer.json {
        printer.print_json(&serde_json::json!({
            "order": layers,
            "replaced": previous,
            "declared_but_unused": unused,
            "deprecated_alias": alias_note,
            "next_step": next_step,
        }));
    } else {
        if let Some(note) = alias_note {
            println!("⚠ {note}");
        }
        println!("✓ Layer order declared (top first):");
        for (rank, layer) in layers.iter().enumerate() {
            let n = usage.get(layer.as_str()).copied().unwrap_or(0);
            println!("{}", layer_usage_line(rank, layer, n));
        }
        if !previous.is_empty() && previous != layers {
            println!("  (replaced: {})", previous.join(" > "));
        }
        if !unused.is_empty() {
            println!(
                "  ⚠ no active intent carries layer(s): {} — the order enforces nothing there until intents declare them",
                unused.join(", ")
            );
        }
        println!("  → Next: {next_step}");
    }
}

fn print_clear_result(previous: &[String], alias_note: Option<&str>, printer: &Printer) {
    if printer.json {
        printer.print_json(&serde_json::json!({
            "cleared": previous,
            "deprecated_alias": alias_note,
            "next_step": "layering_violation is silent until an order is declared again",
        }));
    } else if previous.is_empty() {
        if let Some(note) = alias_note {
            println!("⚠ {note}");
        }
        println!("(no layer order was declared)");
    } else {
        if let Some(note) = alias_note {
            println!("⚠ {note}");
        }
        println!("✓ Layer order cleared (was: {})", previous.join(" > "));
        println!("  → layering_violation is silent until an order is declared again");
    }
}

fn run_order_with_sqlite(
    root: &std::path::Path,
    layers: Vec<String>,
    author: Option<String>,
    printer: &Printer,
    deprecated_alias: bool,
) -> Result<()> {
    // `loom layer order` with NO layers is a query-shaped mistake, not a declare:
    // it must NOT silently clear the order and report "✓ declared" (which would
    // contradict `loom smells`'s "no declared order"). Refuse with the right verb
    // BEFORE the lane gate, so the guidance shows regardless of role.
    if layers.is_empty() {
        anyhow::bail!(
            "No layers given — `loom layer order` DECLARES an order, it does not show one.\n\
             declare:  loom layer order <top> … <bottom>   (top layer first; REPLACES the current order)\n\
             show:     loom layer list\n\
             clear:    loom layer clear"
        );
    }
    gate::acting_in_lane(&gate::lane::SET_LAYER_ORDER, author.as_deref())?;
    validate_layer_order(&layers)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let usage = layer_usage(&store)?;
    let previous = store.set_layer_order(&layers)?;
    print_order_result(
        &layers,
        &previous,
        &usage,
        alias_note(deprecated_alias),
        printer,
    );
    Ok(())
}

fn run_clear_with_sqlite(
    root: &std::path::Path,
    printer: &Printer,
    deprecated_alias: bool,
) -> Result<()> {
    gate::acting_in_lane(&gate::lane::CLEAR_LAYER_ORDER, None)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let previous = store.set_layer_order(&[])?;
    print_clear_result(&previous, alias_note(deprecated_alias), printer);
    Ok(())
}
