//! Composition-proof coverage — the JOURNEY corner of the intent/code/saga
//! triangle, computed as a linear set operation (never the O(N^2) pair grid).
//!
//! The triangle: an Intent is realized in CODE (IMPLEMENTS), and a journey
//! (saga / integration test) is proven by RUNNING that code end-to-end. A leaf
//! intent is fully proven by a local/unit validation; a *participant* — code
//! whose value is how it composes — needs a journey that exercises its assembly,
//! because "every piece passes its unit test but they don't fit together" is the
//! bug units miss.
//!
//! This module is ADDITIVE and READ-ONLY: it classifies each active intent's
//! proof tier (path-proven / leaf-only / unproven) so a driver can SEE what a
//! journey covers vs what only its pieces are proven. It never gates green — the
//! existing horizontal grid still owns the done-condition until this layer is
//! validated to cover as well or better.

use std::collections::HashSet;

use crate::db::queries::snapshot::QuerySnapshot;
use crate::types::Validation;

/// A PASSING validation is a COMPOSITION proof when it exercises the assembled
/// system end-to-end — a `saga` (consumer journey) or an integration test that
/// runs the real binary — versus a LEAF proof (a unit test of one symbol). loom
/// has no symbol-level "this test ran this code" facts, so the tier is INFERRED
/// from the proof's transport; callers surface the command so the inference is
/// auditable, not hidden. Conservative on purpose: an unrecognised `cargo test`
/// reads as leaf, so this never over-claims a journey.
pub fn is_composition_proof(v: &Validation) -> bool {
    let c = v.command.as_str();
    v.validation_type == "saga"
        // `cargo test --test <target>` runs an integration test in tests/ (a real
        // binary invocation), not a `--bin` unit test.
        || c.contains("--test ")
        // loom's own integration suites run the assembled CLI as a subprocess.
        || c.contains("sqlite_regression")
        || c.contains("cold_saga")
}

/// Proof-tier coverage over the composition (journey) corner. Linear in
/// validations + validates edges + intents — no pair enumeration.
#[derive(Debug, Default, Clone)]
pub struct CompositionCoverage {
    pub total: i64,
    /// Reached by a PASSING composition proof — its assembly is actually run.
    pub path_proven: i64,
    /// Only PASSING leaf proofs — pieces proven, no journey covers it. The
    /// "judge: is a real journey missing here, or is this a genuine leaf?" surface.
    pub leaf_only: i64,
    /// No passing proof at all.
    pub unproven: i64,
    pub leaf_only_intents: Vec<(String, String)>,
    pub unproven_intents: Vec<(String, String)>,
}

