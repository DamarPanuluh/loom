//! `loom inbox` — the single intake boundary for free-form language.
//!
//! Inbox items are durable cards, not graph truth. Loom captures raw language,
//! serves mechanical context for triage, and stores the LLM's normalized route
//! proposal. The actual graph mutation still happens through existing commands
//! in v1.

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::cli::InboxCmd;
use crate::db::{ensure_initialized, GraphReadHandle, GraphReadRepository};
use crate::output::{pulse_json, Printer};
use crate::types::InboxItem;

pub use crate::db::schema::INBOX_KINDS;

pub const INBOX_STATUSES: &[&str] = &[
    "new",
    "triaged",
    "routed",
    "rejected",
    "deferred",
    "duplicate",
];

pub const INBOX_SOURCES: &[&str] = &[
    "chat",
    "user",
    "llm",
    "code_audit",
    "validation",
    "import",
    "unknown",
];

pub const INBOX_ROUTE_KINDS: &[&str] = &[
    "intent",
    "hypothesis",
    "validation",
    "quality_rule",
    "vocab",
    "note",
    "ignore",
    "answer",
    "none",
];

const ROUTE_MENU: &[(&str, &str, &str)] = &[
    (
        "new desired behavior",
        "feature_proposal → intent",
        "loom intent add --name … --description … --level feature --lifecycle planned …",
    ),
    (
        "user journey / story walkthrough",
        "validation/saga",
        "write saga YAML → loom saga add <spec.yaml> [--spawn-missing]",
    ),
    (
        "complaint about existing behavior",
        "intent needs_change",
        "loom intent mark <id> --lifecycle needs_change --reason \"…\"",
    ),
    (
        "redesign idea / recurring breakage",
        "hypothesis",
        "loom hypothesis add --name … --claim … --proposal … --predicted-outcome … --target <intent>",
    ),
    (
        "norm or standard to enforce",
        "quality_rule",
        "loom rule add --name … --description … --severity warning|error",
    ),
    (
        "term of art / naming decision",
        "vocab",
        "loom vocab add <term> --why \"covers X, not Y\"",
    ),
    (
        "decision / scope tradeoff",
        "decision_capture → note",
        "loom note add --kind decision --intent <id> --text \"…\"",
    ),
    (
        "constraint / invariant",
        "constraint → quality_rule",
        "loom rule add --name … --description … --severity warning|error",
    ),
    (
        "acceptance criterion / done means",
        "acceptance_criterion → validation",
        "loom validation add --name … --type test|manual_check --command \"…\" --intent <intent>",
    ),
    (
        "endpoint/interface/call coverage gap",
        "interface_gap → validation/saga",
        "loom populate interfaces --from-sagas  OR  write saga YAML → loom saga add <spec.yaml>",
    ),
    (
        "evidence found while working",
        "evidence → note",
        "loom note add --kind justification --intent <id> --text \"…\"",
    ),
    (
        "risk not yet proven as a bug",
        "risk → hypothesis",
        "loom hypothesis add --name … --claim … --proposal … --predicted-outcome … --target <intent>",
    ),
    (
        "later work discovered during another task",
        "follow_up → intent/hypothesis/note",
        "loom intent add …  OR  loom hypothesis add …  OR  loom note add --kind todo …",
    ),
    (
        "possible duplicate/superseded item",
        "duplicate_candidate → note/hypothesis",
        "loom note add --kind decision --text \"why these are/are not duplicates\"",
    ),
    (
        "missing or misleading documentation",
        "docs_gap → intent",
        "loom intent mark <id> --lifecycle needs_change --reason \"docs/self-teaching gap\"",
    ),
    (
        "schema/backfill/upgrade concern",
        "migration_need → validation/populate",
        "loom populate plan  OR  loom validation add --name … --type assertion --command \"…\"",
    ),
    ("question about the system", "answer", "answer from matches; no graph mutation"),
];

