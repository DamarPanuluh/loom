//! Git hook installation for quiet structural sync. Hooks only run `sync`,
//! which may invalidate asserted facts but never authors them (INV-5).

use super::{open, pulse, resolve_root};
use crate::cli::HookCmd;
use crate::Result;
use anyhow::bail;
use std::path::Path;

const HOOKS: &[&str] = &["post-commit", "post-merge"];
// Never block or noise a commit: a missing/old `loom` on PATH degrades to a
// no-op (found by the 2026-07-19 pre-release smoke: a pre---quiet global
// binary errored on every commit). Real sync failures stay visible on stderr.
const CONTENT: &str = "#!/bin/sh\n# Installed by loom; structural facts only. Never blocks the commit.\ncommand -v loom >/dev/null 2>&1 || exit 0\nloom sync --quiet || echo \"loom sync failed (run 'loom sync' manually)\" >&2\n";

/// The ownership marker every loom-authored hook carries, across template
/// versions. Install/remove replace anything bearing it (so upgrades work)
/// and refuse anything without it (so foreign hooks are never clobbered).
const MARKER: &str = "# Installed by loom;";

fn loom_owned(content: &str) -> bool {
    content.contains(MARKER)
}

pub(crate) fn dispatch(graph: Option<&Path>, cmd: HookCmd, json: bool) -> Result<()> {
    let root = resolve_root(graph)?;
    let store = open(Some(&root))?;
    match cmd {
        HookCmd::Install => {
            let dir = root.join(".git/hooks");
            if !root.join(".git").is_dir() {
                bail!(
                    "cannot install loom hooks: {} is not a git worktree",
                    root.display()
                );
            }
            std::fs::create_dir_all(&dir)?;
            for name in HOOKS {
                let path = dir.join(name);
                if path.exists() && !loom_owned(&std::fs::read_to_string(&path)?) {
                    bail!("refusing to clobber foreign git hook {}", path.display());
                }
                std::fs::write(&path, CONTENT)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
                }
            }
            store.append_journal(
                "hook_install",
                "graph",
                serde_json::json!({ "hooks": HOOKS }),
            )?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "installed": HOOKS }),
                "loom status",
                "installed loom sync hooks",
            )
        }
        HookCmd::Remove => {
            let dir = root.join(".git/hooks");
            for name in HOOKS {
                let path = dir.join(name);
                if path.exists() {
                    if !loom_owned(&std::fs::read_to_string(&path)?) {
                        bail!("refusing to remove foreign git hook {}", path.display());
                    }
                    std::fs::remove_file(path)?;
                }
            }
            store.append_journal(
                "hook_remove",
                "graph",
                serde_json::json!({ "hooks": HOOKS }),
            )?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "removed": HOOKS }),
                "loom status",
                "removed loom sync hooks",
            )
        }
    }
}
