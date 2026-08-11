//! Proof strength — how much a proof actually establishes, DERIVED.
//!
//! Plane: derived projection over the store and the working tree. Recomputed by
//! sync, never asserted, never writable.
//!
//! Contract — **the grade is earned by the proof's shape, not claimed by its
//! author.** `proof_level` used to be a string the caller passed in, and
//! `loom journey add` hardcoded `"L5"`. That is precisely how a journey whose
//! only assertion was `expect: {exit_code: 0}` became the strongest evidence
//! class in loom's own graph: three of five journeys were one step, one of them
//! claiming to prove "changing a file re-opens the asserted edges grounded in
//! it" by running `loom sync --json` and checking that the word `files_scanned`
//! appeared in the output. It never changed a file.
//!
//! Each rung below is a conjunct loom can check for itself, and every one is
//! recorded in the [`StrengthWitness`] so `loom validation show` can explain the
//! grade instead of asserting it, and so `deepen` knows which conjunct to go
//! after next.
//!
//! ## S3 evidence model: validation-specific, fail closed
//!
//! S3 is a runtime-shaped claim: *this validation's run* reached code that
//! realizes the intent. Runtime coverage would be the strongest answer, but loom
//! does not capture it yet. Until it does, this module layers two deterministic
//! static sources that can be recomputed from the graph and working tree:
//!
//! 1. an explicit `exercises` edge from the Validation to the CodeFile that is
//!    its entry surface (optionally narrowed by a locator); then
//! 2. entry points derived from the validation's own journey/command (`cargo
//!    test --test …`, test filters, `cargo run --bin …`, direct repo binaries,
//!    and script paths).
//!
//! Only those validation-specific sources may earn the call witness. The old
//! intent-level `implements(role=verifies)` surface remains as a *visible
//! diagnostic fallback* for legacy graphs: the witness records that source and
//! its files, but it is deliberately ineligible for S3. Letting that fallback
//! earn the rung would recreate the original bug — an `echo` journey would
//! inherit a sibling test file's reach merely because both validate one intent.
//!
//! The witness carries a model id and the exact source/file/symbol used. Model
//! changes are compared while the derived facet is rewritten; demotions are
//! journaled so a driver can distinguish a grading-model migration from code
//! drift. The entire derivation stays pure over Store + working tree + call
//! graph, preserving sync convergence (INV-2). Runtime trace/coverage can later
//! become a stronger first source without weakening these fail-closed rules.

use crate::model::{EdgeKind, InspectionStatus, Node, NodeType, TargetKind};
use crate::store::Store;
use crate::Result;
use clap::Parser;
use serde::{Deserialize, Serialize};

use std::path::Path;

/// The derived grade. Ordered — comparisons are meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Strength {
    /// Nothing loom ran, or it did not pass, or the evidence is only claimed.
    S0,
    /// loom ran it and it exited as expected. **Liveness only** — it says the
    /// code did not crash, which is not the same as saying it works.
    S1,
    /// Plus at least one CONTENT assertion. `exit_code` and `status` are
    /// deliberately not counted: counting them is the bug this module exists
    /// to fix, because every command asserts one whether the author meant to
    /// or not.
    S2,
    /// Plus a call witness: the proof's reachable call closure includes a
    /// symbol the intent is actually grounded in. Without this a proof can
    /// pass forever while exercising nothing the behavior is made of.
    S3,
    /// Plus a frozen baseline that replayed with zero deviations.
    S4,
    /// Plus a boundary crossing — an HTTP step, or a CLI step invoking a
    /// binary other than loom itself. A tool proving itself with itself is
    /// the weakest form of end-to-end there is.
    S5,
}

impl Strength {
    pub fn as_str(self) -> &'static str {
        match self {
            Strength::S0 => "S0",
            Strength::S1 => "S1",
            Strength::S2 => "S2",
            Strength::S3 => "S3",
            Strength::S4 => "S4",
            Strength::S5 => "S5",
        }
    }

    pub fn parse(s: &str) -> Option<Strength> {
        Some(match s {
            "S0" => Strength::S0,
            "S1" => Strength::S1,
            "S2" => Strength::S2,
            "S3" => Strength::S3,
            "S4" => Strength::S4,
            "S5" => Strength::S5,
            _ => return None,
        })
    }

    /// The floor for "proven at all": loom ran it and it asserted something
    /// about the output. Below this a proof establishes liveness, not behavior.
    /// The `proven` rung holds every implemented leaf to this.
    pub const MEANINGFUL: Strength = Strength::S2;

    /// The bar for USER-VISIBLE behavior: additionally, what the proof runs
    /// reaches the code the behavior is grounded in. This is what the Journey
    /// axis, compiled proof profile, and the shallow-proof smell hold out for —
    /// everywhere the old code read `proof_level in {L5, L6}`.
    pub const END_TO_END: Strength = Strength::S3;
}

/// Summary of the registered proof state for one intent.
///
/// This is deliberately small: callers still own their presentation, while the
/// business decision about whether a passing proof is meaningful lives here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProofAssessment {
    pub any_registered: bool,
    pub any_passing: bool,
    pub best_passing_strength: Option<Strength>,
    pub meaningful_passing: bool,
}

/// Assess all validations registered for one intent.
///
/// A passing edge below [`Strength::MEANINGFUL`] is still useful evidence that
/// the command ran, but it establishes only liveness and does not close the
/// proof gate.
pub fn assess(store: &Store, intent_id: &str) -> Result<ProofAssessment> {
    let proofs = store.edges_with(Some(EdgeKind::Validates), None, Some(intent_id))?;
    let mut best_passing_strength = None;
    for edge in &proofs {
        if edge.status != InspectionStatus::Passing {
            continue;
        }
        let strength = of(store, &edge.from_id)?;
        best_passing_strength = Some(
            best_passing_strength
                .map(|best: Strength| best.max(strength))
                .unwrap_or(strength),
        );
    }
    let any_passing = best_passing_strength.is_some();
    let meaningful_passing =
        best_passing_strength.is_some_and(|strength| strength >= Strength::MEANINGFUL);
    Ok(ProofAssessment {
        any_registered: !proofs.is_empty(),
        any_passing,
        best_passing_strength,
        meaningful_passing,
    })
}

/// The current interpretation of S3 call evidence. Persisted in every witness
/// so sync can explain model-only grade changes.
pub const STRENGTH_WITNESS_MODEL: &str = "validation-specific-v2";
const LEGACY_STRENGTH_WITNESS_MODEL: &str = "intent-wide-v1";

fn legacy_witness_model() -> String {
    LEGACY_STRENGTH_WITNESS_MODEL.into()
}

/// The validation-owned entry evidence inspected for the S3 call witness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CallEvidenceWitness {
    /// `validation_grounding`, `journey_command`, `validation_command`, or the
    /// legacy diagnostic-only `intent_wide_fallback`.
    pub source: String,
    /// Registered CodeFile used as an entry surface.
    pub file: String,
    /// Entry symbol when the command/locator narrows the file; absent means all
    /// indexed symbols in the file are possible entry points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_symbol: Option<String>,
    /// The realizing symbol reached from this entry, when a path exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grounded_symbol: Option<String>,
    /// False for the visible legacy fallback: useful diagnosis, never S3 credit.
    pub s3_eligible: bool,
}

