//! `loom seed --suggest` — mine CANDIDATE intents from the repo's physical layer
//! so a cold agent on an unknown repo starts from a DRAFT graph, not a blank one.
//!
//! It SUGGESTS, never writes: each candidate names a code unit and its public
//! surface and emits pre-filled `loom intent add` / `codefile add` /
//! `edge implement` commands. The agent must REWRITE each description from "what
//! the code does" into "what it's SUPPOSED to do" (the falsifiable intent) before
//! adopting — code structure is a scaffold, not the intent. Fits the same
//! teach/adapt shape as `loom vocab suggest`.

use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;

use crate::output::Printer;

struct Candidate {
    name: String,
    path: String,
    level: String,
    doc: Option<String>,
    public_symbols: Vec<String>,
    locator: String,
    grade: String,
}

pub fn run(inbox: bool, suggest: bool, limit: usize, printer: &Printer) -> Result<()> {
    let root = crate::db::resolve_root()?;

    if inbox {
        return run_inbox_ingest(&root, printer);
    }

    if !suggest {
        let hint = "Run `loom seed --inbox` to ingest the repo into the inbox to triage (the \
                    full-coverage seed), or `loom seed --suggest` to mine candidate intents.";
        if printer.json {
            printer.print_json(&serde_json::json!({ "next_step": hint }));
        } else {
            println!("{hint}");
        }
        return Ok(());
    }

    // Files already grounded by an IMPLEMENTS edge are skipped (re-runnable on a
    // partial graph). Empty when there's no graph yet (the cold-start case).
    let grounded = already_grounded_paths(&root);
    let mut candidates = mine_candidates(&root, &grounded)?;
    // Strongest API surface first; show `limit` (0 = all).
    candidates.sort_by(|a, b| {
        b.public_symbols
            .len()
            .cmp(&a.public_symbols.len())
            .then_with(|| a.path.cmp(&b.path))
    });
    let total = candidates.len();
    let shown = if limit == 0 { total } else { total.min(limit) };
    candidates.truncate(shown);

    if printer.json {
        render_json(&candidates, total, shown, printer);
    } else {
        render_human(&candidates, total, shown);
    }
    Ok(())
}

/// Ingest the full repo surface (every doc + unmodeled source file) into the
/// inbox as triage items — the disciplined, full-coverage seed: the LLM is forced
/// to process the WHOLE system, not a sketch. Empty repo → seeds one VISION prompt
/// instead (ask the user for the system's intent). Idempotent: skips files already
/// modeled (grounded) or already ingested.
fn run_inbox_ingest(root: &Path, printer: &Printer) -> Result<()> {
    use crate::db::queries::comprehensiveness::is_doc_file;
    let db_file = crate::db::ensure_initialized(root)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&db_file)?;
    let snapshot = store.query_snapshot()?;

    let grounded: HashSet<&str> = snapshot
        .implements
        .iter()
        .map(|im| im.codefile_path.as_str())
        .collect();
    let existing = store.list_inbox_items(None, None)?;
    let ingested: HashSet<String> = existing
        .iter()
        .filter_map(|it| ingest_path(&it.raw_text))
        .collect();

    // The full surface: source files + documentation.
    let mut files: Vec<String> = crate::repo::walk_files(root)?
        .into_iter()
        .filter(|p| !crate::repo::lang_of(p).is_empty() || is_doc_file(p))
        .collect();
    files.sort();

    let now = chrono::Utc::now().to_rfc3339();
    let author = crate::agent::acting(None);

    // EMPTY repo → seed a single VISION prompt (ask for the system's intent).
    if files.is_empty() {
        if !ingested.iter().any(|p| p == "<vision>") {
            let item = ingest_item(
                "<vision>".to_string(),
                "This repo has no code or docs yet. Capture the system VISION: what should it do, \
                 who is it for, what responsibilities must it have? Triage into PLANNED intents \
                 (lifecycle=planned) to build, then realize + prove them."
                    .to_string(),
                &author,
                &now,
            );
            store.insert_inbox_item(&item)?;
        }
        let msg =
            "Empty repo — seeded a VISION prompt in the inbox. Capture the vision (edit/extend \
                   it), then `loom inbox triage` to decompose it into planned intents to build.";
        if printer.json {
            printer.print_json(
                &serde_json::json!({ "ingested": 0, "vision_seeded": true, "next_step": msg }),
            );
        } else {
            println!("{msg}");
        }
        return Ok(());
    }

    let mut created = 0usize;
    for path in &files {
        if grounded.contains(path.as_str()) || ingested.contains(path) {
            continue;
        }
        let hint = if is_doc_file(path) {
            "It is a DOC/SPEC — a contract, not a built system. Triage into PLANNED intent(s) to build."
        } else {
            "It is CODE — if it is existing working code, model the responsibilities it implements as \
             intent(s) and ground them (realized); if it reveals a gap, add planned intent(s)."
        };
        let item = ingest_item(
            path.clone(),
            format!("Triage repo file `{path}`. {hint} Add vocabulary for any new domain terms."),
            &author,
            &now,
        );
        store.insert_inbox_item(&item)?;
        created += 1;
    }

    let next = "Process the inbox to decompose the repo: `loom inbox triage` one item at a time, \
                normalizing each into intent(s). DONE when the inbox is empty AND every resulting \
                intent is realized + proven (`loom complete`).";
    if printer.json {
        printer.print_json(&serde_json::json!({
            "ingested": created,
            "already_modeled_or_ingested": files.len() - created,
            "next_step": next,
        }));
    } else {
        println!("── loom seed --inbox ─────────────────────────────────────────────────");
        println!("  Ingested {created} repo file(s) into the inbox as triage items.");
        if files.len() > created {
            println!(
                "  ({} already modeled or ingested — skipped.)",
                files.len() - created
            );
        }
        println!();
        println!("  → {next}");
    }
    Ok(())
}

