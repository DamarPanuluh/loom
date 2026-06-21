//! `loom detect` — programmable repo introspection: stack + whether there's
//! existing source to map. Runs without an initialised graph (useful for
//! deciding greenfield vs brownfield before `loom init`).

use anyhow::Result;

use crate::output::Printer;

pub fn run(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let d = crate::repo::detect(&cwd)?;

    // Entry-point routing: `detect` runs BEFORE a graph exists, and every later
    // step needs one — so point at `loom init` FIRST, then the driving loop.
    // Carried in BOTH human and --json (parity): without it a cold --json agent
    // gets repo facts with no next action, and even the human form skipped `init`.
    let next_step = if d.has_source {
        "Existing code (brownfield): create the graph, then map it — `loom init .`, \
         read `loom guide`, then drive with `loom status` / `loom next`."
    } else {
        "No source yet (greenfield): create the graph, then design — `loom init .`, \
         then `loom guide --mode seed` to interview and `loom intent add --level system …`."
    };

    if printer.json {
        let mut v = serde_json::to_value(&d)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert(
                "next_step".to_string(),
                serde_json::Value::String(next_step.to_string()),
            );
        }
        printer.print_json(&v);
        return Ok(());
    }

    println!("── loom detect ──────────────────────────────────────────────────────");
    println!("  source files:   {}", d.source_files);
    println!(
        "  stacks:         {}",
        if d.stacks.is_empty() {
            "(none detected)".to_string()
        } else {
            d.stacks.join(", ")
        }
    );
    if d.top_languages.is_empty() {
        println!("  top languages:  (none)");
    } else {
        let langs: Vec<String> = d
            .top_languages
            .iter()
            .map(|l| format!("{} ({})", l.language, l.files))
            .collect();
        println!("  top languages:  {}", langs.join(", "));
    }
    println!("  suggested mode: {}", d.suggested_mode);
    if !d.recommended_packs.is_empty() {
        println!();
        println!("  quality packs for this repo kind (`loom rule seed <pack>`):");
        for p in &d.recommended_packs {
            println!("    {:<8} — {}", p.pack, p.reason);
        }
    }
    println!();
    println!("  → {next_step}");
    Ok(())
}
