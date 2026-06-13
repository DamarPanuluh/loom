use anyhow::Result;
use uuid::Uuid;

use crate::cli::IgnoreCmd;
use crate::db::queries::{insert_ignore, list_ignores};
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;
use crate::types::Ignore;

pub fn run(cmd: IgnoreCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match cmd {
        IgnoreCmd::Add {
            pattern,
            reason,
            author,
        } => {
            let ig = Ignore {
                id: Uuid::new_v4().to_string(),
                pattern,
                reason,
                author: crate::agent::acting(author.as_deref()),
                created_at: chrono::Utc::now().to_rfc3339(),
            };
            insert_ignore(&db, &ig)?;
            if printer.json {
                printer.print_json(&ig);
            } else {
                println!("✓ Ignore pattern added: {}", ig.pattern);
                println!("  reason: {}", ig.reason);
                println!(
                    "  → Affects `loom coverage`; files matching this pattern count as excluded."
                );
            }
        }
        IgnoreCmd::List => {
            let igs = list_ignores(&db)?;
            if printer.json {
                printer.print_json(&igs);
            } else if igs.is_empty() {
                println!(
                    "(no ignore patterns — every non-gitignored file must be mapped or excluded)"
                );
            } else {
                for i in &igs {
                    println!("  {:<30}  — {}  ({})", i.pattern, i.reason, i.author);
                }
            }
        }
    }
    Ok(())
}
