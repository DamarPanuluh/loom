//! Repository-wide terminology — retired proof-level language must not teach.
//!
//! Proof strength is derived (S0–S5). The old authored L0–L6 / L5/L6 scale is
//! gone from the CLI and from journey coverage. This test walks production
//! sources and canonical docs so a help-text fix cannot leave runtime
//! diagnostics and documentation still saying "L5 journey proof".

use std::path::{Path, PathBuf};

/// Paths under the repo root that may still mention L5/L6 while describing
/// the migration away from it (never as current operator vocabulary).
fn path_allowed(rel: &str) -> bool {
    matches!(
        rel,
        // Records the removal; quoting the old names is the point.
        "CHANGELOG.md"
            // Module history + the parse-rejection unit test.
            | "src/proofstrength.rs"
            // Schema migration that strips leftover `proof_level` bodies.
            | "src/store/mod.rs"
    )
}

/// A line may mention L5/L6 only when it is clearly about the retired scale,
/// not instructing an operator to use it.
fn line_allowed(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("used to")
        || l.contains("old code")
        || l.contains("retired")
        || l.contains("removed")
        || l.contains("stripped")
        || l.contains("legacy")
        || l.contains("hardcoded")
        || l.contains("proof_level")
        || l.contains("parse(\"l5\")")
        || l.contains("parse(\"l6\")")
        || l.contains("contains(\"l5\")")
        || l.contains("contains(\"l6\")")
        || l.contains("!help.contains(\"l5\")")
        || l.contains("!help.contains(\"l6\")")
}

fn collect_rs_and_md(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in ["src", "docs"] {
        let base = root.join(dir);
        if !base.exists() {
            continue;
        }
        let mut stack = vec![base];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e == "rs" || e == "md")
                {
                    out.push(path);
                }
            }
        }
    }
    out.sort();
    out
}

/// True when the line names retired proof levels as whole tokens (L5 / L6),
/// not as a substring of another identifier.
fn mentions_retired_level(line: &str) -> bool {
    for token in ["L5", "L6"] {
        let mut rest = line;
        while let Some(at) = rest.find(token) {
            let before = rest[..at].chars().next_back();
            let after = rest[at + token.len()..].chars().next();
            let boundary = |c: Option<char>| {
                c.map(|ch| !ch.is_ascii_alphanumeric() && ch != '_')
                    .unwrap_or(true)
            };
            if boundary(before) && boundary(after) {
                return true;
            }
            rest = &rest[at + token.len()..];
        }
    }
    false
}

#[test]
fn production_and_docs_do_not_teach_retired_proof_levels() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();

    for path in collect_rs_and_md(&root) {
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if path_allowed(&rel) {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for (idx, line) in text.lines().enumerate() {
            if !mentions_retired_level(line) || line_allowed(line) {
                continue;
            }
            offenders.push(format!("{rel}:{}: {}", idx + 1, line.trim()));
        }
    }

    assert!(
        offenders.is_empty(),
        "retired L5/L6 proof-level language remains in production or docs — \
         use \"S3-or-stronger journey proof\" (derived S0–S5 scale):\n{}",
        offenders.join("\n")
    );
}