pub fn normalize_template(id: &str) -> String {
    format!(
        "loom inbox normalize {id} --kind <kind> --claim \"<normalized claim>\" --route <route_kind> --command \"<exact command or answer>\""
    )
}

pub fn normalize_hint(id: &str) -> String {
    format!(
        "loom inbox normalize {id} --kind <kind> --claim \"…\" --route <route_kind> --command \"…\""
    )
}

pub fn run(cmd: InboxCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    match cmd {
        InboxCmd::List {
            status,
            kind,
            limit,
        } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_list_with_db(&db, status.as_deref(), kind.as_deref(), limit, printer)
        }
        InboxCmd::Show { id } => {
            let store =
                crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
            let item = store.resolve_inbox_item(&id)?;
            render_show(&item, printer)
        }
        InboxCmd::Triage { take } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_triage_with_db(&db, take, printer)
        }
        InboxCmd::Add {
            raw_text,
            source,
            tags,
            links,
            author,
        } => {
            ensure_initialized(&cwd)?;
            let store =
                crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
            let item = prepare_new_item(&store, raw_text, source, tags, links, author)?;
            store.insert_inbox_item(&item)?;
            render_show(&item, printer)
        }
        InboxCmd::Normalize {
            id,
            kind,
            claim,
            route_kind,
            command,
            tags,
            links,
            route_target_kind,
            route_target_id,
        } => {
            ensure_initialized(&cwd)?;
            let store =
                crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
            let mut item = store.resolve_inbox_item(&id)?;
            normalize_item(
                &store,
                &mut item,
                kind,
                claim,
                route_kind,
                command,
                tags,
                links,
                route_target_kind,
                route_target_id,
            )?;
            store.update_inbox_item(&item)?;
            render_show(&item, printer)
        }
        InboxCmd::Mark {
            id,
            status,
            reason,
            route_target_kind,
            route_target_id,
        } => {
            ensure_initialized(&cwd)?;
            let store =
                crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
            let mut item = store.resolve_inbox_item(&id)?;
            mark_item(
                &store,
                &mut item,
                status,
                reason,
                route_target_kind,
                route_target_id,
            )?;
            store.update_inbox_item(&item)?;
            render_show(&item, printer)
        }
    }
}

pub fn create_item(
    store: &crate::db::sqlite::SqliteGraphStore,
    raw_text: String,
    source: String,
    tags: Vec<String>,
    links: Vec<String>,
    author: Option<String>,
) -> Result<InboxItem> {
    let item = prepare_new_item(store, raw_text, source, tags, links, author)?;
    store.insert_inbox_item(&item)?;
    Ok(item)
}

fn prepare_new_item(
    store: &crate::db::sqlite::SqliteGraphStore,
    raw_text: String,
    source: String,
    tags: Vec<String>,
    links: Vec<String>,
    author: Option<String>,
) -> Result<InboxItem> {
    crate::gate::require_substantive(
        "raw-text",
        &raw_text,
        "the user/LLM observation to preserve before routing",
    )?;
    validate_enum("source", &source, INBOX_SOURCES)?;
    let tags = normalize_tags(store, tags)?;
    let links = normalize_links(store, links)?;
    let now = chrono::Utc::now().to_rfc3339();
    Ok(InboxItem {
        id: Uuid::new_v4().to_string(),
        raw_text,
        normalized_claim: String::new(),
        kind: "observation".to_string(),
        status: "new".to_string(),
        source,
        author: crate::agent::acting(author.as_deref()),
        tags,
        links,
        route_kind: String::new(),
        route_command: String::new(),
        route_target_kind: String::new(),
        route_target_id: String::new(),
        resolution: String::new(),
        created_at: now.clone(),
        updated_at: now,
    })
}

