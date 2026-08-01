//! Glob expansion for codefile registration.
//!
//! Uses the same battle-tested walker/matcher family as ripgrep:
//! - `ignore` walks the repo while respecting `.gitignore` / `.ignore` and
//!   skipping hidden paths by default (`.git`, `.loom`, dotfiles).
//! - `globset` compiles the user's pattern once and preserves Loom's original
//!   segment semantics: `*` and `?` do not cross `/`; `**` is recursive.
//!
//! Plane: pure path logic + a filesystem walk. No graph awareness.

use crate::Result;
use anyhow::bail;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use std::path::Path;

/// Expand a glob (relative to `root`) into matching relative file paths, sorted.
/// A pattern with no glob metacharacters is returned as a single literal path if
/// it exists as a file; missing literal paths error loudly instead of
/// masquerading as an empty glob match.
pub fn expand(root: &Path, pattern: &str) -> Result<Vec<String>> {
    let pat = pattern.replace('\\', "/");
    if !pat.contains('*') && !pat.contains('?') {
        let p = root.join(&pat);
        // A literal path skips the root-anchored walk, so it is the one way a
        // `../` (or an absolute path) could register a file OUTSIDE the graph
        // root — loom would then hash and extract from a tree it does not own.
        // Require the resolved target to stay under the resolved root.
        if p.is_file() && contains(root, &p) {
            return Ok(vec![pat]);
        }
        bail!(
            "literal path '{}' does not exist, is not a file, or escapes the graph root",
            pat
        );
    }

    let matcher = GlobBuilder::new(&pat)
        .literal_separator(true)
        .build()?
        .compile_matcher();
    let mut out = Vec::new();

    for entry in WalkBuilder::new(root)
        .hidden(true)
        .require_git(false)
        .build()
        .flatten()
    {
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if matcher.is_match(&rel) {
            out.push(rel);
        }
    }

    out.sort();
    out.dedup();
    Ok(out)
}

/// Does `candidate` resolve to a path inside `root`? Both are canonicalized, so
/// `..` segments and symlinks are collapsed before the prefix test — a lexical
/// `starts_with` would be fooled by `root/../root_evil` or a symlink out of the
/// tree. Requires both paths to exist (the caller checks `is_file` first); a
/// path that cannot be canonicalized is treated as NOT contained, failing safe.
pub fn contains(root: &Path, candidate: &Path) -> bool {
    match (root.canonicalize(), candidate.canonicalize()) {
        (Ok(r), Ok(c)) => c.starts_with(&r),
        _ => false,
    }
}

