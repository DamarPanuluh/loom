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
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let snapshot = store.query_snapshot()?;

    match subcommand {
        SourceCmd::Add { id, path } => {
            let id = crate::db::queries::resolve_intent_from_snapshot(&snapshot, &id)?;
            let Some(parsed) = store.add_source_ref(&id, &path, &now)? else {
                anyhow::bail!(crate::output::intent_not_found_find(&id));
            };
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "id": id, "added": path,
                    "source_refs": parsed,
                    "next_step": crate::output::intent_show_command(&id),
                }));
            } else {
                println!("✓ Source ref added to intent {id}: {path}");
                println!("  → Next: {}", crate::output::intent_show_command(&id));
            }
        }
        SourceCmd::Remove { id, path } => {
            let id = crate::db::queries::resolve_intent_from_snapshot(&snapshot, &id)?;
            match store.remove_source_ref(&id, &path, &now)? {
                None => anyhow::bail!(crate::output::intent_not_found_find(&id)),
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
                            "next_step": crate::output::intent_show_command(&id),
                        }));
                    } else {
                        println!("✓ Source ref removed from intent {id}: {path}");
                        println!("  → Next: {}", crate::output::intent_show_command(&id));
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
                .ok_or_else(|| anyhow::anyhow!(crate::output::intent_not_found_find(&id)))?;
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
                    "next_step": crate::output::intent_show_command(&id),
                }));
            } else {
                println!("✓ Intent {id} tagged: [{}]", tags.join(", "));
                println!("  → Next: {}", crate::output::intent_show_command(&id));
            }
        }
        TagCmd::Remove { id, term } => {
            let id = crate::db::queries::resolve_intent_from_snapshot(&snapshot, &id)?;
            let intent = snapshot
                .intents
                .iter()
                .find(|intent| intent.id == id)
                .ok_or_else(|| anyhow::anyhow!(crate::output::intent_not_found_find(&id)))?;
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
                    "next_step": crate::output::intent_show_command(&id),
                }));
            } else {
                println!("✓ Tag '{term}' removed from intent {id}");
                println!("  → Next: {}", crate::output::intent_show_command(&id));
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
            criterion,
            level,
            domain,
            layer,
            aspect,
            lifecycle,
            sources,
            tags,
            visibility,
            boundary,
        } => handle_add(
            &mut store,
            AddIntentArgs {
                name,
                description,
                criterion,
                level,
                domain,
                layer,
                aspect,
                lifecycle,
                sources,
                tags,
                visibility,
                boundary,
            },
            printer,
        )?,
        IntentCmd::Confirm { id, visibility } => {
            handle_confirm(&mut store, id, visibility, printer)?
        }
        IntentCmd::Update {
            id,
            name,
            layer,
            boundary,
            description,
            reword,
            reason,
            criterion,
            extra,
        } => handle_update(
            &mut store,
            UpdateIntentArgs {
                id,
                name,
                layer,
                boundary,
                description,
                reword,
                reason,
                criterion,
                extra,
            },
            printer,
        )?,
        IntentCmd::Mark {
            id,
            lifecycle,
            reason,
        } => handle_mark(&mut store, id, lifecycle, reason, printer)?,
        IntentCmd::Delete { id } => handle_delete(&mut store, id, printer)?,
        IntentCmd::Retire {
            id,
            reason,
            replaced_by,
        } => handle_retire(&mut store, id, reason, replaced_by, printer)?,
        IntentCmd::List {
            status,
            level,
            limit,
        } => handle_list(&store, status, level, limit, printer)?,
        IntentCmd::Show { id } => handle_show(&store, id, printer)?,
        IntentCmd::Source { subcommand } => handle_source(root, subcommand, printer)?,
        IntentCmd::Tag { subcommand } => handle_tag(root, subcommand, printer)?,
    }
    Ok(())
}

fn handle_list(
    store: &crate::db::sqlite::SqliteGraphStore,
    status: Option<String>,
    level: Option<String>,
    limit: usize,
    printer: &Printer,
) -> Result<()> {
    run_list_with_db(store, status, level, limit, printer)
}

