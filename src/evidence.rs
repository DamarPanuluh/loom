//! Evidence — the mechanical link between an asserted fact and something loom
//! can independently re-check.
//!
//! Plane: verdict integrity. This module owns the strength lattice
//! (`Verified > Cited > Claimed > Expired`) and the shapes evidence takes.
//!
//! The organizing rule, stated once: **loom records what an agent asserts, but
//! only counts what loom can re-check.** Before this, `evidence` was a `&str`
//! gated only by `is_placeholder` — which rejects "TBD" and accepts any
//! plausible sentence. An LLM can always produce a plausible sentence, so the
//! gate measured fluency, not truth. Now evidence is typed:
//!
//! - [`Evidence::Run`] — loom executed something and observed the result. It
//!   carries the file-hash set in force at run time, so any later edit to a
//!   covered file expires it. **Callers cannot construct one**: the type a
//!   caller may supply ([`CitedEvidence`]) has no `Run` variant, so "mark it
//!   passed without running it" is a compile error, not a policy.
//! - [`Evidence::Span`] — a `file:line[-line]` citation, fingerprinted at
//!   assert time and re-checked when the file changes. A citation into lines
//!   that never existed fails closed: it is direct evidence of fabrication.
//! - [`Evidence::Journal`] — a `journal:<id>` reference that must resolve in the
//!   append-only journal.
//! - [`Evidence::Claim`] — prose. Recorded, never counted.
//!
//! Citations are parsed conservatively: the path must look like a relative file
//! path with an alphabetic extension, and only paths that exist under the root
//! are stamped or judged — prose, URLs, and tool output that merely look
//! line-ish are ignored rather than guessed at.

use crate::artifact::fingerprint;
use crate::model::{
    Claim, EvidenceKind, Rework, RunProducer, StaleCause, TargetKind, Verification,
};
use crate::Result;
use anyhow::bail;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::LazyLock;

/// The asserted edge facet carrying the citation stamps of the last verdict.
pub const EVIDENCE_SPANS_KEY: &str = "evidence_spans";

/// Maximum distinct citations per verdict (bounds facet size; honest evidence
/// cites a handful of spans, not dozens). Exposed within the crate so sync can
/// treat a legacy facet at this cap as potentially truncated and fail closed.
pub(crate) const MAX_SPANS: usize = 16;

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

/// Something loom ran and observed. Minted ONLY by [`crate::runner`]; there is
/// no path from caller input to this type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub producer: RunProducer,
    pub command: String,
    /// Working directory, relative to the graph root.
    #[serde(default)]
    pub cwd: String,
    pub exit_code: i64,
    /// Fingerprints of the FULL streams; the excerpts below are for humans.
    pub stdout_hash: String,
    pub stderr_hash: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdout_excerpt: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr_excerpt: String,
    /// THE anchor: the file → content-hash set in force when this ran. Any later
    /// edit to any covered file expires the run, which is how a proof stops
    /// counting the moment the code it covered moves under it.
    #[serde(default)]
    pub covered: BTreeMap<String, String>,
    /// How many content assertions the run actually checked. Feeds derived proof
    /// strength: a run that only checks an exit code proves liveness, not
    /// behavior.
    #[serde(default)]
    pub assertions: usize,
    /// Excluded from the evidence identity digest — a re-run that observes the
    /// same thing must be a byte-identical no-op.
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub ran_at: String,
    #[serde(default)]
    pub loom_version: String,
}

/// Evidence as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
    Run(RunRecord),
    Span(SpanStamp),
    Journal { r#ref: String },
    Claim { text: String },
}

impl Evidence {
    pub fn kind(&self) -> EvidenceKind {
        match self {
            Evidence::Run(_) => EvidenceKind::Run,
            Evidence::Span(_) => EvidenceKind::Span,
            Evidence::Journal { .. } => EvidenceKind::Journal,
            Evidence::Claim { .. } => EvidenceKind::Claim,
        }
    }