pub fn composition_coverage_from_snapshot(snapshot: &QuerySnapshot) -> CompositionCoverage {
    // Passing validations, split by tier (one pass).
    let mut comp: HashSet<&str> = HashSet::new();
    let mut leaf: HashSet<&str> = HashSet::new();
    for v in &snapshot.validations {
        if v.last_result != "passed" {
            continue;
        }
        if is_composition_proof(v) {
            comp.insert(v.id.as_str());
        } else {
            leaf.insert(v.id.as_str());
        }
    }
    // Project proof tiers onto intents via VALIDATES (set membership — linear).
    let mut intent_comp: HashSet<&str> = HashSet::new();
    let mut intent_leaf: HashSet<&str> = HashSet::new();
    for e in &snapshot.validates {
        if comp.contains(e.validation_id.as_str()) {
            intent_comp.insert(e.intent_id.as_str());
        } else if leaf.contains(e.validation_id.as_str()) {
            intent_leaf.insert(e.intent_id.as_str());
        }
    }
    // query_snapshot already filters deprecated intents, so snapshot.intents is
    // the active universe.
    let mut cov = CompositionCoverage::default();
    for i in &snapshot.intents {
        cov.total += 1;
        let id = i.id.as_str();
        if intent_comp.contains(id) {
            cov.path_proven += 1;
        } else if intent_leaf.contains(id) {
            cov.leaf_only += 1;
            cov.leaf_only_intents.push((i.id.clone(), i.name.clone()));
        } else {
            cov.unproven += 1;
            cov.unproven_intents.push((i.id.clone(), i.name.clone()));
        }
    }
    cov
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Intent, ValidatesEdge};

    fn intent(id: &str) -> Intent {
        Intent {
            id: id.into(),
            name: format!("intent {id}"),
            description: String::new(),
            criterion: String::new(),
            abstraction_level: "feature".into(),
            domain: String::new(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "confirmed".into(),
            aspect: String::new(),
            tags: Vec::new(),
            visibility: String::new(),
            boundary: String::new(),
            lifecycle: "implemented".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }

    fn val(id: &str, vtype: &str, command: &str, result: &str) -> Validation {
        Validation {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            validation_type: vtype.into(),
            command: command.into(),
            last_run: String::new(),
            last_result: result.into(),
            last_executed_run: String::new(),
            discrimination_status: String::new(),
        }
    }

    fn vedge(vid: &str, iid: &str) -> ValidatesEdge {
        ValidatesEdge {
            id: format!("ve:{vid}:{iid}"),
            validation_id: vid.into(),
            intent_id: iid.into(),
            validation_name: vid.into(),
            intent_name: iid.into(),
            created_at: String::new(),
            inspection_status: "passing".into(),
            notes: String::new(),
        }
    }

    fn snap(
        intents: Vec<Intent>,
        validates: Vec<ValidatesEdge>,
        vals: Vec<Validation>,
    ) -> QuerySnapshot {
        QuerySnapshot::from_parts(
            intents,
            vec![],
            vec![],
            vec![],
            vec![],
            validates,
            vals,
            vec![],
            vec![],
            None,
        )
    }

    #[test]
    fn classifies_each_intent_into_its_proof_tier() {
        // A: covered by a passing SAGA -> path-proven.
        // B: covered by a passing unit test -> leaf-only.
        // C: covered by a passing INTEGRATION test (--test) -> path-proven.
        // D: covered only by a FAILED composition proof -> unproven (not passing).
        // E: no validation -> unproven.
        let intents = vec![
            intent("A"),
            intent("B"),
            intent("C"),
            intent("D"),
            intent("E"),
        ];
        let vals = vec![
            val("v_saga", "saga", "loom saga run checkout", "passed"),
            val(
                "v_unit",
                "test",
                "cargo test --bin loom scoring::tests::x",
                "passed",
            ),
            val(
                "v_int",
                "test",
                "cargo test --test sqlite_regression some_e2e",
                "passed",
            ),
            val("v_fail", "saga", "loom saga run broken", "failed"),
        ];
        let validates = vec![
            vedge("v_saga", "A"),
            vedge("v_unit", "B"),
            vedge("v_int", "C"),
            vedge("v_fail", "D"),
        ];
        let cov = composition_coverage_from_snapshot(&snap(intents, validates, vals));
        assert_eq!(cov.total, 5);
        assert_eq!(
            cov.path_proven, 2,
            "A (saga) + C (integration) are path-proven"
        );
        assert_eq!(cov.leaf_only, 1, "B is proven only by a unit test");
        assert_eq!(cov.unproven, 2, "D's only proof FAILED; E has none");
        assert!(cov.leaf_only_intents.iter().any(|(id, _)| id == "B"));
        assert!(cov.unproven_intents.iter().any(|(id, _)| id == "D"));
        assert!(cov.unproven_intents.iter().any(|(id, _)| id == "E"));
    }

    #[test]
    fn a_passing_composition_proof_outranks_a_leaf_proof_on_the_same_intent() {
        // An intent with BOTH a unit test and an integration test reads path-proven.
        let intents = vec![intent("A")];
        let vals = vec![
            val("u", "test", "cargo test --bin loom foo", "passed"),
            val(
                "i",
                "test",
                "cargo test --test sqlite_regression foo_e2e",
                "passed",
            ),
        ];
        let cov = composition_coverage_from_snapshot(&snap(
            intents,
            vec![vedge("u", "A"), vedge("i", "A")],
            vals,
        ));
        assert_eq!(cov.path_proven, 1);
        assert_eq!(cov.leaf_only, 0);
    }
}
