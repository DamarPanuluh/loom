//! `loom domain` — the declared layer order (which domains sit above which).
//!
//! Imports are directed facts the physical plane always carried; a LAYERING
//! violation only exists relative to a declared order. This command is where
//! that order is declared — replace-only and atomic (one list property on the
//! meta sentinel) — so the `layering_violation` smell stays pure mechanical
//! computation. Undeclared domains are exempt: declare only what you mean to
//! enforce, the same "positive evidence only" stance as tags.

use anyhow::Result;
use std::collections::{HashMap, HashSet};

use crate::cli::DomainCmd;
use crate::db::queries::{get_domain_order, list_active_intents, set_domain_order};
use crate::db::schema::role;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::gate;
use crate::output::Printer;

pub fn run(cmd: DomainCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    // Per-domain active-intent counts — the in-band reality check: an order
    // naming domains nothing uses enforces nothing.
    let mut usage: HashMap<String, usize> = HashMap::new();
    for i in list_active_intents(&db)? {
        if !i.domain.is_empty() {
            *usage.entry(i.domain).or_insert(0) += 1;
        }
    }

    match cmd {
        DomainCmd::Order { domains, author } => {
            gate::acting_in_lane(
                "declare the domain layer order",
                &[role::BUILDER],
                author.as_deref(),
            )?;
            let mut seen: HashSet<&str> = HashSet::new();
            for d in &domains {
                if d.trim().is_empty() {
                    anyhow::bail!("Empty domain name in the order.");
                }
                if !seen.insert(d.as_str()) {
                    anyhow::bail!(
                        "Domain '{d}' appears twice — each domain holds exactly one rank."
                    );
                }
            }
            let previous = get_domain_order(&db)?;
            set_domain_order(&db, &domains)?;
            let unused: Vec<&str> = domains
                .iter()
                .filter(|d| !usage.contains_key(d.as_str()))
                .map(String::as_str)
                .collect();
            let next_step =
                "loom smells — layering_violation now judges every registered import against this order";
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "order": domains,
                    "replaced": previous,
                    "declared_but_unused": unused,
                    "next_step": next_step,
                }));
            } else {
                println!("✓ Domain layer order declared (top first):");
                for (rank, d) in domains.iter().enumerate() {
                    let n = usage.get(d.as_str()).copied().unwrap_or(0);
                    println!("  {rank}. {d:<24} {n:>3} intent(s)");
                }
                if !previous.is_empty() && previous != domains {
                    println!("  (replaced: {})", previous.join(" > "));
                }
                if !unused.is_empty() {
                    println!(
                        "  ⚠ no active intent carries domain(s): {} — the order enforces nothing there until intents declare them",
                        unused.join(", ")
                    );
                }
                println!("  → Next: {next_step}");
            }
        }

        DomainCmd::List => {
            let order = get_domain_order(&db)?;
            let covered: HashSet<&str> = order.iter().map(String::as_str).collect();
            let mut uncovered: Vec<(&str, usize)> = usage
                .iter()
                .filter(|(d, _)| !covered.contains(d.as_str()))
                .map(|(d, n)| (d.as_str(), *n))
                .collect();
            uncovered.sort();
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "order": order,
                    "uncovered": uncovered
                        .iter()
                        .map(|(d, n)| serde_json::json!({"domain": d, "intents": n}))
                        .collect::<Vec<_>>(),
                }));
            } else if order.is_empty() {
                println!("(no layer order declared — layering_violation is silent)");
                println!("  → loom domain order <top> … <bottom>   (top layer first)");
            } else {
                println!("Declared layer order (top first):");
                for (rank, d) in order.iter().enumerate() {
                    let n = usage.get(d.as_str()).copied().unwrap_or(0);
                    println!("  {rank}. {d:<24} {n:>3} intent(s)");
                }
                if !uncovered.is_empty() {
                    println!("\n  Exempt (in use, not in the order):");
                    for (d, n) in &uncovered {
                        println!("  -  {d:<24} {n:>3} intent(s)");
                    }
                }
            }
        }

        DomainCmd::Clear => {
            gate::acting_in_lane("clear the domain layer order", &[role::BUILDER], None)?;
            let previous = get_domain_order(&db)?;
            set_domain_order(&db, &[])?;
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "cleared": previous,
                    "next_step": "layering_violation is silent until an order is declared again",
                }));
            } else if previous.is_empty() {
                println!("(no layer order was declared)");
            } else {
                println!("✓ Layer order cleared (was: {})", previous.join(" > "));
                println!("  → layering_violation is silent until an order is declared again");
            }
        }
    }
    Ok(())
}
