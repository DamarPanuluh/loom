//! Cheap regex pre-screening for derived quality findings.
//!
//! Plane: computed-on-read. Pre-screen hits are produced while a quality work
//! packet is built, mirror debt clusters, and are never stored in the graph.
//! The persisted adjudication remains the derived `Finding`/`CodeRule` pipeline;
//! this module only supplies deterministic candidate evidence for the prompt.

use anyhow::Context;
use regex::Regex;
use serde::Serialize;
use std::fs;

use crate::Result;

/// A textual candidate hit found by the pre-screen pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreScreenHit {
    pub path: String,
    pub line: usize,
    pub pattern: String,
    pub excerpt: String,
}

/// Scans `files` under `root` with valid regex `patterns`, returning at most `cap` hits.
///
/// Invalid regexes and unreadable grounded files are errors: an empty hit list
/// must mean "inspected and found nothing," never "the sensor could not run."
/// Results are deterministic: files and patterns are scanned in lexical order,
/// and each file is scanned by ascending line number.
pub fn prescreen(
    root: &std::path::Path,
    files: &[String],
    patterns: &[String],
    cap: usize,
) -> Result<Vec<PreScreenHit>> {
    if cap == 0 || files.is_empty() || patterns.is_empty() {
        return Ok(Vec::new());
    }

    let mut compiled = Vec::with_capacity(patterns.len());
    for pattern in patterns {
        compiled.push((
            pattern.as_str(),
            Regex::new(pattern)
                .with_context(|| format!("compiling quality pattern '{pattern}'"))?,
        ));
    }
    compiled.sort_by_key(|(left, _)| *left);

    let mut ordered_files = files.iter().map(String::as_str).collect::<Vec<_>>();
    ordered_files.sort_unstable();

    let mut hits = Vec::new();
    for path in ordered_files {
        let text = fs::read_to_string(root.join(path))
            .with_context(|| format!("reading grounded file '{path}' for quality pre-screen"))?;

        let mut skip = SkipState::default();
        for (line_index, line) in text.lines().enumerate() {
            let line_number = line_index + 1;
            if skip.skips(line) {
                continue;
            }
            for (pattern, regex) in &compiled {
                if regex.is_match(line) {
                    hits.push(PreScreenHit {
                        path: path.to_string(),
                        line: line_number,
                        pattern: (*pattern).to_string(),
                        excerpt: truncate_excerpt(line.trim(), 160),
                    });
                    if hits.len() == cap {
                        return Ok(hits);
                    }
                }
            }
        }
    }

    Ok(hits)
}

/// What the scan must not read: prose, and tests.
///
/// A comment is not code. Left unfiltered, a rule's own documentation trips it
/// — the doc comment on `files_realizing` explaining that "a test SHOULD
/// `.unwrap()`" was itself reported as an unchecked failure, and doc comments
/// mentioning DELETE or UPDATE were reported as SQL injection.
///
/// A `#[cfg(test)]` module is a test even when it lives in a `src/` file. The
/// grounding-role split stopped whole test FILES being scanned; this stops the
/// test modules inside source files, which is the same mistake one level down.
/// `src/store/mod.rs` alone contributed 40 such hits, every one of them inside
/// `#[cfg(test)]`.
///
/// Deliberately conservative: only a line that is ENTIRELY a comment is
/// skipped. Stripping a trailing `//` would corrupt any line holding a string
/// with `//` in it, and a real hit sharing a line with a trailing comment is
/// still a real hit.
#[derive(Default)]
struct SkipState {
    /// Brace depth at which the enclosing `#[cfg(test)]` item opened.
    test_depth: Option<i32>,
    /// A `#[cfg(test)]` was seen; the next `{` opens the region it guards.
    pending_test: bool,
    depth: i32,
    in_block_comment: bool,
}

