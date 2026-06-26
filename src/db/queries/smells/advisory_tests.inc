#[cfg(test)]
mod shotgun_surgery_tests {
    use super::*;
    use crate::types::{CodeFile, Implements, Intent, RelatesTo};

    fn intent(id: &str, name: &str) -> Intent {
        Intent {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            criterion: String::new(),
            abstraction_level: "feature".into(),
            domain: String::new(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "confirmed".into(),
            aspect: String::new(),
            tags: Vec::new(),
            visibility: "internal".into(),
            boundary: String::new(),
            lifecycle: "implemented".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }

    fn cf(path: &str) -> CodeFile {
        CodeFile {
            id: path.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: String::new(),
            imports: Vec::new(),
            symbols: Vec::new(),
            symbol_facts: Vec::new(),
            content_hash: String::new(),
            extractor_grade: String::new(),
        }
    }

    fn imp(intent_id: &str, path: &str) -> Implements {
        Implements {
            id: format!("{intent_id}:{path}"),
            intent_id: intent_id.into(),
            codefile_id: path.into(),
            intent_name: intent_id.into(),
            codefile_path: path.into(),
            locator: String::new(),
            created_at: "t".into(),
            inspection_status: "passing".into(),
            last_inspected: "t".into(),
            inspected_by: String::new(),
            criterion: String::new(),
            evidence: String::new(),
            notes: String::new(),
            confidence: 1.0,
        }
    }

    fn rel(a: &str, b: &str) -> RelatesTo {
        RelatesTo {
            id: format!("{a}:{b}"),
            from_id: a.into(),
            from_name: a.into(),
            to_id: b.into(),
            to_name: b.into(),
            inspection_status: "passing".into(),
            last_inspected: "t".into(),
            inspected_by: String::new(),
            criterion: String::new(),
            evidence: String::new(),
            notes: String::new(),
            confidence: 1.0,
            priority_score: 0.0,
            kinds: Vec::new(),
            stable: false,
            discovery_class: String::new(),
            discovery_signals: Vec::new(),
            discovery_centrality: Default::default(),
        }
    }

    fn co_own_snapshot(second_status: &str) -> QuerySnapshot {
        let active = intent("act", "alpha config owner");
        let mut other = intent("dep", "delta config owner");
        other.status = second_status.into();
        QuerySnapshot::from_parts(
            vec![active, other],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![imp("act", "src/shared.rs"), imp("dep", "src/shared.rs")],
            vec![cf("src/shared.rs")],
            Some(Vec::new()),
        )
    }

    fn co_own_report(second_status: &str) -> SmellReport {
        compute_smells_from_parts(
            &co_own_snapshot(second_status),
            SmellInputs {
                notes: &[],
                vocab_terms: &[],
                layer_order: &[],
                proposed_hypotheses: &[],
                targets: &[],
            },
        )
        .unwrap()
    }

    #[test]
    fn deprecated_intents_are_not_file_owners() {
        // Positive control: two ACTIVE intents co-owning one file overlap.
        let active = co_own_report("confirmed");
        let overlap_active = active
            .open
            .iter()
            .filter(|s| s.kind == "overlapping_ownership")
            .count();
        assert!(
            overlap_active >= 1,
            "two active co-owners should raise overlapping_ownership: {:?}",
            active.open
        );
        // `retire_intent` leaves the IMPLEMENTS edge behind; the deprecated
        // intent must no longer count as an owner, so the finding disappears.
        let deprecated = co_own_report("deprecated");
        let overlap_deprecated = deprecated
            .open
            .iter()
            .filter(|s| s.kind == "overlapping_ownership")
            .count();
        assert_eq!(
            overlap_deprecated, 0,
            "a deprecated co-owner must not own the file for smells: {:?}",
            deprecated.open
        );
    }

    fn snap(with_link: bool) -> QuerySnapshot {
        let mut relates = Vec::new();
        if with_link {
            relates.push(rel("hub", "p4"));
        }
        QuerySnapshot::from_parts(
            vec![
                intent("hub", "hub behavior"),
                intent("p1", "partner one"),
                intent("p2", "partner two"),
                intent("p3", "partner three"),
                intent("p4", "partner four"),
            ],
            Vec::new(),
            relates,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![
                imp("hub", "src/hub.rs"),
                imp("p1", "src/p1.rs"),
                imp("p2", "src/p2.rs"),
                imp("p3", "src/p3.rs"),
                imp("p4", "src/p4.rs"),
            ],
            vec![
                cf("src/hub.rs"),
                cf("src/p1.rs"),
                cf("src/p2.rs"),
                cf("src/p3.rs"),
                cf("src/p4.rs"),
            ],
            Some(Vec::new()),
        )
    }

    fn history() -> (HashMap<(String, String), usize>, HashMap<String, usize>) {
        let mut pairs = HashMap::new();
        let mut individual = HashMap::new();
        individual.insert("src/hub.rs".into(), 6);
        for p in ["src/p1.rs", "src/p2.rs", "src/p3.rs", "src/p4.rs"] {
            individual.insert(p.into(), 4);
            pairs.insert(("src/hub.rs".into(), p.into()), 3);
        }
        (pairs, individual)
    }

    #[test]
    fn hub_changing_with_many_unrelated_intents_flags() {
        let (pairs, individual) = history();
        let out = shotgun_surgery_suggestions(&snap(false), &pairs, &individual);
        assert!(
            out.iter()
                .any(|s| s.kind == "shotgun_surgery" && s.summary.contains("hub behavior")),
            "{out:?}"
        );
    }

    #[test]
    fn linked_partner_reduces_pressure_below_threshold() {
        let (pairs, individual) = history();
        assert!(
            shotgun_surgery_suggestions(&snap(true), &pairs, &individual).is_empty(),
            "one linked partner leaves only three unrelated partners"
        );
    }
}

// ---------------------------------------------------------------------------
// proof-locality advisory — the `proven` axis's quality check
// ---------------------------------------------------------------------------

/// `nonlocal_proof` advisories — the static counterpart to the 360° `proven`
/// axis. `proven` counts an implemented leaf with a VALIDATES edge to a PASSED
/// validation; it does NOT check the proof exercises the intent's grounded
/// code. A leaf grounded in file A but proven only by a `test` living in file B
/// reads green while A may have no direct test — partial-coverage overstatement.
///
/// This flags exactly that, STATICALLY (no instrumentation, no coverage run):
/// for each implemented leaf proven ONLY by `test`-type validations, it resolves
/// each test command's selectors to the files they live in (via the graph's
/// test-symbol facts) and flags the leaf when those resolve to a different
/// MODULE (directory) than its grounded code. Module-level — not file-level —
/// because Rust keeps a module's tests beside its code (`mod.rs` / a
/// `#[cfg(test)] mod tests`), so a same-directory test is legitimately local.
///
/// Two deliberate non-firings keep it false-positive-free (the trap the ④ spike
/// named): (1) any non-`test` proof — an `assertion`/`saga`/`manual_check` e2e
/// or subprocess check — exempts the leaf, because that proof legitimately
/// exercises code this static check can't see; (2) a command we can't resolve to
/// any file (an opaque script, a bare `cargo test`) is UNKNOWN, never non-local.
/// Computed OUTSIDE `compute_smells_from` — like `cochange_suggestions`, it is
#[cfg(test)]
mod proof_locality_tests {
    use super::*;
    use crate::types::{CodeFile, Implements, Intent, SymbolFact, ValidatesEdge, Validation};

    fn leaf(id: &str, name: &str) -> Intent {
        Intent {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            criterion: String::new(),
            abstraction_level: "feature".into(),
            domain: String::new(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "confirmed".into(),
            aspect: String::new(),
            tags: Vec::new(),
            visibility: "internal".into(),
            boundary: String::new(),
            lifecycle: "implemented".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }

    fn cf(path: &str, test_syms: &[&str], code_syms: &[&str]) -> CodeFile {
        let mut facts = Vec::new();
        for s in test_syms {
            facts.push(SymbolFact {
                label: format!("fn {s}"),
                name: (*s).into(),
                kind: "fn".into(),
                visibility: "private".into(),
                line_start: 1,
                line_end: 2,
                is_test: true,
                string_literals: Vec::new(),
                panic_marker_count: 0,
                panic_markers: Vec::new(),
                body_hash: String::new(),
                shape_hash: String::new(),
                metrics: crate::types::SymbolMetrics::default(),
            });
        }
        for s in code_syms {
            facts.push(SymbolFact {
                label: format!("pub fn {s}"),
                name: (*s).into(),
                kind: "fn".into(),
                visibility: "public".into(),
                line_start: 1,
                line_end: 2,
                is_test: false,
                string_literals: Vec::new(),
                panic_marker_count: 0,
                panic_markers: Vec::new(),
                body_hash: String::new(),
                shape_hash: String::new(),
                metrics: crate::types::SymbolMetrics::default(),
            });
        }
        CodeFile {
            id: path.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: String::new(),
            imports: Vec::new(),
            symbols: facts.iter().map(|f| f.label.clone()).collect(),
            symbol_facts: facts,
            content_hash: String::new(),
            extractor_grade: String::new(),
        }
    }

    fn imp(intent: &str, path: &str) -> Implements {
        Implements {
            id: format!("imp:{intent}:{path}"),
            intent_id: intent.into(),
            codefile_id: path.into(),
            intent_name: String::new(),
            codefile_path: path.into(),
            inspection_status: "passing".into(),
            criterion: String::new(),
            confidence: 0.0,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: String::new(),
            locator: String::new(),
            notes: String::new(),
            created_at: "t".into(),
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
            id: format!("val:{vid}:{iid}"),
            validation_id: vid.into(),
            intent_id: iid.into(),
            validation_name: String::new(),
            intent_name: String::new(),
            created_at: "t".into(),
            inspection_status: "passing".into(),
            notes: String::new(),
        }
    }

    #[test]
    fn flags_a_leaf_proven_only_by_a_test_in_other_files() {
        let codefiles = vec![
            cf("src/commands/intent.rs", &[], &["delete_intent"]), // grounded, no tests
            cf("src/db/queries/intent.rs", &["retire_is_invisible"], &[]), // the test lives here
        ];
        let intents = vec![leaf("i1", "intent retirement contract")];
        let implements = vec![imp("i1", "src/commands/intent.rs")];
        let validations = vec![val(
            "v1",
            "test",
            "cargo test retire_is_invisible",
            "passed",
        )];
        let validates = vec![vedge("v1", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert_eq!(out.len(), 1, "a non-local test proof should flag: {out:?}");
        assert_eq!(out[0].kind, "nonlocal_proof");
        assert!(
            out[0].summary.contains("intent retirement contract"),
            "{}",
            out[0].summary
        );
    }

    #[test]
    fn a_test_in_the_grounded_file_is_local_and_silent() {
        let codefiles = vec![cf(
            "src/db/queries/intent.rs",
            &["retire_is_invisible"],
            &["resolve_intent"],
        )];
        let intents = vec![leaf("i1", "intent resolution")];
        let implements = vec![imp("i1", "src/db/queries/intent.rs")];
        let validations = vec![val(
            "v1",
            "test",
            "cargo test retire_is_invisible",
            "passed",
        )];
        let validates = vec![vedge("v1", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert!(
            out.is_empty(),
            "a test in the grounded file is local: {out:?}"
        );
    }

    #[test]
    fn an_e2e_assertion_proof_exempts_the_leaf() {
        let codefiles = vec![
            cf("src/commands/intent.rs", &[], &["delete_intent"]),
            cf("src/db/queries/intent.rs", &["retire_is_invisible"], &[]),
        ];
        let intents = vec![leaf("i1", "intent retirement contract")];
        let implements = vec![imp("i1", "src/commands/intent.rs")];
        let validations = vec![
            val("v1", "test", "cargo test retire_is_invisible", "passed"),
            val("v2", "assertion", ".claude/skills/e2e_retire.sh", "passed"),
        ];
        let validates = vec![vedge("v1", "i1"), vedge("v2", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert!(
            out.is_empty(),
            "an e2e/assertion proof exercises code the static check can't see — exempt: {out:?}"
        );
    }

    #[test]
    fn an_unresolvable_command_is_unknown_never_nonlocal() {
        let codefiles = vec![
            cf("src/commands/intent.rs", &[], &["delete_intent"]),
            cf("src/db/queries/intent.rs", &["retire_is_invisible"], &[]),
        ];
        let intents = vec![leaf("i1", "intent retirement contract")];
        let implements = vec![imp("i1", "src/commands/intent.rs")];
        // The selector matches no known test symbol → can't be located → silent.
        let validations = vec![val(
            "v1",
            "test",
            "cargo test no_such_test_name_xyz",
            "passed",
        )];
        let validates = vec![vedge("v1", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert!(
            out.is_empty(),
            "an unresolvable selector is UNKNOWN, not non-local: {out:?}"
        );
    }

    #[test]
    fn a_sibling_test_in_the_same_module_is_local() {
        // loom keeps query-layer tests in queries/mod.rs, not beside each file;
        // module-level locality must treat that as covering the module (this would
        // wrongly flag under file-level locality).
        let codefiles = vec![
            cf("src/db/queries/intent.rs", &[], &["resolve_intent"]), // grounded code
            cf("src/db/queries/mod.rs", &["resolve_intent_roundtrips"], &[]), // tests live here
        ];
        let intents = vec![leaf("i1", "intent resolution")];
        let implements = vec![imp("i1", "src/db/queries/intent.rs")];
        let validations = vec![val(
            "v1",
            "test",
            "cargo test resolve_intent_roundtrips",
            "passed",
        )];
        let validates = vec![vedge("v1", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert!(
            out.is_empty(),
            "a test in a sibling file of the same module is local: {out:?}"
        );
    }

    #[test]
    fn a_module_path_selector_resolves_to_its_directory() {
        let codefiles = vec![cf(
            "src/db/queries/intent.rs",
            &["resolve_intent_roundtrips"],
            &["resolve_intent"],
        )];
        let intents = vec![leaf("i1", "intent resolution")];
        let implements = vec![imp("i1", "src/db/queries/intent.rs")];
        let validations = vec![val(
            "v1",
            "test",
            "cargo test db::queries 2>/dev/null",
            "passed",
        )];
        let validates = vec![vedge("v1", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert!(
            out.is_empty(),
            "a module-path selector resolves to its directory → local: {out:?}"
        );
    }

    #[test]
    fn a_trailing_module_path_selector_resolves_to_its_module() {
        let codefiles = vec![cf(
            "src/saga/spec.rs",
            &["good_spec_parses"],
            &["load_spec"],
        )];
        let intents = vec![leaf("i1", "saga spec")];
        let implements = vec![imp("i1", "src/saga/spec.rs")];
        let validations = vec![val(
            "v1",
            "test",
            "cargo test saga::spec::tests::",
            "passed",
        )];
        let validates = vec![vedge("v1", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert!(
            out.is_empty(),
            "a trailing module selector resolves to the grounded module: {out:?}"
        );
    }

    #[test]
    fn no_symbol_facts_means_the_instrument_is_unarmed() {
        // A graph with zero test-symbol facts must flag nothing, never everything.
        let codefiles = vec![cf("src/commands/intent.rs", &[], &["delete_intent"])];
        let intents = vec![leaf("i1", "intent retirement contract")];
        let implements = vec![imp("i1", "src/commands/intent.rs")];
        let validations = vec![val(
            "v1",
            "test",
            "cargo test retire_is_invisible",
            "passed",
        )];
        let validates = vec![vedge("v1", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert!(
            out.is_empty(),
            "no symbol facts → unarmed → silent: {out:?}"
        );
    }
}

#[cfg(test)]
mod clone_tests {
    use super::*;
    use crate::types::{CodeFile, Hypothesis, Note, SymbolFact};

    fn sym(
        name: &str,
        body_hash: &str,
        shape_hash: &str,
        line_start: usize,
        line_end: usize,
        is_test: bool,
    ) -> SymbolFact {
        SymbolFact {
            label: format!("fn {name}"),
            name: name.into(),
            kind: "fn".into(),
            visibility: "private".into(),
            line_start,
            line_end,
            is_test,
            string_literals: Vec::new(),
            panic_marker_count: 0,
            panic_markers: Vec::new(),
            body_hash: body_hash.into(),
            shape_hash: shape_hash.into(),
            metrics: crate::types::SymbolMetrics::default(),
        }
    }

    fn cf(path: &str, facts: Vec<SymbolFact>) -> CodeFile {
        CodeFile {
            id: path.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: String::new(),
            imports: Vec::new(),
            symbols: facts.iter().map(|f| f.label.clone()).collect(),
            symbol_facts: facts,
            content_hash: String::new(),
            extractor_grade: String::new(),
        }
    }

    fn snap(codefiles: Vec<CodeFile>) -> QuerySnapshot {
        QuerySnapshot::from_parts(
            Vec::new(), // intents
            Vec::new(), // hierarchy
            Vec::new(), // relates
            Vec::new(), // governs
            Vec::new(), // rules
            Vec::new(), // validates
            Vec::new(), // validations
            Vec::new(), // implements
            codefiles,
            Some(Vec::new()), // notes
        )
    }

    fn decision(target_id: &str) -> Note {
        Note {
            id: format!("note-{target_id}"),
            kind: "decision".into(),
            text: "these copies are deliberately independent in this fixture".into(),
            author: "human".into(),
            target_kind: "smell".into(),
            target_id: target_id.into(),
            resolution: String::new(),
            audience: String::new(),
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn hypothesis(status: &str, text: &str) -> Hypothesis {
        Hypothesis {
            id: format!("hyp-{status}"),
            name: format!("{status} clone refactor"),
            claim: text.into(),
            proposal: text.into(),
            predicted_outcome: "the duplicated shape collapses to one definition".into(),
            status: status.into(),
            author: "tester".into(),
            evidence: String::new(),
            inspected_by: String::new(),
            last_inspected: String::new(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn shape_hash_decision_adjudicates_clone_without_lowering_total_and_shape_change_reopens() {
        let s = snap(vec![
            cf(
                "src/a.rs",
                vec![sym("alpha", "BODY_A", "SHAPE", 1, 10, false)],
            ),
            cf(
                "src/b.rs",
                vec![sym("beta", "BODY_B", "SHAPE", 20, 29, false)],
            ),
        ]);
        let advisories = clone_suggestions(&s, &[]);
        assert_eq!(advisories.len(), 1, "fixture should have one physical clone group");
        assert!(
            advisories[0].remedy.contains("--smell \"code_clone:SHAPE\""),
            "the decision note command must target the normalized shape hash: {}",
            advisories[0].remedy
        );

        let open = crate::commands::smells::code_clone_dispositions(
            &s,
            advisories.clone(),
            &[],
            &[],
        );
        assert_eq!(open.total, 1);
        assert_eq!(open.deliberate, 0);
        assert_eq!(open.tracked, 0);
        assert_eq!(open.open, 1);

        let deliberate = crate::commands::smells::code_clone_dispositions(
            &s,
            advisories.clone(),
            &[decision("code_clone:SHAPE")],
            &[],
        );
        assert_eq!(
            deliberate.total, 1,
            "adjudicating a clone must not lower code_clones_total below the physical clone count"
        );
        assert_eq!(deliberate.deliberate, 1);
        assert_eq!(deliberate.tracked, 0);
        assert_eq!(deliberate.open, 0);
        assert!(deliberate.open_advisories.is_empty());
        assert_eq!(deliberate.adjudicated.len(), 1);
        assert_eq!(
            deliberate.adjudicated[0].reopens_when,
            "the clone's normalized shape changes"
        );

        let changed_shape = snap(vec![
            cf(
                "src/a.rs",
                vec![sym("alpha", "BODY_A2", "SHAPE_V2", 1, 10, false)],
            ),
            cf(
                "src/b.rs",
                vec![sym("beta", "BODY_B2", "SHAPE_V2", 20, 29, false)],
            ),
        ]);
        let reopened = crate::commands::smells::code_clone_dispositions(
            &changed_shape,
            clone_suggestions(&changed_shape, &[]),
            &[decision("code_clone:SHAPE")],
            &[],
        );
        assert_eq!(reopened.total, 1);
        assert_eq!(reopened.deliberate, 0);
        assert_eq!(reopened.tracked, 0);
        assert_eq!(reopened.open, 1, "a different shape hash is a different finding");

        let tracked = crate::commands::smells::code_clone_dispositions(
            &s,
            advisories,
            &[],
            &[hypothesis("proposed", "dedupe the clone in src/a.rs after the release")],
        );
        assert_eq!(tracked.total, 1);
        assert_eq!(tracked.deliberate, 0);
        assert_eq!(tracked.tracked, 1);
        assert_eq!(tracked.open, 0);
    }

    #[test]
    fn two_files_with_one_shared_body_hash_flag_once() {
        let s = snap(vec![
            cf("src/a.rs", vec![sym("alpha", "HASH", "", 1, 10, false)]),
            cf("src/b.rs", vec![sym("beta", "HASH", "", 20, 29, false)]),
        ]);
        let out = clone_suggestions(&s, &[]);
        assert_eq!(
            out.len(),
            1,
            "an exact cross-file clone should flag once: {out:?}"
        );
        assert_eq!(out[0].kind, "code_clone");
        assert!(out[0].evidence.contains("body_hash"));
        assert!(out[0].evidence.contains("src/a.rs"));
        assert!(out[0].evidence.contains("src/b.rs"));
    }

    #[test]
    fn shared_shape_hash_flags_renamed_clone() {
        let s = snap(vec![
            cf(
                "src/a.rs",
                vec![sym("alpha", "BODY_A", "SHAPE", 1, 10, false)],
            ),
            cf(
                "src/b.rs",
                vec![sym("beta", "BODY_B", "SHAPE", 20, 29, false)],
            ),
        ]);
        let out = clone_suggestions(&s, &[]);
        assert_eq!(
            out.len(),
            1,
            "a normalized cross-file clone should flag once: {out:?}"
        );
        assert!(out[0].evidence.contains("shape_hash"));
    }

    #[test]
    fn data_declarations_do_not_group_by_shape_hash() {
        let mut alpha = sym("Alpha", "BODY_A", "SHAPE", 1, 10, false);
        alpha.kind = "struct".into();
        alpha.label = "struct Alpha".into();
        let mut beta = sym("Beta", "BODY_B", "SHAPE", 20, 29, false);
        beta.kind = "struct".into();
        beta.label = "struct Beta".into();
        let s = snap(vec![
            cf("src/a.rs", vec![alpha]),
            cf("src/b.rs", vec![beta]),
        ]);
        assert!(
            clone_suggestions(&s, &[]).is_empty(),
            "passive data declarations stay exact-text only"
        );
    }

    #[test]
    fn test_symbols_are_skipped() {
        let s = snap(vec![
            cf("src/a.rs", vec![sym("alpha", "HASH", "", 1, 10, true)]),
            cf("src/b.rs", vec![sym("beta", "HASH", "", 20, 29, true)]),
        ]);
        assert!(
            clone_suggestions(&s, &[]).is_empty(),
            "test fixtures legitimately repeat"
        );
    }

    #[test]
    fn bodies_below_the_size_floor_are_skipped() {
        // span = line_end - line_start + 1 = 4 < MIN_CLONE_LINES (5).
        let s = snap(vec![
            cf("src/a.rs", vec![sym("alpha", "HASH", "", 1, 4, false)]),
            cf("src/b.rs", vec![sym("beta", "HASH", "", 20, 23, false)]),
        ]);
        assert!(
            clone_suggestions(&s, &[]).is_empty(),
            "below the size floor is boilerplate"
        );
    }

    #[test]
    fn an_ignored_file_drops_the_pair_below_cross_file() {
        let pat = glob::Pattern::new("src/generated/**").unwrap();
        let s = snap(vec![
            cf(
                "src/generated/a.rs",
                vec![sym("alpha", "HASH", "", 1, 10, false)],
            ),
            cf("src/b.rs", vec![sym("beta", "HASH", "", 20, 29, false)]),
        ]);
        assert!(
            clone_suggestions(&s, &[pat]).is_empty(),
            "ignoring one copy leaves a single-file group → no cross-file clone"
        );
    }

    #[test]
    fn intra_file_repetition_is_not_a_cross_file_clone() {
        let s = snap(vec![cf(
            "src/a.rs",
            vec![
                sym("alpha", "HASH", "", 1, 10, false),
                sym("beta", "HASH", "", 20, 29, false),
            ],
        )]);
        assert!(
            clone_suggestions(&s, &[]).is_empty(),
            "intra-file repetition is tangled_file's concern, not a cross-file clone"
        );
    }

    #[test]
    fn empty_body_hash_is_unarmed_and_silent() {
        let s = snap(vec![
            cf("src/a.rs", vec![sym("alpha", "", "", 1, 10, false)]),
            cf("src/b.rs", vec![sym("beta", "", "", 20, 29, false)]),
        ]);
        assert!(
            clone_suggestions(&s, &[]).is_empty(),
            "pre-v8 / feature-light facts carry no hash → instrument unarmed"
        );
    }
}