    /// The strength this evidence confers while it holds.
    pub fn strength(&self) -> Verification {
        match self {
            Evidence::Run(_) => Verification::Verified,
            Evidence::Span(_) | Evidence::Journal { .. } => Verification::Cited,
            Evidence::Claim { .. } => Verification::Claimed,
        }
    }

    /// Identity for content-addressing: everything except the volatile fields.
    /// Two observations of the same thing are the same evidence.
    fn identity(&self) -> String {
        match self {
            Evidence::Run(r) => {
                let covered: Vec<String> =
                    r.covered.iter().map(|(f, h)| format!("{f}={h}")).collect();
                format!(
                    "run\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
                    r.producer.as_str(),
                    r.command,
                    r.exit_code,
                    r.stdout_hash,
                    r.stderr_hash,
                    covered.join("\u{1e}")
                )
            }
            Evidence::Span(s) => format!("span\u{1f}{}\u{1f}{}\u{1f}{}", s.file, s.start, s.end),
            Evidence::Journal { r#ref } => format!("journal\u{1f}{ref}", ref = r#ref),
            Evidence::Claim { text } => format!("claim\u{1f}{text}"),
        }
    }
}

/// What a CALLER may supply. Structurally has no `Run` variant — this is the
/// type-level half of "loom produces Run evidence; callers cannot". An agent
/// wanting a `verified` fact has exactly one route: ask loom to run something.
#[derive(Debug, Clone, PartialEq)]
pub enum CitedEvidence {
    Span(SpanStamp),
    Journal(String),
    Claim(String),
}

impl CitedEvidence {
    pub fn into_evidence(self) -> Evidence {
        match self {
            CitedEvidence::Span(s) => Evidence::Span(s),
            CitedEvidence::Journal(r) => Evidence::Journal { r#ref: r },
            CitedEvidence::Claim(t) => Evidence::Claim { text: t },
        }
    }
}

/// A stored evidence row. The re-check bookkeeping is local — it never travels
/// in the export, so a `sync` that only re-verifies never dirties the committed
/// graph file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRow {
    pub id: String,
    pub fact_id: String,
    pub payload: Evidence,
    pub recorded_at: String,
    /// `false` once an anchor has broken.
    #[serde(skip, default = "holds_default")]
    pub holds: bool,
    #[serde(skip, default)]
    pub expiry_reason: Option<StaleCause>,
}

fn holds_default() -> bool {
    true
}

impl EvidenceRow {
    /// Deterministic, content-addressed id, so re-recording the same observation
    /// is an upsert rather than a duplicate.
    pub fn id_for(fact_id: &str, payload: &Evidence) -> String {
        format!(
            "e{}",
            crate::store::fnv_hex_digest(&[fact_id, &payload.identity()])
        )
    }
}

/// A stored fact: one current state per (subject, claim).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    pub id: String,
    pub subject_kind: TargetKind,
    pub subject_id: String,
    pub claim: Claim,
    /// Claim-specific vocabulary (`passing`, `justified`, `ratified`, …).
    pub state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub criterion: String,
    /// Derived from the live evidence; exported so strength travels with the
    /// graph, but recomputed locally on import — a claim of `verified` from
    /// elsewhere is a claim until this filesystem agrees.
    pub verification: Verification,
    #[serde(default)]
    pub confidence: f64,
    pub asserted_by: String,
    pub asserted_at: String,
    #[serde(skip, default)]
    pub stale: Option<StaleReason>,
}

impl Fact {
    pub fn id_for(subject_kind: TargetKind, subject_id: &str, claim: Claim) -> String {
        format!(
            "f{}",
            crate::store::fnv_hex_digest(&[subject_kind.as_str(), subject_id, claim.as_str()])
        )
    }
}

/// Why a fact was re-opened, and what re-closing it will cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaleReason {
    pub cause: StaleCause,
    pub rework: Rework,
    /// The files, symbols, or evidence ids that moved.
    #[serde(default)]
    pub subjects: Vec<String>,
    pub at: String,
}