/// Every conjunct, recorded. The point is that a grade can be argued with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrengthWitness {
    /// Witness interpretation. Old facets deserialize as intent-wide-v1 so a
    /// sync can identify and journal their migration.
    #[serde(default = "legacy_witness_model")]
    pub witness_model: String,
    pub grade: String,
    /// loom ran it and it exited as expected.
    pub ran_and_passed: bool,
    /// How many non-exit-code assertions the proof makes, DECLARED in a spec
    /// loom checks itself.
    pub content_assertions: usize,
    /// Assertions the test runner reported having checked, parsed from the
    /// output loom observed. Weaker than a declared expectation — the tool is
    /// reporting on itself — so the witness keeps the two apart rather than
    /// summing them into one number that hides which kind you have.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_assertions: Option<String>,
    /// The grounded symbol the proof's call closure reaches, if any.
    #[serde(default)]
    pub call_witness: Option<String>,
    /// Which validation-specific source/file/entry earned the witness, or which
    /// intent-wide fallback would have earned it under the legacy model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_evidence: Option<CallEvidenceWitness>,
    pub baseline_clean: bool,
    /// What boundary it crosses, if any.
    pub boundary: Option<String>,
    /// Why it stopped where it did — the next conjunct to go after.
    pub next: String,
}

impl Default for StrengthWitness {
    fn default() -> Self {
        Self {
            witness_model: STRENGTH_WITNESS_MODEL.into(),
            grade: String::new(),
            ran_and_passed: false,
            content_assertions: 0,
            observed_assertions: None,
            call_witness: None,
            call_evidence: None,
            baseline_clean: false,
            boundary: None,
            next: String::new(),
        }
    }
}

/// What the runner said it checked, read from the observation Loom recorded.
///
/// A positive structured assertion count on a passing observed run is the
/// authoritative path. Legacy command runners that did not populate that field
/// may still earn the witness from a summary naming positive passes and zero
/// failures. "ok" alone never counts.
///
/// Deliberately conservative and deliberately separate from declared
/// assertions. The tool is reporting on itself, so this is weaker evidence than
/// an expectation loom checked — the witness records which kind you have.
fn reported_assertions(edge: &Option<crate::model::Edge>, store: &Store) -> Result<Option<String>> {
    let Some(edge) = edge else { return Ok(None) };
    let Some(view) = store.fact(
        &crate::store::Subject::Edge(edge.id.clone()),
        crate::model::Claim::Verdict,
    )?
    else {
        return Ok(None);
    };
    for row in &view.evidence {
        let crate::evidence::Evidence::Run(run) = &row.payload else {
            continue;
        };
        if run.exit_code == 0 && run.assertions > 0 {
            return Ok(Some(run.assertions.to_string()));
        }
    }
    for row in &view.evidence {
        let crate::evidence::Evidence::Run(run) = &row.payload else {
            continue;
        };
        if run.exit_code == 0 {
            if let Some(summary) = parse_runner_summary(&run.stdout_excerpt) {
                return Ok(Some(summary));
            }
        }
    }
    Ok(None)
}

/// Recognise the common runners' summary lines. Returns a human-readable
/// description of what was checked, or `None` when the output does not state it.
pub fn parse_runner_summary(output: &str) -> Option<String> {
    for line in output.lines() {
        let lower = line.to_ascii_lowercase();
        // Rust: "test result: ok. 4 passed; 0 failed; ..."
        if lower.contains("test result:") && lower.contains("passed") {
            let passed = number_before(&lower, "passed")?;
            let failed = number_before(&lower, "failed").unwrap_or(0);
            if passed > 0 && failed == 0 {
                return Some(format!("{passed} test(s) reported passing by the runner"));
            }
        }
        // pytest: "==== 12 passed in 0.4s ====", jest: "Tests: 12 passed, 12 total"
        if (lower.contains("passed") || lower.contains("passing"))
            && !lower.contains("failed")
            && !lower.contains("failing")
        {
            let passed = number_before(&lower, "passed")
                .or_else(|| number_before(&lower, "passing"))
                .unwrap_or(0);
            if passed > 0 {
                return Some(format!("{passed} test(s) reported passing by the runner"));
            }
        }
    }
    None
}

/// The integer in a `<number> <word>` pair on this line, if there is one.
///
/// Scans TOKEN PAIRS rather than searching for the word: `find("failed")`
/// matches the FAILED in "test result: FAILED. 3 passed; 2 failed", takes the
/// token before it, fails to parse it, and defaults the failure count to zero —
/// which graded a failing run as evidence. My own test caught it.
fn number_before(line: &str, word: &str) -> Option<usize> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    tokens.windows(2).find_map(|pair| {
        let tail = pair[1].trim_matches(|c: char| !c.is_ascii_alphabetic());
        (tail == word).then(|| {
            pair[0]
                .trim_matches(|c: char| !c.is_ascii_digit())
                .parse()
                .ok()
        })?
    })
}

/// File-qualified realizing targets. Grading uses these so a same-named
/// symbol in another file cannot share a call witness.
fn grounded_targets(store: &Store, intent_id: &str) -> Result<Vec<(String, String)>> {
    crate::locator::realizing_targets(store, intent_id)
}

/// How far [`call_witness`] walks the call graph.
///
/// Cap of 4 hid exact callers at 6 hops (finding `d3107a6d`: ring32 research
/// tests → `push_notes`). `loom impact <sym> --depth 8` already contradicted
/// the S2 "nothing this proof runs reaches the symbol" grade. Eight matches
/// that diagnostic depth and clears the documented 6-hop case with headroom
/// for a layer or two of helpers.
pub const CALL_WITNESS_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EntryEvidence {
    pub source: &'static str,
    pub file: String,
    pub entry_symbol: Option<String>,
    pub s3_eligible: bool,
}

/// Does this validation-specific entry reach a symbol the intent is grounded
/// in? `impact` walks callers backwards; narrowing by `entry_symbol` prevents a
/// broad file match from crediting a different test in the same file.
fn call_witness(
    store: &Store,
    graph: &crate::callgraph::CallGraph,
    intent_id: &str,
    entries: &[EntryEvidence],
) -> Result<Option<CallEvidenceWitness>> {
    for (file, symbol) in grounded_targets(store, intent_id)? {
        // Exact path from this realizing definition site only — never every
        // same-named symbol in the repo.
        let reach = graph.exact_impact_at(&file, &symbol, CALL_WITNESS_DEPTH);
        for entry in entries {
            if !entry.s3_eligible {
                continue;
            }
            // The entry may itself be the grounded handler. `exact_impact_at`
            // returns callers, so that valid zero-hop path is not present in
            // `reach.callers`; recognize it only by the same exact file+symbol
            // qualification used for multi-hop witnesses. Requiring a symbol
            // keeps bare-file evidence out, while `s3_eligible` above keeps the
            // intent-wide diagnostic fallback out.
            let zero_hop = entry
                .entry_symbol
                .as_deref()
                .is_some_and(|expected| entry.file == file && expected == symbol);
            let reaches = zero_hop
                || reach.callers.iter().any(|caller| {
                    caller.file == entry.file
                        && entry
                            .entry_symbol
                            .as_deref()
                            .is_none_or(|expected| caller.symbol == expected)
                });
            if reaches {
                return Ok(Some(CallEvidenceWitness {
                    source: entry.source.into(),
                    file: entry.file.clone(),
                    entry_symbol: entry.entry_symbol.clone(),
                    grounded_symbol: Some(symbol),
                    s3_eligible: entry.s3_eligible,
                }));
            }
        }
    }
    Ok(None)
}

