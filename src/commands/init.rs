use anyhow::Result;
use std::fs;
use std::path::Path;

use crate::db::schema::SCHEMA_VERSION;

const SKIPPED_NOT_GIT: &str = "skipped (not a git repo)";
use crate::db::{db_path, loom_dir};
use crate::output::Printer;

pub fn run(
    path_str: &str,
    name: Option<&str>,
    observed: bool,
    no_hook: bool,
    printer: &Printer,
) -> Result<()> {
    let target = Path::new(path_str)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(path_str).to_path_buf());

    let loom = loom_dir(&target);
    let db_file = db_path(&target);

    // Create .loom/ directory if it doesn't exist
    if !loom.exists() {
        fs::create_dir_all(&loom)?;
    }

    // Install the green-bar pre-commit hook (best-effort; never fails init).
    let hook_status = if no_hook {
        "skipped (--no-hook)".to_string()
    } else {
        install_pre_commit_hook(&target).unwrap_or_else(|e| format!("not installed ({e})"))
    };

    // Keep the `.loom/` cache out of version control — only the committed
    // loom.graph.json travels. Idempotent + best-effort; never fails init.
    let gitignore_status = ensure_gitignored(&target);

    let store = crate::db::sqlite::SqliteGraphStore::open(&db_file)?;

    // The graph's default human name is the directory it maps.
    let default_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed")
        .to_string();
    let custody = if observed { "observed" } else { "owned" };

    if let Some(meta) = store.graph_meta()? {
        // Re-running init is safe — and it's also the identity touch-point:
        // backfill a missing graph_id (pre-identity graph), and apply
        // explicitly-passed --name/--observed (init is the only meta writer).
        let new_id = if meta.graph_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            meta.graph_id.clone()
        };
        let new_name = match name {
            Some(n) => n.to_string(),
            None if meta.graph_name.is_empty() => default_name,
            None => meta.graph_name.clone(),
        };
        let new_custody = if observed {
            "observed".to_string()
        } else if meta.custody.is_empty() {
            "owned".to_string()
        } else {
            meta.custody.clone()
        };
        let changed =
            new_id != meta.graph_id || new_name != meta.graph_name || new_custody != meta.custody;
        if changed {
            store.set_identity(&new_id, &new_name, &new_custody)?;
        }
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "ok",
                "message": format!("Already initialised at {}", loom.display()),
                "graph_id": new_id, "graph_name": new_name, "custody": new_custody,
                "identity_updated": changed,
                "pre_commit_hook": hook_status,
                "gitignore": gitignore_status,
            }));
        } else {
            println!(
                "✓ Already initialised at {}  (run again is safe)",
                loom.display()
            );
            println!(
                "  graph: '{}' ({})  custody: {}{}",
                new_name,
                new_id,
                new_custody,
                if changed { "  [identity updated]" } else { "" }
            );
            println!("  pre-commit hook: {hook_status}");
            println!("  .gitignore:      {gitignore_status}");
        }
        return Ok(());
    }

    // Insert the meta node to mark this DB as initialised
    let now = chrono::Utc::now().to_rfc3339();
    let graph_id = uuid::Uuid::new_v4().to_string();
    let graph_name = name.map(str::to_string).unwrap_or(default_name);
    store.initialize(SCHEMA_VERSION, &graph_id, &graph_name, custody, &now)?;

    if printer.json {
        printer.print_json(&serde_json::json!({
            "status":  "ok",
            "message": format!("Initialised loom graph at {}", loom.display()),
            "db":      db_file.display().to_string(),
            "graph_id": graph_id,
            "graph_name": graph_name,
            "custody": custody,
            "pre_commit_hook": hook_status,
            "gitignore": gitignore_status,
            "next_steps": [
                "Read the driving protocol: `loom guide`.",
                "SEED the full surface (anti-sketch): `loom seed --inbox` ingests every doc + source file into the inbox to triage (empty repo → a vision prompt).",
                "Process it: `loom inbox triage` decomposes each item into intents (existing code → realized; spec/gap → planned to build).",
                "Then drive with `loom next` / `loom status`; `loom complete` for comprehensiveness gaps.",
            ],
        }));
    } else {
        println!("✓ Initialised loom graph at {}", loom.display());
        println!("  DB:    {}", db_file.display());
        println!(
            "  graph: '{}' ({})  custody: {}",
            graph_name, graph_id, custody
        );
        println!("  pre-commit hook: {hook_status}");
        println!("  .gitignore:      {gitignore_status}");
        if observed {
            println!("  Observed graph: you're mapping code you don't own — build/fix lanes are");
            println!("  disabled; record findings (issue verdicts, notes), not fixes.");
        }
        println!();
        println!("  → Next: `loom guide` to learn the loop, then SEED the full surface:");
        println!(
            "    `loom seed --inbox` ingests every doc + source file into the inbox to triage"
        );
        println!(
            "    (empty repo → a vision prompt); `loom inbox triage` decomposes each into intents."
        );
    }
    Ok(())
}

