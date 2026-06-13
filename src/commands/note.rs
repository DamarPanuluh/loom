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
        NoteCmd::Prune {
            transitions,
            keep_per_target,
            set_cap,
            dry_run,
        } => {
            // --set-cap persists the ceiling sync enforces, and implies a
            // compaction now. So either flag turns transition compaction on.
            let compact = transitions || set_cap.is_some();
            // The keep used for THIS sweep: explicit override → the cap being
            // set → the graph's current cap.
            let keep = keep_per_target
                .or(set_cap)
                .unwrap_or(crate::db::queries::get_transition_cap(&db)?);

            // Dangling: notes whose target id resolves to nothing (the remedy
            // doctor names). Always swept — history on live/retired nodes is
            // never touched.
            let dangling = crate::db::queries::dangling_notes(&db)?;
            let churn = if compact {
                crate::db::queries::prunable_transition_notes(&db, keep)?
            } else {
                Vec::new()
            };
            if !dry_run {
                if let Some(cap) = set_cap {
                    crate::db::queries::set_transition_cap(&db, cap)?;
                }
                if !(dangling.is_empty() && churn.is_empty()) {
                    crate::db::with_transaction(&db, || {
                        for n in dangling.iter().chain(churn.iter()) {
                            crate::db::queries::delete_note_by_id(&db, &n.id)?;
                        }
                        Ok(())
                    })?;
                }
            }
            let verb = if dry_run { "Would prune" } else { "Pruned" };
            // After a transition compaction the committed export drifted; point
            // there. Plain dangling prune stays anchored on the doctor re-check.
            let next_step = if compact && !churn.is_empty() {
                "`loom export` to refresh the committed graph, then `loom status`"
            } else {
                "`loom doctor` re-checks integrity"
            };
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok",
                    "pruned": dangling.len(),
                    "removed": dangling.iter().map(|n| serde_json::json!({
                        "id": n.id, "kind": n.kind,
                        "target_kind": n.target_kind, "target_id": n.target_id,
                    })).collect::<Vec<_>>(),
                    "transitions_pruned": churn.len(),
                    "keep_per_target": keep,
                    "cap_set": set_cap,
                    "dry_run": dry_run,
                    "next_step": next_step,
                }));
            } else {
                if let Some(cap) = set_cap {
                    let v = if dry_run { "Would set" } else { "Set" };
                    println!("✓ {v} transition cap to {cap} per target{}.",
                        if cap == 0 { " (off — strict append-only)" } else { " (`loom sync` enforces it)" });
                }
                if dangling.is_empty() && churn.is_empty() {
                    println!("✓ Nothing to prune (no dangling notes{}).",
                        if compact { " or compactable transition history" } else { "" });
                }
                if !dangling.is_empty() {
                    println!("✓ {verb} {} dangling note(s) (targets no longer exist):", dangling.len());
                    for n in dangling.iter().take(20) {
                        println!("    {} [{}] → missing {} '{}'", n.id, n.kind, n.target_kind, n.target_id);
                    }
                    if let Some(m) = crate::output::more_marker(dangling.len(), 20, "loom doctor --json") {
                        println!("    {m}");
                    }
                }
                if compact {
                    println!(
                        "✓ {verb} {} low-signal transition note(s) — kept the newest {keep} per target + every regression marker (smells + align unchanged).",
                        churn.len()
                    );
                }
                if set_cap.is_some() || !(dangling.is_empty() && churn.is_empty()) {
                    println!("  → Next: {next_step}");
                }
            }
        }

        NoteCmd::Add {
            text,
            kind,
            intent,
            edge,
            file,
            author,
            for_role,
        } => {
            // Validate the kind against the vocabulary.
            kind.parse::<NoteKind>()
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            if [intent.is_some(), edge.is_some(), file.is_some()]
                .iter()
                .filter(|b| **b)
                .count()
                > 1
            {
                anyhow::bail!("A note targets an intent OR an edge OR a code file, not several.");
            }
            let (target_kind, target_id) = match (intent, edge, file) {
                (Some(i), _, _) => (
                    "intent".to_string(),
                    crate::db::queries::resolve_intent(&db, &i)?,
                ),
                (_, Some(e), _) => {
                    if !crate::db::queries::edge_id_exists(&db, &e)? {
                        anyhow::bail!(
                            "Edge '{}' not found. Use the derived edge id shown by `loom edge ...` commands, or run `loom doctor` to find dangling edge notes.",
                            e
                        );
                    }
                    ("edge".to_string(), e)
                }
                (_, _, Some(f)) => {
                    let cf = crate::db::queries::get_codefile_by_id_or_path(&db, &f)?
                        .ok_or_else(|| anyhow::anyhow!(
                            "CodeFile '{}' not found (by id or path).\nRun `loom codefile list` to see what is registered.", f
                        ))?;
                    ("codefile".to_string(), cf.id)
                }
                _ => ("none".to_string(), String::new()),
            };

            let audience = match &for_role {
                Some(r) => {
                    // The canonical lane set (gate.rs reads the same constant) —
                    // tracks automatically if a 6th role is ever added.
                    if !crate::db::schema::ROLES.contains(&r.as_str()) {
                        anyhow::bail!(
                            "--for must be a lane: {roles} (got '{r}').",
                            roles = crate::db::schema::ROLES.join(" | ")
                        );
                    }
                    r.clone()
                }
                None => String::new(),
            };
            let note = Note {
                id: Uuid::new_v4().to_string(),
                kind,
                text,
                author: crate::agent::acting(author.as_deref()),
                target_kind,
                target_id,
                audience,
                created_at: chrono::Utc::now().to_rfc3339(),
            };
            insert_note(&db, &note)?;

            if printer.json {
                printer.print_json(&note);
            } else {
                println!(
                    "✓ Note added  [{}]{}",
                    note.kind,
                    if note.audience.is_empty() {
                        String::new()
                    } else {
                        format!("  → for {}", note.audience)
                    }
                );
                if note.target_kind != "none" {
                    println!("  on {} {}", note.target_kind, note.target_id);
                }
                println!("  {}", note.text);
            }
        }

        NoteCmd::List {
            intent,
            edge,
            file,
            kind,
            for_role,
            limit,
        } => {
            if let Some(ref k) = kind {
                k.parse::<NoteKind>()
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
            }
            let intent = match intent {
                Some(i) => Some(crate::db::queries::resolve_intent(&db, &i)?),
                None => None,
            };
            let file = match file {
                Some(f) => Some(
                    crate::db::queries::get_codefile_by_id_or_path(&db, &f)?
                        .ok_or_else(|| anyhow::anyhow!(
                            "CodeFile '{}' not found (by id or path).\nRun `loom codefile list` to see what is registered.", f
                        ))?
                        .id,
                ),
                None => None,
            };
            let target = intent.or(edge).or(file);
            let mut notes = list_notes(&db, target.as_deref(), kind.as_deref())?;
            // The lane's inbox: only notes explicitly addressed to this role.
            if let Some(r) = &for_role {
                notes.retain(|n| &n.audience == r);
            }
            // Newest LAST in `notes`; keep the tail — the live context.
            let total = notes.len();
            if limit > 0 && total > limit {
                notes.drain(..total - limit);
            }
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "notes": notes,
                    "total": total,
                    "truncated": total > notes.len(),
                }));
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
                    let aud = if n.audience.is_empty() {
                        String::new()
                    } else {
                        format!(" → for {}", n.audience)
                    };
                    println!("  [{:<13}]{} {}", n.kind, aud, n.text);
                    println!("      ({} · {})", n.author, tgt);
                }
                if let Some(m) =
                    crate::output::more_marker(total, notes.len(), "`loom note list --limit 0`")
                {
                    println!("  {}", m);
                }
            }
        }
    }
    Ok(())
}