fn handle_show(
    store: &crate::db::sqlite::SqliteGraphStore,
    id: String,
    printer: &Printer,
) -> Result<()> {
    run_show_with_db(store, id, printer)
}

fn handle_source(root: &std::path::Path, subcommand: SourceCmd, printer: &Printer) -> Result<()> {
    run_source_with_sqlite(root, subcommand, printer)
}

fn handle_tag(root: &std::path::Path, subcommand: TagCmd, printer: &Printer) -> Result<()> {
    run_tag_with_sqlite(root, subcommand, printer)
}

struct AddIntentArgs {
    name: String,
    description: String,
    criterion: String,
    level: String,
    domain: String,
    layer: String,
    aspect: String,
    lifecycle: String,
    sources: Vec<String>,
    tags: Vec<String>,
    visibility: String,
    boundary: String,
}

/// Soft granularity check for intake: a name that joins independent
/// responsibilities with a conjunction usually wants to be SEVERAL atomic
/// intents — the contract is one falsifiable criterion each. Advisory only
/// ("command and control" can legitimately be one concern), so it nudges the
/// driver to split BEFORE coarse seeding triggers the `scattered` smell later;
/// it never blocks. Shared with `loom inbox normalize` so the same nudge fires
/// at the earliest intake point, not only at `intent add`.
pub(crate) fn granularity_advisory(name: &str) -> Option<String> {
    let lower = format!(" {} ", name.to_lowercase());
    let joiner = [" and ", " & ", " plus "]
        .iter()
        .find(|j| lower.contains(**j))?;
    Some(format!(
        "granularity: this joins responsibilities with '{}'. The contract is ONE \
         falsifiable criterion per intent — if each side is independently verifiable, \
         seed them as separate atomic intents under a shared parent rather than one \
         coarse '{}'.",
        joiner.trim(),
        name.trim(),
    ))
}

fn handle_add(
    store: &mut crate::db::sqlite::SqliteGraphStore,
    args: AddIntentArgs,
    printer: &Printer,
) -> Result<()> {
    let AddIntentArgs {
        name,
        description,
        criterion,
        level,
        domain,
        layer,
        aspect,
        lifecycle,
        sources,
        tags,
        visibility,
        boundary,
    } = args;
    gate::acting_in_lane(&gate::lane::ADD_INTENT, None)?;
    let level = level
        .parse::<crate::types::AbstractionLevel>()
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    lifecycle
        .parse::<crate::types::LifecycleState>()
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    if name.trim().is_empty() {
        anyhow::bail!("--name must not be empty. State the responsibility this intent owns.");
    }
    if description.trim().is_empty() {
        anyhow::bail!("--description must not be empty. State the observable behavior or design responsibility this intent captures.");
    }
    if !criterion.trim().is_empty() {
        // First-class criterion held to the same substantive-evidence gate
        // as edge criteria (no placeholders, ≥10 chars).
        gate::require_substantive(
            "criterion",
            &criterion,
            "the ONE falsifiable thing this intent is done/correct by",
        )?;
    }
    if lifecycle != "implemented" {
        store.ensure_owned(&format!(
            "declare a '{lifecycle}' intent (a promise to change the code)"
        ))?;
    }
    if !matches!(visibility.as_str(), "" | "user_visible" | "internal") {
        anyhow::bail!("Invalid --visibility '{visibility}'. Valid: user_visible | internal.");
    }
    if !matches!(boundary.as_str(), "" | "inbound" | "outbound") {
        anyhow::bail!("Invalid --boundary '{boundary}'. Valid: inbound | outbound.");
    }

    let snapshot = store.query_snapshot()?;
    let terms = store.list_vocab_terms()?;
    let tags =
        crate::commands::vocab::validate_tags_from_registry(&tags, &terms, &snapshot.intents)?;
    let has_tags = !tags.is_empty();
    let source_refs = sources.clone();
    let now = chrono::Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();

    let intent = Intent {
        id: id.clone(),
        name: name.clone(),
        description,
        criterion,
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
            if let Some(g) = granularity_advisory(&name) {
                obj.insert("granularity_advisory".to_string(), g.into());
            }
        }
        printer.print_json(&v);
    } else {
        println!("✓ Intent created");
        println!("{}", fmt_intent(&intent));
        if let Some(g) = granularity_advisory(&name) {
            println!("  ⚑ {g}");
        }
        println!("  → Next: {}", tree_step);
        println!("          then ground it: `loom edge implement {} <codefile> --locator \"<symbol>\"` (symbol as written in the file).", id);
        if let Some(ts) = &tag_step {
            println!("          {ts}");
        }
    }
    Ok(())
}