/// Ensure the repo's `.gitignore` excludes the `.loom/` cache so only the
/// committed `loom.graph.json` travels. Idempotent + best-effort (a re-run that
/// finds it already there is a no-op; never fails init). Only acts in a git repo.
fn ensure_gitignored(target: &Path) -> String {
    if !target.join(".git").exists() {
        return SKIPPED_NOT_GIT.to_string();
    }
    let gitignore = target.join(".gitignore");
    let existing = fs::read_to_string(&gitignore).unwrap_or_default();
    let already = existing.lines().any(|line| {
        let entry = line.trim().trim_start_matches('/');
        entry == ".loom" || entry == ".loom/"
    });
    if already {
        return "already ignored".to_string();
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str("# loom: the live graph cache — only loom.graph.json travels\n.loom/\n");
    match fs::write(&gitignore, content) {
        Ok(()) => "added `.loom/`".to_string(),
        Err(e) => format!("not written ({e})"),
    }
}

/// Marker line identifying a loom-written pre-commit hook, so a re-run refreshes
/// OURS but never clobbers a hook the user (or another tool) wrote.
const HOOK_MARKER: &str = "# loom-managed pre-commit hook";

/// Install the green-bar git pre-commit hook (best-effort). Returns a status
/// string for the init output; never errors out of init (a missing/locked git
/// dir just reports "skipped").
fn install_pre_commit_hook(target: &Path) -> Result<String> {
    let git_dir = target.join(".git");
    if !git_dir.exists() {
        return Ok(SKIPPED_NOT_GIT.to_string());
    }
    if !git_dir.is_dir() {
        // Worktrees/submodules use a `.git` FILE pointing at the real gitdir;
        // resolving that is out of scope — report rather than guess.
        return Ok("skipped (.git is a file — worktree/submodule)".to_string());
    }
    let hooks_dir = git_dir.join("hooks");
    fs::create_dir_all(&hooks_dir)?;
    let hook_path = hooks_dir.join("pre-commit");
    if hook_path.exists() {
        let existing = fs::read_to_string(&hook_path).unwrap_or_default();
        if !existing.contains(HOOK_MARKER) {
            return Ok(
                "skipped (a non-loom pre-commit hook already exists — add `loom export --check` \
                 + `loom wiki --check` to it yourself)"
                    .to_string(),
            );
        }
        // It's ours — fall through and refresh it to the latest content.
    }
    fs::write(&hook_path, hook_body())?;
    make_executable(&hook_path);
    Ok("installed (.git/hooks/pre-commit)".to_string())
}

/// The hook script: the UNIVERSAL loom freshness gates, plus a teach-adapt slot
/// for the repo's own build/lint/test bar (loom can't know it — it differs per
/// stack, so it teaches instead of hardcoding). Bypass once with
/// `git commit --no-verify`.
fn hook_body() -> String {
    format!(
        "#!/bin/sh\n\
         {HOOK_MARKER} — regenerate with `loom init` (re-run is safe). Bypass once: git commit --no-verify\n\
         set -e\n\
         \n\
         # --- loom freshness gates (universal: the committed projections must not drift) ---\n\
         if command -v loom >/dev/null 2>&1; then\n\
         \x20 loom export --check\n\
         \x20 loom wiki --check\n\
         else\n\
         \x20 echo 'loom not on PATH — skipping graph freshness gates' >&2\n\
         fi\n\
         \n\
         # --- this repo's bar (teach-adapt: uncomment what applies) ---\n\
         # Rust:   cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo build && cargo test\n\
         # Node:   npm test\n\
         # Python: ruff check . && pytest\n"
    )
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gitignore_adds_loom_cache_idempotently() {
        let dir = std::env::temp_dir().join(format!("loom-gi-{}-{}", std::process::id(), line!()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".git")).unwrap(); // pretend it's a git repo

        // First init adds the entry.
        assert_eq!(ensure_gitignored(&dir), "added `.loom/`");
        let gi = fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert!(gi.contains(".loom/"), "{gi}");

        // Idempotent: a re-run is a no-op and doesn't duplicate the line.
        assert_eq!(ensure_gitignored(&dir), "already ignored");
        assert_eq!(
            fs::read_to_string(dir.join(".gitignore"))
                .unwrap()
                .matches(".loom/")
                .count(),
            1
        );

        // A pre-existing .gitignore is appended to, not clobbered.
        fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        assert_eq!(ensure_gitignored(&dir), "added `.loom/`");
        let gi3 = fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert!(gi3.contains("target/") && gi3.contains(".loom/"), "{gi3}");

        // Not a git repo → skipped (don't litter non-git dirs).
        let nogit =
            std::env::temp_dir().join(format!("loom-gi-nogit-{}-{}", std::process::id(), line!()));
        let _ = fs::remove_dir_all(&nogit);
        fs::create_dir_all(&nogit).unwrap();
        assert_eq!(ensure_gitignored(&nogit), SKIPPED_NOT_GIT);

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&nogit);
    }
}
