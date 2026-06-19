use anyhow::Result;
use uuid::Uuid;

use crate::cli::NoteCmd;
use crate::commands::resolve::resolve_intent_with_db;
use crate::db::{ensure_initialized, GraphReadHandle, GraphReadRepository};
use crate::output::Printer;
use crate::types::{Note, NoteKind};

pub fn run(cmd: NoteCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    match cmd {
        NoteCmd::List {
            intent,
            edge,
            file,
            kind,
            for_role,
            limit,
        } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_list_with_db(&db, intent, edge, file, kind, for_role, limit, printer)
        }
        NoteCmd::Add {
            text,
            kind,
            intent,
            edge,
            file,
            smell,
            author,
            for_role,
        } => {
            ensure_initialized(&cwd)?;
            run_add_with_sqlite(
                &cwd, text, kind, intent, edge, file, smell, author, for_role, printer,
            )
        }
        NoteCmd::Prune {
            transitions,
            keep_per_target,
            set_cap,
            dry_run,
        } => {
            ensure_initialized(&cwd)?;
            run_prune_with_sqlite(
                &cwd,
                transitions,
                keep_per_target,
                set_cap,
                dry_run,
                printer,
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_add_with_sqlite(
    root: &std::path::Path,
    text: String,
    kind: String,
    intent: Option<String>,
    edge: Option<String>,
    file: Option<String>,
    smell: Option<String>,
    author: Option<String>,
    for_role: Option<String>,
    printer: &Printer,
) -> Result<()> {
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let note = prepare_add_note(
        &store,
        text,
        kind,
        intent,
        edge,
        file,
        smell,
        author,
        for_role,
        |edge_id| store.edge_id_exists(edge_id),
    )?;
    store.insert_note(&note)?;
    print_add_result(&note, printer);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn prepare_add_note(
    db: &dyn GraphReadRepository,
    text: String,
    kind: String,
    intent: Option<String>,
    edge: Option<String>,
    file: Option<String>,
    smell: Option<String>,
    author: Option<String>,
    for_role: Option<String>,
    edge_exists: impl Fn(&str) -> Result<bool>,
) -> Result<Note> {
    // Validate the kind against the vocabulary.
    kind.parse::<NoteKind>()
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    if [
        intent.is_some(),
        edge.is_some(),
        file.is_some(),
        smell.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count()
        > 1
    {
        anyhow::bail!(
            "A note targets an intent OR an edge OR a code file OR a smell finding, not several — \
             pass exactly one of `--intent <id>`, `--edge <id>`, `--file <path>`, \
             `--smell \"<kind>:<scope>\"` (or none for a graph-wide note)."
        );
    }
    let (target_kind, target_id) = match (intent, edge, file, smell) {
        (Some(i), _, _, _) => ("intent".to_string(), resolve_intent_with_db(db, &i)?),
        (_, Some(e), _, _) => {
            if !edge_exists(&e)? {
                anyhow::bail!(
                    "Edge '{}' not found. Use the derived edge id shown by `loom edge ...` commands, or run `loom doctor` to find dangling edge notes.",
                    e
                );
            }
            ("edge".to_string(), e)
        }
        (_, _, Some(f), _) => ("codefile".to_string(), resolve_codefile_id_with_db(db, &f)?),
        // A smell-finding identity is a synthetic key (`<kind>:<scope>`), not a
        // stored object, so it is stored verbatim — `loom smells` adjudication
        // matches it against the finding's own identity.
        (_, _, _, Some(s)) => {
            if s.trim().is_empty() {
                anyhow::bail!("--smell needs the finding identity loom smells prints (e.g. \"tangled_file:src/x.rs\").");
            }
            ("smell".to_string(), s)
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

    Ok(Note {
        id: Uuid::new_v4().to_string(),
        kind,
        text,
        author: crate::agent::acting(author.as_deref()),
        target_kind,
        target_id,
        audience,
        created_at: chrono::Utc::now().to_rfc3339(),
    })
}

fn print_add_result(note: &Note, printer: &Printer) {
    // A note is bookkeeping — it never moves graph phase — so this is a LIGHT
    // anchor: the `next_step` line/field without the pulse. An addressed note
    // (`--for <role>`) routes its reader to that lane's queue; a bare note
    // points back at the orientation surface.
    let next_step = if note.audience.is_empty() {
        "`loom note list` to review, or `loom next` to keep working.".to_string()
    } else {
        // The audience is a validated ROLE; map it to its queue's `loom next`
        // mode (builder→build, analyzer→discovery, …) — never assume role==mode.
        let mode = crate::gate::mode_for_role(&note.audience).unwrap_or("build");
        format!(
            "handed off to the {role} lane — that agent picks it up via `loom next --mode {mode}` (or `loom note list --for {role}`).",
            role = note.audience,
        )
    };
    if printer.json {
        let mut payload = serde_json::to_value(note).unwrap_or(serde_json::Value::Null);
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "next_step".to_string(),
                serde_json::Value::String(next_step),
            );
        }
        printer.print_json(&payload);
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
        println!("  → Next: {}", next_step);
    }
}

fn run_prune_with_sqlite(
    root: &std::path::Path,
    transitions: bool,
    keep_per_target: Option<usize>,
    set_cap: Option<usize>,
    dry_run: bool,
    printer: &Printer,
) -> Result<()> {
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let keep = keep_per_target
        .or(set_cap)
        .unwrap_or(store.transition_cap()?);
    let compact_requested = transitions || set_cap.is_some();
    let compact = compact_requested && (keep > 0 || keep_per_target == Some(0));

    let dangling = store.dangling_notes()?;
    let churn = if compact {
        store.prunable_transition_notes(keep)?
    } else {
        Vec::new()
    };

    if !dry_run {
        if let Some(cap) = set_cap {
            store.set_transition_cap(cap)?;
        }
        for note in dangling.iter().chain(churn.iter()) {
            store.delete_note_by_id(&note.id)?;
        }
    }

    let verb = if dry_run { "Would prune" } else { "Pruned" };
    let cap_off_noop = transitions && !compact && set_cap.is_none();
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
            "compaction_skipped": if cap_off_noop {
                Some("transition cap is 0/off and no --keep-per-target override was given")
            } else {
                None
            },
            "next_step": next_step,
        }));
    } else {
        if dangling.is_empty() && churn.is_empty() && !cap_off_noop {
            println!(
                "✓ Nothing to prune (no dangling notes{}).",
                if compact {
                    " or excess transition notes"
                } else {
                    ""
                }
            );
        }
        if !dangling.is_empty() {
            println!(
                "✓ {verb} {} dangling note(s) (targets no longer exist):",
                dangling.len()
            );
            for n in dangling.iter().take(20) {
                println!(
                    "  - {} [{}] on {} {}",
                    n.id, n.kind, n.target_kind, n.target_id
                );
            }
            if dangling.len() > 20 {
                if let Some(marker) =
                    crate::output::more_marker(dangling.len(), 20, "loom doctor --json")
                {
                    println!("  {marker}");
                }
            }
        }
        if !churn.is_empty() {
            println!(
                "✓ {verb} {} routine transition note(s), keeping {} newest per target (regression markers kept).",
                churn.len(),
                keep
            );
        } else if cap_off_noop {
            println!("✓ Transition compaction skipped: transition cap is 0/off; pass --keep-per-target N to prune this sweep.");
        }
        if let Some(cap) = set_cap {
            println!("  transition_cap set to {cap}");
        }
        if set_cap.is_some() || !(dangling.is_empty() && churn.is_empty()) {
            println!("  → Next: {next_step}");
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_list_with_db(
    db: &dyn GraphReadRepository,
    intent: Option<String>,
    edge: Option<String>,
    file: Option<String>,
    kind: Option<String>,
    for_role: Option<String>,
    limit: usize,
    printer: &Printer,
) -> Result<()> {
    if let Some(ref k) = kind {
        k.parse::<NoteKind>()
            .map_err(|e| anyhow::anyhow!("{}", e))?;
    }
    let intent = match intent {
        Some(i) => Some(resolve_intent_with_db(db, &i)?),
        None => None,
    };
    let file = match file {
        Some(f) => Some(resolve_codefile_id_with_db(db, &f)?),
        None => None,
    };
    let target = intent.or(edge).or(file);
    let mut notes = db.list_notes(target.as_deref(), kind.as_deref())?;
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
    Ok(())
}

fn resolve_codefile_id_with_db(db: &dyn GraphReadRepository, key: &str) -> Result<String> {
    db.query_snapshot()?
        .codefiles
        .into_iter()
        .find(|codefile| codefile.id == key || codefile.path == key)
        .map(|codefile| codefile.id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "CodeFile '{}' not found (by id or path).\nRun `loom codefile list` to see what is registered.",
                key
            )
        })
}