fn handle_confirm(
    store: &mut crate::db::sqlite::SqliteGraphStore,
    id: String,
    visibility: Option<String>,
    printer: &Printer,
) -> Result<()> {
    let by = gate::acting_in_lane(&gate::lane::CONFIRM_INTENT, None)?;
    let id = resolve_intent_with_db(store, &id)?;
    let intent = store
        .get_intent(&id)?
        .ok_or_else(|| anyhow::anyhow!(crate::output::intent_not_found_list(&id)))?;
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
        anyhow::bail!(crate::output::intent_not_found_list(&id));
    }
    let confirmed_msg = match visibility.as_deref() {
        Some("internal") => {
            "confirmed + ruled internal — out of the align interview until its meaning is redefined"
        }
        Some("user_visible") => "confirmed + ruled user-visible",
        _ => "confirmed",
    };
    let next_step = "`loom next` serves the next item";
    if printer.json {
        let mut payload = serde_json::json!({"status":"ok","id":id,"new_status":"confirmed"});
        if let Some(v) = visibility.as_deref() {
            payload["visibility"] = serde_json::json!(v);
        }
        payload["next_step"] = serde_json::json!(next_step);
        printer.print_json(&payload);
    } else {
        println!("✓ Intent {} {}", id, confirmed_msg);
        println!("  → Next: {next_step}");
    }
    Ok(())
}

struct UpdateIntentArgs {
    id: String,
    name: Option<String>,
    layer: Option<String>,
    boundary: Option<String>,
    description: Option<String>,
    reword: bool,
    reason: String,
    criterion: Option<String>,
    extra: Vec<String>,
}

fn handle_update(
    store: &mut crate::db::sqlite::SqliteGraphStore,
    args: UpdateIntentArgs,
    printer: &Printer,
) -> Result<()> {
    let UpdateIntentArgs {
        id,
        name,
        layer,
        boundary,
        description,
        reword,
        reason,
        criterion,
        extra,
    } = args;
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
    store.ensure_owned("update an intent (the design decision belongs to the graph's owners)")?;
    gate::require_substantive("reason", &reason, "why the meaning moved")?;
    let id = resolve_intent_with_db(store, &id)?;
    let intent = store
        .get_intent(&id)?
        .ok_or_else(|| anyhow::anyhow!(crate::output::intent_not_found_list(&id)))?;
    if intent.status == "deprecated" {
        anyhow::bail!(
            "Intent '{}' is retired (status=deprecated). Create a successor intent instead of rewriting it.",
            id
        );
    }

    let changes = update_changes(
        &id,
        &intent,
        &name,
        &layer,
        &boundary,
        &description,
        &criterion,
    )?;
    let now = chrono::Utc::now().to_rfc3339();
    store.update_intent_meaning(&id, changes.name, changes.description, &now)?;
    let record_ctx = UpdateRecordContext {
        id: &id,
        intent: &intent,
        reason: &reason,
        by: &by,
        now: &now,
    };
    let ripple = record_update_notes_and_ripple(store, record_ctx, changes, reword)?;
    let rippled = update_rippled(ripple.as_ref());
    let next_step = update_next_step(rippled);
    let print_ctx = UpdatePrintContext {
        id: &id,
        intent: &intent,
    };
    print_update_result(
        printer, print_ctx, changes, reword, &ripple, rippled, next_step,
    );
    Ok(())
}