/// A fresh inbox item anchoring a repo path. The `ingest: <path>` first line makes
/// re-runs idempotent (parsed back by `ingest_path`).
fn ingest_item(path: String, prompt: String, author: &str, now: &str) -> crate::types::InboxItem {
    crate::types::InboxItem {
        id: uuid::Uuid::new_v4().to_string(),
        raw_text: format!("ingest: {path}\n{prompt}"),
        normalized_claim: String::new(),
        kind: "observation".to_string(),
        status: "new".to_string(),
        source: "code_audit".to_string(),
        author: author.to_string(),
        tags: Vec::new(),
        links: Vec::new(),
        route_kind: String::new(),
        route_command: String::new(),
        route_target_kind: String::new(),
        route_target_id: String::new(),
        resolution: String::new(),
        created_at: now.to_string(),
        updated_at: now.to_string(),
    }
}

/// The repo path an ingest item anchors, from its `ingest: <path>` first line.
fn ingest_path(raw_text: &str) -> Option<String> {
    raw_text
        .lines()
        .next()?
        .strip_prefix("ingest: ")
        .map(str::to_string)
}

/// Repo-relative paths already grounded by an IMPLEMENTS edge (read-only; empty
/// if no graph exists yet).
fn already_grounded_paths(root: &Path) -> HashSet<String> {
    if !crate::db::sqlite_db_path(root).exists() {
        return HashSet::new();
    }
    let Ok(store) = crate::db::GraphReadHandle::open(root) else {
        return HashSet::new();
    };
    use crate::db::GraphReadRepository;
    match store.query_snapshot() {
        Ok(snap) => snap
            .implements
            .iter()
            .map(|im| im.codefile_path.clone())
            .collect(),
        Err(_) => HashSet::new(),
    }
}

fn mine_candidates(root: &Path, grounded: &HashSet<String>) -> Result<Vec<Candidate>> {
    let mut out = Vec::new();
    for path in crate::repo::walk_files(root)? {
        if grounded.contains(&path) || crate::repo::lang_of(&path).is_empty() {
            continue; // not source, or already mapped
        }
        let Ok(content) = std::fs::read_to_string(root.join(&path)) else {
            continue; // unreadable / non-utf8
        };
        let facts = crate::repo::extract_physical_facts(root, &path, &content);
        // The public, non-test API surface is what an intent grounds onto.
        let public: Vec<&crate::types::SymbolFact> = facts
            .symbol_facts
            .iter()
            .filter(|f| f.visibility == "public" && !f.is_test)
            .collect();
        if public.is_empty() {
            continue; // no surface to ground — skip (a leaf with no public API)
        }
        let locator = public[0].label.clone();
        out.push(Candidate {
            name: humanize_path(&path),
            level: "feature".to_string(),
            doc: leading_doc(&content),
            public_symbols: public.iter().map(|f| f.label.clone()).collect(),
            locator,
            grade: facts.extractor_grade,
            path,
        });
    }
    Ok(out)
}

