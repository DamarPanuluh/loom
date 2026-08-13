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
//!   The anchor is the content (and its enclosing symbol), not the position:
//!   a span whose body moves intact — within the file, or into one declared
//!   successor file — is re-anchored and journaled (`evidence_reanchor`), not
//!   re-opened. Line numbers are display metadata.
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
pub(crate) const MAX_SEARCH_LINES: usize = 20_000;

/// One cited span, stamped at verdict time. `hash` is the FNV fingerprint of
/// the cited lines joined with `\n`, exactly as [`crate::artifact::fingerprint`]
/// hashes file content.
///
/// The span's IDENTITY is its content (plus the enclosing symbol's, when the
/// language yields one); `start`/`end` are the coordinates it was last seen
/// at — display metadata that re-anchoring rewrites as the code moves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanStamp {
    pub file: String,
    pub start: usize,
    pub end: usize,
    pub hash: String,
    /// Hash of the WHOLE file when this span was stamped, set only where the
    /// claim's scope is the whole file — a realizing grounding that names no
    /// symbol. Such a claim says "the behavior lives in this file", and a
    /// citation surviving verbatim while the code around it is rewritten says
    /// nothing about whether that is still true. Naming a symbol is what buys
    /// the narrower scope; not naming one is not free.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file_hash: String,
    /// Name of the smallest symbol enclosing the cited span at stamp time,
    /// when the file's language has an extractor. A symbol-anchored span
    /// re-anchors by name and body hash, not by line proximity, so a move
    /// anywhere in the file (or into one declared successor file) keeps the
    /// verdict without a re-inspection.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub symbol: String,
    /// Fingerprint of the enclosing symbol's body lines at stamp time. Only an
    /// identical body re-anchors by name — a redefined symbol is a rewritten
    /// anchor, not a moved one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub symbol_hash: String,
    /// 0-based offset of `start` within the enclosing symbol. An intact body
    /// relocates the span to exactly `symbol.line_start + symbol_offset`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub symbol_offset: usize,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

/// Journal event recording that a span's stored coordinates moved while its
/// content stood. Emitted once per re-anchor by the re-verification pass.
pub const REANCHOR_EVENT: &str = "evidence_reanchor";

/// Whether structured Journey assertion names on a [`RunRecord`] may count as
/// compiler-owned machine evidence.
///
/// Public Deserialize, import JSON, and caller-built records are always
/// [`Untrusted`]: field privacy does not constrain Serde, so a JSON payload
/// can populate `observed_assertions` without ever running the Journey.
/// Only the compiler-owned settlement path, and the local store reload of a
/// row that path persisted, mark [`LocallyMinted`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AssertionTrust {
    #[default]
    Untrusted,
    LocallyMinted,
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
    /// Typed assertion names carried on the run for audit and, when
    /// [`assertion_trust`] is [`AssertionTrust::LocallyMinted`], for S3.
    /// Deserialize always leaves trust at [`AssertionTrust::Untrusted`], so a
    /// JSON payload cannot mint compiler-owned provenance merely by populating
    /// this field. Read it through [`RunRecord::observed_assertions`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) observed_assertions: Vec<ObservedAssertion>,
    /// Persisted marker written ONLY by the Store-owned guarded Journey
    /// settlement. A local store reload re-mints [`AssertionTrust::LocallyMinted`]
    /// only when this is set; graph import sanitizes it, so a forged export
    /// cannot smuggle settlement provenance into another repository.
    #[serde(default)]
    pub(crate) locally_minted: bool,
    /// Never serialized. Public Deserialize therefore cannot claim local mint.
    #[serde(skip)]
    pub(crate) assertion_trust: AssertionTrust,
    /// Excluded from the evidence identity digest — a re-run that observes the
    /// same thing must be a byte-identical no-op.
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub ran_at: String,
    #[serde(default)]
    pub loom_version: String,
}

/// One typed assertion a run's owner observed holding. `group` namespaces
/// `assertion` (for a Journey run, the operation id): assertion ids are unique
/// only within their group.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedAssertion {
    pub group: String,
    pub assertion: String,
}

impl RunRecord {
    /// The typed assertions this run's owner observed holding. Names may be
    /// present on deserialized or imported records for audit; S3 credits them
    /// only when [`assertion_trust`] is [`AssertionTrust::LocallyMinted`].
    pub fn observed_assertions(&self) -> &[ObservedAssertion] {
        &self.observed_assertions
    }