#[derive(Clone, Copy)]
struct UpdateChanges<'a> {
    name: Option<&'a str>,
    layer: Option<&'a str>,
    boundary: Option<&'a str>,
    description: Option<&'a str>,
    criterion: Option<&'a str>,
}

#[derive(Clone, Copy)]
struct UpdateRecordContext<'a> {
    id: &'a str,
    intent: &'a Intent,
    reason: &'a str,
    by: &'a str,
    now: &'a str,
}

#[derive(Clone, Copy)]
struct UpdatePrintContext<'a> {
    id: &'a str,
    intent: &'a Intent,
}

fn update_changes<'a>(
    id: &str,
    intent: &'a Intent,
    name: &'a Option<String>,
    layer: &'a Option<String>,
    boundary: &'a Option<String>,
    description: &'a Option<String>,
    criterion: &'a Option<String>,
) -> Result<UpdateChanges<'a>> {
    let changes = UpdateChanges {
        name: name
            .as_deref()
            .filter(|candidate| *candidate != intent.name.as_str()),
        layer: layer
            .as_deref()
            .filter(|candidate| *candidate != intent.layer.as_str()),
        boundary: boundary
            .as_deref()
            .filter(|candidate| *candidate != intent.boundary.as_str()),
        description: description
            .as_deref()
            .filter(|candidate| *candidate != intent.description.as_str()),
        criterion: criterion
            .as_deref()
            .filter(|candidate| *candidate != intent.criterion.as_str()),
    };
    if changes.name.is_none()
        && changes.layer.is_none()
        && changes.boundary.is_none()
        && changes.description.is_none()
        && changes.criterion.is_none()
    {
        anyhow::bail!(
            "Nothing to change: pass --name, --layer, --boundary, --description, and/or --criterion with a value that differs from the current one (`loom intent show {}` prints them).",
            id
        );
    }
    // Validate the new criterion BEFORE any write — otherwise a vacuous
    // --criterion paired with a valid --name would persist the rename and
    // then bail, leaving an asymmetric partial write.
    if let Some(criterion) = changes.criterion {
        gate::require_substantive(
            "criterion",
            criterion,
            "the ONE falsifiable thing this intent is done/correct by",
        )?;
    }
    // Validate the new boundary BEFORE any write — same hazard as criterion:
    // set_intent_boundary bails on an invalid value, but it runs LAST in the
    // write sequence. Without this pre-check, an invalid --boundary paired
    // with a valid --description persists the meaning change and then bails
    // before the redefinition ripple, leaving edges green-but-stale.
    if let Some(boundary) = changes.boundary {
        if !matches!(boundary, "inbound" | "outbound" | "") {
            anyhow::bail!("Invalid --boundary '{boundary}'. Valid: inbound | outbound | \"\".");
        }
    }
    Ok(changes)
}

