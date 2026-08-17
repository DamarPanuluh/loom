//! Git hook installation for quiet structural sync and explicit local CI.
//! Sync hooks may invalidate asserted facts but never author them (INV-5). A
//! caller may additionally opt into a blocking pre-push script; Loom installs
//! the gate but does not infer its command or treat passage as graph truth.

use super::resolve_root;
use crate::cli::HookCmd;
use crate::Result;
use anyhow::{bail, Context};
use std::path::{Component, Path};

const SYNC_HOOKS: &[&str] = &["post-commit", "post-merge"];
const PRE_PUSH_HOOK: &str = "pre-push";
// Never block or noise a commit: a missing/old `loom` on PATH degrades to a
// no-op (found by the 2026-07-19 pre-release smoke: a pre---quiet global
// binary errored on every commit). Real sync failures stay visible on stderr.
const SYNC_CONTENT: &str = "#!/bin/sh\n# Installed by loom; structural facts only. Never blocks the commit.\ncommand -v loom >/dev/null 2>&1 || exit 0\nloom sync --quiet || echo \"loom sync failed (run 'loom sync' manually)\" >&2\n";

/// The ownership marker every loom-authored hook carries, across template
/// versions. Install/remove replace anything bearing it (so upgrades work)
/// and refuse anything without it (so foreign hooks are never clobbered).
const MARKER: &str = "# Installed by loom;";

fn loom_owned(content: &str) -> bool {
    content.contains(MARKER)
}

fn pre_push_content(script: &Path) -> Result<String> {
    if script.as_os_str().is_empty()
        || script.is_absolute()
        || !script
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        bail!("pre-push script must be a confined repository-relative path");
    }
    let script = script
        .to_str()
        .context("pre-push script path must be valid UTF-8")?;
    let quoted = script.replace('\'', "'\\''");
    Ok(format!(
        "#!/bin/sh\n# Installed by loom; local CI gate. Blocks a push when the configured script fails.\nset -eu\nroot=\"$(git rev-parse --show-toplevel)\"\n# Drain git's pre-push ref list before exec so git does not see SIGPIPE.\ncat >/dev/null\nexec \"$root\"/'{quoted}'\n"
    ))
}

fn verify_pre_push_script(root: &Path, script: &Path) -> Result<()> {
    pre_push_content(script)?;
    let canonical_root = root.canonicalize()?;
    let resolved = root
        .join(script)
        .canonicalize()
        .with_context(|| format!("resolving pre-push script {}", script.display()))?;
    if !resolved.starts_with(&canonical_root) || !resolved.is_file() {
        bail!(
            "pre-push script {} must resolve to a file inside the repository",
            script.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(&resolved)?.permissions().mode() & 0o111 == 0 {
            bail!("pre-push script {} is not executable", script.display());
        }
    }
    Ok(())
}

pub(crate) fn dispatch(graph: Option<&Path>, cmd: HookCmd, json: bool) -> Result<()> {
    let root = resolve_root(graph)?;
    match cmd {
        HookCmd::Install { pre_push } => {
            let dir = root.join(".git/hooks");
            if !root.join(".git").is_dir() {
                bail!(
                    "cannot install loom hooks: {} is not a git worktree",
                    root.display()
                );
            }
            std::fs::create_dir_all(&dir)?;
            if let Some(script) = &pre_push {
                verify_pre_push_script(&root, script)?;
            }
            let targets: Vec<(&str, String)> = match &pre_push {
                Some(script) => vec![(PRE_PUSH_HOOK, pre_push_content(script)?)],
                None => SYNC_HOOKS
                    .iter()
                    .map(|name| (*name, SYNC_CONTENT.to_string()))
                    .collect(),
            };
            // Refuse before writing anything so a foreign hook cannot leave a
            // partially installed set behind.
            for (name, _) in &targets {
                let path = dir.join(name);
                if path.exists() && !loom_owned(&std::fs::read_to_string(&path)?) {
                    bail!("refusing to clobber foreign git hook {}", path.display());
                }
            }
            for (name, content) in &targets {
                let path = dir.join(name);
                std::fs::write(&path, content)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
                }
            }
            let payload = serde_json::json!({
                "installed": targets.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
                "pre_push_script": pre_push,
            });
            journal_if_current(&root, "hook_install", &payload)?;
            emit(
                json,
                payload,
                if pre_push.is_some() {
                    "installed Loom local pre-push CI"
                } else {
                    "installed loom sync hooks"
                },
            )
        }
        HookCmd::Remove => {
            let dir = root.join(".git/hooks");
            let mut removed = Vec::new();
            for name in SYNC_HOOKS.iter().copied().chain([PRE_PUSH_HOOK]) {
                let path = dir.join(name);
                if path.exists() {
                    if !loom_owned(&std::fs::read_to_string(&path)?) {
                        continue;
                    }
                    std::fs::remove_file(path)?;
                    removed.push(name);
                }
            }
            let payload = serde_json::json!({ "removed": removed });
            journal_if_current(&root, "hook_remove", &payload)?;
            emit(json, payload, "removed loom-authored git hooks")
        }
    }
}

/// Hook management is repository plumbing, so an old graph must not prevent a
/// developer from installing CI for the code that will replace it. Journal
/// when the local store is current; otherwise leave the incompatible bytes
/// untouched and still manage only Loom-owned hook files.
fn journal_if_current(root: &Path, event: &str, payload: &serde_json::Value) -> Result<()> {
    if let Ok(store) = crate::store::Store::open(root) {
        store.append_journal(event, "graph", payload.clone())?;
    }
    Ok(())
}

fn emit(json: bool, payload: serde_json::Value, line: &str) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("{line}");
    }
    Ok(())
}
