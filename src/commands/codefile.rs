use anyhow::Result;
use std::collections::HashSet;
use uuid::Uuid;

use crate::cli::CodefileCmd;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::db::queries::{
    get_codefile_by_id_or_path, get_intent, insert_codefile, list_all_implements,
    list_codefiles, list_governs_for_intent,
};
use crate::output::Printer;
use crate::types::CodeFile;

pub fn run(cmd: CodefileCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match cmd {
        CodefileCmd::Add { path, language } => {
            crate::gate::acting_in_lane(
                "register a code file",
                &[crate::db::schema::role::BUILDER],
                None,
            )?;
            // A glob pattern (contains * ? [) registers every matching file;
            // a plain path registers just that one. Already-registered paths are
            // skipped so re-running is safe.
            let is_glob = path.contains('*') || path.contains('?') || path.contains('[');
            let targets: Vec<String> = if is_glob {
                let mut v = Vec::new();
                for entry in glob::glob(&path)
                    .map_err(|e| anyhow::anyhow!("Invalid glob '{}': {}", path, e))?
                {
                    if let Ok(p) = entry {
                        if p.is_file() {
                            v.push(p.display().to_string());
                        }
                    }
                }
                v
            } else {
                vec![path.clone()]
            };

            let existing: HashSet<String> =
                list_codefiles(&db)?.into_iter().map(|c| c.path).collect();

            let mut added: Vec<CodeFile> = Vec::new();
            let mut skipped = 0usize;
            for p in targets {
                if existing.contains(&p) {
                    skipped += 1;
                    continue;
                }
                let abs_path = cwd.join(&p);
                let last_modified = crate::repo::mtime_rfc3339(&abs_path).ok_or_else(|| {
                    anyhow::anyhow!("Cannot read mtime for {}", abs_path.display())
                })?;
                let content_hash = std::fs::read(&abs_path)
                    .map(|b| crate::repo::content_hash(&b))
                    .map_err(|e| {
                        anyhow::anyhow!("Cannot read bytes for {}: {}", abs_path.display(), e)
                    })?;
                let cf = CodeFile {
                    id:            Uuid::new_v4().to_string(),
                    path:          p.clone(),
                    language:      language.clone().unwrap_or_else(|| detect_language(&p)),
                    // Stamp the current mtime + content fingerprint so the first
                    // `loom sync` is a no-op and only genuine later edits ripple
                    // needs_reverification.
                    last_modified,
                    imports:       "[]".to_string(), // populated by `loom sync`
                    content_hash,
                };
                insert_codefile(&db, &cf)?;
                added.push(cf);
            }

            if printer.json {
                printer.print_json(&serde_json::json!({
                    "added":       added,
                    "added_count": added.len(),
                    "skipped":     skipped,
                }));
            } else if added.len() == 1 && skipped == 0 {
                let cf = &added[0];
                println!("✓ CodeFile added  (id: {})", cf.id);
                println!("  path:     {}", cf.path);
                println!("  language: {}", cf.language);
                println!("  → Next: ground an intent to it — `loom edge implement <intent> {} --locator \"<symbol>\"` (symbol as it appears in the file, e.g. `def foo`/`fn foo`)", cf.id);
            } else {
                println!("✓ Registered {} code file(s) ({} already present, skipped).", added.len(), skipped);
                for cf in &added {
                    println!("  + {} [{}]", cf.path, cf.language);
                }
                if !added.is_empty() {
                    println!("  → Next: `loom sync` to stamp mtimes, then ground intents with `loom edge implement`.");
                }
            }
        }

        CodefileCmd::Show { path_or_id } => {
            let Some(cf) = get_codefile_by_id_or_path(&db, &path_or_id)? else {
                anyhow::bail!(
                    "CodeFile '{}' not found (by id or path).\nRun `loom codefile list` to see what is registered.",
                    path_or_id
                );
            };
            // The ownership view: every intent claiming this file (via
            // IMPLEMENTS), each with its abstraction level so cross-cutting
            // claims read differently from a feature owning its home file.
            let claims: Vec<_> = list_all_implements(&db)?
                .into_iter()
                .filter(|im| im.codefile_id == cf.id)
                .collect();
            let mut owners = Vec::new();
            for im in &claims {
                let intent = get_intent(&db, &im.intent_id)?;
                let (level, lifecycle) = intent
                    .map(|i| (i.abstraction_level, i.lifecycle))
                    .unwrap_or_default();
                owners.push(serde_json::json!({
                    "intent_id": im.intent_id,
                    "intent_name": im.intent_name,
                    "level": level,
                    "lifecycle": lifecycle,
                    "locator": im.locator,
                    "inspection_status": im.inspection_status,
                }));
            }
            // Quality rules reaching this file through its owning intents.
            let mut rules: Vec<serde_json::Value> = Vec::new();
            let mut seen_rules = HashSet::new();
            for im in &claims {
                for g in list_governs_for_intent(&db, &im.intent_id)? {
                    if seen_rules.insert(format!("{}|{}", g.rule_id, g.intent_id)) {
                        rules.push(serde_json::json!({
                            "rule": g.rule_name,
                            "via_intent": g.intent_name,
                            "inspection_status": g.inspection_status,
                        }));
                    }
                }
            }
            let imports: Vec<String> = match serde_json::from_str(&cf.imports) {
                Ok(imports) => imports,
                Err(e) => vec![format!("(invalid imports JSON: {e})")],
            };
            let tangled = claims.len() >= crate::db::queries::smells::TANGLE_INTENTS;

            if printer.json {
                printer.print_json(&serde_json::json!({
                    "codefile": cf,
                    "owners": owners,
                    "owner_count": owners.len(),
                    "tangled": tangled,
                    "governing_rules": rules,
                    "imports": imports,
                }));
            } else {
                println!("── CodeFile ───────────────────────────────────────────────────────");
                println!("  path:      {}", cf.path);
                println!("  language:  {}", cf.language);
                println!("  modified:  {}", if cf.last_modified.is_empty() { "(never synced)" } else { &cf.last_modified });
                println!("  id:        {}", cf.id);
                println!();
                println!("── Owned by ({} intent(s)){} ────────────────────────────────────────",
                    owners.len(),
                    if tangled { "  ⚠ TANGLED" } else { "" });
                if claims.is_empty() {
                    println!("  (none — unexplained code; ground it: `loom edge implement <intent> {}`)", cf.path);
                } else {
                    for im in &claims {
                        let loc = if im.locator.is_empty() { String::new() } else { format!("  @ {}", im.locator) };
                        let intent = get_intent(&db, &im.intent_id)?;
                        let level = intent.map(|i| i.abstraction_level).unwrap_or_default();
                        println!("  [{:<13}] {}{}  ({})", level, im.intent_name, loc, im.intent_id);
                    }
                }
                if tangled {
                    println!("  ⚠ {} intents in one file (threshold {}) — consider splitting along intent lines.",
                        claims.len(), crate::db::queries::smells::TANGLE_INTENTS);
                }
                println!();
                println!("── Governing rules (via owners) ────────────────────────────────────");
                if rules.is_empty() {
                    println!("  (none)");
                } else {
                    for r in &rules {
                        println!("  [{:<20}] {}  (via '{}')",
                            r["inspection_status"].as_str().unwrap_or(""),
                            r["rule"].as_str().unwrap_or(""),
                            r["via_intent"].as_str().unwrap_or(""));
                    }
                }
                if !imports.is_empty() {
                    println!();
                    println!("── Imports ({}) ─────────────────────────────────────────────────────", imports.len());
                    for i in &imports {
                        println!("  → {}", i);
                    }
                }
            }
        }

        CodefileCmd::Remove { path_or_id } => {
            crate::gate::acting_in_lane(
                "remove a code file",
                &[crate::db::schema::role::BUILDER],
                None,
            )?;
            let removed = crate::db::queries::delete_codefile(&db, &path_or_id)?;
            let Some(cf) = removed else {
                anyhow::bail!(
                    "CodeFile '{}' not found (by id or path).\nRun `loom codefile list` to see what is registered.",
                    path_or_id
                );
            };
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status":  "ok",
                    "removed": cf,
                    "message": "CodeFile and its IMPLEMENTS edges removed. Intents grounded \
                                only here are unrealized again — `loom status` will route to ground.",
                }));
            } else {
                println!("✓ CodeFile removed (with its IMPLEMENTS edges): {}", cf.path);
                println!("  Intents grounded only here are unrealized again — check `loom status`.");
            }
        }

        CodefileCmd::List => {
            let files = list_codefiles(&db)?;
            if printer.json {
                printer.print_json(&files);
            } else if files.is_empty() {
                println!("(no code files registered)");
            } else {
                println!(
                    "  {language:<15}  {mtime:<26}  {path:<50}  id",
                    language = "LANGUAGE",
                    mtime    = "LAST MODIFIED",
                    path     = "PATH",
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
                        lang  = cf.language,
                        mtime = mtime,
                        path  = cf.path,
                        id    = cf.id,
                    );
                }
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
        "rs"               => "rust",
        "ts" | "tsx"       => "typescript",
        "js" | "jsx" | "mjs" => "javascript",
        "py"               => "python",
        "go"               => "go",
        "java"             => "java",
        "kt"               => "kotlin",
        "swift"            => "swift",
        "c" | "h"          => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "cs"               => "csharp",
        "rb"               => "ruby",
        "php"              => "php",
        "sh" | "bash"      => "shell",
        "sql"              => "sql",
        "html" | "htm"     => "html",
        "css" | "scss"     => "css",
        _                  => "unknown",
    }
    .to_string()
}
