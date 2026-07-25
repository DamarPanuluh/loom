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
    /// How many non-exit-code assertions the proof makes.
    pub content_assertions: usize,
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
    expect.body.len() + expect.exists.len() + expect.stdout_contains.len()
        + expect.stderr_contains.len()
}

/// Does this spec cross a boundary loom does not itself control?
pub fn boundary(spec: &JourneySpec) -> Option<String> {
    for step in &spec.steps {
        if !step.is_cli() && !step.request.url.trim().is_empty() {
            return Some(format!("http {}", step.request.url));
        }
        if step.is_cli() {
            let first = step.run.split_whitespace().next().unwrap_or("");
            let bin = first.rsplit('/').next().unwrap_or(first);
            if !bin.is_empty() && bin != "loom" && bin != "cargo" {
                return Some(format!("process {bin}"));
            }
        }
    }
    None
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
        let reach = graph.impact(&symbol, 4);
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
        w.content_assertions = spec.steps.iter().map(|s| content_assertions(&s.expect)).sum();
        w.boundary = boundary(spec);
    }
    if w.content_assertions == 0 {
        w.grade = Strength::S1.as_str().into();
        w.next = "assert something about the OUTPUT, not just the exit code — \
                  `stdout_contains`, `body`, or `exists`"
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
    // `journey` is what `loom journey add` records; `journey_id` is what
    // `loom validation add --journey-id` records. Both name the same spec.
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

    #[test]
    fn grades_are_ordered() {
        assert!(Strength::S1 < Strength::S2);
        assert!(Strength::S5 > Strength::MEANINGFUL);
        assert_eq!(Strength::parse("S3"), Some(Strength::S3));
        assert_eq!(Strength::parse("L5"), None);
    }
}
