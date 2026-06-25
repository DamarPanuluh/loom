//! `loom import` — rebuild a graph from `loom export` output. Restoration
//! into a fresh `loom init`, not a merge.

use anyhow::Result;
use std::fs;

use crate::db::schema::{edge, label, prop};
use crate::db::{ensure_initialized, sqlite_db_path};
use crate::output::Printer;

pub fn run(file: &str, as_planned: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    run_with_sqlite(&cwd, file, as_planned, printer)
}

fn run_with_sqlite(
    root: &std::path::Path,
    file: &str,
    as_planned: bool,
    printer: &Printer,
) -> Result<()> {
    let mut data = read_export(root, file)?;
    let (skipped_nodes, skipped_edges) = if as_planned {
        transform_as_planned(&mut data)?
    } else {
        (0, 0)
    };
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&sqlite_db_path(root))?;
    // `loom import` RESTORES into a fresh graph — it REPLACES all content, it does
    // not merge. Refuse to SILENTLY destroy a non-empty graph (the help even says
    // "into a fresh loom init"): a cold LLM told to "import this graph" must not
    // lose existing intents with a cheerful status:ok.
    let existing_intents = store.query_snapshot()?.intents.len();
    if existing_intents > 0 {
        anyhow::bail!(
            "graph is not empty: {existing_intents} intent(s) here would be DESTROYED — \
             `loom import` RESTORES into a FRESH graph, it does not merge.\n\
             Import into a clean `loom init` (a new directory, or remove `.loom/` and re-init). \
             If you meant to keep THIS graph, `loom export` it first."
        );
    }
    store.import_export_json(&data)?;
    // SECURITY: an imported graph carries shell commands that `loom validate`
    // executes — a supply-chain footgun (a PR-merged / federated / cloned
    // loom.graph.json). Neutralize unvetted pending commands so a bulk
    // `validate --all` cannot run them silently; the operator vets each
    // deliberately with `loom validate <intent>`.
    let unvetted_blocked = store.block_unvetted_imported_commands()?;
    let (nodes, edges) = store.counts()?;
    let next_step = if as_planned {
        "`loom guide --mode port` for the re-realization loop, then `loom next --mode build`."
    } else {
        "`loom sync` to reconcile against this machine's files, then `loom status`."
    };
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "backend": "sqlite",
            "file": file,
            "as_planned": as_planned,
            "nodes": nodes,
            "edges": edges,
            "skipped_nodes": skipped_nodes,
            "skipped_edges": skipped_edges,
            "unvetted_commands_blocked": unvetted_blocked,
            "next_step": next_step,
        }));
    } else if as_planned {
        println!(
            "✓ Design adopted from {file}  ({nodes} nodes, {edges} edges; {skipped_nodes} node(s) + {skipped_edges} edge(s) dropped — the old repo's files/groundings)"
        );
        println!("  Every intent arrived lifecycle=planned; every proof not_run; verdict meta reset to uninspected.");
        println!("  → Next: {next_step}");
    } else {
        println!("✓ SQLite graph imported from {file}  ({nodes} nodes, {edges} edges)");
        if unvetted_blocked > 0 {
            println!(
                "  ⚠ {unvetted_blocked} imported proof(s) carry shell commands `loom validate` would EXECUTE — \
                 blocked so `validate --all` won't run them silently. If this graph is from an UNTRUSTED source, \
                 review them (`loom validation list`) before running `loom validate <intent>` to execute one."
            );
        }
        println!("  → Next: {next_step}");
    }
    Ok(())
}

