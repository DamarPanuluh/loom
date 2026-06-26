use anyhow::Result;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::cli::CodefileCmd;
use crate::db::{ensure_initialized, GraphReadHandle, GraphReadRepository};
use crate::output::Printer;
use crate::types::CodeFile;

/// The single user-facing "CodeFile not found" contract, shared by every
/// lookup-by-key surface (remove, show, and note's codefile resolver) so the
/// message can't drift between them.
pub(crate) fn codefile_not_found(key: &str) -> String {
    format!(
        "CodeFile '{key}' not found (by id or path).\nRun `loom codefile list` to see what is registered."
    )
}

pub fn run(cmd: CodefileCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    match cmd {
        CodefileCmd::List { limit } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_list_with_db(&db, limit, printer)
        }
        CodefileCmd::Show { path_or_id } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_show_with_db(&db, path_or_id, printer)
        }
        CodefileCmd::Add { path, language } => {
            ensure_initialized(&cwd)?;
            run_add_with_sqlite(&cwd, path, language, printer)
        }
        CodefileCmd::Remove { path_or_id } => {
            ensure_initialized(&cwd)?;
            run_remove_with_sqlite(&cwd, path_or_id, printer)
        }
    }
}

fn run_add_with_sqlite(
    root: &std::path::Path,
    path: String,
    language: Option<String>,
    printer: &Printer,
) -> Result<()> {
    crate::gate::acting_in_lane(&crate::gate::lane::ADD_CODEFILE, None)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let existing: HashSet<String> = store
        .list_codefiles()?
        .into_iter()
        .map(|codefile| codefile.path)
        .collect();
    let (added, skipped) = prepare_additions(root, path, language, &existing)?;
    for codefile in &added {
        store.insert_codefile(codefile)?;
    }
    print_add_result(&added, skipped, printer);
    Ok(())
}

fn run_remove_with_sqlite(
    root: &std::path::Path,
    path_or_id: String,
    printer: &Printer,
) -> Result<()> {
    crate::gate::acting_in_lane(&crate::gate::lane::REMOVE_CODEFILE, None)?;
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let Some(cf) = store.delete_codefile(&path_or_id)? else {
        anyhow::bail!(codefile_not_found(&path_or_id));
    };
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status":    "ok",
            "removed":   cf,
            "next_step": "CodeFile and its IMPLEMENTS edges removed. Intents grounded \
                          only here are unrealized again — `loom status` will route to ground.",
        }));
    } else {
        println!(
            "✓ CodeFile removed (with its IMPLEMENTS edges): {}",
            cf.path
        );
        println!("  Intents grounded only here are unrealized again — check `loom status`.");
    }
    Ok(())
}