impl SkipState {
    fn skips(&mut self, line: &str) -> bool {
        let trimmed = line.trim();

        if self.in_block_comment {
            if trimmed.contains("*/") {
                self.in_block_comment = false;
            }
            return true;
        }
        if trimmed.starts_with("/*") && !trimmed.contains("*/") {
            self.in_block_comment = true;
            return true;
        }
        // Line comments and doc comments (`//`, `///`, `//!`).
        if trimmed.starts_with("//") {
            return true;
        }

        if trimmed.starts_with("#[cfg(test)]") {
            self.pending_test = true;
        }
        let opens = line.matches('{').count() as i32;
        let closes = line.matches('}').count() as i32;
        if self.pending_test && opens > 0 {
            self.test_depth = Some(self.depth);
            self.pending_test = false;
        }
        let inside_test = self.test_depth.is_some();
        self.depth += opens - closes;
        if let Some(open_depth) = self.test_depth {
            if self.depth <= open_depth {
                self.test_depth = None;
            }
        }
        inside_test
    }
}

fn truncate_excerpt(excerpt: &str, max_chars: usize) -> String {
    let mut end = excerpt.len();
    for (count, (index, _)) in excerpt.char_indices().enumerate() {
        if count == max_chars {
            end = index;
            break;
        }
    }
    excerpt[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new(name: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "loom-prescan-{name}-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create test root");
            Self { path }
        }

        fn write(&self, relative: &str, content: &str) {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent directory");
            }
            fs::write(path, content).expect("write test file");
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn finds_hits_with_correct_line_numbers() {
        let root = TestRoot::new("lines");
        root.write(
            "src/auth.rs",
            "fn main() {\n    let password = \"0123456789abcdef\";\n}\n",
        );

        let hits = prescreen(
            &root.path,
            &["src/auth.rs".to_string()],
            &[r#"password\s*=\s*\"[A-Za-z0-9]{16,}"#.to_string()],
            20,
        )
        .unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "src/auth.rs");
        assert_eq!(hits[0].line, 2);
        assert_eq!(hits[0].pattern, r#"password\s*=\s*\"[A-Za-z0-9]{16,}"#);
        assert_eq!(hits[0].excerpt, "let password = \"0123456789abcdef\";");
    }

    #[test]
    fn respects_total_cap() {
        let root = TestRoot::new("cap");
        root.write("a.txt", "token=aaaaaaaaaaaaaaaa\ntoken=bbbbbbbbbbbbbbbb\n");
        root.write("b.txt", "token=cccccccccccccccc\n");

        let hits = prescreen(
            &root.path,
            &["b.txt".to_string(), "a.txt".to_string()],
            &[r"token=[a-z]{16}".to_string()],
            2,
        )
        .unwrap();

        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "a.txt");
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[1].path, "a.txt");
        assert_eq!(hits[1].line, 2);
    }

    #[test]
    fn rejects_invalid_regexes() {
        let root = TestRoot::new("invalid");
        root.write("a.txt", "safe\nsecret=abcdefghijklmnop\n");

        let error = prescreen(
            &root.path,
            &["a.txt".to_string()],
            &["(".to_string(), r"secret=[a-z]{16}".to_string()],
            20,
        )
        .unwrap_err();

        assert!(error.to_string().contains("compiling quality pattern"));
    }

    #[test]
    fn returns_deterministic_file_line_pattern_order() {
        let root = TestRoot::new("order");
        root.write("b.txt", "beta alpha\n");
        root.write("a.txt", "beta alpha\nalpha\n");

        let hits = prescreen(
            &root.path,
            &["b.txt".to_string(), "a.txt".to_string()],
            &["beta".to_string(), "alpha".to_string()],
            20,
        )
        .unwrap();

        let tuples = hits
            .iter()
            .map(|hit| (hit.path.as_str(), hit.line, hit.pattern.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            tuples,
            vec![
                ("a.txt", 1, "alpha"),
                ("a.txt", 1, "beta"),
                ("a.txt", 2, "alpha"),
                ("b.txt", 1, "alpha"),
                ("b.txt", 1, "beta"),
            ]
        );
    }
}
