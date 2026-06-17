//! Stamp the build with the git commit (short, + `-dirty`) so `loom --version`
//! distinguishes local binaries — the crate version is intentionally stable at
//! stable across feature work, so the git stamp is the useful identity during
//! dogfood and release checks. Falls back to "unknown" when git is unavailable.

use std::process::Command;

fn main() {
    let build = git_build_id().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=LOOM_BUILD={build}");
    // Re-stamp when HEAD moves (commit) or the working tree's staged state
    // changes (dirty toggle). Missing paths (no .git) are tolerated by cargo.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}

fn git_build_id() -> Option<String> {
    let short = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !short.status.success() {
        return None;
    }
    let mut id = String::from_utf8(short.stdout).ok()?.trim().to_string();
    if id.is_empty() {
        return None;
    }
    if let Ok(status) = Command::new("git").args(["status", "--porcelain"]).output() {
        if status.status.success() && !status.stdout.is_empty() {
            id.push_str("-dirty");
        }
    }
    Some(id)
}