fn prepare_additions(
    root: &std::path::Path,
    path: String,
    language: Option<String>,
    existing: &HashSet<String>,
) -> Result<(Vec<CodeFile>, usize)> {
    // A glob pattern (contains * ? [) registers every matching file; a plain
    // path registers just that one. Already-registered paths are skipped so
    // re-running is safe.
    let is_glob = path.contains('*') || path.contains('?') || path.contains('[');
    let targets: Vec<String> = if is_glob {
        let mut v = Vec::new();
        // Resolve the glob against the GRAPH ROOT, not the process cwd — so a
        // pinned `LOOM_GRAPH` globs the graph's repo no matter where you run
        // from (matches the literal-path branch below + the graph-targeting
        // contract).
        let rooted = root.join(&path);
        let pattern = rooted.to_string_lossy();
        for p in glob::glob(&pattern)
            .map_err(|e| anyhow::anyhow!(crate::output::invalid_glob_msg(&path, &e)))?
            .flatten()
        {
            // Skip symlinks: registering a link AND its target (both glob-matched)
            // would confine to the same canonical path and hit a raw UNIQUE error
            // mid-batch. `is_file()` follows links, so check symlink_metadata.
            let is_symlink = p
                .symlink_metadata()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            if p.is_file() && !is_symlink {
                v.push(p.display().to_string());
            }
        }
        // A glob matching zero files is a silent no-op trap: `edge implement`'s
        // own error tells the AI to run `loom codefile add '<glob>'`, so a false
        // "✓ Registered 0" sends it in circles. Fail loudly with the pattern.
        if v.is_empty() {
            anyhow::bail!(
                "Glob '{}' matched 0 files on disk under {} — nothing to register. \
                 Check the pattern (quote it: `loom codefile add 'src/**/*.rs'`) or the graph root; \
                 a plain path (no * ? [) registers a single file even if absent.",
                path,
                root.display()
            );
        }
        v
    } else {
        vec![path.clone()]
    };

    let mut added: Vec<CodeFile> = Vec::new();
    let mut skipped = 0usize;
    // Two glob matches can confine to the SAME root-relative path (a symlink and
    // its target, or `**` reaching one file two ways). Dedupe within the batch so
    // the second never reaches the INSERT and trips a raw UNIQUE constraint.
    let mut seen_this_batch: std::collections::HashSet<String> = std::collections::HashSet::new();
    for p in targets {
        // Normalize against the graph root: `..`-escapes and outside paths are
        // rejected, absolute-under-root comes back relative (the stored
        // convention — paths must travel across machines).
        let Some(p) = crate::repo::confine(root, std::path::Path::new(&p)) else {
            anyhow::bail!(
                "Path '{}' escapes the graph root {} — register files inside the \
                 repository (paths are stored root-relative).",
                p,
                root.display()
            );
        };
        if existing.contains(&p) || !seen_this_batch.insert(p.clone()) {
            skipped += 1;
            continue;
        }
        let abs_path = root.join(&p);
        let last_modified = crate::repo::mtime_rfc3339(&abs_path).ok_or_else(|| {
            anyhow::anyhow!(
                "Cannot read mtime for {} — restore the file or remove the registration \
                 (`loom codefile remove <path>`), then `loom sync`.",
                abs_path.display()
            )
        })?;
        let bytes = std::fs::read(&abs_path).map_err(|e| {
            anyhow::anyhow!(
                "Cannot read bytes for {}: {} — restore the file or remove the registration \
                 (`loom codefile remove <path>`), then `loom sync`.",
                abs_path.display(),
                e
            )
        })?;
        let content_hash = crate::repo::content_hash(&bytes);
        let content = String::from_utf8_lossy(&bytes);
        let facts = crate::repo::extract_physical_facts(root, &p, &content);
        let codefile = CodeFile {
            id: Uuid::new_v4().to_string(),
            path: p.clone(),
            language: language.clone().unwrap_or_else(|| detect_language(&p)),
            // Stamp hash and physical facts immediately. Otherwise a freshly
            // registered file can be `touch`ed before its first sync and fall
            // back to mtime-only drift, while a hash-only registration would
            // leave the file symbol-less until a real edit.
            last_modified,
            imports: facts.imports,
            symbols: facts.symbols,
            symbol_facts: facts.symbol_facts,
            content_hash,
            extractor_grade: facts.extractor_grade,
        };
        added.push(codefile);
    }

    Ok((added, skipped))
}

fn print_add_result(added: &[CodeFile], skipped: usize, printer: &Printer) {
    if printer.json {
        // next_step parity with the human branch below.
        let mut payload = serde_json::json!({
            "added":       added,
            "added_count": added.len(),
            "skipped":     skipped,
        });
        if added.len() == 1 && skipped == 0 {
            payload["next_step"] = serde_json::Value::String(format!(
                "ground an intent to it — `loom edge implement <intent> {} --locator \"<symbol>\"` \
                 (symbol as it appears in the file, e.g. `def foo`/`fn foo`)",
                added[0].id
            ));
        } else if !added.is_empty() {
            payload["next_step"] = serde_json::Value::String(
                "physical facts are stamped; ground intents with `loom edge implement`."
                    .to_string(),
            );
        }
        printer.print_json(&payload);
    } else if added.len() == 1 && skipped == 0 {
        let cf = &added[0];
        println!("✓ CodeFile added  (id: {})", cf.id);
        println!("  path:     {}", cf.path);
        println!("  language: {}", cf.language);
        println!("  → Next: ground an intent to it — `loom edge implement <intent> {} --locator \"<symbol>\"` (symbol as it appears in the file, e.g. `def foo`/`fn foo`)", cf.id);
    } else {
        println!(
            "✓ Registered {} code file(s) ({} already present, skipped).",
            added.len(),
            skipped
        );
        for cf in added {
            println!("  + {} [{}]", cf.path, cf.language);
        }
        if !added.is_empty() {
            println!(
                "  → Next: physical facts are stamped; ground intents with `loom edge implement`."
            );
        }
    }
}