    pub(crate) fn trust_local_store(&mut self) {
        // Local reload re-mints provenance only for rows the Store-owned
        // guarded settlement itself persisted (the serialized marker it
        // writes). Imported rows keep Untrusted trust forever.
        if self.locally_minted {
            self.assertion_trust = AssertionTrust::LocallyMinted;
        }
    }

    pub(crate) fn has_trusted_journey_assertions(&self) -> bool {
        self.assertion_trust == AssertionTrust::LocallyMinted
            && self.producer == crate::model::RunProducer::Journey
            && self.exit_code == 0
            && !self.covered.is_empty()
            && !self.observed_assertions.is_empty()
    }
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
    pub(crate) fn trust_local_store(&mut self) {
        if let Evidence::Run(run) = self {
            run.trust_local_store();
        }
    }

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
                // Observed assertions participate in the identity digest only
                // when present: pre-existing runs (and every non-Journey run)
                // keep their byte-identical identity, while two runs that
                // observed different typed assertions are different evidence.
                let observed = if r.observed_assertions.is_empty() {
                    String::new()
                } else {
                    let mut sorted = r.observed_assertions.clone();
                    sorted.sort();
                    format!(
                        "\u{1f}{}",
                        sorted
                            .iter()
                            .map(|a| format!("{}:{}", a.group, a.assertion))
                            .collect::<Vec<_>>()
                            .join("\u{1e}")
                    )
                };
                format!(
                    "run\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}{}",
                    r.producer.as_str(),
                    r.command,
                    r.exit_code,
                    r.stdout_hash,
                    r.stderr_hash,
                    covered.join("\u{1e}"),
                    observed
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
    /// Executor profile (for example `loom-auditor`) kept separate from the
    /// authorization identity in `asserted_by`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asserted_profile: Option<String>,
    pub asserted_at: String,
    /// Whether this fact claims an independent judgment or a batch decision.
    #[serde(default)]
    pub decision_mode: crate::model::DecisionMode,
    /// Journal id of the covering `batch_authorization` envelope, when batch.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub batch_id: String,
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
            StaleCause::SeamGone => "the seam this file used is no longer in it",
            StaleCause::ScopeFileChanged => {
                "the file this claim scopes changed around intact evidence"
            }
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
    // ANY broken anchor is dispositive, not merely the weakest one. A fact
    // stands on everything it cited: if the locator still resolves but the span
    // the worker pointed at was rewritten, the recorded justification is gone
    // and the claim has to be looked at again. Taking the max over surviving
    // rows instead would let a decorative citation — or, worse, loom's own
    // probe — hold a claim up after the thing it was ABOUT has gone, which is
    // the failure mode this spine exists to close.
    if rows.iter().any(|r| !r.holds) {
        return Verification::Expired;
    }
    rows.iter()
        .filter(|r| r.holds)
        .map(|r| r.payload.strength())
        .max_by_key(|v| v.rank())
        .unwrap_or(Verification::Expired)
}

/// A `path.ext:start[-end]` citation, where the end may be left open
/// (`path.ext:start-`, clamped to EOF). The path needs an alphabetic extension
/// so version numbers ("1.2:3") and times ("12:30") never match; existence
/// under the root is the real gate. `+ [ ] @` are path characters because
/// route-file conventions depend on them (SvelteKit `+layout.svelte`,
/// `r/[id]/+page@.svelte`; Next.js `[id]`, `@modal`) — without them a
/// citation into such a file silently degrades to "claimed".
static CITATION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?P<file>[A-Za-z0-9_][A-Za-z0-9_./\-+\[\]@]*\.[A-Za-z][A-Za-z0-9_]*):(?P<start>\d{1,7})(?:-(?P<end>\d{1,7})|(?P<open>-))?",
    )
    .expect("citation regex is valid")
});

