use anyhow::Result;
use uuid::Uuid;

use crate::cli::{IntentCmd, SourceCmd, TagCmd};
use crate::commands::resolve::resolve_intent_with_db;
use crate::db::{ensure_initialized, GraphReadHandle, GraphReadRepository};
use crate::gate;
use crate::output::{fmt_edge_row, fmt_intent, fmt_intent_row, Printer};
use crate::types::Intent;

pub fn run(cmd: IntentCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    match cmd {
        IntentCmd::List {
            status,
            level,
            limit,
        } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_list_with_db(&db, status, level, limit, printer)
        }
        IntentCmd::Show { id } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_show_with_db(&db, id, printer)
        }
        IntentCmd::Source { subcommand } => {
            ensure_initialized(&cwd)?;
            run_source_with_sqlite(&cwd, subcommand, printer)
        }
        IntentCmd::Tag { subcommand } => {
            ensure_initialized(&cwd)?;
            run_tag_with_sqlite(&cwd, subcommand, printer)
        }
        cmd => {
            ensure_initialized(&cwd)?;
            run_with_sqlite(&cwd, cmd, printer)
        }
    }
}

fn run_source_with_sqlite(
    root: &std::path::Path,
    subcommand: SourceCmd,
    printer: &Printer,
) -> Result<()> {
    gate::acting_in_lane(&gate::lane::INTENT_SOURCE, None)?;
    let now = chrono::Utc::now().to_rfc3339();
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let snapshot = store.query_snapshot()?;

    match subcommand {
        SourceCmd::Add { id, path } => {
            let id = crate::db::queries::resolve_intent_from_snapshot(&snapshot, &id)?;
            let Some(parsed) = store.add_source_ref(&id, &path, &now)? else {
                anyhow::bail!(
                    "Intent '{}' not found. Run `loom intent list` (or `loom find \"<words>\").",
                    id
                );
            };
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "id": id, "added": path,
                    "source_refs": parsed,
                    "next_step": format!("`loom intent show {id}`"),
                }));
            } else {
                println!("✓ Source ref added to intent {id}: {path}");
                println!("  → Next: `loom intent show {id}`");
            }
        }
        SourceCmd::Remove { id, path } => {
            let id = crate::db::queries::resolve_intent_from_snapshot(&snapshot, &id)?;
            match store.remove_source_ref(&id, &path, &now)? {
                None => anyhow::bail!(
                    "Intent '{}' not found. Run `loom intent list` (or `loom find \"<words>\").",
                    id
                ),
                Some(false) => anyhow::bail!(
                    "Intent {} has no source ref '{}' — `loom intent show {}` lists them.",
                    id,
                    path,
                    id
                ),
                Some(true) => {
                    if printer.json {
                        printer.print_json(&serde_json::json!({
                            "status": "ok", "id": id, "removed": path,
                            "next_step": format!("`loom intent show {id}`"),
                        }));
                    } else {
                        println!("✓ Source ref removed from intent {id}: {path}");
                        println!("  → Next: `loom intent show {id}`");
                    }
                }
            }
        }
    }
    Ok(())
}

fn run_tag_with_sqlite(
    root: &std::path::Path,
    subcommand: TagCmd,
    printer: &Printer,
) -> Result<()> {
    gate::acting_in_lane(&gate::lane::INTENT_TAG, None)?;
    let now = chrono::Utc::now().to_rfc3339();
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let snapshot = store.query_snapshot()?;
    let terms = store.list_vocab_terms()?;
    let active_intents: Vec<Intent> = snapshot
        .intents
        .iter()
        .filter(|intent| intent.status != "deprecated")
        .cloned()
        .collect();

    match subcommand {
        TagCmd::Add { id, term } => {
            let id = crate::db::queries::resolve_intent_from_snapshot(&snapshot, &id)?;
            let intent = snapshot
                .intents
                .iter()
                .find(|intent| intent.id == id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Intent '{}' not found. Run `loom intent list` (or `loom find \"<words>\"`).",
                        id
                    )
                })?;
            let mut tags = intent.tags.clone();
            tags.push(term);
            let tags = crate::commands::vocab::validate_tags_from_registry(
                &tags,
                &terms,
                &active_intents,
            )?;
            store.set_intent_tags(&id, tags.clone(), &now)?;
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "id": id, "tags": tags,
                    "next_step": format!("`loom intent show {id}`"),
                }));
            } else {
                println!("✓ Intent {id} tagged: [{}]", tags.join(", "));
                println!("  → Next: `loom intent show {id}`");
            }
        }
        TagCmd::Remove { id, term } => {
            let id = crate::db::queries::resolve_intent_from_snapshot(&snapshot, &id)?;
            let intent = snapshot
                .intents
                .iter()
                .find(|intent| intent.id == id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Intent '{}' not found. Run `loom intent list` (or `loom find \"<words>\"`).",
                        id
                    )
                })?;
            let term = crate::db::queries::normalize_term(&term)?;
            let mut tags = intent.tags.clone();
            let before = tags.len();
            tags.retain(|tag| *tag != term);
            if tags.len() == before {
                anyhow::bail!(
                    "Intent {} carries no tag '{}' — `loom intent show {}` lists them.",
                    id,
                    term,
                    id
                );
            }
            store.set_intent_tags(&id, tags.clone(), &now)?;
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "id": id, "removed": term, "tags": tags,
                    "next_step": format!("`loom intent show {id}`"),
                }));
            } else {
                println!("✓ Tag '{term}' removed from intent {id}");
                println!("  → Next: `loom intent show {id}`");
            }
        }
    }
    Ok(())
}