/// Explicit Validation→CodeFile entry evidence. This is the schema-level form
/// of a per-validation grounding: unlike `implements`, it owns no behavior/code
/// coverage and exists only to say which code surface this proof exercises.
fn validation_entries(store: &Store, validation_id: &str) -> Result<Vec<EntryEvidence>> {
    let mut out = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Exercises), Some(validation_id), None)? {
        let Some(file) = store.get_node(&edge.to_id)? else {
            continue;
        };
        let locator = store.get_facet(&edge.id, TargetKind::Edge, "locator")?;
        if locator
            .as_deref()
            .is_some_and(crate::locator::is_anchor_locator)
        {
            // Source anchors stabilize navigation only. Even when attached to
            // a callable entry, they must not become an S3 proof declaration.
            out.push(EntryEvidence {
                source: "anchor_navigation",
                file: file.name,
                entry_symbol: None,
                s3_eligible: false,
            });
            continue;
        }
        let locators = locator
            .map(|locator| crate::locator::symbols(&locator))
            .unwrap_or_default();
        // Bare file claim: diagnostic only. Locator-bound exercises is the
        // product's validation-specific entry declaration (see module docs):
        // the operator names the entry surface this validation exercises.
        // Command-derived entries are the other S3 path. Both require a call
        // witness to the realizing symbol — the locator alone is not enough
        // without that reachability check in `call_witness`.
        if locators.is_empty() {
            out.push(EntryEvidence {
                source: "validation_grounding",
                file: file.name,
                entry_symbol: None,
                s3_eligible: false,
            });
        } else {
            out.extend(locators.into_iter().map(|symbol| EntryEvidence {
                source: "validation_grounding",
                file: file.name.clone(),
                entry_symbol: Some(symbol),
                s3_eligible: true,
            }));
        }
    }
    Ok(out)
}

/// Legacy intent-level verifying files. Kept visible so migrated graphs explain
/// what the old grader used, but never eligible for S3 under this model.
fn intent_wide_entries(store: &Store, intent_id: &str) -> Result<Vec<EntryEvidence>> {
    let mut out = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Implements), Some(intent_id), None)? {
        if store.edge_superseded(&edge.id)?
            || store.grounding_role(&edge.id)? != crate::model::GroundingRole::Verifies
        {
            continue;
        }
        if let Some(file) = store.get_node(&edge.to_id)? {
            out.push(EntryEvidence {
                source: "intent_wide_fallback",
                file: file.name,
                entry_symbol: None,
                s3_eligible: false,
            });
        }
    }
    Ok(out)
}

/// Split one command segment into argv-like words with quote awareness.
///
/// Fail closed on shell syntax we do not model (`$`, backticks, redirects,
/// globs, braces). Whitespace-split with quote-stripping previously turned
/// `cargo test -- --test "foo bar"` into three tokens and credited a filter
/// that never ran.
fn argv_has_help_or_version(words: &[String]) -> bool {
    words
        .iter()
        .any(|word| matches!(word.as_str(), "--help" | "-h" | "--version" | "-V"))
}

fn shell_words(segment: &str) -> Vec<String> {
    shell_words_strict(segment).unwrap_or_default()
}

fn shell_words_strict(segment: &str) -> Option<Vec<String>> {
    // Reject operators and expansions we do not interpret. Callers that need
    // compound commands already fail closed at `command_entries`.
    if segment
        .chars()
        .any(|c| matches!(c, '`' | '$' | '>' | '<' | '*' | '?' | '{' | '}' | '~'))
    {
        return None;
    }
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = segment.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    // True once the current token has seen a quote pair (possibly empty).
    let mut quoted_token = false;
    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                quoted_token = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                quoted_token = true;
            }
            '\\' if in_double => {
                // Only shell-escapable characters consume the backslash inside
                // double quotes. `\_` must remain `\_`, or a filter that never
                // ran can be rewritten into a live symbol name.
                let next = chars.next()?;
                if matches!(next, '"' | '\\' | '`' | '$' | '\n') {
                    current.push(next);
                } else {
                    current.push('\\');
                    current.push(next);
                }
            }
            '\\' if !in_single && !in_double => return None,
            c if c.is_whitespace() && !in_single && !in_double => {
                if !current.is_empty() || quoted_token {
                    words.push(std::mem::take(&mut current));
                    quoted_token = false;
                }
            }
            _ => current.push(c),
        }
    }
    if in_single || in_double {
        return None;
    }
    if !current.is_empty() || quoted_token {
        words.push(current);
    }
    // Fail closed on empty argv elements. `cargo test --test ""` cannot select
    // a real surface; inventing a broader match would be false credit.
    if words.iter().any(|word| word.is_empty()) {
        return None;
    }
    // Drop leading env assignments the same way the old splitter did, so
    // `FOO=1 cargo test` still resolves as `cargo test`.
    let mut command_seen = false;
    let filtered: Vec<String> = words
        .into_iter()
        .filter(|word| {
            if command_seen {
                return true;
            }
            // Shell-valid names only: must start with a letter or underscore.
            // `1=x cargo test` is not an env assignment and must not be stripped.
            let is_env_assignment = word.split_once('=').is_some_and(|(name, _)| {
                let mut chars = name.chars();
                matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
                    && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
            }) && !word.starts_with('-');
            if is_env_assignment {
                false
            } else {
                command_seen = true;
                true
            }
        })
        .collect();
    Some(filtered)
}

fn file_stem(path: &str) -> &str {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
}

/// Entry evidence for a fixed, exact entry symbol (e.g. a binary's `main`).
/// Substring matching would credit `main_helper` for a `main` requirement and
/// manufacture a false witness, so the file must define the exact symbol.
fn exact_entry(
    graph: &crate::callgraph::CallGraph,
    source: &'static str,
    file: &str,
    symbol: &str,
) -> Vec<EntryEvidence> {
    if graph.file_defines(file, symbol) {
        vec![EntryEvidence {
            source,
            file: file.into(),
            entry_symbol: Some(symbol.into()),
            s3_eligible: true,
        }]
    } else {
        Vec::new()
    }
}

