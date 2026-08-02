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

use crate::journey::{Expect, JourneySpec};
use crate::model::{EdgeKind, Node, NodeType, TargetKind};
use crate::store::Store;
use crate::Result;
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
    /// reaches the code the behavior is grounded in. This is what the journey
    /// axis, journey coverage, and the shallow-proof smell hold out for —
    /// everywhere the old code read `proof_level in {L5, L6}`.
    pub const END_TO_END: Strength = Strength::S3;
}

/// Every conjunct, recorded. The point is that a grade can be argued with.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StrengthWitness {
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
    pub call_witness: Option<String>,
    pub baseline_clean: bool,
    /// What boundary it crosses, if any.
    pub boundary: Option<String>,
    /// Why it stopped where it did — the next conjunct to go after.
    pub next: String,
}

/// Count the assertions in an `Expect` that say something about CONTENT.
///
/// `status` and `exit_code` are excluded on purpose. A CLI step has an exit
/// code whether or not its author thought about one (it defaults to 0), so
/// counting it would grade every proof that runs at all as though it checked
/// something.
pub fn content_assertions(expect: &Expect) -> usize {
    expect.body.len()
        + expect.exists.len()
        + expect.stdout_contains.len()
        + expect.stderr_contains.len()
}

/// What the runner said it checked, read out of the output loom observed.
///
/// Only a summary that names a POSITIVE number of passing checks AND an
/// explicit zero failures counts. "ok" alone does not: a command can exit zero
/// having run nothing, which is exactly the S1 case this is distinguishing
/// itself from.
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
        if let Some(summary) = parse_runner_summary(&run.stdout_excerpt) {
            return Ok(Some(summary));
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

/// Does this spec cross a boundary loom does not itself control?
pub fn boundary(spec: &JourneySpec) -> Option<String> {
    for step in &spec.steps {
        if !step.is_cli() && !step.request.url.trim().is_empty() {
            return Some(format!("http {}", step.request.url));
        }
        if step.is_cli() {
            if let Some(bin) = crossed_binary(&step.run) {
                return Some(format!("process {bin}"));
            }
        }
    }
    None
}

/// Shell plumbing that carries data INTO a command without being a system the
/// proof crosses into. Piping `printf` at loom's stdin is loom talking to
/// itself with extra steps.
const PLUMBING: &[&str] = &[
    "printf", "echo", "cat", "true", "false", "sh", "bash", "env", "sleep", "yes", "head", "tail",
    "tee", "sed", "awk", "grep", "sort", "jq", "xargs", "test", "[",
];

/// A binary in this command that represents a real boundary, if any.
///
/// Every segment of the pipeline is considered, not just the first token: the
/// first token of `printf … | loom mcp serve` is `printf`, which used to credit
/// an S5 boundary to a step whose only real actor is loom. A grade that counts
/// shell plumbing as "crossing into a system loom does not control" overstates
/// exactly the conjunct that is hardest to earn honestly.
fn crossed_binary(run: &str) -> Option<String> {
    run.split(['|', ';', '&'])
        .filter_map(|segment| {
            let first = segment.split_whitespace().next()?;
            let bin = first.rsplit('/').next().unwrap_or(first);
            let bin = bin.trim();
            (!bin.is_empty()
                && bin != "loom"
                && bin != "cargo"
                && !PLUMBING.contains(&bin)
                && !bin.contains('='))
            .then(|| bin.to_string())
        })
        .next()
}

/// The symbols an intent is grounded in, via its realizing locators.
fn grounded_symbols(store: &Store, intent_id: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Implements), Some(intent_id), None)? {
        if store.edge_superseded(&e.id)? {
            continue;
        }
        if let Some(loc) = store.get_facet(&e.id, TargetKind::Edge, "locator")? {
            if let Some(tok) = loc.split_whitespace().next_back() {
                let tok = tok.split(':').next().unwrap_or(tok);
                let tok = tok.rsplit("::").next().unwrap_or(tok);
                if !tok.is_empty() {
                    out.push(tok.to_string());
                }
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// How far [`call_witness`] walks the call graph.
///
/// Cap of 4 hid exact callers at 6 hops (finding `d3107a6d`: ring32 research
/// tests → `push_notes`). `loom impact <sym> --depth 8` already contradicted
/// the S2 "nothing this proof runs reaches the symbol" grade. Eight matches
/// that diagnostic depth and clears the documented 6-hop case with headroom
/// for a layer or two of helpers.
pub const CALL_WITNESS_DEPTH: usize = 8;

/// Does anything this proof reaches call into a symbol the intent is grounded
/// in? Answered from the real call graph, not from token overlap.
///
/// The proof's own entry points are the symbols defined in the files it is
/// grounded in — for a test file, its test functions. `impact` walks callers
/// backwards, so the question is asked in the direction the graph answers
/// cheaply: does the grounded symbol have, among its transitive callers, a
/// symbol living in a file this proof covers?
fn call_witness(
    store: &Store,
    graph: &crate::callgraph::CallGraph,
    intent_id: &str,
    proof_files: &[String],
) -> Result<Option<String>> {
    if proof_files.is_empty() {
        return Ok(None);
    }
    for symbol in grounded_symbols(store, intent_id)? {
        let reach = graph.impact(&symbol, CALL_WITNESS_DEPTH);
        if reach.callers.iter().any(|c| proof_files.contains(&c.file)) {
            return Ok(Some(symbol));
        }
    }
    Ok(None)
}

/// The files that VERIFY this behavior — the intent's groundings carrying the
/// `verifies` role. That is how a test file attaches to a behavior in the
/// graph; a Validation node is not itself grounded (an `implements` edge must
/// start at an intent), so the proof's reach is read from the intent's own
/// verifying surface.
fn proof_files(store: &Store, intent_id: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Implements), Some(intent_id), None)? {
        if store.edge_superseded(&e.id)? {
            continue;
        }
        if store.grounding_role(&e.id)? != crate::model::GroundingRole::Verifies {
            continue;
        }
        if let Some(cf) = store.get_node(&e.to_id)? {
            out.push(cf.name);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Grade one validation. `intent_id` is the behavior it claims to prove.
pub fn grade(
    store: &Store,
    root: &Path,
    validation: &Node,
    intent_id: &str,
    graph: &crate::callgraph::CallGraph,
) -> Result<StrengthWitness> {
    let mut w = StrengthWitness::default();

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
        };
    if !w.ran_and_passed {
        w.grade = Strength::S0.as_str().into();
        w.next = "let loom run this proof (`loom validation run`) — a reported \
                  outcome does not grade"
            .into();
        return Ok(w);
    }

    // S2 — content assertions. A journey spec carries them per step; a plain
    // command validation has none loom can read, so it stops at S1 until it
    // becomes a journey.
    let spec = journey_spec(root, validation);
    if let Some(spec) = &spec {
        w.content_assertions = spec
            .steps
            .iter()
            .map(|s| content_assertions(&s.expect))
            .sum();
        w.boundary = boundary(spec);
    }
    // A test runner's own summary — "4 passed; 0 failed" — states WHAT was
    // checked, not merely that the process exited zero. That is strictly more
    // than S1 asks for, and refusing to count it told every repo with a real
    // test suite that its suite was liveness-only and it should write a thin
    // journey instead. Backwards: it pushed people away from the proofs they
    // already had.
    if w.content_assertions == 0 {
        w.observed_assertions = reported_assertions(&edge, store)?;
    }
    if w.content_assertions == 0 && w.observed_assertions.is_none() {
        w.grade = Strength::S1.as_str().into();
        w.next = "assert something about the OUTPUT, not just the exit code — \
                  a spec with `stdout_contains`/`body`/`exists`, or a command \
                  whose runner reports what it checked"
            .into();
        return Ok(w);
    }

    // S3 — call witness.
    w.call_witness = call_witness(store, graph, intent_id, &proof_files(store, intent_id)?)?;
    if w.call_witness.is_none() {
        w.grade = Strength::S2.as_str().into();
        w.next = "nothing this proof runs reaches the symbol the behavior is \
                  grounded in — exercise the real code path"
            .into();
        return Ok(w);
    }

    // S4 — a baseline that replayed clean.
    let journey = validation
        .body
        .get("journey")
        .or_else(|| validation.body.get("journey_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    w.baseline_clean = !journey.is_empty()
        && crate::journey::read_baseline(root, journey)?.is_some()
        && validation
            .body
            .get("baseline_deviations")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            == 0;
    if !w.baseline_clean {
        w.grade = Strength::S3.as_str().into();
        w.next = "freeze a baseline (`loom journey freeze`) and replay it — a \
                  baseline nobody replays is a fossil"
            .into();
        return Ok(w);
    }

    // S5 — a boundary loom does not control.
    match &w.boundary {
        Some(_) => w.grade = Strength::S5.as_str().into(),
        None => {
            w.grade = Strength::S4.as_str().into();
            w.next = "everything this proof touches is loom calling loom — cross \
                      a real boundary (an HTTP step, or another binary)"
                .into();
        }
    }
    Ok(w)
}

/// The journey spec behind a validation, if it is a journey proof.
fn journey_spec(root: &Path, validation: &Node) -> Option<JourneySpec> {
    // `artifact` is the path `loom journey add` recorded — the actual file.
    // Prefer it over guessing a filename from the journey NAME, which is prose
    // ("loom serves its capabilities in band") and almost never the basename
    // ("mcp-in-band.yaml"). Guessing meant every well-named journey silently
    // graded S1 with "content assertions: 0" no matter how much it asserted:
    // the spec was never found, so its assertions were never counted.
    if let Some(artifact) = validation.body.get("artifact").and_then(|v| v.as_str()) {
        if let Ok(spec) = crate::journey::parse(&root.join(artifact)) {
            return Some(spec);
        }
    }
    // Fall back to the id-as-basename form, which is what a hand-registered
    // `--journey-id` usually means.
    let journey = validation
        .body
        .get("journey")
        .or_else(|| validation.body.get("journey_id"))
        .and_then(|v| v.as_str())?;
    let path = root.join("journeys").join(format!("{journey}.yaml"));
    crate::journey::parse(&path).ok()
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
        store.set_facet(
            &validation.id,
            TargetKind::Node,
            "proof_strength",
            &serde_json::to_string(&witness)?,
            crate::model::TruthClass::Derived,
        )?;
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

    /// The exact regression that motivated this module: a step asserting only
    /// its exit code establishes liveness and nothing else.
    #[test]
    fn exit_code_is_not_a_content_assertion() {
        let mut expect = Expect {
            exit_code: Some(0),
            status: Some(200),
            ..Default::default()
        };
        assert_eq!(content_assertions(&expect), 0);
        expect.stdout_contains.push("files_scanned".into());
        assert_eq!(content_assertions(&expect), 1);
    }

    /// Shell plumbing is not a boundary. This is the conjunct hardest to earn
    /// honestly, so it is the one most worth refusing to inflate.
    #[test]
    fn piping_into_loom_is_not_crossing_a_boundary() {
        assert_eq!(crossed_binary("printf '{}' | loom mcp serve"), None);
        assert_eq!(crossed_binary("loom sync"), None);
        assert_eq!(crossed_binary("echo hi | sh -c 'loom status'"), None);
        // A real other system still counts, wherever it sits in the pipeline.
        assert_eq!(
            crossed_binary("printf x | psql -c 'select 1'").as_deref(),
            Some("psql")
        );
        assert_eq!(
            crossed_binary("curl -s http://localhost:8080/health").as_deref(),
            Some("curl")
        );
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
