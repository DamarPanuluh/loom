use anyhow::Result;
use std::collections::HashSet;
use std::env;
use uuid::Uuid;

use crate::cli::CodefileCmd;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::db::queries::{insert_codefile, list_codefiles};
use crate::output::Printer;
use crate::types::CodeFile;

pub fn run(cmd: CodefileCmd, printer: &Printer) -> Result<()> {
    let cwd = env::current_dir()?;
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
                let cf = CodeFile {
                    id:            Uuid::new_v4().to_string(),
                    path:          p.clone(),
                    language:      language.clone().unwrap_or_else(|| detect_language(&p)),
                    // Stamp the current mtime so the first `loom sync` is a no-op
                    // and only genuine later edits ripple needs_reverification.
                    last_modified: crate::repo::mtime_rfc3339(&cwd.join(&p)).unwrap_or_default(),
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
                println!("  → Next: ground an intent to it — `loom edge implement <intent> {} --locator \"fn …\"`", cf.id);
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