fn entries_in_file(
    graph: &crate::callgraph::CallGraph,
    source: &'static str,
    file: &str,
    symbol_filter: Option<&str>,
) -> Vec<EntryEvidence> {
    // Only harness-executed test functions are entry candidates: `cargo test`
    // runs the `#[test]` functions (and anything they transitively call),
    // never an uncalled helper sitting in the same file. Emitting every
    // matching symbol would let a dead helper that happens to reach grounded
    // code look like an executed entry.
    let test_symbols = graph.file_test_symbols(file);
    // Cargo's filter selects harness test names, not every helper those tests
    // can reach. Matching a helper name would claim an entry the harness never
    // selected (and may run zero tests for).
    let narrowed: Vec<&str> = match symbol_filter.filter(|filter| !filter.is_empty()) {
        Some(filter) => test_symbols
            .iter()
            .map(String::as_str)
            .filter(|symbol| symbol.contains(filter))
            .collect(),
        None => test_symbols.iter().map(String::as_str).collect(),
    };
    if narrowed.is_empty() {
        return Vec::new();
    }
    narrowed
        .into_iter()
        .map(|symbol| EntryEvidence {
            source,
            file: file.into(),
            entry_symbol: Some(symbol.into()),
            s3_eligible: true,
        })
        .collect()
}

fn cargo_test_entries(
    words: &[String],
    graph: &crate::callgraph::CallGraph,
    source: &'static str,
) -> Vec<EntryEvidence> {
    if words.iter().any(|word| {
        word == "--no-run"
            || word == "--list"
            || word == "--doc"
            || word == "--manifest-path"
            || word.starts_with("--manifest-path=")
            || word == "--target"
            || word.starts_with("--target=")
            || word == "--skip"
            || word.starts_with("--skip=")
            || word == "--exact"
            || word == "--ignored"
            || word == "--include-ignored"
            || word == "-p"
            || word == "--package"
            || word.starts_with("--package=")
            || word == "--lib"
            || word == "--bins"
            || word == "--examples"
            || word == "--benches"
            || word == "--all-targets"
            || word == "--workspace"
            || word == "--all"
            || word == "--exclude"
            || word.starts_with("--exclude=")
            || word == "--bin"
            || word.starts_with("--bin=")
            || word == "--example"
            || word.starts_with("--example=")
            || word == "--bench"
            || word.starts_with("--bench=")
    }) || words
        .windows(2)
        .any(|pair| pair[0] == "--" && pair[1] == "--list")
    {
        return Vec::new();
    }
    let test_name = words
        .iter()
        .find_map(|word| {
            word.strip_prefix("--test=")
                .or_else(|| word.strip_prefix("--bench="))
        })
        .or_else(|| {
            words
                .windows(2)
                .find(|pair| pair[0] == "--test")
                .map(|pair| pair[1].as_str())
        });
    let mut positional = Vec::new();
    let mut index = 2;
    while index < words.len() {
        let word = words[index].as_str();
        if word == "--" {
            positional.extend(words[index + 1..].iter().map(String::as_str));
            break;
        }
        if let Some(value) = word
            .strip_prefix("--package=")
            .or_else(|| word.strip_prefix("--features="))
            .or_else(|| word.strip_prefix("--target="))
            .or_else(|| word.strip_prefix("--target-dir="))
            .or_else(|| word.strip_prefix("--manifest-path="))
            .or_else(|| word.strip_prefix("--profile="))
            .or_else(|| word.strip_prefix("--config="))
            .or_else(|| word.strip_prefix("--test="))
            .or_else(|| word.strip_prefix("--bin="))
            .or_else(|| word.strip_prefix("--example="))
            .or_else(|| word.strip_prefix("--bench="))
        {
            if value.is_empty() {
                return Vec::new();
            }
            index += 1;
            continue;
        }
        if let Some(value) = word.strip_prefix("--color=") {
            if !matches!(value, "auto" | "always" | "never") {
                return Vec::new();
            }
            index += 1;
            continue;
        }
        let takes_value = matches!(
            word,
            "-p" | "--package"
                | "-j"
                | "--jobs"
                | "--features"
                | "--target"
                | "--target-dir"
                | "--manifest-path"
                | "--profile"
                | "--color"
                | "--config"
                | "--test"
                | "--bin"
                | "--example"
                | "--bench"
        );
        if takes_value {
            let Some(value) = words.get(index + 1).map(String::as_str) else {
                return Vec::new();
            };
            if value.starts_with('-') || value.is_empty() {
                return Vec::new();
            }
            if word == "--color" && !matches!(value, "auto" | "always" | "never") {
                return Vec::new();
            }
            if matches!(word, "-j" | "--jobs") && !value.chars().all(|c| c.is_ascii_digit()) {
                return Vec::new();
            }
            index += 2;
            continue;
        }
        // Any other dash-prefixed option is not modeled. Ignoring it would
        // broaden the command into "all harness tests" and invent S3 credit.
        if word.starts_with('-') {
            return Vec::new();
        }
        positional.push(word);
        index += 1;
    }
    // Cargo accepts at most one free filter after options. Extra positionals
    // are not a modeled shape and must not silently use only the first.
    if positional.len() > 1 {
        return Vec::new();
    }
    let filter = positional.first().copied();
    // Without an explicit --test/--bin/--example/--bench target, cargo may run
    // any combination of unit/integration targets (and can disable autotests).
    // Guessing `tests/*.rs` would invent entries that never ran.
    let Some(name) = test_name else {
        return Vec::new();
    };
    // Default cargo integration targets are exactly `tests/<name>.rs` under
    // Cargo's auto-discovery. Custom `[[test]] path = ...`, disabled autotests,
    // and workspace-foreign targets are not modeled — without metadata we only
    // credit a single exact auto-discovered file when the name is a plain
    // identifier.
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Vec::new();
    }
    let stem = name.replace('-', "_");
    // Prefer the underscored form Cargo uses for hyphenated names; if both
    // spellings exist as distinct files, fail closed (ambiguous).
    let a = format!("tests/{stem}.rs");
    let b = format!("tests/{name}.rs");
    let mut hits = Vec::new();
    if graph.files().any(|f| f == a.as_str()) {
        hits.push(a.clone());
    }
    if a != b && graph.files().any(|f| f == b.as_str()) {
        hits.push(b);
    }
    if hits.len() != 1 {
        return Vec::new();
    }
    entries_in_file(graph, source, &hits[0], filter)
}