fn record_update_notes_and_ripple(
    store: &mut crate::db::sqlite::SqliteGraphStore,
    ctx: UpdateRecordContext<'_>,
    changes: UpdateChanges<'_>,
    reword: bool,
) -> Result<Option<crate::db::queries::RedefinitionRipple>> {
    if let Some(criterion) = changes.criterion {
        // Version chain: preserve the prior criterion in a decision note.
        store.set_intent_criterion(ctx.id, criterion, ctx.now)?;
        let prior = if ctx.intent.criterion.is_empty() {
            "(none)".to_string()
        } else {
            ctx.intent.criterion.clone()
        };
        store.insert_note(&crate::types::Note {
            id: Uuid::new_v4().to_string(),
            kind: "decision".to_string(),
            text: format!("criterion updated: {} — was: {prior}", ctx.reason),
            author: ctx.by.to_string(),
            target_kind: "intent".to_string(),
            target_id: ctx.id.to_string(),
            resolution: String::new(),
            created_at: ctx.now.to_string(),
            audience: String::new(),
        })?;
    }
    if let Some(layer) = changes.layer {
        store.set_intent_layer(ctx.id, layer, ctx.now)?;
        store.insert_note(&crate::types::Note {
            id: Uuid::new_v4().to_string(),
            kind: "decision".into(),
            text: format!(
                "layer changed: '{}' -> '{}' ({})",
                if ctx.intent.layer.is_empty() {
                    "<undeclared>"
                } else {
                    &ctx.intent.layer
                },
                if layer.is_empty() {
                    "<undeclared>"
                } else {
                    layer
                },
                ctx.reason
            ),
            author: ctx.by.to_string(),
            target_kind: "intent".into(),
            target_id: ctx.id.to_string(),
            resolution: String::new(),
            audience: String::new(),
            created_at: ctx.now.to_string(),
        })?;
    }
    if let Some(boundary) = changes.boundary {
        store.set_intent_boundary(ctx.id, boundary, ctx.now)?;
        store.insert_note(&crate::types::Note {
            id: Uuid::new_v4().to_string(),
            kind: "decision".into(),
            text: format!(
                "boundary changed: '{}' -> '{}' ({})",
                if ctx.intent.boundary.is_empty() {
                    "<internal>"
                } else {
                    &ctx.intent.boundary
                },
                if boundary.is_empty() {
                    "<internal>"
                } else {
                    boundary
                },
                ctx.reason
            ),
            author: ctx.by.to_string(),
            target_kind: "intent".into(),
            target_id: ctx.id.to_string(),
            resolution: String::new(),
            audience: String::new(),
            created_at: ctx.now.to_string(),
        })?;
    }
    if let Some(n) = changes.name {
        store.insert_note(&crate::types::Note {
            id: Uuid::new_v4().to_string(),
            kind: "decision".into(),
            text: format!("renamed: '{}' -> '{}' ({})", ctx.intent.name, n, ctx.reason),
            author: ctx.by.to_string(),
            target_kind: "intent".into(),
            target_id: ctx.id.to_string(),
            resolution: String::new(),
            audience: String::new(),
            created_at: ctx.now.to_string(),
        })?;
    }
    record_description_update(store, ctx, changes, reword)
}

fn record_description_update(
    store: &mut crate::db::sqlite::SqliteGraphStore,
    ctx: UpdateRecordContext<'_>,
    changes: UpdateChanges<'_>,
    reword: bool,
) -> Result<Option<crate::db::queries::RedefinitionRipple>> {
    let mut ripple = None;
    if let Some(_d) = changes.description {
        if reword {
            store.insert_note(&crate::types::Note {
                id: Uuid::new_v4().to_string(),
                kind: "decision".into(),
                text: format!("reworded: {}\nwas: {}", ctx.reason, ctx.intent.description),
                author: ctx.by.to_string(),
                target_kind: "intent".into(),
                target_id: ctx.id.to_string(),
                resolution: String::new(),
                audience: String::new(),
                created_at: ctx.now.to_string(),
            })?;
        } else {
            store.insert_note(&crate::types::Note {
                id: Uuid::new_v4().to_string(),
                kind: "decision".into(),
                text: format!("redefined: {}\nwas: {}", ctx.reason, ctx.intent.description),
                author: ctx.by.to_string(),
                target_kind: "intent".into(),
                target_id: ctx.id.to_string(),
                resolution: String::new(),
                audience: String::new(),
                created_at: ctx.now.to_string(),
            })?;
            if !ctx.intent.visibility.is_empty() {
                store.set_intent_visibility(ctx.id, "", ctx.now)?;
            }
            ripple = Some(store.ripple_intent_redefinition(
                ctx.id,
                changes.name.unwrap_or(&ctx.intent.name),
                ctx.now,
            )?);
        }
    }
    Ok(ripple)
}

