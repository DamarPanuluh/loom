//! Minimal glob expansion — enough for codefile registration without a glob
//! crate. Supports `**` (any depth), `*` (one path segment, no `/`), `?`, and
//! literal segments. Patterns and paths use `/`.
//!
//! Plane: pure path logic + a filesystem walk. No graph awareness.

use crate::Result;
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
    let pat_segs: Vec<&str> = pat.split('/').filter(|s| !s.is_empty()).collect();
    let mut out = Vec::new();
    walk(root, root, &pat_segs, &mut out)?;
    out.sort();
    out.dedup();
    Ok(out)
}

fn walk(root: &Path, dir: &Path, pat: &[&str], out: &mut Vec<String>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue; // skip dotfiles/dirs (.git, .loom, …)
        }
        let is_dir = path.is_dir();
        match pat.first() {
            None => {}
            Some(&"**") => {
                // `**` matches zero or more segments.
                if is_dir {
                    // consume one dir level, keep `**`
                    walk(root, &path, pat, out)?;
                }
                // or skip `**` entirely and match the rest here
                if pat.len() > 1 {
                    if seg_matches(pat[1], &name) {
                        descend_or_emit(root, &path, &pat[2..], is_dir, out)?;
                    }
                    if is_dir {
                        walk(root, &path, &pat[1..], out)?;
                    }
                }
            }
            Some(&seg) if seg_matches(seg, &name) => {
                descend_or_emit(root, &path, &pat[1..], is_dir, out)?;
            }
            Some(_) => {}
        }
    }
    Ok(())
}

fn descend_or_emit(
    root: &Path,
    path: &Path,
    rest: &[&str],
    is_dir: bool,
    out: &mut Vec<String>,
) -> Result<()> {
    if rest.is_empty() {
        if !is_dir {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    } else if is_dir {
        walk(root, path, rest, out)?;
    }
    Ok(())
}

/// Match a single path segment against a single pattern segment (`*`, `?`, literals).
fn seg_matches(pat: &str, name: &str) -> bool {
    glob_match(pat.as_bytes(), name.as_bytes())
}

/// Wildcard match within one segment: `*` = any run (no `/`), `?` = one char.
fn glob_match(pat: &[u8], s: &[u8]) -> bool {
    // Iterative backtracking matcher.
    let (mut pi, mut si) = (0usize, 0usize);
    let (mut star_pi, mut star_si): (Option<usize>, usize) = (None, 0);
    while si < s.len() {
        if pi < pat.len() && (pat[pi] == b'?' || pat[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < pat.len() && pat[pi] == b'*' {
            star_pi = Some(pi);
            star_si = si;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_si += 1;
            si = star_si;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}
#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn segment_wildcards() {
        assert!(glob_match(b"*.rs", b"main.rs"));
        assert!(glob_match(b"*.rs", b".rs")); // edge: empty stem
        assert!(!glob_match(b"*.rs", b"main.py"));
        assert!(glob_match(b"foo?", b"foob"));
        assert!(!glob_match(b"foo?", b"foobar"));
        assert!(glob_match(b"*", b"anything"));
        assert!(glob_match(b"a*c", b"abc"));
        assert!(glob_match(b"a*c", b"ac"));
        assert!(!glob_match(b"a*c", b"ab"));
    }
}