fn run_list_with_db(db: &dyn GraphReadRepository, limit: usize, printer: &Printer) -> Result<()> {
    let mut files = db.query_snapshot()?.codefiles;
    let total = crate::output::apply_limit(&mut files, limit);
    if printer.json {
        printer.print_json(&serde_json::json!({
            "codefiles": files,
            "total":     total,
            "truncated": files.len() < total,
        }));
    } else if files.is_empty() {
        println!("(no code files registered)");
    } else {
        println!(
            "  {language:<15}  {mtime:<26}  {path:<50}  id",
            language = "LANGUAGE",
            mtime = "LAST MODIFIED",
            path = "PATH",
        );
        println!("  {}", "-".repeat(110));
        for cf in &files {
            let mtime = if cf.last_modified.is_empty() {
                "(never synced)".to_string()
            } else {
                cf.last_modified.clone()
            };
            println!(
                "  {lang:<15}  {mtime:<26}  {path:<50}  {id}",
                lang = cf.language,
                mtime = mtime,
                path = cf.path,
                id = cf.id,
            );
        }
        if let Some(m) =
            crate::output::more_marker(total, files.len(), "`loom codefile list --limit 0`")
        {
            println!("  {m}");
        }
    }
    Ok(())
}

/// Human-readable trust label for a codefile's `extractor_grade`, so the reader
/// knows how much to trust its symbol facts.
fn grade_label(grade: &str) -> &'static str {
    match grade {
        "high" => "tree-sitter (high-fidelity)",
        "low" => "heuristic (low-fidelity)",
        "none" => "no extractor for this language",
        _ => "ungraded — run `loom sync` to refresh",
    }
}

