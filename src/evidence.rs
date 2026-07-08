//! Evidence anchoring — the mechanical link between a verdict and the bytes
//! that justified it.
//!
//! Plane: verdict integrity. At verdict time, every `file:line[-line]`
//! citation in the evidence that resolves to a real file under the graph root
//! is stamped with a fingerprint of the cited span (the asserted
//! `evidence_spans` edge facet). Sync then re-checks the stamps when the cited
//! file changes, so a re-opened claim can say "cited span untouched — cheap
//! re-confirm" versus "cited span rewritten — full re-inspection". A citation
//! into lines that never existed fails closed at record time: it is direct
//! evidence of a fabricating recorder.
//!
//! Citations are parsed conservatively: the path must look like a relative
//! file path with an alphabetic extension, and only paths that exist under the
//! root are stamped or judged — prose, URLs, and tool output that merely look
//! line-ish are ignored rather than guessed at.

use crate::artifact::fingerprint;
use crate::Result;
use anyhow::bail;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::LazyLock;

/// The asserted edge facet carrying the citation stamps of the last verdict.
pub const EVIDENCE_SPANS_KEY: &str = "evidence_spans";

/// Stamp at most this many distinct citations per verdict (bounds facet size;
/// honest evidence cites a handful of spans, not dozens).
const MAX_SPANS: usize = 16;

/// Window-search re-anchoring is skipped for files larger than this many
/// lines — the exact-position check still runs, only the "moved but intact"
/// classification degrades to "rewritten" on degenerate inputs.
const MAX_SEARCH_LINES: usize = 20_000;

/// One cited span, stamped at verdict time. `hash` is the FNV fingerprint of
/// the cited lines joined with `\n`, exactly as [`crate::artifact::fingerprint`]
/// hashes file content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanStamp {
    pub file: String,
    pub start: usize,
    pub end: usize,
    pub hash: String,
}

/// A `path.ext:start[-end]` citation. The path needs an alphabetic extension
/// so version numbers ("1.2:3") and times ("12:30") never match; existence
/// under the root is the real gate.
static CITATION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?P<file>[A-Za-z0-9_][A-Za-z0-9_./\-]*\.[A-Za-z][A-Za-z0-9_]*):(?P<start>\d{1,7})(?:-(?P<end>\d{1,7}))?",
    )
    .expect("citation regex is valid")
});

/// Parse and stamp every file:line citation in `evidence` that resolves to a
/// readable file under `root`.
///
/// Fails closed when an EXISTING file is cited with a line range that does not
/// exist in it (start of 0, inverted range, or end beyond EOF) — the recorder
/// is describing bytes nobody can read. Citations whose path does not resolve
/// (URLs, tool output, deleted files, absolute/parent-escaping paths) are
/// skipped, never guessed at.
pub fn stamp(root: &Path, evidence: &str) -> Result<Vec<SpanStamp>> {
    let mut seen: BTreeSet<(String, usize, usize)> = BTreeSet::new();
    let mut stamps = Vec::new();
    for cap in CITATION_RE.captures_iter(evidence) {
        let file = &cap["file"];
        // Only relative paths inside the root are citable code.
        if file.starts_with('/') || file.split('/').any(|seg| seg == "..") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(root.join(file)) else {
            continue; // not a file under this root — prose, URL, or gone
        };
        let start: usize = cap["start"].parse().unwrap_or(0);
        let end: usize = cap
            .name("end")
            .map(|m| m.as_str().parse().unwrap_or(0))
            .unwrap_or(start);
        if !seen.insert((file.to_string(), start, end)) {
            continue;
        }
        let loc = content.lines().count();
        if start < 1 || end < start || end > loc {
            bail!(
                "evidence cites {file}:{start}{} but {file} has {loc} lines — \
                 evidence must cite lines that exist",
                if end != start {
                    format!("-{end}")
                } else {
                    String::new()
                }
            );
        }
        let lines: Vec<&str> = content.lines().collect();
        // The integrity gate above runs for EVERY citation; only the stored
        // stamps are capped.
        if stamps.len() < MAX_SPANS {
            stamps.push(SpanStamp {
                file: file.to_string(),
                start,
                end,
                hash: fingerprint(&lines[start - 1..end].join("\n")),
            });
        }
    }
    Ok(stamps)
}

/// How the spans a verdict cited in `file` fared against its new `content`:
/// `None` when the verdict cited nothing in this file; `Some(true)` when every
/// cited span is intact (at its original position or moved verbatim);
/// `Some(false)` when at least one cited span was rewritten.
pub fn spans_status(spans: &[SpanStamp], file: &str, content: &str) -> Option<bool> {
    let cited: Vec<&SpanStamp> = spans.iter().filter(|s| s.file == file).collect();
    if cited.is_empty() {
        return None;
    }
    let lines: Vec<&str> = content.lines().collect();
    Some(cited.into_iter().all(|s| span_intact(s, &lines)))
}

/// Whether one stamped span still exists in the file: first at its recorded
/// position (the common case — edits elsewhere in the file), then anywhere as
/// a verbatim window of the same height (the span moved).
fn span_intact(s: &SpanStamp, lines: &[&str]) -> bool {
    let height = s.end - s.start + 1;
    if s.end <= lines.len() && fingerprint(&lines[s.start - 1..s.end].join("\n")) == s.hash {
        return true;
    }
    if lines.len() > MAX_SEARCH_LINES || height > lines.len() {
        return false;
    }
    (0..=lines.len() - height).any(|i| fingerprint(&lines[i..i + height].join("\n")) == s.hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_and_ranged_citations() {
        let caps: Vec<_> = CITATION_RE
            .captures_iter("checked src/foo.rs:10-40 and lib/bar.py:7 — ok")
            .map(|c| c["file"].to_string())
            .collect();
        assert_eq!(caps, vec!["src/foo.rs", "lib/bar.py"]);
    }

    #[test]
    fn ignores_versions_and_times() {
        assert!(CITATION_RE
            .captures_iter("v1.2:3 at 12:30")
            .next()
            .is_none());
    }

    #[test]
    fn moved_span_still_intact() {
        let stamp = SpanStamp {
            file: "f.rs".into(),
            start: 1,
            end: 2,
            hash: fingerprint("a\nb"),
        };
        // Two prepended lines: the cited span moved but is verbatim-intact.
        assert_eq!(
            spans_status(std::slice::from_ref(&stamp), "f.rs", "x\ny\na\nb"),
            Some(true)
        );
        // The span's body changed: rewritten.
        assert_eq!(
            spans_status(std::slice::from_ref(&stamp), "f.rs", "a\nc"),
            Some(false)
        );
        // A file the verdict never cited: no signal.
        assert_eq!(spans_status(&[stamp], "other.rs", "a\nb"), None);
    }
}