/// A `path.ext:@symbol` citation — the span is resolved server-side from the
/// symbol's declaration, so the recorder names WHAT it read and never counts
/// lines at all. The path charset matches [`CITATION_RE`].
static SYMBOL_CITATION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?P<file>[A-Za-z0-9_][A-Za-z0-9_./\-+\[\]@]*\.[A-Za-z][A-Za-z0-9_]*):@(?P<symbol>[A-Za-z0-9_:]+)",
    )
    .expect("symbol citation regex is valid")
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
    // Placeholder prose ("…", "TBD", "<reason>") is not weak evidence — it is
    // NO evidence, so it produces no row at all. The floor then refuses the
    // write for the honest reason: there is nothing here to re-check. This is
    // where the old `is_placeholder` gate went: from a special case to a
    // consequence of the lattice.
    if !crate::model::is_placeholder(prose) {
        out.push(CitedEvidence::Claim(prose.trim().to_string()));
    }
    Ok(out)
}

/// One parsed citation, before its span is resolved against the file.
enum RawCitation {
    /// `file:start`, `file:start-end`, or `file:start-` (open — clamp to EOF).
    Lines {
        start: usize,
        end: Option<usize>,
        open: bool,
    },
    /// `file:@symbol` — resolved from the declaration, never line-counted.
    Symbol(String),
}

pub fn stamp(root: &Path, evidence: &str) -> Result<Vec<SpanStamp>> {
    for cap in JOURNAL_RE.captures_iter(evidence) {
        let id = &cap[1];
        if !crate::journal::exists(root, id)? {
            bail!("evidence cites journal:{id}, but no such append-only journal entry exists");
        }
    }
    let mut raws: Vec<(String, RawCitation)> = Vec::new();
    for cap in CITATION_RE.captures_iter(evidence) {
        raws.push((
            cap["file"].to_string(),
            RawCitation::Lines {
                start: cap["start"].parse().unwrap_or(0),
                end: cap.name("end").map(|m| m.as_str().parse().unwrap_or(0)),
                open: cap.name("open").is_some(),
            },
        ));
    }
    for cap in SYMBOL_CITATION_RE.captures_iter(evidence) {
        raws.push((
            cap["file"].to_string(),
            RawCitation::Symbol(cap["symbol"].to_string()),
        ));
    }

    let mut seen: BTreeSet<(String, usize, usize)> = BTreeSet::new();
    // Symbols are extracted once per cited file, not once per citation into it.
    let mut symbol_cache: BTreeMap<String, Vec<crate::extract::Symbol>> = BTreeMap::new();
    let mut stamps = Vec::new();
    for (file, raw) in raws {
        // Only relative paths inside the root are citable code.
        if file.starts_with('/') || file.split('/').any(|seg| seg == "..") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(root.join(&file)) else {
            continue; // not a file under this root — prose, URL, or gone
        };
        let lines: Vec<&str> = content.lines().collect();
        let loc = lines.len();
        let (start, end) = match raw {
            RawCitation::Lines { start, end, open } => {
                // A start beyond EOF is the fabrication signal and stays
                // fatal. An OPEN end clamps to EOF — the recorder said "from
                // here down", which always names real lines when the start
                // does. An explicit end beyond EOF stays rejected: it claims
                // a precise boundary that does not exist.
                let end = if open { loc } else { end.unwrap_or(start) };
                if start < 1 || start > loc || end < start || end > loc {
                    bail!(
                        "evidence cites {file}:{start}{} but {file} has {loc} lines — \
                         evidence must cite lines that exist{}",
                        if end != start {
                            format!("-{end}")
                        } else {
                            String::new()
                        },
                        if !open && end > loc && start >= 1 && start <= loc {
                            format!(" (an open range {file}:{start}- clamps to EOF)")
                        } else {
                            String::new()
                        }
                    );
                }
                (start, end)
            }
            RawCitation::Symbol(name) => {
                let symbols = symbol_cache
                    .entry(file.clone())
                    .or_insert_with(|| crate::extract::extract(&file, &content).symbols);
                let Some(candidate) = crate::locator::symbol(&name) else {
                    bail!("evidence cites {file}:@{name}, which is not a symbol name");
                };
                let hits: Vec<&crate::extract::Symbol> =
                    symbols.iter().filter(|s| s.name == candidate).collect();
                match hits.as_slice() {
                    [] => bail!(
                        "evidence cites {file}:@{name} but no symbol '{candidate}' is declared \
                         in {file} — evidence must cite what exists"
                    ),
                    [sym] => (sym.line_start, sym.line_end),
                    many => bail!(
                        "evidence cites {file}:@{name} but {} symbols named '{candidate}' are \
                         declared in {file} — cite file:start-end lines to disambiguate",
                        many.len()
                    ),
                }
            }
        };
        if !seen.insert((file.clone(), start, end)) {
            continue;
        }
        if stamps.len() == MAX_SPANS {
            bail!(
                "evidence exceeds max_spans={MAX_SPANS} distinct citations — reduce it to the \
                 most decision-relevant citations"
            );
        }
        let symbols = symbol_cache
            .entry(file.clone())
            .or_insert_with(|| crate::extract::extract(&file, &content).symbols);
        let (symbol, symbol_hash, symbol_offset) = enclosing_symbol(symbols, start, end)
            .map(|sym| {
                (
                    sym.name.clone(),
                    symbol_body_hash(&lines, sym),
                    start - sym.line_start,
                )
            })
            .unwrap_or_default();
        stamps.push(SpanStamp {
            file: file.to_string(),
            start,
            end,
            hash: fingerprint(&lines[start - 1..end].join("\n")),
            // Set by the caller that knows the claim's scope; a bare citation
            // carries none, so it is span-scoped by default.
            file_hash: String::new(),
            symbol,
            symbol_hash,
            symbol_offset,
        });
    }
    Ok(stamps)
}