fn update_rippled(ripple: Option<&crate::db::queries::RedefinitionRipple>) -> bool {
    ripple.is_some_and(|r| {
        r.relates_to_flagged
            + r.governs_flagged
            + r.targets_flagged
            + r.implements_flagged
            + r.validations_invalidated
            > 0
    })
}

fn update_next_step(rippled: bool) -> &'static str {
    if rippled {
        "`loom next --mode fix` re-inspects staled claims; `loom next --mode quality` re-earns flagged quality green; `loom validate` re-runs invalidated proofs."
    } else {
        "`loom next` serves the next item"
    }
}

fn print_update_result(
    printer: &Printer,
    ctx: UpdatePrintContext<'_>,
    changes: UpdateChanges<'_>,
    reword: bool,
    ripple: &Option<crate::db::queries::RedefinitionRipple>,
    rippled: bool,
    next_step: &str,
) {
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok", "id": ctx.id,
            "renamed": changes.name.is_some(),
            "layer_changed": changes.layer.is_some(),
            "boundary_changed": changes.boundary.is_some(),
            "redefined": changes.description.is_some() && !reword,
            "reworded": changes.description.is_some() && reword,
            "visibility_cleared": changes.description.is_some() && !reword && !ctx.intent.visibility.is_empty(),
            "ripple": ripple,
            "next_step": next_step,
        }));
    } else {
        print_update_human_summary(ctx.id, ctx.intent, changes, reword, ripple, rippled);
        println!("  → Next: {next_step}");
    }
}