fn run_show_with_db(
    db: &dyn GraphReadRepository,
    path_or_id: String,
    printer: &Printer,
) -> Result<()> {
    let snapshot = db.query_snapshot()?;
    let Some(cf) = snapshot
        .codefiles
        .iter()
        .find(|codefile| codefile.id == path_or_id || codefile.path == path_or_id)
        .cloned()
    else {
        anyhow::bail!(codefile_not_found(&path_or_id));
    };

    // The ownership view: every intent claiming this file (via IMPLEMENTS),
    // each with its abstraction level so cross-cutting claims read differently
    // from a feature owning its home file.
    let claims: Vec<_> = snapshot
        .implements
        .iter()
        .filter(|im| im.codefile_id == cf.id)
        .cloned()
        .collect();
    let intent_meta: HashMap<String, (String, String)> = snapshot
        .intents
        .iter()
        .map(|intent| {
            (
                intent.id.clone(),
                (intent.abstraction_level.clone(), intent.lifecycle.clone()),
            )
        })
        .collect();
    let mut owners = Vec::new();
    for im in &claims {
        let (level, lifecycle) = intent_meta.get(&im.intent_id).cloned().unwrap_or_default();
        owners.push(serde_json::json!({
            "intent_id": im.intent_id.clone(),
            "intent_name": im.intent_name.clone(),
            "level": level,
            "lifecycle": lifecycle,
            "locator": im.locator.clone(),
            "inspection_status": im.inspection_status.clone(),
        }));
    }

    // Quality rules reaching this file through its owning intents.
    let mut rules: Vec<serde_json::Value> = Vec::new();
    let mut seen_rules = HashSet::new();
    for im in &claims {
        for g in db.list_governs_for_intent(&im.intent_id)? {
            if seen_rules.insert(format!("{}|{}", g.rule_id, g.intent_id)) {
                rules.push(serde_json::json!({
                    "rule": g.rule_name,
                    "via_intent": g.intent_name,
                    "inspection_status": g.inspection_status,
                }));
            }
        }
    }
    let imports = cf.imports.clone();
    let tangled = claims.len() >= crate::db::queries::smells::TANGLE_INTENTS;
    // Notes targeting the file itself — where a tangled_file adjudication
    // (`loom note add --file … --kind decision`) lives.
    let notes = db.notes_for_target(&cf.id)?;
    // Sections inside show are bounded (SECTION_CAP) in human mode; the full
    // view is one command away.
    let fetch = format!("`loom codefile show {} --json`", cf.path);
    let cap = crate::output::SECTION_CAP;

    if printer.json {
        // Bound the sub-sections (invariant 3): a central file can own many
        // intents and accumulate many notes — cap each at SECTION_CAP (notes
        // keeping the NEWEST) and report the true *_total so the agent knows to
        // dig, matching `loom intent show`.
        let owners_json: Vec<_> = owners.iter().take(cap).collect();
        let rules_json: Vec<_> = rules.iter().take(cap).collect();
        let imports_json: Vec<_> = imports.iter().take(cap).collect();
        let notes_json: Vec<_> = notes.iter().skip(notes.len().saturating_sub(cap)).collect();
        printer.print_json(&serde_json::json!({
            "codefile": cf,
            "owners": owners_json,
            "owner_count": owners.len(),
            "owners_total": owners.len(),
            "tangled": tangled,
            "governing_rules": rules_json,
            "governing_rules_total": rules.len(),
            "imports": imports_json,
            "imports_total": imports.len(),
            "notes": notes_json,
            "notes_total": notes.len(),
        }));
    } else {
        println!("── CodeFile ───────────────────────────────────────────────────────");
        println!("  path:      {}", cf.path);
        println!("  language:  {}", cf.language);
        println!(
            "  facts:     {} symbol(s) · {}",
            cf.symbol_facts.len(),
            grade_label(&cf.extractor_grade)
        );
        println!(
            "  modified:  {}",
            if cf.last_modified.is_empty() {
                "(never synced)"
            } else {
                &cf.last_modified
            }
        );
        println!("  id:        {}", cf.id);
        println!();
        println!(
            "── Owned by ({} intent(s)){} ────────────────────────────────────────",
            owners.len(),
            if tangled { "  ⚠ TANGLED" } else { "" }
        );
        if claims.is_empty() {
            println!(
                "  (none — unexplained code; ground it: `loom edge implement <intent> {}`)",
                cf.path
            );
        } else {
            for im in claims.iter().take(cap) {
                let loc = if im.locator.is_empty() {
                    String::new()
                } else {
                    format!("  @ {}", im.locator)
                };
                let level = intent_meta
                    .get(&im.intent_id)
                    .map(|(level, _)| level.as_str())
                    .unwrap_or("");
                println!(
                    "  [{:<13}] {}{}  ({})",
                    level, im.intent_name, loc, im.intent_id
                );
            }
            if let Some(m) = crate::output::more_marker(claims.len(), claims.len().min(cap), &fetch)
            {
                println!("  {m}");
            }
        }
        if tangled {
            println!("  ⚠ {} intents in one file (threshold {}) — split along intent lines, or record why the",
                claims.len(), crate::db::queries::smells::TANGLE_INTENTS);
            println!("    cohabitation is deliberate: `loom note add --file {} --kind decision --text \"…\"`", cf.path);
        }
        println!();
        println!("── Governing rules (via owners) ────────────────────────────────────");
        if rules.is_empty() {
            println!("  (none)");
        } else {
            for r in rules.iter().take(cap) {
                println!(
                    "  [{:<20}] {}  (via '{}')",
                    r["inspection_status"].as_str().unwrap_or(""),
                    r["rule"].as_str().unwrap_or(""),
                    r["via_intent"].as_str().unwrap_or("")
                );
            }
            if let Some(m) = crate::output::more_marker(rules.len(), rules.len().min(cap), &fetch) {
                println!("  {m}");
            }
        }
        if !imports.is_empty() {
            println!();
            println!(
                "── Imports ({}) ─────────────────────────────────────────────────────",
                imports.len()
            );
            for i in imports.iter().take(cap) {
                println!("  → {}", i);
            }
            if let Some(m) =
                crate::output::more_marker(imports.len(), imports.len().min(cap), &fetch)
            {
                println!("  {m}");
            }
        }
        if !notes.is_empty() {
            println!();
            println!(
                "── Notes ({}) ───────────────────────────────────────────────────────",
                notes.len()
            );
            for n in notes.iter().rev().take(cap) {
                println!("  [{}] {}  ({})", n.kind, n.text, n.author);
            }
            if let Some(m) = crate::output::more_marker(notes.len(), notes.len().min(cap), &fetch) {
                println!("  {m}");
            }
        }
    }
    Ok(())
}

/// Guess language from file extension.
fn detect_language(path: &str) -> String {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "rs" => "rust",
        "ts" | "tsx" | "mts" | "cts" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" => "python",
        "dart" => "dart",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp",
        "rb" => "ruby",
        "php" => "php",
        "sh" | "bash" => "shell",
        "sql" => "sql",
        "html" | "htm" => "html",
        "css" | "scss" => "css",
        "svelte" => "svelte",
        _ => "unknown",
    }
    .to_string()
}