/// The smallest declared symbol fully enclosing `start..=end`, if any. The
/// name becomes the span's identity across moves; a smaller enclosing symbol
/// is a more precise anchor than an outer one (a method, not its module).
fn enclosing_symbol(
    symbols: &[crate::extract::Symbol],
    start: usize,
    end: usize,
) -> Option<&crate::extract::Symbol> {
    symbols
        .iter()
        .filter(|s| s.line_start <= start && s.line_end >= end)
        .min_by_key(|s| s.line_end - s.line_start)
}

/// Fingerprint of a symbol's body lines — the same join [`stamp`] and the
/// re-verification pass both hash, so a moved-but-intact body compares equal.
fn symbol_body_hash(lines: &[&str], sym: &crate::extract::Symbol) -> String {
    fingerprint(&lines[sym.line_start - 1..sym.line_end].join("\n"))
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
    Some(cited.into_iter().all(|s| {
        matches!(
            span_fate(s, file, content),
            SpanFate::Intact | SpanFate::Moved { .. }
        )
    }))
}

/// How a stamped span fared against the current file content. The distinction
/// that matters downstream is not merely "does it still hold" but what it
/// would COST to settle again: a citation that still reads as recorded — at
/// its recorded coordinates or relocated — is a re-confirm, one that is gone
/// is a re-inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanFate {
    /// At the recorded position with the recorded content.
    Intact,
    /// Verbatim at new coordinates in the same file — relocated through the
    /// enclosing symbol's identity when one is anchored, else as a verbatim
    /// window. The verdict stands; the stored stamp re-anchors.
    Moved {
        start: usize,
        end: usize,
    },
    /// The cited lines survive, but the file this claim scoped changed.
    ScopeChanged,
    Rewritten,
}

pub fn span_fate(s: &SpanStamp, file: &str, content: &str) -> SpanFate {
    let lines: Vec<&str> = content.lines().collect();
    let at_position = cited_at_recorded_position(s, &lines);
    // A file-scoped stamp is falsified by any edit to the file, whatever
    // happened to the cited lines themselves.
    if !s.file_hash.is_empty() && fingerprint(&lines.join("\n")) != s.file_hash {
        return if at_position {
            SpanFate::ScopeChanged
        } else {
            SpanFate::Rewritten
        };
    }
    if at_position {
        return SpanFate::Intact;
    }
    match relocate_within(s, file, &lines) {
        Some((start, end)) => SpanFate::Moved { start, end },
        None => SpanFate::Rewritten,
    }
}

/// The cited lines exactly where the stamp last saw them.
fn cited_at_recorded_position(s: &SpanStamp, lines: &[&str]) -> bool {
    // Lines are 1-based and a range runs low→high. A zero start or an inverted
    // range (end < start) names no real span — it is a malformed or imported
    // stamp, and treating it as holding nothing avoids a usize underflow on
    // `s.end - s.start + 1` and an out-of-range slice at `s.start - 1`.
    if s.start == 0 || s.end < s.start {
        return false;
    }
    s.end <= lines.len() && fingerprint(&lines[s.start - 1..s.end].join("\n")) == s.hash
}