fn run_with_sqlite(root: &std::path::Path, cmd: IntentCmd, printer: &Printer) -> Result<()> {
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    match cmd {
        IntentCmd::Add {
            name,
            description,
            level,
            domain,
            layer,
            aspect,
            lifecycle,
            sources,
            tags,
            visibility,
            boundary,
        } => {
            gate::acting_in_lane(&gate::lane::ADD_INTENT, None)?;
            let level = level
                .parse::<crate::types::AbstractionLevel>()
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            lifecycle
                .parse::<crate::types::LifecycleState>()
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            if name.trim().is_empty() {
                anyhow::bail!(
                    "--name must not be empty. State the responsibility this intent owns."
                );
            }
            if description.trim().is_empty() {
                anyhow::bail!("--description must not be empty. State the observable behavior or design responsibility this intent captures.");
            }
            if lifecycle != "implemented" {
                store.ensure_owned(&format!(
                    "declare a '{lifecycle}' intent (a promise to change the code)"
                ))?;
            }
            if !matches!(visibility.as_str(), "" | "user_visible" | "internal") {
                anyhow::bail!(
                    "Invalid --visibility '{visibility}'. Valid: user_visible | internal."
                );
            }
            if !matches!(boundary.as_str(), "" | "inbound" | "outbound") {
                anyhow::bail!("Invalid --boundary '{boundary}'. Valid: inbound | outbound.");
            }

            let snapshot = store.query_snapshot()?;
            let terms = store.list_vocab_terms()?;
            let tags = crate::commands::vocab::validate_tags_from_registry(
                &tags,
                &terms,
                &snapshot.intents,
            )?;
            let has_tags = !tags.is_empty();
            let source_refs = sources.clone();
            let now = chrono::Utc::now().to_rfc3339();
            let id = Uuid::new_v4().to_string();

            let intent = Intent {
                id: id.clone(),
                name: name.clone(),
                description,
                abstraction_level: level.to_string(),
                domain,
                layer,
                source_refs,
                status: "proposed".to_string(),
                aspect,
                tags,
                visibility,
                boundary,
                lifecycle,
                created_at: now.clone(),
                updated_at: now,
            };
            store.insert_intent(&intent)?;

            let is_root = intent.abstraction_level == "system";
            let tree_step = if is_root {
                format!("Decompose it: add child intents, then link with `loom edge hierarchy {} <child-id>` (this is the tree's root).", id)
            } else {
                format!("Attach it to the tree: `loom edge hierarchy <parent-id> {}` (every non-system intent needs exactly one parent).", id)
            };
            let registry_size = terms.len();
            let tag_step = (!has_tags && registry_size > 0).then(|| format!(
                "Optional now, audit-relevant once grounded: tag it from the {registry_size}-term vocabulary (`loom vocab list`, then `loom intent tag add {id} <term>`) so duplicate-responsibility detection has its strongest signal."
            ));

            if printer.json {
                let mut v = serde_json::to_value(&intent)?;
                if let Some(obj) = v.as_object_mut() {
                    let mut steps = vec![
                        tree_step,
                        "Ground it to code: `loom edge implement <intent> <codefile> --locator \"<symbol>\"` (the symbol as it appears in the file — e.g. `def foo`, `fn foo`; required for leaf intents).".to_string(),
                        "Relate it to other intents — `loom next` will surface unexplored pairs (optional).".to_string(),
                        "If this is a feature, add its sad/fallback siblings (--aspect).".to_string(),
                    ];
                    if let Some(ts) = &tag_step {
                        steps.push(ts.clone());
                    }
                    obj.insert("next_steps".to_string(), serde_json::json!(steps));
                }
                printer.print_json(&v);
            } else {
                println!("✓ Intent created");
                println!("{}", fmt_intent(&intent));
                println!("  → Next: {}", tree_step);
                println!("          then ground it: `loom edge implement {} <codefile> --locator \"<symbol>\"` (symbol as written in the file).", id);
                if let Some(ts) = &tag_step {
                    println!("          {ts}");
                }
            }
        }

        IntentCmd::Confirm { id, visibility } => {
            let by = gate::acting_in_lane(&gate::lane::CONFIRM_INTENT, None)?;
            let id = resolve_intent_with_db(&store, &id)?;
            let intent = store.get_intent(&id)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
                    id
                )
            })?;
            if intent.status == "deprecated" {
                anyhow::bail!(
                    "Intent '{}' is retired (status=deprecated). Retirement is permanent history: create a successor intent and link the lineage instead of confirming it.",
                    id
                );
            }
            if let Some(v) = visibility.as_deref() {
                if !matches!(v, "user_visible" | "internal") {
                    anyhow::bail!("Invalid --visibility '{v}'. Valid: user_visible | internal.");
                }
            }
            let now = chrono::Utc::now().to_rfc3339();
            if !store.confirm_intent(&id, visibility.as_deref(), &by, &now)? {
                anyhow::bail!(
                    "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
                    id
                );
            }
            let confirmed_msg = match visibility.as_deref() {
                Some("internal") => "confirmed + ruled internal — out of the align interview until its meaning is redefined",
                Some("user_visible") => "confirmed + ruled user-visible",
                _ => "confirmed",
            };
            let next_step = "`loom next` serves the next item";
            if printer.json {
                let mut payload =
                    serde_json::json!({"status":"ok","id":id,"new_status":"confirmed"});
                if let Some(v) = visibility.as_deref() {
                    payload["visibility"] = serde_json::json!(v);
                }
                payload["next_step"] = serde_json::json!(next_step);
                printer.print_json(&payload);
            } else {
                println!("✓ Intent {} {}", id, confirmed_msg);
                println!("  → Next: {next_step}");
            }
        }

        IntentCmd::Update {
            id,
            name,
            layer,
            boundary,
            description,
            reword,
            reason,
            extra,
        } => {
            if let Some(first) = extra.first() {
                anyhow::bail!(
                    "Unexpected positional text {first:?} — new wording travels through flags:\n  \
                     loom intent update \"{id}\" --description \"<new meaning>\" --reason \"<why it moved>\"\n  \
                     (--reword when only the words change; --name \"<new>\" for a cosmetic rename)"
                );
            }
            if reason.trim().is_empty() {
                anyhow::bail!("--reason is required: the recorded WHY behind the change.");
            }
            let by = gate::acting_in_lane(&gate::lane::UPDATE_INTENT, None)?;
            store.ensure_owned(
                "update an intent (the design decision belongs to the graph's owners)",
            )?;
            gate::require_substantive("reason", &reason, "why the meaning moved")?;
            let id = resolve_intent_with_db(&store, &id)?;
            let intent = store.get_intent(&id)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
                    id
                )
            })?;
            if intent.status == "deprecated" {
                anyhow::bail!(
                    "Intent '{}' is retired (status=deprecated). Create a successor intent instead of rewriting it.",
                    id
                );
            }
            let new_name = name
                .as_deref()
                .filter(|candidate| *candidate != intent.name.as_str());
            let new_layer = layer
                .as_deref()
                .filter(|candidate| *candidate != intent.layer.as_str());
            let new_boundary = boundary
                .as_deref()
                .filter(|candidate| *candidate != intent.boundary.as_str());
            let new_desc = description
                .as_deref()
                .filter(|candidate| *candidate != intent.description.as_str());
            if new_name.is_none()
                && new_layer.is_none()
                && new_boundary.is_none()
                && new_desc.is_none()
            {
                anyhow::bail!(
                    "Nothing to change: pass --name, --layer, --boundary, and/or --description with a value that differs from the current one (`loom intent show {}` prints them).",
                    id
                );
            }
            let now = chrono::Utc::now().to_rfc3339();
            store.update_intent_meaning(&id, new_name, new_desc, &now)?;
            if let Some(layer) = new_layer {
                store.set_intent_layer(&id, layer, &now)?;
                store.insert_note(&crate::types::Note {
                    id: Uuid::new_v4().to_string(),
                    kind: "decision".into(),
                    text: format!(
                        "layer changed: '{}' -> '{}' ({})",
                        if intent.layer.is_empty() {
                            "<undeclared>"
                        } else {
                            &intent.layer
                        },
                        if layer.is_empty() {
                            "<undeclared>"
                        } else {
                            layer
                        },
                        reason
                    ),
                    author: by.clone(),
                    target_kind: "intent".into(),
                    target_id: id.clone(),
                    audience: String::new(),
                    created_at: now.clone(),
                })?;
            }
            if let Some(boundary) = new_boundary {
                store.set_intent_boundary(&id, boundary, &now)?;
                store.insert_note(&crate::types::Note {
                    id: Uuid::new_v4().to_string(),
                    kind: "decision".into(),
                    text: format!(
                        "boundary changed: '{}' -> '{}' ({})",
                        if intent.boundary.is_empty() {
                            "<internal>"
                        } else {
                            &intent.boundary
                        },
                        if boundary.is_empty() {
                            "<internal>"
                        } else {
                            boundary
                        },
                        reason
                    ),
                    author: by.clone(),
                    target_kind: "intent".into(),
                    target_id: id.clone(),
                    audience: String::new(),
                    created_at: now.clone(),
                })?;
            }
            if let Some(n) = new_name {
                store.insert_note(&crate::types::Note {
                    id: Uuid::new_v4().to_string(),
                    kind: "decision".into(),
                    text: format!("renamed: '{}' -> '{}' ({})", intent.name, n, reason),
                    author: by.clone(),
                    target_kind: "intent".into(),
                    target_id: id.clone(),
                    audience: String::new(),
                    created_at: now.clone(),
                })?;
            }
            let mut ripple = None;
            if let Some(_d) = new_desc {
                if reword {
                    store.insert_note(&crate::types::Note {
                        id: Uuid::new_v4().to_string(),
                        kind: "decision".into(),
                        text: format!("reworded: {}\nwas: {}", reason, intent.description),
                        author: by.clone(),
                        target_kind: "intent".into(),
                        target_id: id.clone(),
                        audience: String::new(),
                        created_at: now.clone(),
                    })?;
                } else {
                    store.insert_note(&crate::types::Note {
                        id: Uuid::new_v4().to_string(),
                        kind: "decision".into(),
                        text: format!("redefined: {}\nwas: {}", reason, intent.description),
                        author: by.clone(),
                        target_kind: "intent".into(),
                        target_id: id.clone(),
                        audience: String::new(),
                        created_at: now.clone(),
                    })?;
                    if !intent.visibility.is_empty() {
                        store.set_intent_visibility(&id, "", &now)?;
                    }
                    ripple = Some(store.ripple_intent_redefinition(
                        &id,
                        new_name.unwrap_or(&intent.name),
                        &now,
                    )?);
                }
            }
            let rippled = ripple.as_ref().is_some_and(|r| {
                r.relates_to_flagged
                    + r.governs_flagged
                    + r.targets_flagged
                    + r.implements_flagged
                    + r.validations_invalidated
                    > 0
            });
            let next_step = if rippled {
                "`loom next --mode fix` re-inspects staled claims; `loom next --mode quality` re-earns flagged quality green; `loom validate` re-runs invalidated proofs."
            } else {
                "`loom next` serves the next item"
            };
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "id": id,
                    "renamed": new_name.is_some(),
                    "layer_changed": new_layer.is_some(),
                    "boundary_changed": new_boundary.is_some(),
                    "redefined": new_desc.is_some() && !reword,
                    "reworded": new_desc.is_some() && reword,
                    "visibility_cleared": new_desc.is_some() && !reword && !intent.visibility.is_empty(),
                    "ripple": ripple,
                    "next_step": next_step,
                }));
            } else {
                match (new_name, new_desc) {
                    (_, Some(_)) if reword => println!(
                        "✓ Intent {id} reworded (same concept, clearer words — no ripple)."
                    ),
                    (Some(n), Some(_)) => println!("✓ Intent {id} renamed to '{n}' and redefined."),
                    (Some(n), None) => {
                        println!("✓ Intent {id} renamed to '{n}' (cosmetic — no ripple).")
                    }
                    (None, Some(_)) => println!("✓ Intent {id} redefined."),
                    (None, None) => {
                        if let Some(layer) = new_layer {
                            println!("✓ Intent {id} layer → '{}' (metadata — no ripple).", layer);
                        }
                        if let Some(boundary) = new_boundary {
                            println!(
                                "✓ Intent {id} boundary → '{}' (metadata — no ripple).",
                                if boundary.is_empty() {
                                    "<internal>"
                                } else {
                                    boundary
                                }
                            );
                        }
                    }
                }
                if new_desc.is_some() && !reword && !intent.visibility.is_empty() {
                    println!(
                        "  visibility ruling '{}' cleared — the new meaning's audience is unknown; the align interview re-triages it.",
                        intent.visibility
                    );
                }
                if let Some(r) = &ripple {
                    if rippled {
                        println!("  SEMANTIC RIPPLE (claims earned against the old wording):");
                        if r.relates_to_flagged > 0 {
                            println!(
                                "    · {} RELATES_TO verdict(s) → needs_reverification",
                                r.relates_to_flagged
                            );
                        }
                        if r.implements_flagged > 0 {
                            println!(
                                "    · {} IMPLEMENTS grounding(s) → needs_reverification",
                                r.implements_flagged
                            );
                        }
                        if r.governs_flagged > 0 {
                            println!(
                                "    · {} GOVERNS verdict(s) → needs_reverification",
                                r.governs_flagged
                            );
                        }
                        if r.targets_flagged > 0 {
                            println!(
                                "    · {} hypothesis TARGETS edge(s) → needs_reverification",
                                r.targets_flagged
                            );
                        }
                        if r.validations_invalidated > 0 {
                            println!(
                                "    · {} validation(s) → not_run",
                                r.validations_invalidated
                            );
                        }
                    } else {
                        println!("  No earned claims touched this intent — nothing to re-verify.");
                    }
                }
                println!("  → Next: {next_step}");
            }
        }

        IntentCmd::Mark {
            id,
            lifecycle,
            reason,
        } => {
            let by = gate::acting_in_lane(&gate::lane::SET_LIFECYCLE, None)?;
            store.ensure_owned(
                "change an intent's lifecycle (a claim about building/changing the code)",
            )?;
            lifecycle
                .parse::<crate::types::LifecycleState>()
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            let id = resolve_intent_with_db(&store, &id)?;
            let now = chrono::Utc::now().to_rfc3339();
            if !store.set_intent_lifecycle(&id, &lifecycle, &by, &now)? {
                anyhow::bail!(
                    "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
                    id
                );
            }
            if let Some(ref r) = reason {
                store.insert_note(&crate::types::Note {
                    id: Uuid::new_v4().to_string(),
                    kind: "decision".to_string(),
                    text: format!("lifecycle → {}: {}", lifecycle, r),
                    author: by.clone(),
                    target_kind: "intent".to_string(),
                    target_id: id.clone(),
                    audience: String::new(),
                    created_at: now.clone(),
                })?;
            }
            let next_step = match lifecycle.as_str() {
                "planned" | "needs_change" => "`loom next --mode build` will surface it.",
                "implemented" => {
                    "if this leaf is fully grounded, prove it: `loom next --mode validate`"
                }
                "deferred" => {
                    "parked: out of the build queue and never blocks a roll-up. Record WHY with `loom note add --intent <id> --kind decision`; resume with `--lifecycle planned`."
                }
                _ => "`loom next` serves the next item",
            };
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "id": id, "lifecycle": lifecycle,
                    "next_step": next_step,
                }));
            } else {
                println!("✓ Intent {} → lifecycle '{}'", id, lifecycle);
                println!("  → Next: {next_step}");
            }
        }

        IntentCmd::Delete { id } => {
            gate::acting_in_lane(&gate::lane::DELETE_INTENT, None)?;
            let id = resolve_intent_with_db(&store, &id)?;
            if !store.delete_intent(&id)? {
                anyhow::bail!(
                    "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
                    id
                );
            }
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "id": id, "deleted": true,
                    "next_step": "`loom status` re-checks the compass",
                }));
            } else {
                println!("✓ Intent {} deleted (with its edges and notes).", id);
                println!("  → Next: `loom status` re-checks the compass");
            }
        }

        IntentCmd::Retire {
            id,
            reason,
            replaced_by,
        } => {
            gate::acting_in_lane(&gate::lane::RETIRE_INTENT, None)?;
            store.ensure_owned(
                "retire an intent (the design decision belongs to the graph's owners)",
            )?;
            gate::require_substantive("reason", &reason, "why this design was superseded")?;
            let id = resolve_intent_with_db(&store, &id)?;
            let successor = match &replaced_by {
                Some(k) => {
                    let sid = resolve_intent_with_db(&store, k)?;
                    if sid == id {
                        anyhow::bail!("--replaced-by points at the intent being retired — pass a different successor or omit --replaced-by.");
                    }
                    Some(sid)
                }
                None => None,
            };
            let fallout = store.retire_fallout(&id)?;
            let now = chrono::Utc::now().to_rfc3339();
            if !store.retire_intent(&id, &reason, successor.as_deref(), &now)? {
                anyhow::bail!(
                    "Intent '{}' not found. Run `loom intent list` (or `loom find \"<words>\"`).",
                    id
                );
            }
            let next_step =
                "`loom status` re-checks the compass; `loom coverage` shows any new gaps.";
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "id": id, "retired": true,
                    "replaced_by": successor, "fallout": fallout,
                    "next_step": next_step,
                }));
            } else {
                println!("✓ Intent {id} retired (status=deprecated — history kept, computation stops counting it).");
                if let Some(s) = &successor {
                    println!("  replaced by: {s}");
                }
                let f = &fallout;
                if f.orphaned_children.is_empty()
                    && f.solely_grounded_files.is_empty()
                    && f.dangling_validations.is_empty()
                {
                    println!(
                        "  No fallout: no children, no solely-owned files, no dangling proofs."
                    );
                } else {
                    println!("  TRIGGERED WORK:");
                    for c in &f.orphaned_children {
                        println!("    · child '{c}' lost its parent — re-parent (`loom edge hierarchy <new-parent> …`) or retire it too");
                    }
                    for p in &f.solely_grounded_files {
                        println!("    · {p} lost its only owner — it now reads UNREACHED (ground under a successor or `loom ignore`)");
                    }
                    for v in &f.dangling_validations {
                        println!("    · validation '{v}' proves only retired design — re-link (`loom edge validates …`) or `loom validation delete`");
                    }
                }
                if f.edges_leaving_computation > 0 {
                    println!("  {} RELATES_TO edge(s) leave every queue/centrality computation (kept as history); verified ones are flagged, so living neighbours surface in `loom next --mode align` for the user to re-affirm.", f.edges_leaving_computation);
                }
                println!("  → Next: {next_step}");
            }
        }

        IntentCmd::List {
            status,
            level,
            limit,
        } => run_list_with_db(&store, status, level, limit, printer)?,
        IntentCmd::Show { id } => run_show_with_db(&store, id, printer)?,
        IntentCmd::Source { subcommand } => run_source_with_sqlite(root, subcommand, printer)?,
        IntentCmd::Tag { subcommand } => run_tag_with_sqlite(root, subcommand, printer)?,
    }
    Ok(())
}

