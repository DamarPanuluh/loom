#[cfg(test)]
mod large_behavioral_symbol_tests {
    use super::*;
    use crate::types::{CodeFile, Note, SymbolFact};

    fn sym(
        kind: &str,
        label: &str,
        line_start: usize,
        line_end: usize,
        is_test: bool,
    ) -> SymbolFact {
        SymbolFact {
            label: label.into(),
            name: label.split_whitespace().last().unwrap_or(label).into(),
            kind: kind.into(),
            visibility: "private".into(),
            line_start,
            line_end,
            is_test,
            string_literals: Vec::new(),
            panic_marker_count: 0,
            panic_markers: Vec::new(),
            body_hash: String::new(),
            shape_hash: String::new(),
        }
    }

    fn cf(path: &str, last_modified: &str, facts: Vec<SymbolFact>) -> CodeFile {
        CodeFile {
            id: path.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: last_modified.into(),
            imports: Vec::new(),
            symbols: facts.iter().map(|f| f.label.clone()).collect(),
            symbol_facts: facts,
            content_hash: String::new(),
            extractor_grade: String::new(),
        }
    }

    fn report(codefiles: Vec<CodeFile>, notes: &[Note]) -> SmellReport {
        let snapshot = QuerySnapshot::from_parts(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            codefiles,
            None,
        );
        compute_smells_from_parts(
            &snapshot,
            SmellInputs {
                notes,
                vocab_terms: &[],
                layer_order: &[],
                proposed_hypotheses: &[],
                targets: &[],
            },
        )
        .unwrap()
    }

    fn of_kind<'a>(r: &'a SmellReport, kind: &str) -> Vec<&'a Smell> {
        r.open
            .iter()
            .chain(r.advisory.iter())
            .filter(|s| s.kind == kind)
            .collect()
    }

    #[test]
    fn non_test_behavioral_symbol_at_threshold_is_flagged_advisory() {
        let r = report(
            vec![cf(
                "src/big.rs",
                "2026-06-15T00:00:00+00:00",
                vec![sym(
                    "fn",
                    "fn huge_behavior",
                    1,
                    LARGE_BEHAVIORAL_SYMBOL_LINES,
                    false,
                )],
            )],
            &[],
        );
        let found = of_kind(&r, "large_behavioral_symbol");
        assert_eq!(found.len(), 1, "large behavior should flag once: {found:?}");
        // LOC/size smells are ADVISORY (a flag, never a gate): present in the
        // surfaced advisory bucket, absent from the gating open set.
        assert!(r
            .advisory
            .iter()
            .any(|s| s.kind == "large_behavioral_symbol"));
        assert!(!r.open.iter().any(|s| s.kind == "large_behavioral_symbol"));
        assert!(found[0].summary.contains("huge_behavior"));
        assert!(found[0].evidence.contains(&format!(
            "above the {}-line threshold",
            LARGE_BEHAVIORAL_SYMBOL_LINES
        )));
        assert_eq!(
            found[0].teaching.done_when,
            teaching_for("large_behavioral_symbol").done_when
        );
    }

    #[test]
    fn tests_data_declarations_and_small_functions_are_skipped() {
        let r = report(
            vec![cf(
                "src/mixed.rs",
                "2026-06-15T00:00:00+00:00",
                vec![
                    sym(
                        "fn",
                        "fn huge_test",
                        1,
                        LARGE_BEHAVIORAL_SYMBOL_LINES + 50,
                        true,
                    ),
                    sym(
                        "struct",
                        "struct HugeData",
                        1,
                        LARGE_BEHAVIORAL_SYMBOL_LINES + 50,
                        false,
                    ),
                    sym(
                        "fn",
                        "fn small_behavior",
                        1,
                        LARGE_BEHAVIORAL_SYMBOL_LINES - 1,
                        false,
                    ),
                ],
            )],
            &[],
        );
        assert!(
            of_kind(&r, "large_behavioral_symbol").is_empty(),
            "only non-test behavioral symbols over the threshold flag"
        );
    }

    #[test]
    fn current_file_decision_adjudicates_large_symbol() {
        let note = Note {
            id: "n1".into(),
            kind: "decision".into(),
            text: "large parser stays linear for now".into(),
            author: "llm".into(),
            target_kind: "smell".into(),
            target_id: "large_behavioral_symbol:src/big.rs:method parse_everything".into(),
            resolution: String::new(),
            audience: String::new(),
            created_at: "2026-06-16T00:00:00+00:00".into(),
        };
        let r = report(
            vec![cf(
                "src/big.rs",
                "2026-06-15T00:00:00+00:00",
                vec![sym(
                    "method",
                    "method parse_everything",
                    10,
                    LARGE_BEHAVIORAL_SYMBOL_LINES + 20,
                    false,
                )],
            )],
            &[note],
        );
        assert!(
            of_kind(&r, "large_behavioral_symbol").is_empty(),
            "a current file decision suppresses the open finding"
        );
        assert!(
            r.adjudicated
                .iter()
                .any(|s| s.kind == "large_behavioral_symbol"
                    && s.summary.contains("parse_everything")),
            "suppressed finding should be visible as adjudicated"
        );
    }
}

