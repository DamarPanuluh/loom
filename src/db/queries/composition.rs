//! Composition-proof coverage — the JOURNEY corner of the intent/code/saga
//! triangle, computed as a linear set operation (never the O(N^2) pair grid).
//!
//! The triangle: an Intent is realized in CODE (IMPLEMENTS), and a journey is
//! proven by RUNNING that code as an assembly. A leaf intent is fully proven by a
//! local/unit validation; a *participant* — code whose value is how it composes —
//! needs a proof that exercises its assembly, because "every piece passes its unit
//! test but they don't fit together" is the bug units miss.
//!
//! REPO-AGNOSTIC by construction. A "composition proof" is recognised from the
//! GRAPH's own topology, never from a test-runner command string (cargo `--test`,
//! pytest, jest, `go test` — each differs per language and per repo, and baking
//! any of them in is exactly the hardcoding that can't travel to a repo we didn't
//! anticipate). The three signals are universal:
//!   - DECLARED journey: `validation_type == "saga"` — loom's own journey
//!     primitive (`loom saga`), part of the data model, not a guessed repo string.
//!   - structural SPAN: the proof validates >= 2 intents — it exercises more than
//!     one responsibility, so it is proving their composition.
//!   - structural ASSEMBLY: the proof validates a NON-LEAF intent — a parent whose
//!     criterion IS the composed behaviour of its children.
//!
//! A proof of exactly one LEAF intent is a leaf proof. `loom paths` discloses the
//! per-signal breakdown so the inference is auditable.
//!
//! This module is ADDITIVE and READ-ONLY: it classifies each active intent's proof
//! tier (path-proven / leaf-only / unproven) so a driver can SEE what a journey
//! covers vs what only its pieces are proven. It never gates green.

use std::collections::{HashMap, HashSet};

use crate::db::queries::snapshot::QuerySnapshot;

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
    /// Disclosure — composition proofs by the SIGNAL that recognised them, so the
    /// graph-derived (not command-string) basis is visible and auditable.
    pub proofs_declared_journey: i64,
    pub proofs_multi_intent: i64,
    pub proofs_assembly: i64,
}

pub fn composition_coverage_from_snapshot(snapshot: &QuerySnapshot) -> CompositionCoverage {
    // Non-leaf (assembly) intents = any parent in the hierarchy tree.
    let non_leaf: HashSet<&str> = snapshot.hierarchy.iter().map(|(p, _)| p.as_str()).collect();

    // query_snapshot already filters deprecated intents, so snapshot.intents is
    // the active universe — confine VALIDATES projection to it.
    let active: HashSet<&str> = snapshot.intents.iter().map(|i| i.id.as_str()).collect();

    // Per-validation: which active intents it covers (from VALIDATES) — one pass.
    let mut covers: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in &snapshot.validates {
        if active.contains(e.intent_id.as_str()) {
            covers
                .entry(e.validation_id.as_str())
                .or_default()
                .push(e.intent_id.as_str());
        }
    }

    // Classify each PASSING validation as a composition proof (and how) or a leaf
    // proof, from the graph-derived signals only.
    let mut comp: HashSet<&str> = HashSet::new();
    let mut leaf: HashSet<&str> = HashSet::new();
    let mut cov = CompositionCoverage::default();
    for v in &snapshot.validations {
        if v.last_result != "passed" {
            continue;
        }
        let ints = covers.get(v.id.as_str()).map(Vec::as_slice).unwrap_or(&[]);
        if v.validation_type == "saga" {
            comp.insert(v.id.as_str());
            cov.proofs_declared_journey += 1;
        } else if ints.len() >= 2 {
            comp.insert(v.id.as_str());
            cov.proofs_multi_intent += 1;
        } else if ints.iter().any(|i| non_leaf.contains(i)) {
            comp.insert(v.id.as_str());
            cov.proofs_assembly += 1;
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
    use crate::types::{Intent, ValidatesEdge, Validation};

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

    fn val(id: &str, vtype: &str, result: &str) -> Validation {
        Validation {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            // command is deliberately IRRELEVANT now — the classifier never reads it.
            validation_type: vtype.into(),
            command: "anything at all".into(),
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
        hierarchy: Vec<(String, String)>,
        validates: Vec<ValidatesEdge>,
        vals: Vec<Validation>,
    ) -> QuerySnapshot {
        QuerySnapshot::from_parts(
            intents,
            hierarchy,
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
    fn recognises_composition_proofs_from_graph_topology_not_command_strings() {
        // par : non-leaf parent, proven by a single-intent TEST -> ASSEMBLY signal.
        // leaf: child of par, no proof -> unproven.
        // x   : proven by a declared SAGA -> DECLARED-journey signal.
        // y1,y2: proven by ONE validation covering both -> SPAN signal (both path-proven).
        // u   : proven by a single-leaf-intent test -> leaf-only.
        // none: no proof -> unproven.
        let intents = vec![
            intent("par"),
            intent("leaf"),
            intent("x"),
            intent("y1"),
            intent("y2"),
            intent("u"),
            intent("none"),
        ];
        let hierarchy = vec![("par".to_string(), "leaf".to_string())];
        let vals = vec![
            val("v_saga", "saga", "passed"),
            val("v_par", "test", "passed"),
            val("v_multi", "test", "passed"),
            val("v_unit", "test", "passed"),
        ];
        let validates = vec![
            vedge("v_saga", "x"),
            vedge("v_par", "par"),
            vedge("v_multi", "y1"),
            vedge("v_multi", "y2"),
            vedge("v_unit", "u"),
        ];
        let cov = composition_coverage_from_snapshot(&snap(intents, hierarchy, validates, vals));
        assert_eq!(cov.total, 7);
        assert_eq!(
            cov.path_proven, 4,
            "x (saga) + par (assembly) + y1,y2 (span) are path-proven"
        );
        assert_eq!(
            cov.leaf_only, 1,
            "u is proven only by a single-leaf unit test"
        );
        assert_eq!(cov.unproven, 2, "leaf + none have no passing proof");
        // disclosure: each signal recognised exactly one proof.
        assert_eq!(cov.proofs_declared_journey, 1);
        assert_eq!(cov.proofs_multi_intent, 1);
        assert_eq!(cov.proofs_assembly, 1);
        assert!(cov.leaf_only_intents.iter().any(|(id, _)| id == "u"));
        assert!(cov.unproven_intents.iter().any(|(id, _)| id == "none"));
    }

    #[test]
    fn a_failed_composition_proof_does_not_path_prove() {
        let intents = vec![intent("a")];
        let vals = vec![val("v", "saga", "failed")];
        let cov =
            composition_coverage_from_snapshot(&snap(intents, vec![], vec![vedge("v", "a")], vals));
        assert_eq!(cov.path_proven, 0);
        assert_eq!(cov.unproven, 1);
    }
}