impl StaleReason {
    pub fn new(cause: StaleCause, subjects: Vec<String>, at: String) -> StaleReason {
        StaleReason {
            cause,
            rework: cause.rework(),
            subjects,
            at,
        }
    }

    /// One line an operator can act on.
    pub fn describe(&self) -> String {
        let what = match self.cause {
            StaleCause::RunCoveredFileChanged => "a file the recorded run covered changed",
            StaleCause::RunCommandChanged => "the command that produced the run changed",
            StaleCause::SpanRewritten => "the cited evidence was rewritten",
            StaleCause::SpanFileDeleted => "the cited file is gone",
            StaleCause::JournalMissing => "the cited journal entry is unreachable",
            StaleCause::SubjectRedefined => "the claim's subject was redefined",
            StaleCause::RoleChanged => "the grounding role changed",
            StaleCause::Rehomed => "the grounding was rehomed",
            StaleCause::AnchorMissing => "nothing re-checkable anchors this claim",
        };
        if self.subjects.is_empty() {
            what.to_string()
        } else {
            format!("{what}: {}", self.subjects.join(", "))
        }
    }
}

/// The strength of a fact: the strongest evidence still holding.
///
/// THE definition of truth strength in loom. `Claim` evidence never expires
/// (prose cannot rot mechanically) and never rises above `Claimed`, so a fact
/// justified only by a sentence can never satisfy a rung.
pub fn level(rows: &[EvidenceRow]) -> Verification {
    rows.iter()
        .filter(|r| r.holds)
        .map(|r| r.payload.strength())
        .max_by_key(|v| v.rank())
        .unwrap_or(Verification::Expired)
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

static JOURNAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"journal:([A-Za-z0-9_-]+)").expect("journal citation regex is valid")
});

/// Parse and stamp every file:line citation in `evidence` that resolves to a
/// readable file under `root`.
///
/// Fails closed when an EXISTING file is cited with a line range that does not
/// exist in it (start of 0, inverted range, or end beyond EOF) — the recorder
/// is describing bytes nobody can read. Citations whose path does not resolve
/// (URLs, tool output, deleted files, absolute/parent-escaping paths) are
/// skipped, never guessed at.
/// Turn caller-supplied prose into the evidence it actually anchors.
///
/// Every `file:line` citation becomes a [`CitedEvidence::Span`], every
/// `journal:<id>` a [`CitedEvidence::Journal`], and the prose itself is kept as
/// a [`CitedEvidence::Claim`] so the human-readable justification survives even
/// when it does not count. There is deliberately no path here to
/// [`Evidence::Run`]: `CitedEvidence` has no such variant.
pub fn cite(root: &Path, prose: &str) -> Result<Vec<CitedEvidence>> {
    let mut out: Vec<CitedEvidence> = Vec::new();
    for cap in JOURNAL_RE.captures_iter(prose) {
        let id = &cap[1];
        if !crate::journal::exists(root, id)? {
            bail!("evidence cites journal:{id}, but no such append-only journal entry exists");
        }
        out.push(CitedEvidence::Journal(id.to_string()));
    }
    out.extend(stamp(root, prose)?.into_iter().map(CitedEvidence::Span));
    if !prose.trim().is_empty() {
        out.push(CitedEvidence::Claim(prose.trim().to_string()));
    }
    Ok(out)
}

pub fn stamp(root: &Path, evidence: &str) -> Result<Vec<SpanStamp>> {
    for cap in JOURNAL_RE.captures_iter(evidence) {
        let id = &cap[1];
        if !crate::journal::exists(root, id)? {
            bail!("evidence cites journal:{id}, but no such append-only journal entry exists");
        }
    }
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
        if stamps.len() == MAX_SPANS {
            bail!(
                "evidence cites more than {MAX_SPANS} distinct spans — reduce it to the most \
                 decision-relevant citations"
            );
        }
        stamps.push(SpanStamp {
            file: file.to_string(),
            start,
            end,
            hash: fingerprint(&lines[start - 1..end].join("\n")),
        });
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