/// Where this span's content now lives in the same file, if anywhere. Symbol
/// identity first — the name is the anchor, the line is display — then a
/// verbatim window of the same height for spans no symbol encloses.
fn relocate_within(s: &SpanStamp, file: &str, lines: &[&str]) -> Option<(usize, usize)> {
    if s.start == 0 || s.end < s.start {
        return None;
    }
    let height = s.end - s.start + 1;
    if !s.symbol.is_empty() {
        let content = lines.join("\n");
        for sym in &crate::extract::extract(file, &content).symbols {
            if sym.name == s.symbol && symbol_body_hash(lines, sym) == s.symbol_hash {
                let start = sym.line_start + s.symbol_offset;
                let end = start + height - 1;
                // The body hash already proves the content; the span-level
                // check guards the offset arithmetic against a malformed stamp.
                if end <= lines.len() && fingerprint(&lines[start - 1..end].join("\n")) == s.hash {
                    return Some((start, end));
                }
            }
        }
    }
    verbatim_window(s, lines, height)
}

/// The first verbatim window of `height` lines matching the stamp's hash.
fn verbatim_window(s: &SpanStamp, lines: &[&str], height: usize) -> Option<(usize, usize)> {
    if lines.len() > MAX_SEARCH_LINES || height > lines.len() {
        return None;
    }
    (0..=lines.len() - height).find_map(|i| {
        (fingerprint(&lines[i..i + height].join("\n")) == s.hash).then_some((i + 1, i + height))
    })
}

