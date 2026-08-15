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
    cli(tmp.path(), HookCmd::Install { pre_push: None }).unwrap();
    let hook = tmp.path().join(".git/hooks/post-commit");
    assert!(hook.exists());
    assert!(std::fs::read_to_string(&hook)
        .unwrap()
        .contains("loom sync --quiet"));
    cli(tmp.path(), HookCmd::Install { pre_push: None }).unwrap();
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
    cli(tmp.path(), HookCmd::Install { pre_push: None }).unwrap();
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
    let err = cli(tmp.path(), HookCmd::Install { pre_push: None }).unwrap_err();
    assert!(err.to_string().contains("refusing to clobber foreign"));
}

#[test]
fn opt_in_pre_push_runs_only_a_confined_executable_script() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    let hooks = tmp.path().join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(
        hooks.join("post-commit"),
        "#!/bin/sh\n# Foreign hook remains untouched.\n",
    )
    .unwrap();
    std::fs::create_dir_all(tmp.path().join("scripts")).unwrap();
    let script = tmp.path().join("scripts/local-ci.sh");
    std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    cli(
        tmp.path(),
        HookCmd::Install {
            pre_push: Some("scripts/local-ci.sh".into()),
        },
    )
    .unwrap();
    let hook = tmp.path().join(".git/hooks/pre-push");
    let content = std::fs::read_to_string(&hook).unwrap();
    assert!(content.contains("local CI gate"));
    assert!(content.contains("exec \"$root\"/'scripts/local-ci.sh'"));
    assert!(std::fs::read_to_string(hooks.join("post-commit"))
        .unwrap()
        .contains("Foreign hook remains untouched"));
    #[cfg(unix)]
    assert_ne!(std::fs::metadata(&hook).unwrap().mode() & 0o111, 0);

    cli(tmp.path(), HookCmd::Remove).unwrap();
    assert!(!hook.exists());
}

#[test]
fn pre_push_refuses_escape_non_executable_and_foreign_hook_without_partial_install() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    let hooks = tmp.path().join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();

    let escape = cli(
        tmp.path(),
        HookCmd::Install {
            pre_push: Some("../ci.sh".into()),
        },
    )
    .unwrap_err();
    assert!(escape.to_string().contains("repository-relative"));

    let script = tmp.path().join("ci.sh");
    std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        let error = cli(
            tmp.path(),
            HookCmd::Install {
                pre_push: Some("ci.sh".into()),
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("not executable"));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(hooks.join("pre-push"), "#!/bin/sh\necho foreign\n").unwrap();
    let foreign = cli(
        tmp.path(),
        HookCmd::Install {
            pre_push: Some("ci.sh".into()),
        },
    )
    .unwrap_err();
    assert!(foreign.to_string().contains("refusing to clobber foreign"));
    assert_eq!(
        std::fs::read_to_string(hooks.join("pre-push")).unwrap(),
        "#!/bin/sh\necho foreign\n"
    );
}

#[test]
fn local_ci_builds_the_adapter_then_runs_the_isolated_dogfood_gate() {
    let script = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/scripts/local-ci.sh"
    ))
    .unwrap();
    let build = script
        .find("cargo build")
        .expect("local CI must build the trusted local adapter");
    let gate = script
        .find("scripts/dogfood.sh --check")
        .expect("local CI must run the isolated Journey-root dogfood gate");
    assert!(
        build < gate,
        "the adapter must be built before the gate runs it"
    );
    // The gate is isolated by construction; local CI must never reach for the
    // live graph itself.
    for forbidden in ["loom import", "loom init", "--graph", "migrate"] {
        assert!(
            !script.contains(forbidden),
            "local-ci.sh must not migrate or replace the live graph: {forbidden}"
        );
    }
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