fn run_list_with_db(
    db: &dyn GraphReadRepository,
    status: Option<String>,
    level: Option<String>,
    limit: usize,
    printer: &Printer,
) -> Result<()> {
    // Validate filter values against the domain vocabulary.
    if let Some(ref s) = status {
        s.parse::<crate::types::IntentStatus>()
            .map_err(|e| anyhow::anyhow!("{}", e))?;
    }
    let level = match level {
        Some(l) => Some(
            l.parse::<crate::types::AbstractionLevel>()
                .map_err(|e| anyhow::anyhow!("{}", e))?
                .to_string(),
        ),
        None => None,
    };
    let mut intents = db.list_intents(status.as_deref(), level.as_deref())?;
    let total = crate::output::apply_limit(&mut intents, limit);
    if printer.json {
        printer.print_json(&serde_json::json!({
            "intents": intents,
            "total": total,
            "truncated": intents.len() < total,
        }));
    } else if intents.is_empty() {
        println!("(no intents found)");
    } else {
        println!(
            "  {status:>20}   {level:<15}  {name:<40}  id",
            status = "STATUS",
            level = "LEVEL",
            name = "NAME",
        );
        println!("  {}", "-".repeat(90));
        for i in &intents {
            println!("{}", fmt_intent_row(i));
        }
        if let Some(m) =
            crate::output::more_marker(total, intents.len(), "loom intent list --limit 0")
        {
            println!("  {m}");
        }
    }
    Ok(())
}