#[allow(clippy::too_many_arguments)]
fn normalize_item(
    store: &crate::db::sqlite::SqliteGraphStore,
    item: &mut InboxItem,
    kind: String,
    claim: String,
    route_kind: String,
    command: String,
    tags: Vec<String>,
    links: Vec<String>,
    route_target_kind: Option<String>,
    route_target_id: Option<String>,
) -> Result<()> {
    validate_enum("kind", &kind, INBOX_KINDS)?;
    validate_enum("route", &route_kind, INBOX_ROUTE_KINDS)?;
    crate::gate::require_substantive(
        "claim",
        &claim,
        "the normalized loom-vocabulary reading of the raw card",
    )?;
    crate::gate::require_substantive(
        "command",
        &command,
        "the exact command or answer proposal the operator can review",
    )?;
    let tags = normalize_tags(store, tags)?;
    let links = normalize_links(store, links)?;
    item.kind = kind;
    item.normalized_claim = claim;
    item.route_kind = route_kind;
    item.route_command = command;
    item.tags = tags;
    item.links = links;
    item.route_target_kind = route_target_kind.unwrap_or_default();
    item.route_target_id = route_target_id.unwrap_or_default();
    item.status = "triaged".to_string();
    item.updated_at = chrono::Utc::now().to_rfc3339();
    Ok(())
}

fn mark_item(
    store: &crate::db::sqlite::SqliteGraphStore,
    item: &mut InboxItem,
    status: String,
    reason: String,
    route_target_kind: Option<String>,
    route_target_id: Option<String>,
) -> Result<()> {
    validate_enum(
        "status",
        &status,
        &["routed", "rejected", "duplicate", "deferred"],
    )?;
    crate::gate::require_substantive(
        "reason",
        &reason,
        "what decision was made or what graph command/result handled this card",
    )?;
    if let (Some(kind), Some(id)) = (route_target_kind.as_deref(), route_target_id.as_deref()) {
        validate_target_ref(store, kind, id)
            .with_context(|| format!("invalid --target-kind/--target-id {kind}:{id}"))?;
    }
    item.status = status;
    item.resolution = reason;
    if let Some(kind) = route_target_kind {
        item.route_target_kind = kind;
    }
    if let Some(id) = route_target_id {
        item.route_target_id = id;
    }
    item.updated_at = chrono::Utc::now().to_rfc3339();
    Ok(())
}

fn run_list_with_db(
    db: &dyn GraphReadRepository,
    status: Option<&str>,
    kind: Option<&str>,
    limit: usize,
    printer: &Printer,
) -> Result<()> {
    if let Some(status) = status {
        validate_enum("status", status, INBOX_STATUSES)?;
    }
    if let Some(kind) = kind {
        validate_enum("kind", kind, INBOX_KINDS)?;
    }
    let mut items = db.list_inbox_items(status, kind)?;
    let total = items.len();
    items.reverse();
    if limit > 0 && items.len() > limit {
        items.truncate(limit);
    }
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "count": items.len(),
            "total": total,
            "filters": {
                "status": status,
                "kind": kind,
                "limit": limit,
            },
            "items": items,
        }));
        return Ok(());
    }
    if items.is_empty() {
        println!("✓ Inbox empty for the requested filter.");
    }
    for item in &items {
        println!(
            "{}  [{} / {}]  {}",
            short_id(&item.id),
            item.status,
            item.kind,
            item.raw_text
        );
        if !item.normalized_claim.is_empty() {
            println!("    claim: {}", item.normalized_claim);
        }
        if !item.route_command.is_empty() {
            println!("    route: {} → {}", item.route_kind, item.route_command);
        }
    }
    Ok(())
}