/// Compile a set of glob patterns into a matcher tested directly against
/// relative paths (no filesystem walk). Same segment semantics as [`expand`]:
/// `*`/`?` do not cross `/`, `**` is recursive. Invalid patterns are skipped so
/// one bad rule cannot poison the whole set. An empty set matches nothing.
pub fn matcher<I, S>(patterns: I) -> Result<GlobSet>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        let pat = p.as_ref().replace('\\', "/");
        if let Ok(glob) = GlobBuilder::new(&pat).literal_separator(true).build() {
            builder.add(glob);
        }
    }
    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {
    use super::expand;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TmpRoot(PathBuf);

    impl TmpRoot {
        fn new(prefix: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path =
                std::env::temp_dir().join(format!("{prefix}-{}-{nanos}-{n}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, text).unwrap();
        }
    }

    impl Drop for TmpRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn expands_recursive_globs_sorted_and_deduped() {
        let tmp = TmpRoot::new("loom-fsglob-recursive");
        tmp.write("src/main.rs", "");
        tmp.write("src/nested/lib.rs", "");
        tmp.write("src/main.py", "");

        let got = expand(tmp.path(), "src/**/*.rs").unwrap();
        assert_eq!(got, vec!["src/main.rs", "src/nested/lib.rs"]);
    }

    #[test]
    fn segment_wildcards_do_not_cross_slashes() {
        let tmp = TmpRoot::new("loom-fsglob-segment");
        tmp.write("foo.rs", "");
        tmp.write("src/foo.rs", "");

        let got = expand(tmp.path(), "*.rs").unwrap();
        assert_eq!(got, vec!["foo.rs"]);
    }

    #[test]
    fn glob_walk_respects_gitignore_and_hidden_paths() {
        let tmp = TmpRoot::new("loom-fsglob-ignore");
        tmp.write(".gitignore", "ignored.rs\n");
        tmp.write("kept.rs", "");
        tmp.write("ignored.rs", "");
        tmp.write(".hidden.rs", "");

        let got = expand(tmp.path(), "*.rs").unwrap();
        assert_eq!(got, vec!["kept.rs"]);
    }

    /// A literal path that escapes the graph root must be refused, not
    /// registered. It is the one branch that skips the root-anchored walk, so
    /// without the containment check `loom codefile add '../secret'` would hash
    /// and extract from a tree loom does not own.
    #[test]
    fn literal_paths_that_escape_the_root_are_rejected() {
        let parent = TmpRoot::new("loom-fsglob-escape");
        parent.write("secret.rs", "");
        parent.write("root/inside.rs", "");
        let root = parent.path().join("root");

        // A contained literal still resolves.
        assert_eq!(
            super::expand(&root, "inside.rs").unwrap(),
            vec!["inside.rs"]
        );
        // A `..` escape out of the root is refused.
        assert!(super::expand(&root, "../secret.rs").is_err());
    }
}

/// Globs that WOULD match source files in this tree, ranked by how many.
///
/// A glob matching nothing is loom's worst first impression: `loom codefile add
/// 'src/**/*.rs'` on a monorepo whose crates live under `services/*/backend/`
/// registers zero files and says so cheerfully. Found on the first repository
/// loom was ever pointed at — 1331 Rust files, none of them under a root `src/`.
///
/// So when a glob comes back empty, loom looks at the tree it actually has and
/// says what would have worked. Grouped by the first two path segments, which
/// is where the shape of a monorepo lives.
pub fn suggest(root: &Path, extension: Option<&str>) -> Vec<(String, usize)> {
    const SOURCE_EXTS: &[&str] = &["rs", "py", "go", "ts", "tsx", "js", "jsx", "mjs", "cjs"];
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for entry in WalkBuilder::new(root)
        .hidden(true)
        .require_git(false)
        .build()
        .flatten()
    {
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        let Some(ext) = rel.rsplit('.').next() else {
            continue;
        };
        if !SOURCE_EXTS.contains(&ext) {
            continue;
        }
        if let Some(want) = extension {
            if ext != want {
                continue;
            }
        }
        // Generated and vendored trees are noise in a suggestion.
        if rel.contains("node_modules/") || rel.contains("/target/") || rel.starts_with("target/") {
            continue;
        }
        let segments: Vec<&str> = rel.split('/').collect();
        let prefix = match segments.len() {
            0 | 1 => String::new(),
            2 => format!("{}/", segments[0]),
            _ => format!("{}/{}/", segments[0], segments[1]),
        };
        *counts.entry(format!("{prefix}**/*.{ext}")).or_default() += 1;
    }
    let mut out: Vec<(String, usize)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out.truncate(6);
    out
}

#[cfg(test)]
mod suggest_tests {
    use super::*;

    /// Found on the first repository loom was ever pointed at: a monorepo with
    /// 1331 Rust files, none under a root `src/`. The quickstart glob matched
    /// zero and loom reported that cheerfully — the worst possible first
    /// impression, and one a newcomer has no way to debug.
    #[test]
    fn suggests_the_globs_a_monorepo_actually_needs() {
        let root = std::env::temp_dir().join(format!("loom-suggest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for path in [
            "services/pulse/src/main.rs",
            "services/pulse/src/api.rs",
            "services/grid/src/lib.rs",
            "packages/ui/src/App.tsx",
            "node_modules/dep/index.js",
            "target/debug/build.rs",
        ] {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, "// x\n").unwrap();
        }

        let all = suggest(&root, None);
        let globs: Vec<&str> = all.iter().map(|(g, _)| g.as_str()).collect();
        assert_eq!(
            all.first().map(|(g, n)| (g.as_str(), *n)),
            Some(("services/pulse/**/*.rs", 2)),
            "ranked by how many files each would reach: {all:?}"
        );
        assert!(globs.contains(&"packages/ui/**/*.tsx"), "{globs:?}");
        // Vendored and generated trees are noise in a suggestion.
        assert!(
            !globs
                .iter()
                .any(|g| g.contains("node_modules") || g.starts_with("target/")),
            "{globs:?}"
        );

        // Asking about one extension answers about that extension.
        let rust_only = suggest(&root, Some("rs"));
        assert!(
            rust_only.iter().all(|(g, _)| g.ends_with(".rs")),
            "{rust_only:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
