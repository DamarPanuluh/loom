//! `loom detect` — programmable repo introspection: stack + whether there's
//! existing source to map. Runs without an initialised graph (useful for
//! deciding greenfield vs brownfield before `loom init`).

use anyhow::Result;

use crate::output::Printer;

pub fn run(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let d = crate::repo::detect(&cwd)?;

    if printer.json {
        printer.print_json(&d);
        return Ok(());
    }

    println!("── loom detect ──────────────────────────────────────────────────────");
    println!("  source files:   {}", d.source_files);
    println!(
        "  stacks:         {}",
        if d.stacks.is_empty() { "(none detected)".to_string() } else { d.stacks.join(", ") }
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
    if d.has_source {
        println!("  → Existing code found — brownfield: map it (`loom guide`, then `loom next`).");
    } else {
        println!("  → No source yet — greenfield: design intents first (`loom guide`).");
    }
    Ok(())
}
