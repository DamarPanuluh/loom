use anyhow::Result;
use uuid::Uuid;

use crate::cli::CorpusCmd;
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;
use crate::types::InboxItem;

pub fn run(cmd: CorpusCmd, printer: &Printer) -> Result<()> {
    let root = crate::db::resolve_root()?;
    match cmd {
        CorpusCmd::Coverage => {
            let store = GraphReadHandle::open(&root)?;
            let snapshot = store.query_snapshot()?;
            let inbox = store.list_inbox_items(None, None)?;
            let report = crate::db::queries::source_corpus_coverage(&root, &snapshot, &inbox);
            if printer.json {
                printer.print_json(&report);
            } else {
                println!("── Source Corpus Coverage ─────────────────────────────────────────");
                if report.doc_files == 0 {
                    println!("No documentation files detected.");
                    return Ok(());
                }
                println!(
                    "docs: {} total · {} structured · {} unstructured",
                    report.doc_files, report.structured_doc_files, report.unstructured_doc_files
                );
                println!(
                    "ids: {} total · {} modeled · {} resolved · {} unresolved",
                    report.ids_total, report.modeled, report.resolved, report.unresolved
                );
                if !report.by_prefix.is_empty() {
                    let parts = report
                        .by_prefix
                        .iter()
                        .map(|(prefix, count)| format!("{prefix} {count}"))
                        .collect::<Vec<_>>()
                        .join(" · ");
                    println!("prefixes: {parts}");
                }
                if !report.warning.is_empty() {
                    println!("⚑ {}", report.warning);
                }
                for item in &report.examples {
                    println!("  · {} at {}:{}", item.id, item.path, item.line);
                }
                if report.unstructured_doc_files > 0 {
                    println!("→ For non-conventional docs, run `loom seed --inbox` and let the LLM triage the prose.");
                }
            }
        }
        CorpusCmd::Ignore { id, source, reason } => {
            ignore_id(&root, &id, &source, &reason, printer)?
        }
    }
    Ok(())
}

fn ignore_id(
    root: &std::path::Path,
    id: &str,
    source: &str,
    reason: &str,
    printer: &Printer,
) -> Result<()> {
    crate::db::ensure_initialized(root)?;
    crate::gate::require_substantive(
        "reason",
        reason,
        "why this documented requirement is intentionally not modeled",
    )?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let now = chrono::Utc::now().to_rfc3339();
    let item = InboxItem {
        id: Uuid::new_v4().to_string(),
        raw_text: format!("corpus:{id}\nsource:{source}\nignore documented requirement {id}"),
        normalized_claim: format!(
            "Documented requirement {id} from {source} is intentionally not modeled."
        ),
        kind: "docs_gap".to_string(),
        status: "rejected".to_string(),
        source: "import".to_string(),
        author: crate::agent::acting(None),
        tags: Vec::new(),
        links: Vec::new(),
        route_kind: "ignore".to_string(),
        route_command: format!("loom corpus ignore {id} --source {source} --reason \"…\""),
        route_target_kind: "none".to_string(),
        route_target_id: id.to_string(),
        resolution: reason.to_string(),
        created_at: now.clone(),
        updated_at: now,
    };
    store.insert_inbox_item(&item)?;
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "ignored": id,
            "source": source,
            "inbox_item": item,
            "next_step": "loom corpus coverage",
        }));
    } else {
        println!("✓ Corpus requirement {id} marked ignored/resolved");
        println!("  source: {source}");
        println!("  → Next: loom corpus coverage");
    }
    Ok(())
}