fn print_update_human_summary(
    id: &str,
    intent: &Intent,
    changes: UpdateChanges<'_>,
    reword: bool,
    ripple: &Option<crate::db::queries::RedefinitionRipple>,
    rippled: bool,
) {
    match (changes.name, changes.description) {
        (_, Some(_)) if reword => {
            println!("✓ Intent {id} reworded (same concept, clearer words — no ripple).")
        }
        (Some(n), Some(_)) => println!("✓ Intent {id} renamed to '{n}' and redefined."),
        (Some(n), None) => println!("✓ Intent {id} renamed to '{n}' (cosmetic — no ripple)."),
        (None, Some(_)) => println!("✓ Intent {id} redefined."),
        (None, None) => {
            if let Some(layer) = changes.layer {
                println!("✓ Intent {id} layer → '{}' (metadata — no ripple).", layer);
            }
            if let Some(boundary) = changes.boundary {
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
    if changes.description.is_some() && !reword && !intent.visibility.is_empty() {
        println!(
            "  visibility ruling '{}' cleared — the new meaning's audience is unknown; the align interview re-triages it.",
            intent.visibility
        );
    }
    if let Some(r) = ripple {
        print_update_ripple(r, rippled);
    }
}

fn print_update_ripple(r: &crate::db::queries::RedefinitionRipple, rippled: bool) {
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

fn handle_mark(
    store: &mut crate::db::sqlite::SqliteGraphStore,
    id: String,
    lifecycle: String,
    reason: Option<String>,
    printer: &Printer,
) -> Result<()> {
    let by = gate::acting_in_lane(&gate::lane::SET_LIFECYCLE, None)?;
    store
        .ensure_owned("change an intent's lifecycle (a claim about building/changing the code)")?;
    lifecycle
        .parse::<crate::types::LifecycleState>()
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    let id = resolve_intent_with_db(store, &id)?;
    let now = chrono::Utc::now().to_rfc3339();
    if !store.set_intent_lifecycle(&id, &lifecycle, &by, &now)? {
        anyhow::bail!(crate::output::intent_not_found_list(&id));
    }
    if let Some(ref r) = reason {
        store.insert_note(&crate::types::Note {
            id: Uuid::new_v4().to_string(),
            kind: "decision".to_string(),
            text: format!("lifecycle → {}: {}", lifecycle, r),
            author: by.clone(),
            target_kind: "intent".to_string(),
            target_id: id.clone(),
            resolution: String::new(),
            audience: String::new(),
            created_at: now.clone(),
        })?;
    }
    // Mark → implemented closes the build loop, so report the RESULTING state
    // instead of leaving the driver to re-derive it from `loom status`, and flag
    // the honesty gap the loop was thinnest on: a realized leaf with no proof is
    // implemented-but-UNPROVEN — its criterion is asserted, never checked.
    let advisory = if lifecycle == "implemented" {
        let has_children = store
            .list_hierarchy_for_intent(&id)?
            .iter()
            .any(|h| h.parent_id == id);
        if has_children {
            Some(
                "roll-up marked: its children carry the proof — confirm each still meets its criterion (`loom intent show <id>` lists them).".to_string(),
            )
        } else if store.validations_for_intent(&id)?.is_empty() {
            Some(format!(
                "implemented but UNPROVEN — this leaf has no validation, so its criterion is asserted, not checked. Encode the criterion as a proof: `loom validation add --type test --command \"…\" --intent {id}` then `loom validate {id}`."
            ))
        } else {
            Some(
                "leaf realized with proof(s) on file — re-run them if a code change staled them: `loom validate <id>`.".to_string(),
            )
        }
    } else {
        None
    };
    let next_step = match lifecycle.as_str() {
        "planned" | "needs_change" => "`loom next --mode build` will surface it.",
        "implemented" => "if this leaf is fully grounded, prove it: `loom next --mode validate`",
        "deferred" => {
            "parked: out of the build queue and never blocks a roll-up. Record WHY with `loom note add --intent <id> --kind decision`; resume with `--lifecycle planned`."
        }
        _ => "`loom next` serves the next item",
    };
    if printer.json {
        let mut body = serde_json::json!({
            "status": "ok", "id": id, "lifecycle": lifecycle,
            "next_step": next_step,
        });
        if let Some(ref a) = advisory {
            body["advisory"] = serde_json::Value::String(a.clone());
        }
        printer.print_json(&body);
    } else {
        println!("✓ Intent {} → lifecycle '{}'", id, lifecycle);
        if let Some(ref a) = advisory {
            println!("  ⚑ {a}");
        }
        println!("  → Next: {next_step}");
    }
    Ok(())
}

fn handle_delete(
    store: &mut crate::db::sqlite::SqliteGraphStore,
    id: String,
    printer: &Printer,
) -> Result<()> {
    gate::acting_in_lane(&gate::lane::DELETE_INTENT, None)?;
    let id = resolve_intent_with_db(store, &id)?;
    if !store.delete_intent(&id)? {
        anyhow::bail!(crate::output::intent_not_found_list(&id));
    }
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok", "id": id, "deleted": true,
            "next_step": crate::output::STATUS_RECHECK_NEXT_STEP,
        }));
    } else {
        println!("✓ Intent {} deleted (with its edges and notes).", id);
        println!("  → Next: {}", crate::output::STATUS_RECHECK_NEXT_STEP);
    }
    Ok(())
}

fn handle_retire(
    store: &mut crate::db::sqlite::SqliteGraphStore,
    id: String,
    reason: String,
    replaced_by: Option<String>,
    printer: &Printer,
) -> Result<()> {
    gate::acting_in_lane(&gate::lane::RETIRE_INTENT, None)?;
    store.ensure_owned("retire an intent (the design decision belongs to the graph's owners)")?;
    gate::require_substantive("reason", &reason, "why this design was superseded")?;
    let id = resolve_intent_with_db(store, &id)?;
    let successor = match &replaced_by {
        Some(k) => {
            let sid = resolve_intent_with_db(store, k)?;
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
        anyhow::bail!(crate::output::intent_not_found_find(&id));
    }
    let next_step = "`loom status` re-checks the compass; `loom coverage` shows any new gaps.";
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
            println!("  No fallout: no children, no solely-owned files, no dangling proofs.");
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
        None => anyhow::bail!(crate::output::intent_not_found_list(&id)),
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
                        &crate::output::note_list_intent_command(&id),
                    ) {
                        println!("  {m}");
                    }
                }
            }
        }
    }
    Ok(())
}
