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
use globset::GlobBuilder;
use ignore::WalkBuilder;
use std::path::Path;

/// Expand a glob (relative to `root`) into matching relative file paths, sorted.
/// A pattern with no glob metacharacters is returned as a single literal path if
/// it exists as a file.
pub fn expand(root: &Path, pattern: &str) -> Result<Vec<String>> {
    let pat = pattern.replace('\\', "/");
    if !pat.contains('*') && !pat.contains('?') {
        let p = root.join(&pat);
        return Ok(if p.is_file() { vec![pat] } else { vec![] });
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
}