fn run_triage_with_db(db: &dyn GraphReadRepository, take: usize, printer: &Printer) -> Result<()> {
    let take = take.clamp(1, 50);
    let mut items = db.list_inbox_items(None, None)?;
    items.retain(|item| matches!(item.status.as_str(), "new" | "triaged"));
    items.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let queue_total = items.len();
    items.truncate(take);
    let snapshot = db.query_snapshot()?;
    let gs = db.graph_state(&snapshot)?;
    let vocab = db.list_vocab_terms()?;
    let triage_items: Vec<_> = items
        .iter()
        .map(|item| {
            let (intents, _) = db.find_intents(&item.raw_text, 5)?;
            let planes = db.door_matches(&item.raw_text, 5)?;
            Ok(serde_json::json!({
                "item": item,
                "matches": {
                    "intents": intents,
                    "vocab": planes.vocab,
                    "sagas": planes.sagas,
                    "rules": planes.rules,
                },
                "normalize_template": normalize_template(&item.id),
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let normalize_templates: Vec<_> = triage_items
        .iter()
        .filter_map(|value| value["normalize_template"].as_str().map(str::to_string))
        .collect();
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "mode": "inbox_triage",
            "count": triage_items.len(),
            "taken": triage_items.len(),
            "queue_total": queue_total,
            "items": triage_items,
            "normalize_templates": normalize_templates,
            "route_menu": route_menu_json(),
            "vocab_terms": vocab,
            "doctrine": "Inbox captures raw language first. Normalize one card at a time; run the proposed graph command separately; then mark the card routed/rejected/duplicate/deferred.",
            "next_step": "loom inbox normalize <id> …  → run proposed command → loom inbox mark <id> --status routed --reason \"…\"",
            "graph_state": pulse_json(&gs),
        }));
        return Ok(());
    }
    println!("── Inbox Triage ───────────────────────────────────────────────────");
    for value in &triage_items {
        let item: InboxItem = serde_json::from_value(value["item"].clone())?;
        println!();
        println!("{}  [{} / {}]", item.id, item.status, item.kind);
        println!("  raw: {}", item.raw_text);
        println!("  normalize: {}", normalize_hint(&item.id));
    }
    println!();
    println!("Route menu:");
    for (when, route, command) in ROUTE_MENU {
        println!("  · {when} → {route}: {command}");
    }
    Ok(())
}

fn render_show(item: &InboxItem, printer: &Printer) -> Result<()> {
    // A card routed to become an intent is held to the granularity contract at
    // the EARLIEST intake point — so a coarse "X and Y" claim is flagged here,
    // before `intent add`, not after the `scattered` smell fires later.
    let granularity = (item.route_kind == "intent")
        .then(|| crate::commands::intent::granularity_advisory(&item.normalized_claim))
        .flatten();
    if printer.json {
        let mut body = serde_json::json!({
            "status": "ok",
            "item": item,
            "next_step": next_step_for(item),
        });
        if let Some(g) = &granularity {
            body["granularity_advisory"] = g.clone().into();
        }
        printer.print_json(&body);
        return Ok(());
    }
    println!(
        "── Inbox Item {} ─────────────────────────────────",
        item.id
    );
    println!(
        "status: {}    kind: {}    source: {}",
        item.status, item.kind, item.source
    );
    println!("raw: {}", item.raw_text);
    if !item.normalized_claim.is_empty() {
        println!("claim: {}", item.normalized_claim);
    }
    if !item.tags.is_empty() {
        println!("tags: {}", item.tags.join(", "));
    }
    if !item.links.is_empty() {
        println!("links: {}", item.links.join(", "));
    }
    if !item.route_command.is_empty() {
        println!("route: {} → {}", item.route_kind, item.route_command);
    }
    if !item.resolution.is_empty() {
        println!("resolution: {}", item.resolution);
    }
    if let Some(g) = &granularity {
        println!("⚑ {g}");
    }
    println!("→ Next: {}", next_step_for(item));
    Ok(())
}

fn next_step_for(item: &InboxItem) -> &'static str {
    match item.status.as_str() {
        "new" => crate::commands::INBOX_TRIAGE_COMMAND,
        "triaged" => {
            "run the route command, then `loom inbox mark <id> --status routed --reason \"…\"`"
        }
        "routed" | "rejected" | "duplicate" | "deferred" => "loom inbox list",
        _ => "loom inbox list",
    }
}

