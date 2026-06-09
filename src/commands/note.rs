use anyhow::Result;
use std::env;
use uuid::Uuid;

use crate::cli::NoteCmd;
use crate::db::queries::{insert_note, list_notes};
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;
use crate::types::{Note, NoteKind};

pub fn run(cmd: NoteCmd, printer: &Printer) -> Result<()> {
    let cwd = env::current_dir()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match cmd {
        NoteCmd::Add { text, kind, intent, edge, author } => {
            // Validate the kind against the vocabulary.
            kind.parse::<NoteKind>().map_err(|e| anyhow::anyhow!("{}", e))?;
            if intent.is_some() && edge.is_some() {
                anyhow::bail!("A note targets an intent OR an edge, not both.");
            }
            let (target_kind, target_id) = match (intent, edge) {
                (Some(i), _) => ("intent".to_string(), i),
                (_, Some(e)) => ("edge".to_string(), e),
                _            => ("none".to_string(), String::new()),
            };

            let note = Note {
                id:          Uuid::new_v4().to_string(),
                kind,
                text,
                author:      crate::agent::acting(author.as_deref()),
                target_kind,
                target_id,
                created_at:  chrono::Utc::now().to_rfc3339(),
            };
            insert_note(&db, &note)?;

            if printer.json {
                printer.print_json(&note);
            } else {
                println!("✓ Note added  [{}]", note.kind);
                if note.target_kind != "none" {
                    println!("  on {} {}", note.target_kind, note.target_id);
                }
                println!("  {}", note.text);
            }
        }

        NoteCmd::List { intent, edge, kind } => {
            if let Some(ref k) = kind {
                k.parse::<NoteKind>().map_err(|e| anyhow::anyhow!("{}", e))?;
            }
            let target = intent.or(edge);
            let notes = list_notes(&db, target.as_deref(), kind.as_deref())?;
            if printer.json {
                printer.print_json(&notes);
            } else if notes.is_empty() {
                println!("(no notes)");
            } else {
                for n in &notes {
                    let tgt = if n.target_kind == "none" {
                        "—".to_string()
                    } else {
                        let short = &n.target_id[..n.target_id.len().min(8)];
                        format!("{} {}", n.target_kind, short)
                    };
                    println!("  [{:<13}] {}", n.kind, n.text);
                    println!("      ({} · {})", n.author, tgt);
                }
            }
        }
    }
    Ok(())
}