fn cargo_run_entries(
    words: &[String],
    graph: &crate::callgraph::CallGraph,
    source: &'static str,
) -> Vec<EntryEvidence> {
    // Strict known-option parse up to `--`. Unknown/malformed options fail
    // closed so `cargo run --bin svc --bogus` cannot credit svc::main.
    let mut binary: Option<String> = None;
    let mut index = 2;
    while index < words.len() {
        let word = words[index].as_str();
        if word == "--" {
            // Program argv after `--` is unmodeled; help,
            // subcommands, and filters can all change what executes.
            if index + 1 < words.len() {
                return Vec::new();
            }
            break;
        }
        if let Some(value) = word.strip_prefix("--bin=") {
            if value.is_empty() || binary.is_some() {
                return Vec::new();
            }
            // Keep the Cargo target name as written. Do not rewrite '-' to
            // '_' — that would credit `src/bin/svc_api.rs` for `--bin svc-api`.
            binary = Some(value.to_string());
            index += 1;
            continue;
        }
        if word == "--bin" {
            let Some(value) = words.get(index + 1).map(String::as_str) else {
                return Vec::new();
            };
            if value.starts_with('-') || value.is_empty() || binary.is_some() {
                return Vec::new();
            }
            binary = Some(value.to_string());
            index += 2;
            continue;
        }
        if let Some(value) = word.strip_prefix("--color=") {
            if !matches!(value, "auto" | "always" | "never") {
                return Vec::new();
            }
            index += 1;
            continue;
        }
        if word == "--color" {
            let Some(value) = words.get(index + 1).map(String::as_str) else {
                return Vec::new();
            };
            if !matches!(value, "auto" | "always" | "never") {
                return Vec::new();
            }
            index += 2;
            continue;
        }
        // Package/target/manifest selection is not modeled for binary mapping.
        if word == "-p"
            || word == "--package"
            || word.starts_with("--package=")
            || word == "--manifest-path"
            || word.starts_with("--manifest-path=")
            || word == "--target"
            || word.starts_with("--target=")
            || word.starts_with('-')
        {
            return Vec::new();
        }
        // Unexpected positional before `--` is not a known cargo-run shape.
        return Vec::new();
    }
    // Exactly one candidate file. Multiple workspace packages with the same
    // binary name would otherwise let one package's run credit another's main.
    let mut candidates: Vec<&str> = graph
        .files()
        .filter(|file| match &binary {
            Some(binary) => {
                (file.starts_with("src/bin/") || file.contains("/src/bin/"))
                    && file_stem(file) == binary
            }
            None => *file == "src/main.rs" || file.ends_with("/src/main.rs"),
        })
        .collect();
    candidates.sort();
    candidates.dedup();
    if candidates.len() != 1 {
        return Vec::new();
    }
    exact_entry(graph, source, candidates[0], "main")
}

/// Map supported command shapes to indexed entry symbols. Unknown commands
/// yield no evidence rather than guessing. The mapping is intentionally small
/// and deterministic; runtime trace/coverage is the future general solution.
pub fn command_entries(
    command: &str,
    graph: &crate::callgraph::CallGraph,
    source: &'static str,
) -> Vec<EntryEvidence> {
    command_entries_from(command, graph, source, CommandOrigin::Untrusted)
}

/// Where a command string came from determines whether bare `loom` is a
/// trustworthy name for this checkout's binary.
///
/// Generic validation commands may resolve `loom` through an arbitrary PATH,
/// so the public mapper remains fail-closed. Recorded journey steps are
/// different: the journey runner executes them through `subprocess`, which
/// binds both direct and compound bare `loom` invocations to `current_exe`.
/// Only `derived_entries` can confer that narrowly proven origin, after it has
/// matched the validation's exact outer journey runner and artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandOrigin {
    Untrusted,
    CheckoutBoundJourneyStep,
}