/// Find this span's content in exactly one candidate file — the declared
/// successors of a deleted citation. `candidates` is `(path, content)` pairs
/// of registered codefiles still on disk.
///
/// Uniqueness is the honesty gate: a body found in two successor files is a
/// coincidence loom cannot arbitrate, so the move fails closed and the claim
/// re-opens for a human look. A file-scoped stamp never relocates across
/// files — the claim was about THAT file, not its content.
pub fn find_elsewhere(
    s: &SpanStamp,
    candidates: &[(String, String)],
) -> Option<(String, usize, usize)> {
    if s.start == 0 || s.end < s.start || !s.file_hash.is_empty() {
        return None;
    }
    let height = s.end - s.start + 1;
    let mut found: Option<(String, usize, usize)> = None;
    for (path, content) in candidates {
        if path == &s.file {
            continue;
        }
        let lines: Vec<&str> = content.lines().collect();
        let Some((start, end)) = verbatim_window(s, &lines, height) else {
            continue;
        };
        if found.is_some() {
            return None; // ambiguous — could be a copy, not the move
        }
        // Prefer symbol-exact coordinates within the successor when the span
        // is symbol-anchored; the window hit is already exact content either
        // way.
        let (start, end) = relocate_within(s, path, &lines).unwrap_or((start, end));
        found = Some((path.clone(), start, end));
    }
    found
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

    /// Route-file conventions (SvelteKit, Next.js) put `+`, `[`, `]`, `@` in
    /// real paths; a citation into one must parse as the whole path, not a
    /// truncated suffix that fails to resolve.
    #[test]
    fn parses_route_convention_paths() {
        let caps: Vec<_> = CITATION_RE
            .captures_iter(
                "web/src/routes/+layout.svelte:14-18 and web/src/routes/r/[id]/+page@.svelte:3",
            )
            .map(|c| c["file"].to_string())
            .collect();
        assert_eq!(
            caps,
            vec![
                "web/src/routes/+layout.svelte",
                "web/src/routes/r/[id]/+page@.svelte"
            ]
        );
    }

    #[test]
    fn ignores_versions_and_times() {
        assert!(CITATION_RE
            .captures_iter("v1.2:3 at 12:30")
            .next()
            .is_none());
    }

    /// A scratch root with one source file behind it, for stamp() tests.
    struct StampRoot(std::path::PathBuf);

    impl StampRoot {
        fn new(test: &str, content: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("loom-evidence-stamp-{test}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("f.rs"), content).unwrap();
            StampRoot(dir)
        }
    }

    impl Drop for StampRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `file:@symbol` resolves the span server-side: the recorder names WHAT
    /// it read and never counts lines. The stamp anchors to the symbol.
    #[test]
    fn symbol_citation_resolves_server_side() {
        let root = StampRoot::new(
            "symbol",
            "pub fn alpha() {\n    let a = 1;\n}\n\npub fn beta() {}\n",
        );
        let stamps = stamp(&root.0, "checked f.rs:@alpha — looks right").unwrap();
        assert_eq!(stamps.len(), 1);
        let s = &stamps[0];
        assert_eq!((s.start, s.end), (1, 3), "the symbol's declaration span");
        assert_eq!(s.symbol, "alpha");
        assert!(!s.symbol_hash.is_empty());

        let missing = stamp(&root.0, "checked f.rs:@gamma").unwrap_err();
        assert!(
            missing.to_string().contains("no symbol 'gamma'"),
            "a symbol that does not exist is the fabrication signal: {missing}"
        );
    }

    /// Two symbols sharing a name cannot be told apart by `@name` alone.
    #[test]
    fn ambiguous_symbol_citation_is_refused() {
        let root = StampRoot::new(
            "ambiguous",
            "mod a {\n    pub fn dup() {}\n}\nmod b {\n    pub fn dup() {}\n}\n",
        );
        let err = stamp(&root.0, "checked f.rs:@dup").unwrap_err();
        assert!(
            err.to_string().contains("disambiguate"),
            "ambiguity must ask for lines, not guess: {err}"
        );
    }

    /// `file:N-` clamps to EOF — "from here down" always names real lines
    /// when the start does. A start beyond EOF stays fatal, and so does an
    /// EXPLICIT end beyond EOF: it claims a precise boundary that is not there.
    #[test]
    fn open_ranges_clamp_but_explicit_overreach_is_rejected() {
        let root = StampRoot::new("open-range", "one\ntwo\nthree\n");
        let stamps = stamp(&root.0, "read f.rs:2- to the end").unwrap();
        assert_eq!((stamps[0].start, stamps[0].end), (2, 3));

        let over_start = stamp(&root.0, "read f.rs:9-").unwrap_err();
        assert!(
            over_start.to_string().contains("has 3 lines"),
            "a start beyond EOF is the fabrication signal: {over_start}"
        );

        let over_end = stamp(&root.0, "read f.rs:1-9").unwrap_err();
        assert!(
            over_end.to_string().contains("has 3 lines"),
            "an explicit end beyond EOF stays rejected: {over_end}"
        );
        assert!(
            over_end.to_string().contains("f.rs:1- clamps to EOF"),
            "and the retry is spelled out: {over_end}"
        );
    }

    fn bare_stamp(start: usize, end: usize, hash: String) -> SpanStamp {
        SpanStamp {
            file: "f.rs".into(),
            start,
            end,
            hash,
            file_hash: String::new(),
            symbol: String::new(),
            symbol_hash: String::new(),
            symbol_offset: 0,
        }
    }

    /// A file-scoped stamp is falsified by an edit anywhere in the file, even
    /// one that leaves the cited lines verbatim — the claim was about the file.
    #[test]
    fn file_scoped_span_falls_when_the_file_changes_around_it() {
        let stamp = SpanStamp {
            file_hash: fingerprint("a\nb\nc"),
            ..bare_stamp(1, 2, fingerprint("a\nb"))
        };
        assert_eq!(
            spans_status(std::slice::from_ref(&stamp), "f.rs", "a\nb\nc"),
            Some(true),
            "unchanged file: the claim stands"
        );
        assert_eq!(
            spans_status(std::slice::from_ref(&stamp), "f.rs", "a\nb\nDIFFERENT"),
            Some(false),
            "the cited lines survive verbatim, but the file they scope did not"
        );
    }

    #[test]
    fn moved_span_still_intact() {
        let stamp = bare_stamp(1, 2, fingerprint("a\nb"));
        // Two prepended lines: the cited span moved but is verbatim-intact.
        assert_eq!(
            span_fate(&stamp, "f.rs", "x\ny\na\nb"),
            SpanFate::Moved { start: 3, end: 4 }
        );
        assert_eq!(
            spans_status(std::slice::from_ref(&stamp), "f.rs", "x\ny\na\nb"),
            Some(true)
        );
        // The span's body changed: rewritten.
        assert_eq!(span_fate(&stamp, "f.rs", "a\nc"), SpanFate::Rewritten);
        // A file the verdict never cited: no signal.
        assert_eq!(spans_status(&[stamp], "other.rs", "a\nb"), None);
    }

    /// A symbol-anchored span relocates through the symbol's identity: body
    /// intact anywhere in the file re-anchors to the exact new coordinates;
    /// a redefined body is a rewrite, not a move.
    #[test]
    fn symbol_anchored_span_tracks_the_symbol() {
        let original = "pub fn f() {\n    let a = 1;\n    let b = 2;\n}\n\npub fn g() {}\n";
        let moved = "// header\n// more\npub fn f() {\n    let a = 1;\n    let b = 2;\n}\n\npub fn g() {}\n";
        let redefined = "pub fn f() {\n    let a = 1;\n    let b = 3;\n}\n";
        let symbols = crate::extract::extract("f.rs", original).symbols;
        let sym = symbols.iter().find(|s| s.name == "f").expect("f extracted");
        let lines: Vec<&str> = original.lines().collect();
        let stamp = SpanStamp {
            symbol: "f".into(),
            symbol_hash: symbol_body_hash(&lines, sym),
            symbol_offset: 1, // cited span starts one line into the body
            ..bare_stamp(2, 3, fingerprint("    let a = 1;\n    let b = 2;"))
        };
        assert_eq!(
            span_fate(&stamp, "f.rs", moved),
            SpanFate::Moved { start: 4, end: 5 },
            "the intact body re-anchors by name, not by line proximity"
        );
        assert_eq!(
            span_fate(&stamp, "f.rs", redefined),
            SpanFate::Rewritten,
            "a redefined symbol is a rewritten anchor, not a moved one"
        );
    }

    /// A deleted file's span re-anchors into the ONE registered successor
    /// holding its content; two candidates holding it is a coincidence loom
    /// cannot arbitrate, and a file-scoped claim never crosses files.
    #[test]
    fn successor_search_requires_a_unique_match() {
        let stamp = bare_stamp(1, 2, fingerprint("a\nb"));
        let one = [("new.rs".to_string(), "x\na\nb\ny".to_string())];
        assert_eq!(
            find_elsewhere(&stamp, &one),
            Some(("new.rs".to_string(), 2, 3))
        );
        let two = [
            ("new.rs".to_string(), "x\na\nb".to_string()),
            ("other.rs".to_string(), "a\nb\nz".to_string()),
        ];
        assert_eq!(
            find_elsewhere(&stamp, &two),
            None,
            "ambiguous move fails closed"
        );
        let scoped = SpanStamp {
            file_hash: fingerprint("a\nb"),
            ..bare_stamp(1, 2, fingerprint("a\nb"))
        };
        assert_eq!(
            find_elsewhere(&scoped, &one),
            None,
            "a file-scoped claim was about THAT file"
        );
    }

    #[test]
    fn deserialized_run_record_cannot_claim_locally_minted_assertions() {
        let forged: RunRecord = serde_json::from_value(serde_json::json!({
            "producer": "journey",
            "command": "loom journey run flow --profile proof",
            "exit_code": 0,
            "stdout_hash": "h",
            "stderr_hash": "h",
            "covered": {"src/cli.rs": "h"},
            "assertions": 1,
            "observed_assertions": [{"group": "act-op", "assertion": "act-ok"}]
        }))
        .unwrap();
        assert_eq!(forged.observed_assertions().len(), 1);
        assert_eq!(forged.assertion_trust, AssertionTrust::Untrusted);
        assert!(!forged.has_trusted_journey_assertions());
    }

    #[test]
    fn pre_fix_compiler_v4_run_record_cannot_be_trusted_assertion_evidence() {
        // Pre-fix compiler-v4 settlements recorded a Journey run with coverage
        // and a human-facing stdout report, but no structured assertion names.
        let mut pre_fix: RunRecord = serde_json::from_value(serde_json::json!({
            "producer": "journey",
            "command": "loom journey run flow --profile proof",
            "exit_code": 0,
            "stdout_hash": "h",
            "stderr_hash": "h",
            "stdout_excerpt": "{\"passed_assertions\":[{\"operation_id\":\"act-op\",\"assertion_id\":\"act-ok\"}]}",
            "covered": {"src/cli.rs": "h"},
            "assertions": 1
        }))
        .unwrap();
        pre_fix.trust_local_store();
        assert!(pre_fix.observed_assertions().is_empty());
        assert!(
            !pre_fix.has_trusted_journey_assertions(),
            "stdout excerpts must not become trusted assertion provenance"
        );
    }
}