fn transform_as_planned(data: &mut serde_json::Value) -> Result<(usize, usize)> {
    let nodes = data
        .get_mut("nodes")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| anyhow::anyhow!("Export is missing `nodes` object"))?;

    let mut skipped_nodes = 0usize;
    for skipped in [label::CODE_FILE, label::IGNORE, label::DELEGATION] {
        if let Some(items) = nodes
            .get_mut(skipped)
            .and_then(serde_json::Value::as_array_mut)
        {
            skipped_nodes += items.len();
            items.clear();
        }
    }

    if let Some(intents) = nodes
        .get_mut(label::INTENT)
        .and_then(serde_json::Value::as_array_mut)
    {
        for item in intents {
            if let Some(obj) = item.as_object_mut() {
                obj.insert(prop::LIFECYCLE.to_string(), serde_json::json!("planned"));
            }
        }
    }
    if let Some(validations) = nodes
        .get_mut(label::VALIDATION)
        .and_then(serde_json::Value::as_array_mut)
    {
        for item in validations {
            if let Some(obj) = item.as_object_mut() {
                obj.insert(prop::LAST_RESULT.to_string(), serde_json::json!("not_run"));
                obj.insert(prop::LAST_RUN.to_string(), serde_json::json!(""));
            }
        }
    }
    if let Some(hypotheses) = nodes
        .get_mut(label::HYPOTHESIS)
        .and_then(serde_json::Value::as_array_mut)
    {
        for item in hypotheses {
            if let Some(obj) = item.as_object_mut() {
                let status = obj.get(prop::STATUS).and_then(serde_json::Value::as_str);
                match status {
                    Some("supported" | "refuted") => {
                        obj.insert(prop::STATUS.to_string(), serde_json::json!("proposed"));
                    }
                    Some("confirmed") => {
                        obj.insert(prop::STATUS.to_string(), serde_json::json!("adopted"));
                    }
                    _ => {}
                }
                obj.insert(prop::EVIDENCE.to_string(), serde_json::json!(""));
                obj.insert(prop::INSPECTED_BY.to_string(), serde_json::json!(""));
                obj.insert(prop::LAST_INSPECTED.to_string(), serde_json::json!(""));
            }
        }
    }

    let edges = data
        .get_mut("edges")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| anyhow::anyhow!("Export is missing `edges` object"))?;
    let skipped_edges = edges
        .get_mut(edge::IMPLEMENTS)
        .and_then(serde_json::Value::as_array_mut)
        .map(|items| {
            let n = items.len();
            items.clear();
            n
        })
        .unwrap_or(0);

    for items in edges
        .values_mut()
        .filter_map(serde_json::Value::as_array_mut)
    {
        for item in items {
            if let Some(obj) = item.as_object_mut() {
                if obj.contains_key(prop::INSPECTION_STATUS) {
                    obj.insert(
                        prop::INSPECTION_STATUS.to_string(),
                        serde_json::json!("uninspected"),
                    );
                }
                for key in [prop::EVIDENCE, prop::LAST_INSPECTED, prop::INSPECTED_BY] {
                    if obj.contains_key(key) {
                        obj.insert(key.to_string(), serde_json::json!(""));
                    }
                }
                for key in [prop::CONFIDENCE, prop::PRIORITY_SCORE] {
                    if obj.contains_key(key) {
                        obj.insert(key.to_string(), serde_json::json!(0));
                    }
                }
            }
        }
    }

    Ok((skipped_nodes, skipped_edges))
}

fn read_export(root: &std::path::Path, file: &str) -> Result<serde_json::Value> {
    let raw = fs::read_to_string(root.join(file))
        .map_err(|e| anyhow::anyhow!("Cannot read '{}': {} — expects a `loom export` JSON (e.g. `loom import loom.graph.json`).", file, e))?;
    let data: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("'{}' is not valid JSON: {} — expects a `loom export` JSON (e.g. `loom import loom.graph.json`).", file, e))?;

    // CodeFile paths are later read from disk by `loom sync` (content hash +
    // locator probes): a path escaping the graph root would turn the graph
    // into a contents oracle against any readable file. Reject at the
    // boundary, in the two-phase spirit — a hostile export never half-imports.
    if let Some(items) = data.pointer("/nodes/CodeFile").and_then(|v| v.as_array()) {
        for item in items {
            if let Some(p) = item.get("path").and_then(|v| v.as_str()) {
                if crate::repo::confine(root, std::path::Path::new(p)).is_none() {
                    anyhow::bail!(
                        "Export contains CodeFile path '{}' that escapes the graph root {} — \
                         edit the offending `nodes.CodeFile[].path` in '{}' to a root-relative \
                         path (or delete that node) and re-run `loom import`. Nothing was imported.",
                        p,
                        root.display(),
                        file
                    );
                }
            }
        }
    }

    // Refuse an export NEWER than this binary understands. An older loom
    // re-inserting a newer graph either hard-fails mid-transaction on a value
    // outside an older CHECK constraint (after clear_all has already run) or
    // silently drops additive columns and then stamps the graph with its OWN
    // (lower) schema_version — so even doctor's post-import version check can't
    // see the loss. Reject at the boundary; nothing is imported. (Older→newer is
    // fine: the schema is created on open and import normalizes to the active
    // version.)
    let binary_version: u32 = crate::db::schema::SCHEMA_VERSION.parse().unwrap_or(0);
    let export_version = data.get("schema_version").and_then(|v| {
        v.as_str()
            .and_then(|s| s.parse::<u32>().ok())
            .or_else(|| v.as_u64().map(|n| n as u32))
    });
    if let Some(export_version) = export_version {
        if export_version > binary_version {
            anyhow::bail!(
                "Export '{}' is schema v{} but this loom understands only v{}. Importing a \
                 newer graph into an older binary corrupts it (values outside this binary's \
                 constraints, or silently dropped fields). Upgrade loom, or re-export from a \
                 v{}-compatible loom. Nothing was imported.",
                file,
                export_version,
                binary_version,
                binary_version
            );
        }
    }
    Ok(data)
}
