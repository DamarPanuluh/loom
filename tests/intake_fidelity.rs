//! Intake fidelity — forbid stale inbox-source recipes in operator-facing prose.
//!
//! The binary rejects `inbox add --source question|code_audit` (and related
//! evidence sources). README, docs, and the vendored skill must not teach them.
//! Behavioral gates live in ring9; this test only greps the prose surface.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn collect_markdown(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if matches!(name, "target" | ".git" | ".loom" | "node_modules") {
                    continue;
                }
                stack.push(path);
            } else if path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                out.push(path);
            }
        }
    }
    out
}

/// Matches teaching forms like `inbox add "…" --source question` while allowing
/// ring9/docs that *describe* the rejection (e.g. `` `inbox add --source question` is rejected ``).
fn is_forbidden_teaching_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if !lower.contains("inbox add") {
        return false;
    }
    if !lower.contains("--source question") && !lower.contains("--source code_audit") {
        return false;
    }
    // Allow contract/test/doc lines that state the rejection.
    if lower.contains("reject")
        || lower.contains("forbidden")
        || lower.contains("never use")
        || lower.contains("must not")
        || lower.contains("belong")
        || lower.contains("pointing to")
        || lower.contains("contract:")
    {
        return false;
    }
    true
}

#[test]
fn operator_prose_never_teaches_rejected_inbox_sources() {
    let root = repo_root();
    let mut scanned = Vec::new();
    for rel in ["README.md", "docs", "skills"] {
        let path = root.join(rel);
        if path.is_file() {
            scanned.push(path);
        } else if path.is_dir() {
            scanned.extend(collect_markdown(&path));
        }
    }
    // The loom-driver skill is authoritative at the global skill root since the
    // repo copy was removed (ecbea8b); the repo's own docs remain the
    // deterministic instruction surface this test guards. The global skill is
    // deliberately NOT scanned: a test must never depend on mutable state
    // outside the checkout.

    let mut violations = Vec::new();
    for path in &scanned {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for (idx, line) in text.lines().enumerate() {
            if is_forbidden_teaching_line(line) {
                violations.push(format!("{}:{}: {}", path.display(), idx + 1, line.trim()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "stale inbox-source recipes found (use loom question add / loom finding add):\n{}",
        violations.join("\n")
    );
}