#[cfg(test)]
mod lexical_signal_tests {
    use super::*;
    use crate::types::Intent;

    fn intent(id: usize) -> Intent {
        Intent {
            id: format!("i{id}"),
            name: format!("feature {id} handler"),
            description:
                "feature processes requests and transforms data within the synthetic benchmark system"
                    .into(),
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

    #[test]
    fn ubiquitous_tokens_do_not_create_pairwise_twin_floods() {
        let snapshot = QuerySnapshot::from_parts(
            (0..40).map(intent).collect(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
        );
        let report = compute_smells_from_parts(
            &snapshot,
            SmellInputs {
                notes: &[],
                vocab_terms: &[],
                layer_order: &[],
                proposed_hypotheses: &[],
                targets: &[],
            },
        )
        .unwrap();
        assert!(
            report.open.iter().all(|s| s.kind != "twin_intents"),
            "{:?}",
            report
                .open
                .iter()
                .filter(|s| s.kind == "twin_intents")
                .collect::<Vec<_>>()
        );
    }
}

#[cfg(test)]
mod source_fact_smell_tests {
    use super::*;
    use crate::types::{CodeFile, Note, StringLiteralFact, SymbolFact};

    fn sym(label: &str) -> SymbolFact {
        SymbolFact {
            label: label.into(),
            name: label.split_whitespace().last().unwrap_or(label).into(),
            kind: "fn".into(),
            visibility: "private".into(),
            line_start: 1,
            line_end: 20,
            is_test: false,
            string_literals: Vec::new(),
            panic_marker_count: 0,
            panic_markers: Vec::new(),
            body_hash: String::new(),
            shape_hash: String::new(),
        }
    }

    fn with_string(mut fact: SymbolFact, value: &str, line: usize) -> SymbolFact {
        fact.string_literals.push(StringLiteralFact {
            value: value.into(),
            line,
        });
        fact
    }

    fn with_panic(mut fact: SymbolFact, markers: &[&str]) -> SymbolFact {
        fact.panic_marker_count = markers.len();
        fact.panic_markers = markers.iter().map(|m| (*m).into()).collect();
        fact
    }

    fn cf(path: &str, facts: Vec<SymbolFact>) -> CodeFile {
        CodeFile {
            id: path.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: "2026-06-15T00:00:00+00:00".into(),
            imports: Vec::new(),
            symbols: facts.iter().map(|f| f.label.clone()).collect(),
            symbol_facts: facts,
            content_hash: String::new(),
            extractor_grade: String::new(),
        }
    }

    fn report(codefiles: Vec<CodeFile>, notes: &[Note]) -> SmellReport {
        let snapshot = QuerySnapshot::from_parts(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            codefiles,
            Some(Vec::new()),
        );
        compute_smells_from_parts(
            &snapshot,
            SmellInputs {
                notes,
                vocab_terms: &[],
                layer_order: &[],
                proposed_hypotheses: &[],
                targets: &[],
            },
        )
        .unwrap()
    }

    fn of_kind<'a>(r: &'a SmellReport, kind: &str) -> Vec<&'a Smell> {
        r.open
            .iter()
            .chain(r.advisory.iter())
            .filter(|s| s.kind == kind)
            .collect()
    }

    fn decision_note(target_id: &str, created_at: &str) -> Note {
        Note {
            id: "n1".into(),
            kind: "decision".into(),
            text: "deliberate".into(),
            author: "human".into(),
            target_kind: "smell".into(),
            target_id: target_id.into(),
            resolution: String::new(),
            created_at: created_at.into(),
            audience: String::new(),
        }
    }

    /// Adjudication is keyed on FINDING IDENTITY (`<kind>:<scope>`): a ruling
    /// scoped to one finding cannot launder a different finding that merely
    /// shares the file (the god-file root cause).
    #[test]
    fn adjudication_is_kind_scoped_no_cross_kind_laundering() {
        let mut big = sym("fn big_handler");
        big.line_start = 1;
        big.line_end = 1 + LARGE_BEHAVIORAL_SYMBOL_LINES + 50; // span well over the threshold
        let file = cf("src/x.rs", vec![big]);

        // Unadjudicated → the large_behavioral_symbol finding is open.
        let r = report(vec![file.clone()], &[]);
        assert_eq!(of_kind(&r, "large_behavioral_symbol").len(), 1);

        // A ruling scoped to a DIFFERENT finding (a tangled_file on the same
        // file) must NOT clear it — pre-fix, any file-scoped note would have.
        let wrong = decision_note("tangled_file:src/x.rs", "2026-06-20T00:00:00+00:00");
        let r = report(vec![file.clone()], std::slice::from_ref(&wrong));
        assert_eq!(
            of_kind(&r, "large_behavioral_symbol").len(),
            1,
            "a tangled_file ruling must not launder a large_behavioral_symbol finding"
        );

        // The correctly-scoped ruling adjudicates exactly this finding.
        let right = decision_note(
            "large_behavioral_symbol:src/x.rs:fn big_handler",
            "2026-06-20T00:00:00+00:00",
        );
        let r = report(vec![file], std::slice::from_ref(&right));
        assert!(
            of_kind(&r, "large_behavioral_symbol").is_empty(),
            "the correctly-scoped --smell ruling clears the finding"
        );
        assert!(r
            .adjudicated
            .iter()
            .any(|a| a.kind == "large_behavioral_symbol"));
    }

    #[test]
    fn repeated_long_string_contract_flags_once() {
        let text = "Run loom sync before reading the status output again";
        let r = report(
            vec![
                cf("src/a.rs", vec![with_string(sym("fn alpha"), text, 4)]),
                cf("src/b.rs", vec![with_string(sym("fn beta"), text, 9)]),
            ],
            &[],
        );
        let found = of_kind(&r, "string_contract_duplicate");
        assert_eq!(found.len(), 1, "repeated contract should flag: {found:?}");
        assert!(found[0].summary.contains("string contract repeated"));
        assert!(found[0].evidence.contains("src/a.rs:4"));
        assert!(found[0].evidence.contains("src/b.rs:9"));
    }

    #[test]
    fn short_or_path_like_strings_do_not_flag() {
        let r = report(
            vec![
                cf(
                    "src/a.rs",
                    vec![
                        with_string(sym("fn alpha"), "tiny label", 4),
                        with_string(sym("fn beta"), "src/db/queries/smells.rs", 5),
                    ],
                ),
                cf(
                    "src/b.rs",
                    vec![
                        with_string(sym("fn gamma"), "tiny label", 6),
                        with_string(sym("fn delta"), "src/db/queries/smells.rs", 7),
                    ],
                ),
            ],
            &[],
        );
        assert!(
            of_kind(&r, "string_contract_duplicate").is_empty(),
            "trivial strings should stay quiet"
        );
    }

    #[test]
    fn panic_marker_in_behavior_flags() {
        let r = report(
            vec![cf(
                "src/commands/run.rs",
                vec![with_panic(sym("fn run"), &["unwrap", "expect"])],
            )],
            &[],
        );
        let found = of_kind(&r, "panic_marker_risk");
        assert_eq!(found.len(), 1, "panic marker should flag: {found:?}");
        assert!(found[0].evidence.contains("markers=[unwrap, expect]"));
        assert!(found[0].evidence.contains("command/public surface"));
    }

    #[test]
    fn panic_marker_file_decision_adjudicates() {
        let note = Note {
            id: "n1".into(),
            kind: "decision".into(),
            text: "unwraps are construction-time invariants here".into(),
            author: "llm".into(),
            target_kind: "smell".into(),
            target_id: "panic_marker_risk:src/a.rs:fn alpha".into(),
            resolution: String::new(),
            audience: String::new(),
            created_at: "2026-06-16T00:00:00+00:00".into(),
        };
        let r = report(
            vec![cf(
                "src/a.rs",
                vec![with_panic(sym("fn alpha"), &["unwrap"])],
            )],
            &[note],
        );
        assert!(of_kind(&r, "panic_marker_risk").is_empty());
        assert!(r.adjudicated.iter().any(|s| s.kind == "panic_marker_risk"));
    }
}