fn validate_enum(name: &str, value: &str, valid: &[&str]) -> Result<()> {
    if valid.contains(&value) {
        Ok(())
    } else {
        anyhow::bail!("Unknown {name} '{}'. Valid: {}", value, valid.join(", "))
    }
}

fn normalize_tags(
    store: &crate::db::sqlite::SqliteGraphStore,
    tags: Vec<String>,
) -> Result<Vec<String>> {
    let known: std::collections::HashSet<String> = store
        .list_vocab_terms()?
        .into_iter()
        .map(|term| term.name)
        .collect();
    let mut out = tags
        .into_iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    out.sort();
    out.dedup();
    for tag in &out {
        if !known.contains(tag) {
            anyhow::bail!(
                "Unknown vocab tag '{}'. Run `loom vocab list` or add it first.",
                tag
            );
        }
    }
    Ok(out)
}

fn normalize_links(
    store: &crate::db::sqlite::SqliteGraphStore,
    links: Vec<String>,
) -> Result<Vec<String>> {
    let mut out = links
        .into_iter()
        .map(|link| link.trim().to_string())
        .filter(|link| !link.is_empty())
        .collect::<Vec<_>>();
    out.sort();
    out.dedup();
    for link in &out {
        let (kind, id) = link
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("Inbox link '{}' must be kind:value", link))?;
        validate_target_ref(store, kind, id).with_context(|| format!("invalid link '{link}'"))?;
    }
    Ok(out)
}

fn validate_target_ref(
    store: &crate::db::sqlite::SqliteGraphStore,
    kind: &str,
    id: &str,
) -> Result<()> {
    match kind {
        "intent" => {
            let snapshot = store.query_snapshot()?;
            crate::db::queries::resolve_intent_from_snapshot(&snapshot, id)?;
        }
        "file" => {
            // Accept any existing repo file — docs, configs, and source are
            // all valid link targets. The old behavior (only registered
            // CodeFiles) rejected docs that the guide recommends linking.
            let candidate = std::path::Path::new(id);
            let resolved = if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                std::env::current_dir().unwrap_or_default().join(id)
            };
            if !resolved.exists() {
                // Fall back to checking registered codefiles (by id or path)
                // for backward compatibility with stored graph references.
                if !store
                    .list_codefiles()?
                    .iter()
                    .any(|file| file.id == id || file.path == id)
                {
                    anyhow::bail!(
                        "no file matches '{}' — the path does not exist in the repo and is not a registered code file",
                        id
                    );
                }
            }
        }
        "validation" => {
            if !store
                .list_validations()?
                .iter()
                .any(|validation| validation.id == id || validation.name == id)
            {
                anyhow::bail!("no validation matches '{}'", id);
            }
        }
        "hypothesis" => {
            store.resolve_hypothesis(id)?;
        }
        "rule" => {
            if !store
                .list_rules()?
                .iter()
                .any(|rule| rule.id == id || rule.name == id)
            {
                anyhow::bail!("no quality rule matches '{}'", id);
            }
        }
        "vocab" => {
            if !store.list_vocab_terms()?.iter().any(|term| term.name == id || term.id == id) {
                anyhow::bail!("no vocab term matches '{}'", id);
            }
        }
        "inbox" => {
            store.resolve_inbox_item(id)?;
        }
        other => anyhow::bail!(
            "Unknown link kind '{}'. Valid: intent, file, validation, hypothesis, rule, vocab, inbox",
            other
        ),
    }
    Ok(())
}

fn route_menu_json() -> Vec<serde_json::Value> {
    ROUTE_MENU
        .iter()
        .map(|(when, route, command)| {
            serde_json::json!({
                "when": when,
                "route": route,
                "command": command,
            })
        })
        .collect()
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}