/// Turn a repo-relative source path into a draft intent name: drop a leading
/// `src/`/`lib/`/`app/`, the extension, and `mod`/`lib`/`main` stems, then turn
/// path + `_`/`-` separators into spaces. `src/db/expr_parser.rs` → `db expr parser`.
fn humanize_path(path: &str) -> String {
    let mut p = path;
    for prefix in ["src/", "lib/", "app/", "./"] {
        if let Some(rest) = p.strip_prefix(prefix) {
            p = rest;
        }
    }
    let no_ext = p.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(p);
    let words: Vec<&str> = no_ext
        .split(['/', '_', '-'])
        .filter(|w| !w.is_empty() && !matches!(*w, "mod" | "lib" | "main" | "index"))
        .collect();
    let name = words.join(" ");
    if name.is_empty() {
        no_ext.replace(['/', '_', '-'], " ")
    } else {
        name
    }
}

/// The leading documentation of a file — the description DRAFT. Handles a
/// Python-style triple-quoted module docstring (`"""…"""` / `'''…'''`),
/// `//`-style (rust/go/js/ts/dart/kotlin/swift/c/c++/java) and `#`-style
/// (python/ruby) leading comment blocks; skips a shebang. `None` if there's none.
fn leading_doc(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    // Skip leading blanks / shebang to the first content line.
    let mut start = 0;
    while start < lines.len() {
        let t = lines[start].trim();
        if t.is_empty() || t.starts_with("#!") {
            start += 1;
        } else {
            break;
        }
    }
    if start >= lines.len() {
        return None;
    }

    // Triple-quoted module docstring (Python).
    let first = lines[start].trim_start();
    for quote in ["\"\"\"", "'''"] {
        if let Some(rest) = first.strip_prefix(quote) {
            if let Some(end) = rest.find(quote) {
                return non_empty_doc(rest[..end].trim()); // single-line docstring
            }
            let mut collected = vec![rest.trim().to_string()];
            for line in &lines[start + 1..] {
                if let Some(pos) = line.find(quote) {
                    collected.push(line[..pos].trim().to_string());
                    return non_empty_doc(collected.join(" ").trim());
                }
                collected.push(line.trim().to_string());
            }
            return non_empty_doc(collected.join(" ").trim());
        }
    }

    // Leading `//` / `#` line-comment block.
    let mut collected: Vec<String> = Vec::new();
    for raw in &lines[start..] {
        let line = raw.trim_start();
        let body = if let Some(s) = line.strip_prefix("//!") {
            Some(s)
        } else if let Some(s) = line.strip_prefix("///") {
            Some(s)
        } else if let Some(s) = line.strip_prefix("//") {
            Some(s)
        } else if line.starts_with('#') && !line.starts_with("#!") {
            Some(line.trim_start_matches('#'))
        } else {
            None
        };
        match body {
            Some(s) => {
                let s = s.trim();
                if !s.is_empty() {
                    collected.push(s.to_string());
                }
            }
            None => break,
        }
    }
    non_empty_doc(collected.join(" ").trim())
}

fn non_empty_doc(doc: &str) -> Option<String> {
    if doc.is_empty() {
        None
    } else {
        Some(truncate_chars(doc, 200))
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => format!("{}…", &s[..idx]),
        None => s.to_string(),
    }
}

fn grade_label(grade: &str) -> &'static str {
    match grade {
        "high" => "tree-sitter",
        "low" => "heuristic",
        _ => "ungraded",
    }
}

