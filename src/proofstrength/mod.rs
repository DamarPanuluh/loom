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

mod command;
mod entries;
mod runner_summary;

pub use command::command_entries;
pub use entries::{EntryEvidence, CALL_WITNESS_DEPTH};
pub use runner_summary::parse_runner_summary;

use crate::model::{EdgeKind, InspectionStatus, Node, NodeType, TargetKind};
use crate::store::Store;
use crate::Result;
use serde::{Deserialize, Serialize};

use std::path::Path;

use entries::{
    call_witness, compiled_journey_proves_edge, dedup_entries, derived_entries,
    intent_wide_entries, journey_owned_entries, journey_s2_next, validation_entries,
};
use runner_summary::reported_assertions;

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
    /// `validation_grounding`, `journey_command`, `validation_command`,
    /// `journey_operation_exercise`, or the legacy diagnostic-only
    /// `intent_wide_fallback`.
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
    /// Journey operation that declared a downstream exercise entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    /// Stable exercise id within that operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exercise_id: Option<String>,
    /// Output assertion that observed the boundary crossing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_by: Option<String>,
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
    w.observed_assertions = reported_assertions(&edge, store, !claims_journey)?;
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
    //
    // Compiler-owned Journey proofs grade strictly against the canonical
    // expected-exercise projection: only exact agreement between the compiled
    // topology/facets and the accepted surface yields S3-eligible entries.
    // A projection the accepted surface can no longer satisfy (stale
    // acceptance, missing live code, corrupt bindings) fails closed at S2.
    let journey_projection: Option<crate::journey_exercises::ExpectedExerciseProjection> =
        if compiled_proves.is_some() {
            match crate::journey_exercises::expected_projection_for_validation(store, validation) {
                Ok(projection) => projection,
                Err(error) => {
                    w.grade = Strength::S2.as_str().into();
                    w.next = format!(
                        "compiled Journey is S2: the accepted surface no longer yields a valid operation-exercise projection: {error:#}. Update the authored surface manifest, then run `loom journey surface-accept`, `loom journey compile`, and `loom journey run`."
                    );
                    return Ok(w);
                }
            }
        } else {
            None
        };
    if compiled_proves.is_some() && journey_projection.is_none() {
        // Unreachable while `compiled_journey_proves_edge` and this
        // projection share their checks, but never let a compiler-owned
        // Journey fall through to the generic locator-facet path: that would
        // reopen exactly the public-entry fallback this gate closes.
        w.grade = Strength::S2.as_str().into();
        w.next = "compiled Journey is S2: no current accepted-surface operation-exercise projection could be derived; re-accept the surface and recompile.".into();
        return Ok(w);
    }
    let mut entries;
    let mut provenance_problems: Option<String> = None;
    if let Some(projection) = &journey_projection {
        let (owned, problems) = journey_owned_entries(store, &validation.id, projection)?;
        provenance_problems = problems;
        entries = owned;
    } else {
        let mut legacy = validation_entries(store, &validation.id)?;
        // Compiler-owned Journey proofs must use their Exercises edges. A
        // generic command Validation may also derive its own entry from the
        // exact command.
        legacy.extend(derived_entries(validation, graph));
        entries = legacy;
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
        w.call_evidence = Some(entry.clone().into_call_evidence(None));
    } else if let Some(entry) = entries
        .iter()
        .find(|entry| entry.source == "journey_provenance_mismatch")
    {
        // Broken compiler provenance stays visible for diagnosis, but can
        // never witness: the grade below refuses S3 regardless.
        w.call_evidence = Some(entry.clone().into_call_evidence(None));
    } else if entries.is_empty() {
        let mut fallback = intent_wide_entries(store, intent_id)?;
        dedup_entries(&mut fallback);
        w.call_evidence = match call_witness(store, graph, intent_id, &fallback)? {
            Some(evidence) => Some(evidence),
            None => fallback
                .first()
                .cloned()
                .map(|entry| entry.into_call_evidence(None)),
        };
    }
    if w.call_witness.is_none() {
        w.grade = Strength::S2.as_str().into();
        w.next = if compiled_proves.is_some() {
            journey_s2_next(&entries, provenance_problems.as_deref())
        } else {
            match &w.call_evidence {
                Some(evidence) if evidence.source == "intent_wide_fallback" => format!(
                    "nothing this proof runs reaches the symbol the behavior is grounded in — legacy intent-wide evidence '{}' is visible but cannot earn S3; attach it to this validation with `loom edge exercises` or run it through the journey",
                    evidence.file
                ),
                _ => "nothing this proof runs reaches the symbol the behavior is \
                      grounded in — exercise the real code path"
                    .into(),
            }
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

        let store = Store::init_with_identity(
            &root,
            Some("multi-symbol witness"),
            false,
            crate::identity::ExecutionIdentity::solo(),
        )
        .unwrap();
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
