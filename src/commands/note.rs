use anyhow::Result;
use uuid::Uuid;

use crate::cli::NoteCmd;
use crate::db::queries::{insert_note, list_notes};
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;
use crate::types::{Note, NoteKind};

pub fn run(cmd: NoteCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match cmd {
        NoteCmd::Add { text, kind, intent, edge, author, for_role } => {
            // Validate the kind against the vocabulary.
            kind.parse::<NoteKind>().map_err(|e| anyhow::anyhow!("{}", e))?;
            if intent.is_some() && edge.is_some() {
                anyhow::bail!("A note targets an intent OR an edge, not both.");
            }
            let (target_kind, target_id) = match (intent, edge) {
                (Some(i), _) => ("intent".to_string(), crate::db::queries::resolve_intent(&db, &i)?),
                (_, Some(e)) => ("edge".to_string(), e),
                _            => ("none".to_string(), String::new()),
            };

            let audience = match &for_role {
                Some(r) => {
                    use crate::db::schema::role;
                    if ![role::BUILDER, role::ANALYZER, role::FIXER, role::VALIDATOR, role::QUALITY]
                        .contains(&r.as_str())
                    {
                        anyhow::bail!(
                            "--for must be a lane: builder | analyzer | fixer | validator | quality (got '{r}')."
                        );
                    }
                    r.clone()
                }
                None => String::new(),
            };
            let note = Note {
                id:          Uuid::new_v4().to_string(),
                kind,
                text,
                author:      crate::agent::acting(author.as_deref()),
                target_kind,
                target_id,
                audience,
                created_at:  chrono::Utc::now().to_rfc3339(),
            };
            insert_note(&db, &note)?;

            if printer.json {
                printer.print_json(&note);
            } else {
                println!("✓ Note added  [{}]{}", note.kind,
                    if note.audience.is_empty() { String::new() } else { format!("  → for {}", note.audience) });
                if note.target_kind != "none" {
                    println!("  on {} {}", note.target_kind, note.target_id);
                }
                println!("  {}", note.text);
            }
        }

        NoteCmd::List { intent, edge, kind, for_role } => {
            if let Some(ref k) = kind {
                k.parse::<NoteKind>().map_err(|e| anyhow::anyhow!("{}", e))?;
            }
            let intent = match intent {
                Some(i) => Some(crate::db::queries::resolve_intent(&db, &i)?),
                None => None,
            };
            let target = intent.or(edge);
            let mut notes = list_notes(&db, target.as_deref(), kind.as_deref())?;
            // The lane's inbox: only notes explicitly addressed to this role.
            if let Some(r) = &for_role {
                notes.retain(|n| &n.audience == r);
            }
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
                    let aud = if n.audience.is_empty() { String::new() } else { format!(" → for {}", n.audience) };
                    println!("  [{:<13}]{} {}", n.kind, aud, n.text);
                    println!("      ({} · {})", n.author, tgt);
                }
            }
        }
    }
    Ok(())
}