fn render_human(candidates: &[Candidate], total: usize, shown: usize) {
    println!("── loom seed --suggest ──────────────────────────────────────────────");
    if total == 0 {
        println!("  No public source symbols found to seed from.");
        println!(
            "  → Capture intents directly: `loom guide --mode seed`, then \
             `loom intent add --level system …`."
        );
        return;
    }
    println!(
        "  {total} candidate intent(s) mined from your code. These are DRAFTS — each NAMES a code"
    );
    println!(
        "  unit; REWRITE its description into what it's SUPPOSED to do (a falsifiable intent),"
    );
    println!("  adopt the ones that name a real responsibility, reject the rest.");
    println!();
    for c in candidates {
        println!(
            "  [{}]  {}   {}  ({})",
            c.level,
            c.name,
            c.path,
            grade_label(&c.grade)
        );
        if let Some(doc) = &c.doc {
            println!("    doc:    {doc}");
        }
        let syms = if c.public_symbols.len() > 4 {
            format!(
                "{}, … +{} more",
                c.public_symbols[..4].join(", "),
                c.public_symbols.len() - 4
            )
        } else {
            c.public_symbols.join(", ")
        };
        println!("    public: {syms}");
        println!(
            "    adopt:  loom intent add --name \"{}\" --level {} --description \"<the SUPPOSED-TO>\"",
            c.name, c.level
        );
        println!(
            "            loom codefile add {} && loom edge implement \"{}\" {} --locator \"{}\"",
            c.path, c.name, c.path, c.locator
        );
        println!();
    }
    if shown < total {
        println!(
            "  … +{} more — `loom seed --suggest --limit 0` for all.",
            total - shown
        );
    }
    println!(
        "  → Next: adopt candidates (edit each description into an intent), then `loom status` \
         to drive the loop."
    );
}

fn render_json(candidates: &[Candidate], total: usize, shown: usize, printer: &Printer) {
    let items: Vec<serde_json::Value> = candidates
        .iter()
        .map(|c| {
            serde_json::json!({
                "name": c.name,
                "path": c.path,
                "level": c.level,
                "doc": c.doc,
                "public_symbols": c.public_symbols,
                "suggested_locator": c.locator,
                "extractor_grade": c.grade,
                "adopt": [
                    format!("loom intent add --name \"{}\" --level {} --description \"<the SUPPOSED-TO>\"", c.name, c.level),
                    format!("loom codefile add {}", c.path),
                    format!("loom edge implement \"{}\" {} --locator \"{}\"", c.name, c.path, c.locator),
                ],
            })
        })
        .collect();
    printer.print_json(&serde_json::json!({
        "candidates": items,
        "total": total,
        "shown": shown,
        "truncated": shown < total,
        "note": "DRAFTS mined from code structure — rewrite each description into a falsifiable intent (what it is SUPPOSED to do) before adopting; SUGGEST-only, nothing was written.",
        "next_step": "Adopt the candidates that name a real responsibility (run their `adopt` commands, editing each description), then `loom status` to drive the loop.",
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize_drops_prefix_ext_and_noise() {
        assert_eq!(humanize_path("src/db/expr_parser.rs"), "db expr parser");
        assert_eq!(humanize_path("lib/auth.py"), "auth");
        assert_eq!(humanize_path("src/parser/mod.rs"), "parser"); // `mod` filtered
        assert_eq!(
            humanize_path("app/widgets/button-bar.tsx"),
            "widgets button bar"
        );
    }

    #[test]
    fn leading_doc_handles_every_comment_style() {
        assert_eq!(
            leading_doc("//! Rust module doc.\npub fn a() {}").as_deref(),
            Some("Rust module doc.")
        );
        assert_eq!(
            leading_doc("// Package store persists records.\npackage store").as_deref(),
            Some("Package store persists records.")
        );
        assert_eq!(
            leading_doc("# Python line comment.\nx = 1").as_deref(),
            Some("Python line comment.")
        );
        // Triple-quoted docstring, single + multi line.
        assert_eq!(
            leading_doc("\"\"\"One-line docstring.\"\"\"\ndef f(): pass").as_deref(),
            Some("One-line docstring.")
        );
        assert_eq!(
            leading_doc("'''Multi\nline doc.'''\ndef f(): pass").as_deref(),
            Some("Multi line doc.")
        );
        // Shebang is skipped; a non-comment first line yields nothing.
        assert_eq!(
            leading_doc("#!/usr/bin/env python\n# real doc\nx=1").as_deref(),
            Some("real doc")
        );
        assert_eq!(leading_doc("pub fn a() {}").as_deref(), None);
    }
}