fn command_entries_from(
    command: &str,
    graph: &crate::callgraph::CallGraph,
    source: &'static str,
    origin: CommandOrigin,
) -> Vec<EntryEvidence> {
    if command.contains("||")
        || command.contains("&&")
        || command.contains('|')
        || command.contains(';')
    {
        return Vec::new();
    }
    let words = shell_words(command);
    let Some(first) = words.first().map(String::as_str) else {
        return Vec::new();
    };
    // Help/version never execute the claimed surface. Fail closed for every
    // derived command shape (cargo, loom, scripts, binaries).
    if argv_has_help_or_version(&words) {
        return Vec::new();
    }
    let mut out = Vec::new();
    if first == "cargo" && words.get(1).map(String::as_str) == Some("test") {
        out.extend(cargo_test_entries(&words, graph, source));
    } else if first == "cargo" && words.get(1).map(String::as_str) == Some("run") {
        out.extend(cargo_run_entries(&words, graph, source));
    } else {
        let _binary = first.rsplit('/').next().unwrap_or(first);
        // Only a checkout-bound loom binary. Bare `loom` normally resolves
        // through PATH and may be a different install; it is accepted only
        // when the caller proved this is a recorded journey step, whose runner
        // binds bare loom to current_exe. Absolute paths outside the checkout
        // remain untrusted. Accept `./loom` and exact target paths everywhere.
        // Exact checkout-bound binaries only. Lexical `target/**/loom` would
        // accept `target/../../tmp/loom` and credit an external binary.
        let is_checkout_loom = matches!(
            first,
            "./loom" | "target/debug/loom" | "target/release/loom"
        ) || (first == "loom"
            && origin == CommandOrigin::CheckoutBoundJourneyStep);
        if is_checkout_loom {
            // Parse the real typed CLI rather than approximating Clap's flag,
            // enum, positional, help, and nested-subcommand grammar. The
            // explicit route table below then names the exact dispatcher or
            // leaf handler entered by `commands::run`.
            if let Some(handler) = loom_cli_handler(&words) {
                out.extend(exact_entry(graph, source, handler.file, handler.symbol));
            }
        } else if first.contains('/')
            || first.ends_with(".py")
            || first.ends_with(".js")
            || first.ends_with(".ts")
            || first.ends_with(".rs")
            || matches!(first, "python" | "python3" | "node" | "bash" | "sh" | "zsh")
                && words.get(1).is_some_and(|arg| {
                    arg.ends_with(".py") || arg.ends_with(".js") || arg.ends_with(".ts")
                })
        {
            // Direct script paths map only when the file has a single obvious
            // entry symbol named exactly `main`/`run`/`handler`; a script with
            // many possible entry points cannot prove which one executes, and
            // substring look-alikes must not manufacture evidence. The file
            // itself must also be unambiguous: a bare `check.py` must not
            // credit an unrelated program the shell resolves from PATH, so only
            // a single registered file with that name (or an explicit
            // repo-relative path with a single canonical match) qualifies.
            // An interpreter prefix (`python3 script.py`) is consumed; the
            // script argument is the entry surface.
            let script = if matches!(first, "python" | "python3" | "node" | "bash" | "sh" | "zsh") {
                words.get(1).map(String::as_str).unwrap_or(first)
            } else {
                first
            };
            // Unmodeled trailing argv (including --help already handled) can
            // select a different path than bare script entry. Fail closed if
            // anything follows the script token.
            let script_index =
                if matches!(first, "python" | "python3" | "node" | "bash" | "sh" | "zsh") {
                    1
                } else {
                    0
                };
            if words.len() > script_index + 1 {
                return Vec::new();
            }
            let candidate = script.trim_start_matches("./");
            // Bare basenames resolve through PATH/cwd at runtime; only an
            // explicit repo-relative path can be matched against registered
            // files without inventing the wrong surface.
            if !candidate.contains('/') {
                return Vec::new();
            }
            // Exact registered path only. Suffix matching would credit
            // `pkg/tools/check.py` for command `tools/check.py`.
            let matches: Vec<&str> = graph.files().filter(|file| file == &candidate).collect();
            if matches.len() == 1 {
                let file = matches[0];
                for entry in ["main", "run", "handler"] {
                    if graph.file_defines(file, entry)
                        // A definition alone proves nothing: the script must
                        // actually invoke the entry at top level. Only a
                        // file-scope call in the script itself qualifies (e.g.
                        // `if __name__ == "__main__": main()` — its caller has
                        // no enclosing symbol). A call from another dead
                        // function is not execution and must fail closed.
                        && graph.edges().iter().any(|edge| {
                            edge.to_file == file
                                && edge.to_symbol == entry
                                && edge.from_file == file
                                && edge.from_symbol.is_empty()
                        })
                    {
                        out.push(EntryEvidence {
                            source,
                            file: file.into(),
                            entry_symbol: Some(entry.into()),
                            s3_eligible: true,
                        });
                        break;
                    }
                }
            }
        } else {
            // A bare command name resolves through PATH at runtime. Mapping it
            // to `src/bin/<name>.rs` invents a repo surface the shell may never
            // execute. Require `cargo run --bin` (or an explicit path script).
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LoomCliHandler {
    file: &'static str,
    symbol: &'static str,
}

fn cli_handler(file: &'static str, symbol: &'static str) -> Option<LoomCliHandler> {
    Some(LoomCliHandler { file, symbol })
}

/// Resolve a syntactically and semantically valid Loom argv to the exact entry
/// selected by `commands::run`. Unsupported command families return no credit;
/// extending this table requires pointing at a real, uniquely identified
/// dispatcher or leaf handler.
fn loom_cli_handler(words: &[String]) -> Option<LoomCliHandler> {
    let cli = crate::cli::Cli::try_parse_from(words).ok()?;
    let json = cli.json;
    match cli.command? {
        crate::cli::Command::Welcome => cli_handler("src/commands/orient_cmd.rs", "welcome"),
        crate::cli::Command::Sync { .. } => cli_handler("src/commands/status_cmd.rs", "sync_cmd"),
        crate::cli::Command::Status => cli_handler("src/commands/status_cmd.rs", "status"),
        crate::cli::Command::Next { mode, all, full } => match (mode, all, full) {
            // These are the same semantic branches and refusals as
            // `commands::run`; Clap alone cannot express the --full coupling.
            (Some(_), true, false) => cli_handler("src/commands/status_cmd.rs", "queue_list"),
            (None, true, false) => cli_handler("src/commands/status_cmd.rs", "next_all"),
            (None, true, true) if json => cli_handler("src/commands/status_cmd.rs", "next_all"),
            (_, false, false) => cli_handler("src/commands/status_cmd.rs", "next_cmd"),
            _ => None,
        },
        crate::cli::Command::Guide { .. } => cli_handler("src/commands/orient_cmd.rs", "guide"),
        crate::cli::Command::Find { .. } => cli_handler("src/commands/discover_cmd.rs", "find_cmd"),
        crate::cli::Command::Explain { .. } => {
            cli_handler("src/commands/discover_cmd.rs", "explain_cmd")
        }
        crate::cli::Command::Coverage => {
            cli_handler("src/commands/diagnostics_cmd.rs", "coverage_cmd")
        }
        crate::cli::Command::Impact { .. } => {
            cli_handler("src/commands/diagnostics_cmd.rs", "impact_cmd")
        }
        crate::cli::Command::Audit { cmd: None, .. } => {
            cli_handler("src/commands/diagnostics_cmd.rs", "audit_cmd")
        }
        crate::cli::Command::Deepen { .. } => {
            cli_handler("src/commands/diagnostics_cmd.rs", "deepen_cmd")
        }
        crate::cli::Command::Smells => cli_handler("src/commands/diagnostics_cmd.rs", "smells_cmd"),
        crate::cli::Command::Doctor => cli_handler("src/commands/diagnostics_cmd.rs", "doctor_cmd"),
        crate::cli::Command::Whoami => cli_handler("src/commands/diagnostics_cmd.rs", "whoami_cmd"),
        crate::cli::Command::Export { .. } => cli_handler("src/commands/status_cmd.rs", "export"),
        crate::cli::Command::Observe { .. } => {
            cli_handler("src/commands/proof_cmd.rs", "observe_cmd")
        }
        crate::cli::Command::Decide { .. } => {
            cli_handler("src/commands/capture_cmd.rs", "decide_cmd")
        }
        crate::cli::Command::Door { .. } => cli_handler("src/commands/capture_cmd.rs", "door"),
        crate::cli::Command::Codefile {
            cmd: crate::cli::CodefileCmd::List { .. },
        } => cli_handler("src/commands/codefile_cmd.rs", "dispatch"),
        crate::cli::Command::Inbox {
            cmd: crate::cli::InboxCmd::Mark { .. } | crate::cli::InboxCmd::Remove { .. },
        } => cli_handler("src/commands/capture_cmd.rs", "inbox"),
        _ => None,
    }
}

fn derived_entries(validation: &Node, graph: &crate::callgraph::CallGraph) -> Vec<EntryEvidence> {
    validation
        .body
        .get("command")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(|command| command_entries(command, graph, "validation_command"))
        .unwrap_or_default()
}

fn projection_current(status: InspectionStatus) -> bool {
    matches!(
        status,
        InspectionStatus::Uninspected | InspectionStatus::Passing
    )
}

/// A Journey proof is compiler-owned graph structure, never an authored spec
/// inferred from a path on the Validation. This deliberately duplicates the
/// readiness signature at the grading boundary so a raw Journey artifact, or a
/// hand-authored sibling Validation, cannot borrow compiled proof strength.
fn compiled_journey_proves_edge(
    store: &Store,
    validation: &Node,
) -> Result<Option<crate::model::Edge>> {
    if validation.body.get("type").and_then(|value| value.as_str()) != Some("journey")
        || validation
            .body
            .get("profile")
            .and_then(|value| value.as_str())
            != Some("proof")
        || !validation
            .body
            .get("compiler_version")
            .and_then(|value| value.as_str())
            .is_some_and(|version| !version.trim().is_empty())
    {
        return Ok(None);
    }

    let proves: Vec<_> = store
        .edges_with(Some(EdgeKind::Proves), Some(&validation.id), None)?
        .into_iter()
        .filter(|edge| projection_current(edge.status))
        .collect();
    let [proves] = proves.as_slice() else {
        return Ok(None);
    };
    let Some(journey) = store.get_node(&proves.to_id)? else {
        return Ok(None);
    };
    if journey.node_type != NodeType::Journey {
        return Ok(None);
    }
    let Some(journey_hash) = journey
        .body
        .get("semantic_hash")
        .and_then(|value| value.as_str())
    else {
        return Ok(None);
    };
    if validation
        .body
        .get("journey_hash")
        .and_then(|value| value.as_str())
        != Some(journey_hash)
    {
        return Ok(None);
    }
    let Some(surface_hash) = crate::journey::surface_projection_hash(store, &journey)? else {
        return Ok(None);
    };
    if validation
        .body
        .get("surface_hash")
        .and_then(|value| value.as_str())
        != Some(surface_hash.as_str())
    {
        return Ok(None);
    }

    let mut accepted_surfaces = std::collections::BTreeSet::new();
    for edge in store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)? {
        if projection_current(edge.status)
            && store
                .get_facet(&edge.id, TargetKind::Edge, "journey_hash")?
                .as_deref()
                == Some(journey_hash)
        {
            accepted_surfaces.insert(edge.to_id);
        }
    }
    let calls_current_surface = store
        .edges_with(Some(EdgeKind::Calls), Some(&validation.id), None)?
        .into_iter()
        .any(|edge| projection_current(edge.status) && accepted_surfaces.contains(&edge.to_id));
    if !calls_current_surface {
        return Ok(None);
    }
    let mut exercises_live_code = false;
    for edge in store.edges_with(Some(EdgeKind::Exercises), Some(&validation.id), None)? {
        if projection_current(edge.status)
            && store
                .get_node(&edge.to_id)?
                .is_some_and(|node| node.node_type == NodeType::CodeFile)
        {
            exercises_live_code = true;
            break;
        }
    }
    Ok(exercises_live_code.then(|| proves.clone()))
}

fn dedup_entries(entries: &mut Vec<EntryEvidence>) {
    entries.sort();
    entries.dedup();
}

/// Grade one validation. `intent_id` is the behavior it claims to prove.
pub fn grade(
    store: &Store,
    _root: &Path,
    validation: &Node,
    intent_id: &str,
    graph: &crate::callgraph::CallGraph,
) -> Result<StrengthWitness> {
    let mut w = StrengthWitness::default();
    let claims_journey =
        validation.body.get("type").and_then(|value| value.as_str()) == Some("journey");
    let compiled_proves = if claims_journey {
        compiled_journey_proves_edge(store, validation)?
    } else {
        None
    };
    if claims_journey && compiled_proves.is_none() {
        w.grade = Strength::S0.as_str().into();
        w.next = "compile this Journey from its current semantic hash and accepted surface; raw authored specs and incomplete proof topology do not grade".into();
        return Ok(w);
    }

    // S1 — loom ran it and it passed. Asked of the FACT, so a reported outcome
    // cannot reach even the bottom rung.
    let edge = store
        .edges_with(Some(EdgeKind::Validates), Some(&validation.id), None)?
        .into_iter()
        .find(|e| e.to_id == intent_id);
    w.ran_and_passed = validation.status == "passed"
        && match &edge {
            Some(e) => store.edge_verification(&e.id)? == crate::model::Verification::Verified,
            None => false,
        }
        && match &compiled_proves {
            Some(proves) => {
                store.edge_verification(&proves.id)? == crate::model::Verification::Verified
            }
            None => true,
        };
    if !w.ran_and_passed {
        w.grade = Strength::S0.as_str().into();
        w.next = "let loom run this proof (`loom validation run`) — a reported \
                  outcome does not grade"
            .into();
        return Ok(w);
    }

    // S2 — a test runner's own summary — "4 passed; 0 failed" — states WHAT was
    // checked, not merely that the process exited zero. That is strictly more
    // than S1 asks for, and refusing to count it told every repo with a real
    // test suite that its suite was liveness-only and it should write a thin
    // journey instead. Backwards: it pushed people away from the proofs they
    // already had.
    w.observed_assertions = reported_assertions(&edge, store)?;
    if w.observed_assertions.is_none() {
        w.grade = Strength::S1.as_str().into();
        w.next = "assert something about the OUTPUT, not just the exit code — \
                  run a proof whose observed runner output reports positive \
                  checked assertions and zero failures"
            .into();
        return Ok(w);
    }

    // S3 — validation-specific call witness. Explicit exercises edges and
    // journey/command-derived entry points can earn the rung. Intent-wide
    // verifies files are diagnostic-only legacy fallback.
    let mut entries = validation_entries(store, &validation.id)?;
    // Compiler-owned Journey proofs must use their Exercises edges. A generic
    // command Validation may still derive its own entry from the exact command.
    if compiled_proves.is_none() {
        entries.extend(derived_entries(validation, graph));
    }
    dedup_entries(&mut entries);
    if let Some(evidence) = call_witness(store, graph, intent_id, &entries)? {
        w.call_witness = evidence.grounded_symbol.clone();
        w.call_evidence = Some(evidence);
    } else if let Some(entry) = entries
        .iter()
        .find(|entry| entry.source == "anchor_navigation")
    {
        // Preserve the explicit diagnostic provenance while keeping it
        // visibly ineligible. Otherwise an operator sees only "nothing
        // reaches" and cannot tell that the locator was intentionally a
        // navigation-only anchor rather than a missing entry declaration.
        w.call_evidence = Some(CallEvidenceWitness {
            source: entry.source.into(),
            file: entry.file.clone(),
            entry_symbol: None,
            grounded_symbol: None,
            s3_eligible: false,
        });
    } else if entries.is_empty() {
        let mut fallback = intent_wide_entries(store, intent_id)?;
        dedup_entries(&mut fallback);
        w.call_evidence = match call_witness(store, graph, intent_id, &fallback)? {
            Some(evidence) => Some(evidence),
            None => fallback.first().map(|entry| CallEvidenceWitness {
                source: entry.source.into(),
                file: entry.file.clone(),
                entry_symbol: entry.entry_symbol.clone(),
                grounded_symbol: None,
                s3_eligible: false,
            }),
        };
    }
    if w.call_witness.is_none() {
        w.grade = Strength::S2.as_str().into();
        w.next = match &w.call_evidence {
            Some(evidence) if evidence.source == "intent_wide_fallback" => format!(
                "nothing this proof runs reaches the symbol the behavior is grounded in — legacy intent-wide evidence '{}' is visible but cannot earn S3; attach it to this validation with `loom edge exercises` or run it through the journey",
                evidence.file
            ),
            _ => "nothing this proof runs reaches the symbol the behavior is \
                  grounded in — exercise the real code path"
                .into(),
        };
        return Ok(w);
    }

    // The retired raw-spec runner/baseline API cannot honestly establish S4/S5.
    // Keep the wire grades for compatibility; the semantic compiler may restore
    // those rungs only with explicit compiled-proof evidence.
    w.grade = Strength::S3.as_str().into();
    w.next =
        "add stronger compiler-observed replay/boundary evidence when that proof API exists".into();
    Ok(w)
}

/// Persist one derived witness and record model-driven demotions in the
/// append-only journal. The facet remains deterministic; the journal entry is
/// emitted only on the transition, never on an unchanged sync.
pub fn store_witness(store: &Store, validation_id: &str, witness: &StrengthWitness) -> Result<()> {
    let previous = store
        .get_facet(validation_id, TargetKind::Node, "proof_strength")?
        .and_then(|raw| serde_json::from_str::<StrengthWitness>(&raw).ok());
    let migration = previous.as_ref().and_then(|previous| {
        let old = Strength::parse(&previous.grade).unwrap_or(Strength::S0);
        let new = Strength::parse(&witness.grade).unwrap_or(Strength::S0);
        (old > new
            && previous.witness_model == LEGACY_STRENGTH_WITNESS_MODEL
            && previous.witness_model != witness.witness_model)
            .then(|| {
                serde_json::json!({
                    "from": previous.grade,
                    "to": witness.grade,
                    "reason": "witness_model_change: intent-wide → validation-specific",
                    "previous_witness_model": previous.witness_model,
                    "witness_model": witness.witness_model,
                    "previous_call_witness": previous.call_witness,
                    "call_evidence": witness.call_evidence,
                })
            })
    });
    if let Some(payload) = migration {
        let model = payload["witness_model"].clone();
        let previous_model = payload["previous_witness_model"].clone();
        store.append_journal_once("proof_strength_changed", validation_id, payload, |entry| {
            entry.event == "proof_strength_changed"
                && entry.target_id == validation_id
                && entry.payload["witness_model"] == model
                && entry.payload["previous_witness_model"] == previous_model
        })?;
    }
    store.set_facet(
        validation_id,
        TargetKind::Node,
        "proof_strength",
        &serde_json::to_string(witness)?,
        crate::model::TruthClass::Derived,
    )?;
    Ok(())
}

/// Recompute every validation's grade. Called by sync; the result is a derived
/// facet, so INV-2 holds — wipe and re-sync reproduces it byte-identically.
pub fn recompute(store: &Store, root: &Path) -> Result<usize> {
    let graph = crate::callgraph::build(store)?;
    let mut graded = 0usize;
    for validation in store.list_nodes(Some(NodeType::Validation), usize::MAX)? {
        let mut best: Option<StrengthWitness> = None;
        for e in store.edges_with(Some(EdgeKind::Validates), Some(&validation.id), None)? {
            let w = grade(store, root, &validation, &e.to_id, &graph)?;
            let better = best
                .as_ref()
                .map(|b| Strength::parse(&w.grade) > Strength::parse(&b.grade))
                .unwrap_or(true);
            if better {
                best = Some(w);
            }
        }
        let witness = best.unwrap_or_else(|| StrengthWitness {
            grade: Strength::S0.as_str().into(),
            next: "this proof is not attached to any behavior".into(),
            ..Default::default()
        });
        store_witness(store, &validation.id, &witness)?;
        graded += 1;
    }
    Ok(graded)
}

/// One validation's grade, read back. `S0` when never computed.
pub fn of(store: &Store, validation_id: &str) -> Result<Strength> {
    let raw = store.get_facet(validation_id, TargetKind::Node, "proof_strength")?;
    Ok(raw
        .and_then(|j| serde_json::from_str::<StrengthWitness>(&j).ok())
        .and_then(|w| Strength::parse(&w.grade))
        .unwrap_or(Strength::S0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "loom-proofstrength-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn node(store: &Store, kind: NodeType, name: &str) -> Node {
        store
            .add_node(kind, name, "", "", serde_json::json!({}))
            .unwrap()
    }

    #[test]
    fn grades_are_ordered() {
        assert!(Strength::S1 < Strength::S2);
        assert!(Strength::S5 > Strength::MEANINGFUL);
        assert_eq!(Strength::parse("S3"), Some(Strength::S3));
        assert_eq!(Strength::parse("L5"), None);
    }

    /// Pin the hop budget that finding d3107a6d exposed as too shallow.
    /// The witness case is an exact caller at 6 hops; the constant must clear
    /// that, and stays aligned with `loom impact --depth 8`.
    #[test]
    fn call_witness_depth_clears_the_documented_six_hop_case() {
        const {
            assert!(
                CALL_WITNESS_DEPTH >= 6,
                "CALL_WITNESS_DEPTH would still miss the ring32→push_notes exact caller at 6 hops"
            );
        }
        assert_eq!(CALL_WITNESS_DEPTH, 8);
    }

    #[test]
    fn semicolon_locator_earns_a_witness_through_its_first_symbol() {
        let root = temp_root("multi-symbol");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("tests")).unwrap();
        std::fs::write(
            root.join("src/subjects.rs"),
            "pub fn get_subject_case() {}\npub fn list_subject_cases() {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("tests/recovery.rs"),
            "pub fn exercise_isolated_writer_rotation() { get_subject_case(); }\n",
        )
        .unwrap();

        let store = Store::init(&root, Some("multi-symbol witness"), false).unwrap();
        let intent = node(&store, NodeType::Intent, "release recovery path works");
        let implementation = node(&store, NodeType::CodeFile, "src/subjects.rs");
        let realizing = store
            .add_edge(
                EdgeKind::Implements,
                &intent.id,
                &implementation.id,
                crate::model::TruthClass::Asserted,
            )
            .unwrap();
        store
            .set_facet(
                &realizing.id,
                TargetKind::Edge,
                "locator",
                "get_subject_case; list_subject_cases",
                crate::model::TruthClass::Asserted,
            )
            .unwrap();

        let proof = node(&store, NodeType::CodeFile, "tests/recovery.rs");
        let validation = node(&store, NodeType::Validation, "release recovery proof");
        let exercises = store
            .add_edge(
                EdgeKind::Exercises,
                &validation.id,
                &proof.id,
                crate::model::TruthClass::Asserted,
            )
            .unwrap();
        store
            .set_facet(
                &exercises.id,
                TargetKind::Edge,
                "locator",
                "exercise_isolated_writer_rotation",
                crate::model::TruthClass::Asserted,
            )
            .unwrap();

        crate::sync::run(&store, &root).unwrap();
        let graph = crate::callgraph::build(&store).unwrap();
        let entries = validation_entries(&store, &validation.id).unwrap();
        let witness = call_witness(&store, &graph, &intent.id, &entries)
            .unwrap()
            .expect("validation-specific entry should reach the first grounded symbol");
        assert_eq!(witness.grounded_symbol.as_deref(), Some("get_subject_case"));
        assert_eq!(witness.source, "validation_grounding");
        assert!(witness.s3_eligible);

        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod runner_summary_tests {
    use super::parse_runner_summary;

    /// A runner's own summary states WHAT it checked. Refusing to read it told
    /// every repo with a real test suite that its suite was liveness-only.
    #[test]
    fn common_runners_are_understood() {
        assert!(
            parse_runner_summary("test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured")
                .unwrap()
                .contains('4')
        );
        assert!(parse_runner_summary("==== 12 passed in 0.42s ====")
            .unwrap()
            .contains("12"));
        assert!(parse_runner_summary("Tests:       7 passed, 7 total")
            .unwrap()
            .contains('7'));
    }

    /// Exiting zero having checked nothing is exactly the S1 case this
    /// distinguishes itself from, so it must not be mistaken for evidence.
    #[test]
    fn a_bare_success_is_not_an_assertion() {
        assert_eq!(parse_runner_summary(""), None);
        assert_eq!(parse_runner_summary("ok"), None);
        assert_eq!(parse_runner_summary("Done in 0.2s"), None);
        // Zero tests ran: nothing was checked.
        assert_eq!(
            parse_runner_summary("test result: ok. 0 passed; 0 failed"),
            None
        );
        // Something failed: the run is not evidence the behavior holds.
        assert_eq!(
            parse_runner_summary("test result: FAILED. 3 passed; 2 failed"),
            None
        );
    }
}
