//! `loom interface` — read the externally callable surface inventory.

use anyhow::Result;

use crate::cli::InterfaceCmd;
use crate::db::ensure_initialized;
use crate::output::Printer;

pub fn run(cmd: InterfaceCmd, printer: &Printer) -> Result<()> {
    match cmd {
        InterfaceCmd::List => list(printer),
        InterfaceCmd::Gaps => gaps(printer),
        InterfaceCmd::Show { surface } => show(&surface, printer),
        InterfaceCmd::Remove { surface } => remove(&surface, printer),
    }
}

fn remove(key: &str, printer: &Printer) -> Result<()> {
    let mut store = open_store()?;
    store.ensure_owned("remove an interface surface")?;
    let surface = store.resolve_interface_surface(key)?;
    let removed = store.delete_interface_surface(&surface.id)?;
    if !removed {
        anyhow::bail!("Interface surface '{key}' could not be removed (already gone?).");
    }
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "removed": { "id": surface.id, "name": surface.name },
            "next_step": "loom interface gaps  (re-check the surface plane)",
        }));
        return Ok(());
    }
    println!(
        "✓ Removed interface surface '{}' (its CALLS edges went with it).",
        surface.name
    );
    Ok(())
}

fn open_store() -> Result<crate::db::sqlite::SqliteGraphStore> {
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))
}

fn list(printer: &Printer) -> Result<()> {
    let store = open_store()?;
    let surfaces = store.list_interface_surfaces()?;
    let calls = store.list_all_calls()?;

    if printer.json {
        let rows: Vec<_> = surfaces
            .iter()
            .map(|surface| {
                let callers: Vec<_> = calls
                    .iter()
                    .filter(|call| call.interface_id == surface.id)
                    .collect();
                serde_json::json!({
                    "id": surface.id,
                    "name": surface.name,
                    "kind": surface.surface_kind,
                    "method": surface.method,
                    "target": surface.target,
                    "description": surface.description,
                    "calls": callers.len(),
                    "sagas": unique_saga_count(&callers),
                })
            })
            .collect();
        printer.print_json(&serde_json::json!({
            "interfaces": rows,
            "total": rows.len(),
            "truncated": false,
        }));
        return Ok(());
    }

    if surfaces.is_empty() {
        println!("(no interface surfaces registered — `loom saga add <spec.yaml>` creates HTTP surfaces from steps)");
        return Ok(());
    }

    println!("  {:<18}  {:<8}  {:<42}  calls", "KIND", "METHOD", "TARGET");
    println!("  {}", "-".repeat(84));
    for surface in &surfaces {
        let count = calls
            .iter()
            .filter(|call| call.interface_id == surface.id)
            .count();
        println!(
            "  {:<18}  {:<8}  {:<42}  {}",
            surface.surface_kind, surface.method, surface.target, count
        );
    }
    Ok(())
}

fn gaps(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
    let gaps = crate::commands::populate::interface_gaps_with_repo(&store)?;

    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "interface_gaps": crate::commands::populate::interface_gaps_json(&gaps),
            "next_step": if gaps.is_pending() {
                "repair the interface plane, or adjudicate the boundary/interface meaning"
            } else {
                "no interface-plane gaps detected"
            },
        }));
        return Ok(());
    }

    if !gaps.is_pending() {
        println!("{}", crate::commands::populate::NO_INTERFACE_GAPS_MESSAGE);
        return Ok(());
    }

    println!("── Interface Gaps ──────────────────────────────────────────────");
    println!(
        "  {}",
        crate::commands::populate::interface_gap_totals_line(&gaps)
    );
    println!();
    for gap in &gaps.examples {
        println!("  - {}: {}", gap.kind, gap.summary);
        if !gap.surface.is_empty() {
            println!("    surface: {} ({})", gap.surface, gap.surface_id);
        }
        if !gap.intent.is_empty() {
            println!("    intent:  {} ({})", gap.intent, gap.intent_id);
        }
        if !gap.validation.is_empty() {
            println!("    proof:   {} ({})", gap.validation, gap.validation_id);
        }
        println!("    action:  {}", gap.suggested_action);
    }
    Ok(())
}

fn show(key: &str, printer: &Printer) -> Result<()> {
    let store = open_store()?;
    let surface = store.resolve_interface_surface(key)?;
    let calls = store.list_calls_for_interface(&surface.id)?;

    if printer.json {
        printer.print_json(&serde_json::json!({
            "interface": {
                "id": surface.id,
                "name": surface.name,
                "description": surface.description,
                "kind": surface.surface_kind,
                "method": surface.method,
                "target": surface.target,
                "created_at": surface.created_at,
                "updated_at": surface.updated_at,
            },
            "calls": calls.iter().map(|call| serde_json::json!({
                "id": call.id,
                "validation_id": call.validation_id,
                "saga": call.validation_name,
                "step_index": call.step_index,
                "step_name": call.step_name,
                "intent_id": call.intent_id,
                "intent": call.intent_name,
                "created_at": call.created_at,
            })).collect::<Vec<_>>(),
            "calls_total": calls.len(),
        }));
        return Ok(());
    }

    println!("{}  ({})", surface.name, surface.id);
    println!("  kind:   {}", surface.surface_kind);
    println!("  method: {}", empty_dash(&surface.method));
    println!("  target: {}", surface.target);
    if !surface.description.trim().is_empty() {
        println!("  desc:   {}", surface.description);
    }
    println!();
    if calls.is_empty() {
        println!("  (no saga steps call this surface)");
    } else {
        println!("  Called by:");
        for call in &calls {
            println!(
                "    - {} step {} '{}' → intent '{}'",
                call.validation_name, call.step_index, call.step_name, call.intent_name
            );
        }
    }
    Ok(())
}

fn unique_saga_count(calls: &[&crate::types::CallsEdge]) -> usize {
    calls
        .iter()
        .map(|call| call.validation_id.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len()
}

fn empty_dash(value: &str) -> &str {
    if value.is_empty() {
        "-"
    } else {
        value
    }
}