fn run_show_with_db(db: &dyn GraphReadRepository, id: String, printer: &Printer) -> Result<()> {
    let id = resolve_intent_with_db(db, &id)?;
    let intent = db.get_intent(&id)?;
    match intent {
        None => anyhow::bail!(
            "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
            id
        ),
        Some(ref i) => {
            let mut edges = db.edges_for_intent(&id)?;
            let edges_total = crate::output::apply_limit(&mut edges, crate::output::SECTION_CAP);
            let mut hierarchy = db.list_hierarchy_for_intent(&id)?;
            let hierarchy_total =
                crate::output::apply_limit(&mut hierarchy, crate::output::SECTION_CAP);
            let mut implements = db.list_implements_for_intent(&id)?;
            let implements_total =
                crate::output::apply_limit(&mut implements, crate::output::SECTION_CAP);
            let mut notes = db.notes_for_target(&id)?;
            let notes_total = notes.len();
            if notes_total > crate::output::SECTION_CAP {
                // notes_for_target returns oldest-first; keep the NEWEST.
                notes.drain(..notes_total - crate::output::SECTION_CAP);
            }
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "intent": i,
                    "edges": edges,
                    "edges_total": edges_total,
                    "hierarchy": hierarchy,
                    "hierarchy_total": hierarchy_total,
                    "implements": implements,
                    "implements_total": implements_total,
                    "notes": notes,
                    "notes_total": notes_total,
                }));
            } else {
                println!("── Intent ─────────────────────────────────────────────────────────");
                println!("{}", fmt_intent(i));
                println!();
                println!(
                    "── RELATES_TO edges ({}) ────────────────────────────────────────────",
                    edges_total
                );
                if edges.is_empty() {
                    println!("  (none)");
                } else {
                    for e in &edges {
                        println!("{}", fmt_edge_row(e));
                    }
                    if let Some(m) = crate::output::more_marker(
                        edges_total,
                        edges.len(),
                        &format!("loom cluster {id}"),
                    ) {
                        println!("  {m}");
                    }
                }
                println!();
                println!(
                    "── Hierarchy ({}) ───────────────────────────────────────────────────",
                    hierarchy_total
                );
                if hierarchy.is_empty() {
                    println!("  (none — no parent/child intents)");
                } else {
                    for h in &hierarchy {
                        if h.parent_id == id {
                            println!("  ↓ child:  {} ({})", h.child_name, h.child_id);
                        } else {
                            println!("  ↑ parent: {} ({})", h.parent_name, h.parent_id);
                        }
                    }
                    if let Some(m) = crate::output::more_marker(
                        hierarchy_total,
                        hierarchy.len(),
                        &format!("loom cluster {id}"),
                    ) {
                        println!("  {m}");
                    }
                }
                println!();
                println!(
                    "── Implements ({}) ──────────────────────────────────────────────────",
                    implements_total
                );
                if implements.is_empty() {
                    println!("  (none — intent not yet grounded to code)");
                } else {
                    for im in &implements {
                        let loc = if im.locator.is_empty() {
                            String::new()
                        } else {
                            format!("  @ {}", im.locator)
                        };
                        println!(
                            "  → {}{}  [{}]",
                            im.codefile_path, loc, im.inspection_status
                        );
                    }
                    if let Some(m) = crate::output::more_marker(
                        implements_total,
                        implements.len(),
                        &format!("loom cluster {id}"),
                    ) {
                        println!("  {m}");
                    }
                }
                println!();
                println!(
                    "── Notes ({}) ───────────────────────────────────────────────────────",
                    notes_total
                );
                if notes.is_empty() {
                    println!("  (none)");
                } else {
                    for n in &notes {
                        println!("  [{}] {}  ({})", n.kind, n.text, n.author);
                    }
                    if let Some(m) = crate::output::more_marker(
                        notes_total,
                        notes.len(),
                        &format!("loom note list --intent {id}"),
                    ) {
                        println!("  {m}");
                    }
                }
            }
        }
    }
    Ok(())
}
