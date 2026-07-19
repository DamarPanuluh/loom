//! Ring 20 — local git hooks keep sync automatic without overwriting foreign hooks.

use loom::cli::{Cli, Command, HookCmd};
use loom::store::Store;
mod common;
use common::*;

fn cli(root: &std::path::Path, cmd: HookCmd) -> loom::Result<()> {
    loom::commands::run(Cli {
        graph: Some(root.to_path_buf()),
        json: true,
        command: Some(Command::Hook { cmd }),
    })
}

#[test]
fn install_is_executable_idempotent_and_remove_cleans_up() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    std::fs::create_dir_all(tmp.path().join(".git/hooks")).unwrap();
    cli(tmp.path(), HookCmd::Install).unwrap();
    let hook = tmp.path().join(".git/hooks/post-commit");
    assert!(hook.exists());
    assert!(std::fs::read_to_string(&hook)
        .unwrap()
        .contains("loom sync --quiet"));
    cli(tmp.path(), HookCmd::Install).unwrap();
    #[cfg(unix)]
    assert_ne!(std::fs::metadata(&hook).unwrap().mode() & 0o111, 0);
    cli(tmp.path(), HookCmd::Remove).unwrap();
    assert!(!hook.exists());
}

#[test]
fn install_upgrades_an_older_loom_authored_hook() {
    // Regression (2026-07-19 pre-release smoke): the idempotency check compared
    // exact content, so a hook written by an OLDER loom template was treated as
    // foreign and upgrades were impossible. Ownership is the marker line, not
    // byte equality.
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    let hooks = tmp.path().join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(
        hooks.join("post-commit"),
        "#!/bin/sh\n# Installed by loom; structural facts only.\nloom sync --quiet\n",
    )
    .unwrap();
    cli(tmp.path(), HookCmd::Install).unwrap();
    let content = std::fs::read_to_string(hooks.join("post-commit")).unwrap();
    assert!(
        content.contains("command -v loom"),
        "old loom template must be upgraded to the current one"
    );
    // And removal recognizes its own current template.
    cli(tmp.path(), HookCmd::Remove).unwrap();
    assert!(!hooks.join("post-commit").exists());
}

#[test]
fn install_refuses_a_foreign_hook() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    let hooks = tmp.path().join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(hooks.join("post-commit"), "#!/bin/sh\necho foreign\n").unwrap();
    let err = cli(tmp.path(), HookCmd::Install).unwrap_err();
    assert!(err.to_string().contains("refusing to clobber foreign"));
}

#[cfg(unix)]
trait Mode {
    fn mode(&self) -> u32;
}

#[cfg(unix)]
impl Mode for std::fs::Metadata {
    fn mode(&self) -> u32 {
        use std::os::unix::fs::MetadataExt;
        MetadataExt::mode(self)
    }
}
