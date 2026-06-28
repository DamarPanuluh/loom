#[cfg(test)]
mod cycle_island_tests {
    use super::*;
    use crate::types::Intent;

    fn intent(id: &str, level: &str) -> Intent {
        Intent {
            id: id.into(),
            name: format!("intent {id}"),
            description: format!("does {id} things"),
            criterion: String::new(),
            abstraction_level: level.into(),
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

    fn rel(from: &str, to: &str) -> crate::types::RelatesTo {
        // Default to a GROUNDED (passing) verdict with a real last_inspected, so
        // a reciprocal pair built from two `rel(...)`s is a deliberate
        // double-grounding (the case the detector fires on).
        rel_st(from, to, "passing")
    }

    fn rel_st(from: &str, to: &str, status: &str) -> crate::types::RelatesTo {
        // Grounded statuses carry a timestamp; uninspected/independent do not —
        // mirrors how the store records verdicts vs mechanically-created edges.
        let last_inspected = match status {
            "passing" | "failing" | "needs_reverification" => "2026-06-15T00:00:00+00:00",
            _ => "",
        };
        crate::types::RelatesTo {
            id: format!("rt:{from}:{to}"),
            from_id: from.into(),
            to_id: to.into(),
            from_name: from.into(),
            to_name: to.into(),
            inspection_status: status.into(),
            criterion: String::new(),
            confidence: 0.0,
            evidence: String::new(),
            last_inspected: last_inspected.into(),
            inspected_by: String::new(),
            priority_score: 0.0,
            notes: String::new(),
            kinds: Vec::new(),
            stable: false,
            discovery_class: String::new(),
            discovery_signals: Vec::new(),
            discovery_centrality: Default::default(),
        }
    }

    fn report(
        intents: Vec<Intent>,
        hierarchy: Vec<(String, String)>,
        relates: Vec<crate::types::RelatesTo>,
    ) -> SmellReport {
        report_with_notes(intents, hierarchy, relates, &[])
    }

    fn report_with_notes(
        intents: Vec<Intent>,
        hierarchy: Vec<(String, String)>,
        relates: Vec<crate::types::RelatesTo>,
        notes: &[Note],
    ) -> SmellReport {
        let snapshot = QuerySnapshot::from_parts(
            intents,
            hierarchy,
            relates,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
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
        r.open.iter().filter(|s| s.kind == kind).collect()
    }

    // A system root parents a..c so they are never islands; the cycle, if any,
    // is purely in the RELATES_TO direction.
    fn rooted(children: &[&str]) -> (Vec<Intent>, Vec<(String, String)>) {
        let mut intents = vec![intent("sys", "system")];
        let mut hierarchy = Vec::new();
        for c in children {
            intents.push(intent(c, "feature"));
            hierarchy.push(("sys".to_string(), c.to_string()));
        }
        (intents, hierarchy)
    }

    #[test]
    fn a_long_directed_cycle_is_not_a_dependency_cycle() {
        // a→b→c→a: a directed loop of single-direction grounded edges. Stored
        // direction is a typing-order artifact, so this is NOT flagged — the
        // behavior change from the SCC era, and the central de-noising win.
        let (intents, hierarchy) = rooted(&["a", "b", "c"]);
        let relates = vec![rel("a", "b"), rel("b", "c"), rel("c", "a")];
        let r = report(intents, hierarchy, relates);
        assert!(
            of_kind(&r, "dependency_cycle").is_empty(),
            "a single-direction long cycle is noise, not a finding"
        );
    }

    #[test]
    fn a_reciprocal_grounded_pair_is_a_dependency_cycle() {
        let (intents, hierarchy) = rooted(&["a", "b"]);
        // BOTH directions grounded with a verdict → a deliberate double-assert.
        let relates = vec![rel("a", "b"), rel("b", "a")];
        let r = report(intents, hierarchy, relates);
        let found = of_kind(&r, "dependency_cycle");
        assert_eq!(
            found.len(),
            1,
            "one finding for the reciprocal pair: {found:?}"
        );
        assert!(found[0].summary.contains("intent a") && found[0].summary.contains("intent b"));
    }

    #[test]
    fn an_acyclic_chain_reports_no_cycle() {
        let (intents, hierarchy) = rooted(&["a", "b", "c"]);
        let relates = vec![rel("a", "b"), rel("b", "c")]; // single-direction DAG
        let r = report(intents, hierarchy, relates);
        assert!(of_kind(&r, "dependency_cycle").is_empty());
    }

    #[test]
    fn an_uninspected_reciprocal_pair_does_not_fire() {
        // The saga fingerprint: `loom saga add` writes BOTH directions as
        // mechanically-created uninspected path edges (a round-trip journey).
        // Requiring a verdict on both directions excludes this — no false gate.
        let (intents, hierarchy) = rooted(&["a", "b"]);
        let relates = vec![
            rel_st("a", "b", "uninspected"),
            rel_st("b", "a", "uninspected"),
        ];
        let r = report(intents, hierarchy, relates);
        assert!(
            of_kind(&r, "dependency_cycle").is_empty(),
            "mechanically-created uninspected round-trip is not graph-hygiene debt"
        );
    }

    #[test]
    fn a_half_grounded_reciprocal_pair_does_not_fire() {
        // One direction grounded by an agent, the reverse left uninspected
        // (e.g. a saga path edge) — not a deliberate double-grounding.
        let (intents, hierarchy) = rooted(&["a", "b"]);
        let relates = vec![rel_st("a", "b", "passing"), rel_st("b", "a", "uninspected")];
        let r = report(intents, hierarchy, relates);
        assert!(of_kind(&r, "dependency_cycle").is_empty());
    }

    #[test]
    fn an_independent_reverse_does_not_make_a_reciprocal_pair() {
        // a→b grounded, b→a explicitly independent (no relationship that way) —
        // only ONE real relationship, so not a redundant double-grounding.
        let (intents, hierarchy) = rooted(&["a", "b"]);
        let relates = vec![rel_st("a", "b", "passing"), rel_st("b", "a", "independent")];
        let r = report(intents, hierarchy, relates);
        assert!(of_kind(&r, "dependency_cycle").is_empty());
    }

    #[test]
    fn reciprocal_pair_evidence_surfaces_disagreeing_verdicts() {
        // The two stored rows can silently disagree — the evidence must show it.
        let (intents, hierarchy) = rooted(&["a", "b"]);
        let relates = vec![rel_st("a", "b", "passing"), rel_st("b", "a", "failing")];
        let r = report(intents, hierarchy, relates);
        let found = of_kind(&r, "dependency_cycle");
        assert_eq!(found.len(), 1);
        assert!(
            found[0].evidence.contains("passing") && found[0].evidence.contains("failing"),
            "evidence surfaces the disagreement: {}",
            found[0].evidence
        );
    }

    #[test]
    fn an_adjudicated_reciprocal_pair_is_suppressed() {
        // A decision note on the smaller-id anchor, newer than both edges'
        // last_inspected, suppresses the OPEN finding (shows as adjudicated).
        let (intents, hierarchy) = rooted(&["a", "b"]);
        let relates = vec![rel("a", "b"), rel("b", "a")];
        let note = Note {
            id: "n1".into(),
            kind: "decision".into(),
            text: "a and b are deliberate mutual peers because the two grounded verdicts assert opposite contract directions".into(),
            author: "llm".into(),
            target_kind: "smell".into(),
            target_id: "dependency_cycle:a".into(), // <kind>:<smaller-id anchor>
            resolution: String::new(),
            audience: String::new(),
            created_at: "2026-06-16T00:00:00+00:00".into(), // newer than the edges
        };
        let r = report_with_notes(intents, hierarchy, relates, &[note]);
        assert!(
            of_kind(&r, "dependency_cycle").is_empty(),
            "a current decision on the anchor suppresses the open finding"
        );
        assert!(
            r.adjudicated.iter().any(|a| a.kind == "dependency_cycle"),
            "it surfaces as adjudicated, not silently gone"
        );
    }

    #[test]
    fn a_disconnected_subgraph_is_an_island() {
        // sys—a is connected; x—y float free with no path to a system root.
        let mut intents = vec![intent("sys", "system"), intent("a", "feature")];
        intents.push(intent("x", "feature"));
        intents.push(intent("y", "feature"));
        let hierarchy = vec![("sys".to_string(), "a".to_string())];
        let relates = vec![rel("x", "y")];
        let r = report(intents, hierarchy, relates);
        let islands = of_kind(&r, "intent_island");
        assert_eq!(
            islands.len(),
            1,
            "the x—y component is one island: {islands:?}"
        );
        assert!(islands[0].summary.contains("intent x"));
        assert!(islands[0].summary.contains("intent y"));
    }

    #[test]
    fn a_fully_connected_graph_has_no_islands() {
        let (mut intents, mut hierarchy) = rooted(&["a", "b"]);
        intents.push(intent("c", "feature"));
        // c reaches the system root via a RELATES_TO edge, not hierarchy.
        hierarchy.retain(|_| true);
        let relates = vec![rel("a", "c")];
        let r = report(intents, hierarchy, relates);
        assert!(of_kind(&r, "intent_island").is_empty());
    }

    #[test]
    fn island_detector_is_unarmed_without_a_system_root() {
        // No system intent at all → nothing to be an island relative to.
        let intents = vec![intent("a", "feature"), intent("b", "feature")];
        let relates = vec![rel("a", "b")];
        let r = report(intents, vec![], relates);
        assert!(
            of_kind(&r, "intent_island").is_empty(),
            "silent without a system root (missing-root is a different problem)"
        );
    }

    fn with_aspect(id: &str, aspect: &str) -> Intent {
        let mut i = intent(id, "feature");
        aspect.clone_into(&mut i.aspect);
        i
    }

    #[test]
    fn ui_state_populated_without_empty_or_error_is_happy_path_only() {
        // A screen with a populated state but no empty/error sibling — the new
        // UI-state aspect family, sharing the happy_path_only detector.
        let intents = vec![
            intent("sys", "system"),
            intent("screen", "component"),
            with_aspect("populated_state", "populated"),
        ];
        let hierarchy = vec![
            ("sys".to_string(), "screen".to_string()),
            ("screen".to_string(), "populated_state".to_string()),
        ];
        let r = report(intents, hierarchy, vec![]);
        let hp = of_kind(&r, "happy_path_only");
        assert_eq!(
            hp.len(),
            1,
            "populated with no empty/error sibling fires: {hp:?}"
        );
        assert!(
            hp[0].summary.contains("populated") && hp[0].summary.contains("empty/error"),
            "names the UI-state family: {}",
            hp[0].summary
        );
    }

    #[test]
    fn behavioral_happy_without_sad_or_fallback_still_fires() {
        // Regression: the original behavioral family still works through the
        // generalized families table.
        let intents = vec![
            intent("sys", "system"),
            intent("feat", "component"),
            with_aspect("sunny", "happy"),
        ];
        let hierarchy = vec![
            ("sys".to_string(), "feat".to_string()),
            ("feat".to_string(), "sunny".to_string()),
        ];
        let r = report(intents, hierarchy, vec![]);
        let hp = of_kind(&r, "happy_path_only");
        assert_eq!(hp.len(), 1);
        assert!(hp[0].summary.contains("happy") && hp[0].summary.contains("sad/fallback"));
    }

    #[test]
    fn a_loading_only_state_does_not_trigger() {
        // `loading` is recognized but never a trigger and never required.
        let intents = vec![
            intent("sys", "system"),
            intent("screen", "component"),
            with_aspect("spinner", "loading"),
        ];
        let hierarchy = vec![
            ("sys".to_string(), "screen".to_string()),
            ("screen".to_string(), "spinner".to_string()),
        ];
        let r = report(intents, hierarchy, vec![]);
        assert!(
            of_kind(&r, "happy_path_only").is_empty(),
            "loading alone is not a triggering aspect"
        );
    }

    // happy_path_only is anchored on the newest aspect-tagged child. Re-tagging an
    // EXISTING child's aspect (`loom intent update --aspect`) bumps updated_at but
    // not created_at, so folding updated_at into the anchor is what re-opens a
    // ruling made under the old aspect mix.
    #[test]
    fn a_child_aspect_change_reopens_a_previously_adjudicated_happy_path_only() {
        let ground = "2026-06-15T00:00:00+00:00";
        let ruling = "2026-06-20T00:00:00+00:00";
        let report_at = |child_updated: &str| {
            let mut child = with_aspect("sunny", "happy");
            child.created_at = ground.into();
            child.updated_at = child_updated.into();
            let intents = vec![intent("sys", "system"), intent("feat", "component"), child];
            let hierarchy = vec![
                ("sys".to_string(), "feat".to_string()),
                ("feat".to_string(), "sunny".to_string()),
            ];
            let note = Note {
                id: "n1".into(),
                kind: "decision".into(),
                text: "accepted: the sad/fallback path is intentionally out of scope here".into(),
                author: "llm".into(),
                target_kind: "smell".into(),
                target_id: "happy_path_only:feat".into(),
                resolution: String::new(),
                created_at: ruling.into(),
                audience: String::new(),
            };
            report_with_notes(intents, hierarchy, vec![], &[note])
        };

        // Ruling newer than the newest aspect child's last change → suppressed.
        let settled = report_at("2026-06-19T00:00:00+00:00");
        assert!(
            of_kind(&settled, "happy_path_only").is_empty(),
            "a ruling newer than the aspect mix suppresses the finding"
        );
        assert!(
            settled
                .adjudicated
                .iter()
                .any(|a| a.kind == "happy_path_only"),
            "and it surfaces as adjudicated"
        );

        // Re-tagging the child's aspect after the ruling bumps updated_at past it
        // → the stale ruling no longer holds → the finding re-opens.
        let changed = report_at("2026-06-21T00:00:00+00:00");
        assert_eq!(
            of_kind(&changed, "happy_path_only").len(),
            1,
            "an aspect change after the ruling re-opens the finding (the gap this fix closes)"
        );
    }
}

#[cfg(test)]
mod transitive_layering_tests {
    use super::*;
    use crate::types::{CodeFile, Implements, Intent};

    fn intent(id: &str, layer: &str) -> Intent {
        Intent {
            id: id.into(),
            name: format!("intent {id}"),
            description: String::new(),
            criterion: String::new(),
            abstraction_level: "component".into(),
            domain: String::new(),
            layer: layer.into(),
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
    fn cf(path: &str, imports: &[&str]) -> CodeFile {
        CodeFile {
            id: path.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: String::new(),
            imports: imports.iter().map(|s| s.to_string()).collect(),
            symbols: Vec::new(),
            symbol_facts: Vec::new(),
            content_hash: String::new(),
            extractor_grade: String::new(),
        }
    }
    fn imp(intent: &str, path: &str) -> Implements {
        Implements {
            id: format!("imp:{intent}:{path}"),
            intent_id: intent.into(),
            codefile_id: path.into(),
            intent_name: intent.into(),
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
    fn report(
        intents: Vec<Intent>,
        codefiles: Vec<CodeFile>,
        implements: Vec<Implements>,
        order: &[&str],
    ) -> SmellReport {
        let snapshot = QuerySnapshot::from_parts(
            intents,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            implements,
            codefiles,
            None,
        );
        let order: Vec<String> = order.iter().map(|s| s.to_string()).collect();
        compute_smells_from_parts(
            &snapshot,
            SmellInputs {
                notes: &[],
                vocab_terms: &[],
                layer_order: &order,
                proposed_hypotheses: &[],
                targets: &[],
            },
        )
        .unwrap()
    }
    fn of_kind<'a>(r: &'a SmellReport, kind: &str) -> Vec<&'a Smell> {
        r.open.iter().filter(|s| s.kind == kind).collect()
    }

    // storage → (unlayered) → presentation: each hop is exempt, but storage
    // depending on presentation across the chain violates the order.
    #[test]
    fn a_clean_chain_through_an_unlayered_intermediate_is_flagged() {
        let intents = vec![
            intent("a", "storage"),
            intent("m", ""), // unlayered intermediate hides the violation
            intent("c", "presentation"),
        ];
        let codefiles = vec![
            cf("a.rs", &["m.rs"]),
            cf("m.rs", &["c.rs"]),
            cf("c.rs", &[]),
        ];
        let implements = vec![imp("a", "a.rs"), imp("m", "m.rs"), imp("c", "c.rs")];
        let r = report(
            intents,
            codefiles,
            implements,
            &["presentation", "application", "storage"],
        );
        let found = of_kind(&r, "transitive_layering_violation");
        assert_eq!(
            found.len(),
            1,
            "the masked up-dependency must surface: {found:?}"
        );
        assert!(found[0].summary.contains("intent a") && found[0].summary.contains("intent c"));
        assert!(found[0].evidence.contains("unlayered"));
        // It is NOT a direct layering_violation (no single hop violates).
        assert!(of_kind(&r, "layering_violation").is_empty());
    }

    // All hops point DOWN the order (presentation→application→storage): clean
    // end-to-end, nothing to flag.
    #[test]
    fn a_downward_chain_is_clean() {
        let intents = vec![
            intent("a", "presentation"),
            intent("m", ""),
            intent("c", "storage"),
        ];
        let codefiles = vec![
            cf("a.rs", &["m.rs"]),
            cf("m.rs", &["c.rs"]),
            cf("c.rs", &[]),
        ];
        let implements = vec![imp("a", "a.rs"), imp("m", "m.rs"), imp("c", "c.rs")];
        let r = report(
            intents,
            codefiles,
            implements,
            &["presentation", "application", "storage"],
        );
        assert!(of_kind(&r, "transitive_layering_violation").is_empty());
    }

    // A DIRECT up-dependency is the direct check's job (6b), not the transitive
    // one — the transitive detector must not double-report it.
    #[test]
    fn a_direct_violation_is_not_double_reported_as_transitive() {
        let intents = vec![intent("a", "storage"), intent("c", "presentation")];
        let codefiles = vec![cf("a.rs", &["c.rs"]), cf("c.rs", &[])];
        let implements = vec![imp("a", "a.rs"), imp("c", "c.rs")];
        let r = report(
            intents,
            codefiles,
            implements,
            &["presentation", "application", "storage"],
        );
        assert_eq!(
            of_kind(&r, "layering_violation").len(),
            1,
            "direct check fires"
        );
        assert!(
            of_kind(&r, "transitive_layering_violation").is_empty(),
            "transitive check defers direct pairs to 6b"
        );
    }

    #[test]
    fn passing_architecture_verdict_over_a_layering_violation_is_flagged() {
        let intents = vec![intent("a", "storage"), intent("c", "presentation")];
        let codefiles = vec![cf("a.rs", &["c.rs"]), cf("c.rs", &[])];
        let implements = vec![imp("a", "a.rs"), imp("c", "c.rs")];
        let rule = crate::types::QualityRule {
            id: "r1".into(),
            name: "layered-architecture".into(),
            description: String::new(),
            detection_logic: String::new(),
            kind: "architecture".into(),
            severity: "error".into(),
            inspection_effort: "mid".into(),
            evidence_examples: String::new(),
            signal_expectations: String::new(),
            applies_when: String::new(),
        };
        let governs = vec![crate::types::Governs {
            id: "gov:r1:a".into(),
            rule_id: "r1".into(),
            intent_id: "a".into(),
            rule_name: "layered-architecture".into(),
            intent_name: "intent a".into(),
            inspection_status: "passing".into(),
            criterion: "dependencies point down the layer order".into(),
            confidence: 0.9,
            evidence: "reviewed the imports".into(),
            last_inspected: "t".into(),
            inspected_by: "llm:quality".into(),
            notes: String::new(),
            created_at: "t".into(),
            covers_descendants: String::new(),
        }];
        let order: Vec<String> = ["presentation", "application", "storage"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let snapshot = QuerySnapshot::from_parts(
            intents,
            vec![],
            vec![],
            governs,
            vec![rule],
            vec![],
            vec![],
            implements,
            codefiles,
            None,
        );
        let r = compute_smells_from_parts(
            &snapshot,
            SmellInputs {
                notes: &[],
                vocab_terms: &[],
                layer_order: &order,
                proposed_hypotheses: &[],
                targets: &[],
            },
        )
        .unwrap();
        assert_eq!(
            of_kind(&r, "layering_violation").len(),
            1,
            "the violation fires"
        );
        assert_eq!(
            of_kind(&r, "architecture_verdict_contradicts_layering").len(),
            1,
            "a passing architecture verdict over a layering violation is the contradiction"
        );
    }

    // A layer/boundary edit moves no grounding, so anchoring an adjudication on
    // newest_grounding alone let a ruling keep suppressing a layering smell after
    // the very layer it adjudicated changed. The anchor now folds in the intent's
    // updated_at (bumped by every metadata edit), so re-declaring a layer re-opens
    // a stale ruling for re-adjudication.
    #[test]
    fn a_metadata_change_reopens_a_previously_adjudicated_layering_violation() {
        let ground = "2026-06-15T00:00:00+00:00";
        let ruling = "2026-06-20T00:00:00+00:00";
        let report_at = |a_updated: &str| {
            let mut a = intent("a", "storage");
            a.updated_at = a_updated.into();
            let c = intent("c", "presentation");
            let codefiles = vec![cf("a.rs", &["c.rs"]), cf("c.rs", &[])];
            let mut ia = imp("a", "a.rs");
            ia.created_at = ground.into();
            let mut ic = imp("c", "c.rs");
            ic.created_at = ground.into();
            let note = crate::types::Note {
                id: "n1".into(),
                kind: "decision".into(),
                text: "deliberate: this storage module legitimately reaches the presentation surface"
                    .into(),
                author: "llm:architecture".into(),
                target_kind: "smell".into(),
                target_id: "layering_violation:a".into(),
                resolution: String::new(),
                created_at: ruling.into(),
                audience: String::new(),
            };
            let snapshot = QuerySnapshot::from_parts(
                vec![a, c],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![ia, ic],
                codefiles,
                None,
            );
            let order: Vec<String> = ["presentation", "application", "storage"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            compute_smells_from_parts(
                &snapshot,
                SmellInputs {
                    notes: &[note],
                    vocab_terms: &[],
                    layer_order: &order,
                    proposed_hypotheses: &[],
                    targets: &[],
                },
            )
            .unwrap()
        };

        // Ruling NEWER than the last metadata change → still valid → suppressed.
        let settled = report_at("2026-06-19T00:00:00+00:00");
        assert!(
            !settled.open.iter().any(|s| s.kind == "layering_violation"),
            "a ruling that post-dates the last metadata change suppresses the violation"
        );
        assert!(
            settled
                .adjudicated
                .iter()
                .any(|a| a.kind == "layering_violation"),
            "and it surfaces as adjudicated, not silently gone"
        );

        // A layer re-declaration AFTER the ruling bumps updated_at past it → the
        // ruling no longer holds → the violation re-opens for re-adjudication.
        let changed = report_at("2026-06-21T00:00:00+00:00");
        assert!(
            changed.open.iter().any(|s| s.kind == "layering_violation"),
            "a metadata change after the ruling re-opens the violation (the gap this fix closes)"
        );
        assert!(
            !changed
                .adjudicated
                .iter()
                .any(|a| a.kind == "layering_violation"),
            "the stale ruling no longer adjudicates it"
        );
    }
}
