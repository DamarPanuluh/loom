//! Database query layer, split by concern.
//!
//! Each submodule owns the queries for exactly one node or edge type (plus
//! `row` for shared value extraction, `scoring` for `loom next`, and `stats`
//! for reports). `mod.rs` only wires them together and re-exports a flat API so
//! the rest of the crate keeps importing `crate::db::queries::<fn>` unchanged.
//!
//! Reliability rule that shaped this layer: grafeo 0.5.x cannot reliably
//! match/filter a relationship by its own property. Edges are matched via their
//! endpoint nodes, or scanned and filtered in Rust. See `relates_to` and the
//! project memory `grafeo-relationship-matching`.

mod row;

pub mod codefile;
pub mod completeness;
pub mod delegation;
pub mod governs;
pub mod hierarchy;
pub mod ignore;
pub mod implements;
pub mod integrity;
pub mod intent;
pub mod meta;
pub mod note;
pub mod portability;
pub mod relates_to;
pub mod rule;
pub mod scoring;
pub mod smells;
pub mod stats;
pub mod validates;
pub mod validation;

// Flat re-export: callers use `crate::db::queries::<fn>` regardless of which
// submodule a query lives in. `row` stays internal — only the submodules need
// its helpers (via `super::row`).
pub use codefile::*;
pub use completeness::*;
pub use delegation::*;
pub use governs::*;
pub use hierarchy::*;
pub use ignore::*;
pub use implements::*;
pub use integrity::*;
pub use intent::*;
pub use meta::*;
pub use note::*;
pub use portability::*;
pub use relates_to::*;
pub use rule::*;
pub use scoring::*;
pub use smells::*;
pub use stats::*;
pub use validates::*;
pub use validation::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{GrafeoDb, LoomDb};
    use crate::types::{CodeFile, Ignore, Intent, Note, QualityRule};

    fn intent(id: &str, name: &str) -> Intent {
        Intent {
            id:                id.to_string(),
            name:              name.to_string(),
            description:       "d".to_string(),
            abstraction_level: "feature".to_string(),
            domain:            "test".to_string(),
            source_refs:       "[]".to_string(),
            status:            "proposed".to_string(),
            aspect:            String::new(),
            lifecycle:         "implemented".to_string(),
            created_at:        "t0".to_string(),
            updated_at:        "t0".to_string(),
        }
    }

    fn db_with_intents(n: usize) -> (GrafeoDb, Vec<String>) {
        let db = GrafeoDb::in_memory();
        let ids: Vec<String> = (0..n).map(|i| format!("intent-{i}")).collect();
        for (i, id) in ids.iter().enumerate() {
            insert_intent(&db, &intent(id, &format!("I{i}"))).unwrap();
        }
        (db, ids)
    }

    /// Regression: creating an edge and reading it back used to fail
    /// nondeterministically because relationships were matched by their own
    /// `id` property. Every created edge must be retrievable by id, by
    /// endpoints, and via a full list.
    #[test]
    fn every_created_edge_is_retrievable() {
        let (db, ids) = db_with_intents(8);
        let mut created = Vec::new();
        let mut k = 0;
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                let e = get_or_create_relates_to(&db, &format!("e{k}"), &ids[i], &ids[j], "t").unwrap();
                assert_eq!(e.from_id, ids[i]);
                assert_eq!(e.to_id, ids[j]);
                assert_eq!(e.inspection_status, "uninspected");
                created.push((e.id.clone(), ids[i].clone(), ids[j].clone()));
                k += 1;
            }
        }
        let all = list_relates_to(&db, None).unwrap();
        assert_eq!(all.len(), created.len(), "list lost edges");
        for (eid, from, to) in &created {
            assert!(get_relates_to(&db, eid).unwrap().is_some(), "edge {eid} missing by id");
            assert!(get_relates_to_between(&db, from, to).unwrap().is_some(), "edge {from}->{to} missing by endpoints");
        }
    }

    /// get_or_create is idempotent: re-requesting the same pair returns the
    /// existing edge and never creates a duplicate.
    #[test]
    fn get_or_create_is_idempotent() {
        let (db, ids) = db_with_intents(2);
        let first = get_or_create_relates_to(&db, "e0", &ids[0], &ids[1], "t").unwrap();
        for k in 0..10 {
            let again = get_or_create_relates_to(&db, &format!("e{k}x"), &ids[0], &ids[1], "t").unwrap();
            assert_eq!(again.id, first.id);
        }
        assert_eq!(list_relates_to(&db, None).unwrap().len(), 1);
    }

    /// ground / issue persist the new state and meta, and the status filter
    /// (done in Rust) reflects it.
    #[test]
    fn ground_and_issue_persist() {
        let (db, ids) = db_with_intents(3);
        get_or_create_relates_to(&db, "e0", &ids[0], &ids[1], "t").unwrap();
        get_or_create_relates_to(&db, "e1", &ids[0], &ids[2], "t").unwrap();

        assert!(update_relates_to_ground(&db, &ids[0], &ids[1], "crit", 0.9, "llm", "t").unwrap());
        let e0 = get_relates_to_between(&db, &ids[0], &ids[1]).unwrap().unwrap();
        assert_eq!(e0.inspection_status, "passing");
        assert_eq!(e0.criterion, "crit");
        assert!((e0.confidence - 0.9).abs() < 1e-9);

        assert!(update_relates_to_issue(&db, &ids[0], &ids[2], "c", "ev", 0.9, "llm", "t").unwrap());
        let failing = list_relates_to(&db, Some("failing")).unwrap();
        assert_eq!(failing.len(), 1);
        assert_eq!(failing[0].evidence, "ev");
        assert_eq!(list_relates_to(&db, Some("passing")).unwrap().len(), 1);
    }

    /// independent is a state on RELATES_TO, not a separate edge.
    #[test]
    fn independent_persists() {
        let (db, ids) = db_with_intents(2);
        get_or_create_relates_to(&db, "e0", &ids[0], &ids[1], "t").unwrap();
        assert!(update_relates_to_independent(&db, &ids[0], &ids[1], "unrelated", "llm", "t").unwrap());
        let e = get_relates_to_between(&db, &ids[0], &ids[1]).unwrap().unwrap();
        assert_eq!(e.inspection_status, "independent");
        assert_eq!(e.notes, "unrelated");
    }

    /// fix marks the edge passing and ripples needs_reverification to passing
    /// neighbours that share an endpoint.
    #[test]
    fn fix_edge_ripples_to_neighbours() {
        let (db, ids) = db_with_intents(3);
        get_or_create_relates_to(&db, "e0", &ids[0], &ids[1], "t").unwrap();
        get_or_create_relates_to(&db, "e1", &ids[0], &ids[2], "t").unwrap();
        // e0 is passing (shares node 0 with e1); e1 is failing
        update_relates_to_ground(&db, &ids[0], &ids[1], "c", 0.9, "llm", "t").unwrap();
        update_relates_to_issue(&db, &ids[0], &ids[2], "c", "ev", 0.9, "llm", "t").unwrap();

        let e1 = get_relates_to_between(&db, &ids[0], &ids[2]).unwrap().unwrap();
        assert!(fix_edge(&db, &e1.id, "fixed", "llm:fixer", "t").unwrap());

        assert_eq!(get_relates_to_between(&db, &ids[0], &ids[2]).unwrap().unwrap().inspection_status, "passing");
        assert_eq!(get_relates_to_between(&db, &ids[0], &ids[1]).unwrap().unwrap().inspection_status, "needs_reverification");
    }

    /// Discovery surfaces existing uninspected edges; when none remain it falls
    /// back to unexplored intent pairs.
    #[test]
    fn discovery_seeds_unexplored_pairs() {
        let (db, ids) = db_with_intents(3);
        assert!(scored_candidates(&db, "discovery").unwrap().is_empty());

        let pairs = unexplored_pairs_scored(&db).unwrap();
        assert_eq!(pairs.len(), 3); // C(3,2)
        assert!(pairs.iter().all(|(e, _)| e.inspection_status == "unexplored" && e.id.is_empty()));

        get_or_create_relates_to(&db, "e0", &ids[0], &ids[1], "t").unwrap();
        assert_eq!(unexplored_pairs_scored(&db).unwrap().len(), 2);
        assert_eq!(scored_candidates(&db, "discovery").unwrap().len(), 1);
    }

    // --- Note layer ---

    fn note(id: &str, kind: &str, tk: &str, tid: &str) -> Note {
        Note {
            id:          id.to_string(),
            kind:        kind.to_string(),
            text:        "t".to_string(),
            author:      "llm".to_string(),
            target_kind: tk.to_string(),
            target_id:   tid.to_string(),
            created_at:  "t0".to_string(),
        }
    }

    #[test]
    fn notes_round_trip_and_filter() {
        let db = GrafeoDb::in_memory();
        insert_note(&db, &note("n1", "idea", "none", "")).unwrap();
        insert_note(&db, &note("n2", "justification", "intent", "i1")).unwrap();
        insert_note(&db, &note("n3", "question", "edge", "e1")).unwrap();

        assert_eq!(list_notes(&db, None, None).unwrap().len(), 3);
        assert_eq!(list_notes(&db, Some("i1"), None).unwrap().len(), 1);
        assert_eq!(list_notes(&db, None, Some("idea")).unwrap().len(), 1);
        let on_i1 = notes_for_target(&db, "i1").unwrap();
        assert_eq!(on_i1.len(), 1);
        assert_eq!(on_i1[0].kind, "justification");
        assert_eq!(on_i1[0].text, "t");
    }

    // --- doctor / integrity ---

    /// Init helper that also writes the LoomMeta sentinel doctor expects.
    fn db_inited(n: usize) -> (GrafeoDb, Vec<String>) {
        let (db, ids) = db_with_intents(n);
        db.execute(&crate::db::schema::insert_meta(crate::db::schema::SCHEMA_VERSION, "t", "g-test", "testgraph", "owned"))
            .unwrap();
        (db, ids)
    }

    #[test]
    fn ignore_round_trip() {
        let db = GrafeoDb::in_memory();
        insert_ignore(&db, &Ignore {
            id: "ig1".into(), pattern: "fixtures/**".into(), reason: "fixtures".into(),
            author: "llm".into(), created_at: "t".into(),
        }).unwrap();
        let list = list_ignores(&db).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].pattern, "fixtures/**");
        assert_eq!(list[0].reason, "fixtures");
    }

    #[test]
    fn doctor_passes_on_well_formed_graph() {
        let (db, ids) = db_inited(3);
        get_or_create_relates_to(&db, "e0", &ids[0], &ids[1], "t").unwrap();
        // The criterion must be substantive — doctor audits verdicts for
        // vacuous criteria (the write-time gate enforces the same rule).
        update_relates_to_ground(
            &db, &ids[0], &ids[1], "both intents persist via the same session", 0.9, "llm", "t",
        ).unwrap();
        insert_note(&db, &note("n1", "idea", "intent", &ids[0])).unwrap();
        insert_ignore(&db, &Ignore {
            id: "ig1".into(), pattern: "fixtures/**".into(), reason: "fixtures".into(),
            author: "llm".into(), created_at: "t".into(),
        }).unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(rep.healthy(), "expected healthy, issues: {:?}", rep.issues);
    }

    #[test]
    fn doctor_flags_missing_node_property() {
        let (db, _) = db_inited(0);
        // Raw insert an Intent missing `status` (simulates a query typo).
        db.execute(
            "INSERT (:Intent {id:'bad', name:'x', description:'', abstraction_level:'feature', \
             domain:'', source_refs:'[]', created_at:'t', updated_at:'t'})",
        )
        .unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(
            rep.issues.iter().any(|i| i.contains("missing property 'status'")),
            "issues: {:?}",
            rep.issues
        );
        assert!(!rep.healthy());
    }

    #[test]
    fn doctor_flags_missing_edge_property() {
        let (db, ids) = db_inited(2);
        // Raw RELATES_TO missing `inspection_status`.
        db.execute(&format!(
            "MATCH (a:Intent {{id:'{}'}}),(b:Intent {{id:'{}'}}) \
             INSERT (a)-[:RELATES_TO {{id:'e', criterion:'', confidence:0.0, evidence:'', \
             last_inspected:'', inspected_by:'', priority_score:0.0, notes:'', created_at:'t'}}]->(b)",
            ids[0], ids[1]
        ))
        .unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(
            rep.issues.iter().any(|i| i.contains("missing property 'inspection_status'")),
            "issues: {:?}",
            rep.issues
        );
    }

    #[test]
    fn doctor_flags_invalid_value() {
        let (db, _) = db_inited(0);
        db.execute(
            "INSERT (:Intent {id:'bad', name:'x', description:'', abstraction_level:'feature', \
             domain:'', source_refs:'[]', status:'bogus', created_at:'t', updated_at:'t'})",
        )
        .unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(
            rep.issues.iter().any(|i| i.contains("invalid status")),
            "issues: {:?}",
            rep.issues
        );
    }

    #[test]
    fn doctor_flags_dangling_note() {
        let (db, _) = db_inited(0);
        insert_note(&db, &note("n", "idea", "intent", "ghost")).unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(
            rep.issues.iter().any(|i| i.contains("missing intent")),
            "issues: {:?}",
            rep.issues
        );
    }

    fn codefile(id: &str, path: &str) -> CodeFile {
        CodeFile { id: id.into(), path: path.into(), language: "rust".into(), last_modified: "".into(), imports: "[]".into(), content_hash: "".into() }
    }

    /// IMPLEMENTS is a structural grounding assertion → defaults to `passing`,
    /// not `uninspected` (so it never sits as perpetual unresolved work).
    #[test]
    fn implements_defaults_passing() {
        let (db, ids) = db_with_intents(1);
        db.execute(&crate::db::schema::insert_meta(crate::db::schema::SCHEMA_VERSION, "t", "g-test", "testgraph", "owned")).unwrap();
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, "im", &ids[0], "cf", "fn x", "", "t").unwrap();
        let imps = list_implements_for_intent(&db, &ids[0]).unwrap();
        assert_eq!(imps[0].inspection_status, "passing");
    }

    /// Coherence regression: the status compass must never say "discovery" when
    /// `loom next` has no discovery work — `graph_state` and the next-loop use the
    /// same candidate computation (incl. hierarchy-pair exclusion).
    #[test]
    fn compass_agrees_with_next_when_complete() {
        let (db, ids) = db_inited(2);
        get_or_create_relates_to(&db, "e0", &ids[0], &ids[1], "t").unwrap();
        update_relates_to_ground(&db, &ids[0], &ids[1], "c", 0.9, "llm", "t").unwrap();
        // Both intents are implemented leaves, so BOTH must be grounded for the
        // vertical spine to be complete (the stricter completeness model).
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, "im0", &ids[0], "cf", "fn x", "", "t").unwrap();
        insert_implements(&db, "im1", &ids[1], "cf", "fn y", "", "t").unwrap();

        assert!(scored_candidates(&db, "discovery").unwrap().is_empty(), "next has discovery work");
        assert!(unexplored_pairs_scored(&db).unwrap().is_empty(), "unexplored pairs remain");
        let gs = graph_state(&db).unwrap();
        assert!(gs.vertically_complete, "spine should be complete: {:?}", vertical_completeness(&db).unwrap());
        assert!(gs.horizontally_explored, "grid should be explored");
        assert_eq!(gs.phase, "complete", "compass said '{}' but next is empty", gs.phase);
    }

    /// The stricter completeness model: an implemented leaf intent with no code
    /// is an unrealized gap → vertically incomplete → compass routes to `ground`,
    /// not `complete`. Grounding it (or marking it `planned`) clears the gap.
    #[test]
    fn unrealized_leaf_blocks_vertical_completeness() {
        let (db, ids) = db_inited(2);
        get_or_create_relates_to(&db, "e0", &ids[0], &ids[1], "t").unwrap();
        update_relates_to_ground(&db, &ids[0], &ids[1], "c", 0.9, "llm", "t").unwrap();
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, "im0", &ids[0], "cf", "fn x", "", "t").unwrap();
        // ids[1] is an implemented leaf with no IMPLEMENTS → unrealized.

        let vc = vertical_completeness(&db).unwrap();
        assert_eq!(vc.unrealized_leaves.len(), 1, "{vc:?}");
        assert!(!vc.complete);
        let gs = graph_state(&db).unwrap();
        assert!(!gs.vertically_complete);
        assert_eq!(gs.phase, "ground", "expected ground, got '{}'", gs.phase);

        // Grounding it closes the spine.
        insert_implements(&db, "im1", &ids[1], "cf", "fn y", "", "t").unwrap();
        assert!(vertical_completeness(&db).unwrap().complete);
    }

    /// An orphan CodeFile (no IMPLEMENTS reaches it) breaks the physical→semantic
    /// join and blocks vertical completeness.
    #[test]
    fn unreached_codefile_blocks_vertical_completeness() {
        let (db, ids) = db_inited(1);
        insert_codefile(&db, &codefile("cf0", "src/used.rs")).unwrap();
        insert_codefile(&db, &codefile("cf1", "src/orphan.rs")).unwrap();
        insert_implements(&db, "im", &ids[0], "cf0", "fn x", "", "t").unwrap();

        let vc = vertical_completeness(&db).unwrap();
        assert_eq!(vc.unreached_codefiles, vec!["src/orphan.rs".to_string()], "{vc:?}");
        assert!(!vc.complete);
    }

    /// HIERARCHY is enforced as a tree at insert time: a second parent and a
    /// cycle are both rejected.
    #[test]
    fn hierarchy_enforces_tree_shape() {
        let (db, ids) = db_with_intents(3); // a, b, c
        // a -> b is fine.
        insert_hierarchy(&db, "h0", &ids[0], &ids[1], "", "t").unwrap();
        // a -> b again: duplicate, rejected.
        assert!(insert_hierarchy(&db, "h0d", &ids[0], &ids[1], "", "t").is_err());
        // c -> b: b would get a second parent, rejected.
        assert!(insert_hierarchy(&db, "h1", &ids[2], &ids[1], "", "t").is_err());
        // b -> a: would create a cycle (a is already an ancestor of b), rejected.
        assert!(insert_hierarchy(&db, "h2", &ids[1], &ids[0], "", "t").is_err());
        // b -> c is fine (extends the chain a -> b -> c).
        insert_hierarchy(&db, "h3", &ids[1], &ids[2], "", "t").unwrap();
        // c -> a: would close the cycle a -> b -> c -> a, rejected.
        assert!(insert_hierarchy(&db, "h4", &ids[2], &ids[0], "", "t").is_err());

        let all = list_all_hierarchy(&db).unwrap();
        assert_eq!(all.len(), 2, "only the two valid edges should exist: {all:?}");
        // Tree is well-formed → doctor sees no hierarchy issues.
        db.execute(&crate::db::schema::insert_meta(crate::db::schema::SCHEMA_VERSION, "t", "g-test", "testgraph", "owned")).unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(!rep.issues.iter().any(|i| i.contains("HIERARCHY") || i.contains("parent")), "{:?}", rep.issues);
    }

    /// Non-leaf intents (have children) are realized through their children, so
    /// they don't themselves need IMPLEMENTS — only leaves do.
    #[test]
    fn non_leaf_intents_need_no_direct_grounding() {
        let (db, ids) = db_inited(2); // parent, child
        insert_hierarchy(&db, "h0", &ids[0], &ids[1], "", "t").unwrap();
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        // Ground only the leaf (child); the parent is realized via the child.
        insert_implements(&db, "im", &ids[1], "cf", "fn x", "", "t").unwrap();

        let vc = vertical_completeness(&db).unwrap();
        assert_eq!(vc.roots, 1);
        assert_eq!(vc.leaves, 1);
        assert!(vc.unrealized_leaves.is_empty(), "{vc:?}");
        assert!(vc.complete, "{vc:?}");
    }

    /// Lifecycle/build: planned + needs_change intents become build candidates
    /// (needs_change outranks planned), drive phase=build, and clear when marked
    /// implemented. This is the "known issue / fix-pending" capability.
    #[test]
    fn lifecycle_build_candidates_and_compass() {
        let (db, ids) = db_inited(3);
        assert!(set_intent_lifecycle(&db, &ids[1], "needs_change", "t").unwrap());
        assert!(set_intent_lifecycle(&db, &ids[2], "planned", "t").unwrap());

        let bc = build_candidates(&db).unwrap();
        assert_eq!(bc.len(), 2);
        assert_eq!(bc[0].intent.lifecycle, "needs_change", "needs_change should outrank planned");
        assert_eq!(bc[1].intent.lifecycle, "planned");
        assert_eq!(graph_state(&db).unwrap().phase, "build");

        set_intent_lifecycle(&db, &ids[1], "implemented", "t").unwrap();
        set_intent_lifecycle(&db, &ids[2], "implemented", "t").unwrap();
        assert!(build_candidates(&db).unwrap().is_empty());
        assert_ne!(graph_state(&db).unwrap().phase, "build");
    }

    /// A Validation links to an intent via VALIDATES (the one-step path
    /// `loom validation add --intent` uses), and reads back as that intent's
    /// proof — clearing it from "intents without validations".
    #[test]
    fn validates_link_round_trips() {
        use crate::types::Validation;
        let (db, ids) = db_inited(2);
        let v = Validation {
            id: "v0".into(), name: "smoke".into(), description: String::new(),
            validation_type: "manual_check".into(), command: "true".into(),
            last_run: String::new(), last_result: "not_run".into(),
        };
        insert_validation(&db, &v).unwrap();
        insert_validates(&db, "ve0", "v0", &ids[0], "", "t").unwrap();

        let linked = validations_for_intent(&db, &ids[0]).unwrap();
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].id, "v0");
        // ids[0] now has a validation; ids[1] still doesn't.
        let no_val: Vec<_> = intents_without_validations(&db).unwrap().into_iter().map(|i| i.id).collect();
        assert!(!no_val.contains(&ids[0]));
        assert!(no_val.contains(&ids[1]));
    }

    /// GOVERNS is the green gate: applying a rule defaults to `uninspected`
    /// (green is earned, not assumed), carries a `confidence` field, and once the
    /// vertical spine is complete it drives phase=quality (`loom rule check`).
    #[test]
    fn governs_default_uninspected_drives_quality_phase() {
        let (db, ids) = db_inited(1);
        // Realize the single leaf so the vertical spine is complete and the
        // compass can advance to the quality lane.
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, "im", &ids[0], "cf", "fn x", "", "t").unwrap();
        insert_rule(&db, &QualityRule {
            id: "r0".into(), name: "no_god_objects".into(), description: "d".into(),
            detection_logic: "many concerns in one unit".into(), severity: "warning".into(),
        }).unwrap();
        insert_governs(&db, "g0", "r0", &ids[0], "no god objects", "t").unwrap();

        let gov = list_governs_for_intent(&db, &ids[0]).unwrap();
        assert_eq!(gov.len(), 1);
        assert_eq!(gov[0].inspection_status, "uninspected", "green must be earned");
        assert_eq!(gov[0].confidence, 0.0);
        let gs = graph_state(&db).unwrap();
        assert!(gs.vertically_complete, "spine should be complete: {:?}", vertical_completeness(&db).unwrap());
        assert_eq!(gs.phase, "quality", "uninspected quality gate should drive the quality lane");
    }

    /// Recurrence memory: verdict transitions are auto-recorded as transition
    /// notes, and a target that keeps regressing surfaces as recurrent_trouble.
    #[test]
    fn transition_history_feeds_recurrent_smell() {
        let (db, ids) = db_inited(2);
        get_or_create_relates_to(&db, "e0", &ids[0], &ids[1], "t").unwrap();
        // fail → fix → fail again: two regressions.
        update_relates_to_issue(&db, &ids[0], &ids[1], "criterion long enough", "evidence one", 0.9, "llm:analyzer", "t1").unwrap();
        let e = get_relates_to_between(&db, &ids[0], &ids[1]).unwrap().unwrap();
        fix_edge(&db, &e.id, "patched once", "llm:fixer", "t2").unwrap();
        update_relates_to_issue(&db, &ids[0], &ids[1], "criterion long enough", "evidence two", 0.9, "llm:analyzer", "t3").unwrap();

        let transitions = list_notes(&db, Some(&e.id), Some("transition")).unwrap();
        assert!(transitions.len() >= 3, "every verdict change recorded: {transitions:?}");

        let smells = compute_smells(&db).unwrap();
        let rec: Vec<_> = smells.iter().filter(|s| s.kind == "recurrent_trouble").collect();
        assert_eq!(rec.len(), 1, "{smells:?}");
        assert!(rec[0].summary.contains("regressed 2 times"), "{}", rec[0].summary);
    }

    /// Undeclared coupling: file A imports file B, their owning intents have no
    /// edge → flagged; recording the relationship silences it. The same link
    /// boosts discovery suspicion.
    #[test]
    fn undeclared_coupling_from_imports() {
        let (db, _) = db_inited(0);
        insert_intent(&db, &intent("a", "alpha engine")).unwrap();
        insert_intent(&db, &intent("b", "beta surface")).unwrap();
        insert_codefile(&db, &codefile("cfa", "src/a.rs")).unwrap();
        insert_codefile(&db, &codefile("cfb", "src/b.rs")).unwrap();
        insert_implements(&db, "im1", "a", "cfa", "", "", "t").unwrap();
        insert_implements(&db, "im2", "b", "cfb", "", "", "t").unwrap();
        update_codefile_imports(&db, "cfa", "[\"src/b.rs\"]").unwrap();

        let smells = compute_smells(&db).unwrap();
        assert!(smells.iter().any(|s| s.kind == "undeclared_coupling"
            && s.evidence.contains("src/a.rs → src/b.rs")), "{smells:?}");
        let pairs = unexplored_pairs_scored(&db).unwrap();
        assert!(pairs[0].0.notes.contains("imports each other"), "{}", pairs[0].0.notes);

        get_or_create_relates_to(&db, "e0", "a", "b", "t").unwrap();
        update_relates_to_ground(&db, "a", "b", "alpha calls beta through its public surface", 0.9, "llm:analyzer", "t").unwrap();
        assert!(!compute_smells(&db).unwrap().iter().any(|s| s.kind == "undeclared_coupling"));
    }

    /// Portability: export is deterministic, and an import into a fresh graph
    /// reproduces every node and edge with its meta intact.
    #[test]
    fn export_import_round_trip() {
        use crate::types::Validation;
        let (db, ids) = db_inited(2);
        insert_hierarchy(&db, "h0", &ids[0], &ids[1], "", "t").unwrap();
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        update_codefile_imports(&db, "cf", "[\"src/y.rs\"]").unwrap();
        insert_implements(&db, "im", &ids[1], "cf", "fn x", "", "t").unwrap();
        get_or_create_relates_to(&db, "e0", &ids[0], &ids[1], "t").unwrap();
        update_relates_to_ground(&db, &ids[0], &ids[1], "parent and child coexist by design", 0.8, "llm:analyzer", "t").unwrap();
        insert_rule(&db, &QualityRule {
            id: "r0".into(), name: "no_sql".into(), description: "d".into(),
            detection_logic: "dl".into(), severity: "warning".into(),
        }).unwrap();
        insert_governs(&db, "g0", "r0", &ids[1], "", "t").unwrap();
        insert_validation(&db, &Validation {
            id: "v0".into(), name: "smoke".into(), description: String::new(),
            validation_type: "test".into(), command: "true".into(),
            last_run: String::new(), last_result: "not_run".into(),
        }).unwrap();
        insert_validates(&db, "ve0", "v0", &ids[1], "", "t").unwrap();

        let export = export_graph(&db).unwrap();
        let again = export_graph(&db).unwrap();
        assert_eq!(
            serde_json::to_string(&export).unwrap(),
            serde_json::to_string(&again).unwrap(),
            "export must be deterministic"
        );

        let db2 = GrafeoDb::in_memory();
        let report = import_graph(&db2, &export).unwrap();
        assert!(report.nodes >= 5 && report.edges >= 5, "{report:?}");
        // Spot-check fidelity: verdict meta and imports survive the trip.
        let e = get_relates_to_between(&db2, &ids[0], &ids[1]).unwrap().unwrap();
        assert_eq!(e.inspection_status, "passing");
        assert_eq!(e.criterion, "parent and child coexist by design");
        assert!((e.confidence - 0.8).abs() < 1e-9);
        let cf = list_codefiles(&db2).unwrap();
        assert_eq!(cf[0].imports, "[\"src/y.rs\"]");
        // Re-import into the same graph must refuse (restoration, not merge).
        assert!(import_graph(&db2, &export).is_err());
    }

    /// Name addressing: exact id → exact name → unique fragment; ambiguity and
    /// no-match are errors that teach, never guesses.
    #[test]
    fn resolve_intent_by_name_and_fragment() {
        let (db, _) = db_inited(0);
        insert_intent(&db, &intent("i1", "parsing engine")).unwrap();
        insert_intent(&db, &intent("i2", "rendering surface")).unwrap();
        insert_intent(&db, &intent("i3", "render cache")).unwrap();

        assert_eq!(resolve_intent(&db, "i2").unwrap(), "i2"); // exact id wins
        assert_eq!(resolve_intent(&db, "Parsing Engine").unwrap(), "i1"); // name, case-insensitive
        assert_eq!(resolve_intent(&db, "cache").unwrap(), "i3"); // unique fragment
        let err = resolve_intent(&db, "render").unwrap_err().to_string();
        assert!(err.contains("ambiguous"), "{err}"); // matches i2 and i3
        assert!(resolve_intent(&db, "nonexistent").is_err());
    }

    /// The arithmetic pair count must always agree with the full enumeration —
    /// including when both directions of a pair carry an edge, and when a pair
    /// is linked by HIERARCHY instead of RELATES_TO.
    #[test]
    fn count_unexplored_matches_enumeration() {
        let (db, ids) = db_with_intents(5); // C(5,2) = 10 pairs
        assert_eq!(count_unexplored_pairs(&db).unwrap(), 10);

        get_or_create_relates_to(&db, "e0", &ids[0], &ids[1], "t").unwrap();
        // Reverse direction of the same pair — must not double-count.
        get_or_create_relates_to(&db, "e1", &ids[1], &ids[0], "t").unwrap();
        insert_hierarchy(&db, "h0", &ids[2], &ids[3], "", "t").unwrap();

        let counted = count_unexplored_pairs(&db).unwrap();
        let enumerated = unexplored_pairs_scored(&db).unwrap().len() as i64;
        assert_eq!(counted, enumerated, "cheap count must equal full enumeration");
        assert_eq!(counted, 8); // 10 − {0,1} − {2,3}
    }

    /// `blocked` is a recorded "can't run yet": it leaves the validator queue
    /// (not nagging about work nobody can do), the compass stops routing to it,
    /// and a later code-change sync does NOT flip it back to not_run.
    #[test]
    fn blocked_validation_leaves_queue_and_survives_sync() {
        use crate::types::Validation;
        let (db, ids) = db_inited(1);
        insert_validation(&db, &Validation {
            id: "v0".into(), name: "external smoke".into(), description: String::new(),
            validation_type: "manual_check".into(), command: String::new(),
            last_run: String::new(), last_result: "not_run".into(),
        }).unwrap();
        insert_validates(&db, "ve0", "v0", &ids[0], "", "t").unwrap();
        // not_run → in the validator queue
        assert!(validate_candidates(&db).unwrap().iter().any(|c| c.intent.id == ids[0]));

        update_validation_result(&db, "v0", "blocked", "t1").unwrap();
        set_validates_status_for_validation(&db, "v0", "uninspected", "blocked: needs a live target URL").unwrap();
        // blocked → out of the queue, compass no longer routes to validate
        assert!(!validate_candidates(&db).unwrap().iter().any(|c| c.intent.id == ids[0]));
        assert_ne!(graph_state(&db).unwrap().phase, "validate");

        // a code change doesn't unblock it (and doesn't erase the state)
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, "im", &ids[0], "cf", "", "", "t").unwrap();
        let n = invalidate_validations_for_codefile(&db, "cf").unwrap();
        assert_eq!(n, 0, "blocked proofs are not flipped to not_run");
        assert_eq!(get_validation(&db, "v0").unwrap().unwrap().last_result, "blocked");
    }

    /// `intent source add/remove` — source_refs is editable after creation
    /// (docs and code alike), idempotent on add, honest on a missing remove.
    #[test]
    fn source_refs_add_remove_roundtrip() {
        let (db, ids) = db_with_intents(1);
        let refs = |db: &GrafeoDb| -> Vec<String> {
            serde_json::from_str(&get_intent(db, &ids[0]).unwrap().unwrap().source_refs).unwrap()
        };
        assert!(add_source_ref(&db, &ids[0], "docs/CONTRACT.md", "t1").unwrap());
        assert!(add_source_ref(&db, &ids[0], "src/main.rs", "t2").unwrap());
        assert!(add_source_ref(&db, &ids[0], "docs/CONTRACT.md", "t3").unwrap()); // idempotent
        assert_eq!(refs(&db), vec!["docs/CONTRACT.md".to_string(), "src/main.rs".to_string()]);
        assert_eq!(remove_source_ref(&db, &ids[0], "src/main.rs", "t4").unwrap(), Some(true));
        assert_eq!(remove_source_ref(&db, &ids[0], "src/main.rs", "t5").unwrap(), Some(false));
        assert_eq!(refs(&db), vec!["docs/CONTRACT.md".to_string()]);
        assert!(remove_source_ref(&db, "ghost", "x", "t6").unwrap().is_none());
    }

    /// The content fingerprint round-trips and is the sync change baseline.
    #[test]
    fn content_hash_roundtrip() {
        let (db, _) = db_with_intents(0);
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        assert_eq!(list_codefiles(&db).unwrap()[0].content_hash, "");
        let h = crate::repo::content_hash(b"fn main() {}");
        update_codefile_hash(&db, "cf", &h).unwrap();
        assert_eq!(list_codefiles(&db).unwrap()[0].content_hash, h);
        // deterministic + content-sensitive
        assert_eq!(h, crate::repo::content_hash(b"fn main() {}"));
        assert_ne!(h, crate::repo::content_hash(b"fn main() { }"));
    }

    /// A sync flip explains itself: the transition note names the changed file.
    #[test]
    fn sync_flip_note_names_the_cause() {
        let (db, ids) = db_with_intents(1);
        record_sync_flip(&db, "edge", "e0", "passing", "needs_reverification",
            "src/db/mod.rs changed", "t").unwrap();
        let notes = notes_for_target(&db, "e0").unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, "transition");
        assert!(notes[0].text.contains("(sync: src/db/mod.rs changed)"), "{}", notes[0].text);
        // …and it never reads as a verdict regression to the recurrence smell.
        assert!(!notes[0].text.ends_with("→ failing"));
        let _ = ids;
    }

    /// Doctor catches verdicts that read as inspected without having been
    /// inspected: confidence still 0.0, or no last_inspected timestamp.
    #[test]
    fn doctor_flags_defaulted_verdicts() {
        let (db, ids) = db_inited(2);
        get_or_create_relates_to(&db, "e0", &ids[0], &ids[1], "t").unwrap();
        // Verdict recorded with the 0.0 default — query layer permits it; the
        // command-layer gate normally prevents it; doctor must catch it.
        update_relates_to_ground(&db, &ids[0], &ids[1], "a real, falsifiable criterion",
            0.0, "llm", "t1").unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(rep.issues.iter().any(|i| i.contains("confidence 0.0")), "{:?}", rep.issues);

        // Erase the timestamp behind the verdict → second flavour of the same lie.
        db.execute(&format!(
            "MATCH (a:Intent {{id: '{}'}})-[r:RELATES_TO]->(b:Intent {{id: '{}'}}) \
             SET r.last_inspected = '', r.confidence = 0.9",
            ids[0], ids[1]
        )).unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(rep.issues.iter().any(|i| i.contains("last_inspected is empty")), "{:?}", rep.issues);
    }

    /// Solo-mode provenance is legal but worth a nudge: all-bare verdicts →
    /// hint; a declared role anywhere → no hint.
    #[test]
    fn doctor_hints_solo_mode_provenance() {
        let (db, ids) = db_inited(2);
        get_or_create_relates_to(&db, "e0", &ids[0], &ids[1], "t").unwrap();
        update_relates_to_ground(&db, &ids[0], &ids[1], "a real, falsifiable criterion",
            0.9, "llm", "t1").unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(rep.hints.iter().any(|h| h.contains("solo mode")), "{:?}", rep.hints);
        assert!(rep.healthy(), "hints never fail doctor: {:?}", rep.issues);

        update_relates_to_ground(&db, &ids[0], &ids[1], "a real, falsifiable criterion",
            0.9, "llm:analyzer", "t2").unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(!rep.hints.iter().any(|h| h.contains("solo mode")), "{:?}", rep.hints);
    }

    /// Federation: a graph has an identity that travels with its export, and a
    /// restore ADOPTS it (the imported graph IS that graph, not a new one).
    #[test]
    fn graph_identity_travels_through_export_import() {
        let (db, _) = db_with_intents(1);
        db.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION, "t", "g-grid", "grid", "observed",
        )).unwrap();
        let m = get_meta(&db).unwrap().unwrap();
        assert_eq!((m.graph_id.as_str(), m.graph_name.as_str(), m.custody.as_str()),
                   ("g-grid", "grid", "observed"));
        assert!(m.observed());

        let export = export_graph(&db).unwrap();
        assert_eq!(export["graph_id"], "g-grid");
        assert_eq!(export["graph_name"], "grid");
        assert_eq!(export["custody"], "observed");

        // Fresh init elsewhere gets a placeholder identity; import adopts.
        let db2 = GrafeoDb::in_memory();
        db2.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION, "t", "g-fresh", "fresh", "owned",
        )).unwrap();
        import_graph(&db2, &export).unwrap();
        let m2 = get_meta(&db2).unwrap().unwrap();
        assert_eq!(m2.graph_id, "g-grid", "restore adopts the exported identity");
        assert_eq!(m2.custody, "observed");
    }

    /// The custody gate: an observed graph (someone else's code) rejects
    /// actions that claim building/fixing; an owned graph passes.
    #[test]
    fn custody_gate_blocks_observed_graphs() {
        let (db, _) = db_with_intents(0);
        db.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION, "t", "g-x", "vendor-sdk", "observed",
        )).unwrap();
        let err = ensure_owned(&db, "mark an edge fixed").unwrap_err().to_string();
        assert!(err.contains("OBSERVES"), "{err}");
        assert!(err.contains("vendor-sdk"), "names the graph: {err}");

        let (db2, _) = db_inited(0); // owned
        assert!(ensure_owned(&db2, "anything").is_ok());
    }

    /// Delegations round-trip and travel with the export (they're a node label).
    #[test]
    fn delegation_roundtrip() {
        use crate::types::Delegation;
        let (db, _) = db_inited(0);
        insert_delegation(&db, &Delegation {
            id: "d0".into(), pattern: "services/grid/**".into(),
            target: "services/grid/loom.graph.json".into(),
            author: "llm:builder".into(), created_at: "t".into(),
        }).unwrap();
        let ds = list_delegations(&db).unwrap();
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].pattern, "services/grid/**");
        assert_eq!(ds[0].target, "services/grid/loom.graph.json");
        let export = export_graph(&db).unwrap();
        assert_eq!(export["nodes"]["Delegation"].as_array().unwrap().len(), 1);
    }

    /// A GOVERNS verdict inherits DOWN the hierarchy: a rule held against a
    /// component covers its coded descendants in the unmeasured-intents smell,
    /// so measuring at the honest altitude clears the smell instead of leaving
    /// a wall of per-leaf busywork. Uncovered siblings still flag.
    #[test]
    fn unmeasured_smell_respects_ancestor_verdicts() {
        let (db, ids) = db_with_intents(3); // 0 = component, 1 = its child, 2 = uncovered
        insert_hierarchy(&db, "h0", &ids[0], &ids[1], "", "t").unwrap();
        // All three have code (the smell only measures coded intents).
        for (k, iid) in ids.iter().enumerate() {
            insert_codefile(&db, &codefile(&format!("cf{k}"), &format!("src/f{k}.rs"))).unwrap();
            insert_implements(&db, &format!("im{k}"), iid, &format!("cf{k}"), "", "", "t").unwrap();
        }
        insert_rule(&db, &QualityRule {
            id: "r0".into(), name: "no-eval".into(), description: "d".into(),
            detection_logic: "dl".into(), severity: "error".into(),
        }).unwrap();
        insert_governs(&db, "g0", "r0", &ids[0], "", "t").unwrap();
        update_governs_verdict(
            &db, "r0", &ids[0], "passing", "no dynamic evaluation in this component",
            "workspace clippy denial covers the whole subtree", 0.9, "llm:quality", "t",
        ).unwrap();

        let unmeasured: Vec<_> = compute_smells(&db).unwrap()
            .into_iter()
            .filter(|s| s.kind == "unmeasured_intents")
            .collect();
        assert_eq!(unmeasured.len(), 1, "one rule → one finding");
        let f = &unmeasured[0];
        assert!(f.summary.contains("1 intent(s)"), "child covered by ancestor: {}", f.summary);
        assert!(f.evidence.contains("I2"), "only the uncovered sibling flags: {}", f.evidence);
    }

    /// `loom validation update`: a corrected command resets the proof (the old
    /// result proved a different command); `loom validation delete` removes the
    /// node + edges so the intent is provably unproven again.
    #[test]
    fn validation_update_resets_proof_and_delete_removes_it() {
        use crate::types::Validation;
        let (db, ids) = db_with_intents(1);
        insert_validation(&db, &Validation {
            id: "v0".into(), name: "ledger write".into(), description: String::new(),
            validation_type: "test".into(), command: "cargo test -p wrong-pkg".into(),
            last_run: "t".into(), last_result: "passed".into(),
        }).unwrap();
        insert_validates(&db, "ve0", "v0", &ids[0], "", "t").unwrap();
        set_validates_status_for_validation(&db, "v0", "passing", "ran green").unwrap();

        // The command-layer flow: definition updated, then proof reset.
        assert!(update_validation_definition(&db, "v0", Some("cargo test -p right-pkg"), None).unwrap());
        update_validation_result(&db, "v0", "not_run", "").unwrap();
        set_validates_status_for_validation(&db, "v0", "uninspected", "command updated — proof must be re-run").unwrap();
        let v = get_validation(&db, "v0").unwrap().unwrap();
        assert_eq!(v.command, "cargo test -p right-pkg");
        assert_eq!(v.last_result, "not_run");
        assert_eq!(list_validates_for_intent(&db, &ids[0]).unwrap()[0].inspection_status, "uninspected");
        // …and the intent is back on the validator queue.
        assert!(validate_candidates(&db).unwrap().iter().any(|c| c.intent.id == ids[0]));

        assert!(delete_validation(&db, "v0").unwrap());
        assert!(get_validation(&db, "v0").unwrap().is_none());
        assert!(list_validates_for_intent(&db, &ids[0]).unwrap().is_empty());
        assert!(!delete_validation(&db, "v0").unwrap(), "second delete reports not-found");
    }

    /// `loom validation mark` path: a manual_check with no command can be given a
    /// verdict by hand — node last_result + the per-intent VALIDATES edge both move.
    #[test]
    fn validation_mark_records_manual_verdict() {
        use crate::types::Validation;
        let (db, ids) = db_inited(1);
        insert_validation(&db, &Validation {
            id: "v0".into(), name: "manual smoke".into(), description: String::new(),
            validation_type: "manual_check".into(), command: String::new(),
            last_run: String::new(), last_result: "not_run".into(),
        }).unwrap();
        insert_validates(&db, "ve0", "v0", &ids[0], "", "t").unwrap();

        update_validation_result(&db, "v0", "passed", "t").unwrap();
        let n = set_validates_status_for_validation(&db, "v0", "passing", "checked by hand").unwrap();
        assert_eq!(n, 1);
        assert_eq!(get_validation(&db, "v0").unwrap().unwrap().last_result, "passed");
        let edges = list_validates_for_intent(&db, &ids[0]).unwrap();
        assert_eq!(edges[0].inspection_status, "passing");
        assert_eq!(edges[0].notes, "checked by hand");
        assert_eq!(resolve_validation(&db, "manual smoke").unwrap(), "v0");
    }

    #[test]
    fn doctor_flags_version_mismatch() {
        let (db, _) = db_with_intents(0);
        db.execute(&crate::db::schema::insert_meta("999", "t", "g-test", "testgraph", "owned")).unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(!rep.version_ok, "expected version mismatch");
        assert!(!rep.healthy());
    }

    /// The quality write path: `rule verdict` persists status + criterion +
    /// evidence + provenance on the GOVERNS edge (endpoint-matched), the
    /// quality queue surfaces unearned green and drops it once earned.
    #[test]
    fn governs_verdict_round_trip_and_quality_queue() {
        let (db, ids) = db_inited(1);
        insert_rule(&db, &QualityRule {
            id: "r0".into(), name: "no_god_objects".into(), description: "d".into(),
            detection_logic: "many concerns in one unit".into(), severity: "warning".into(),
        }).unwrap();
        insert_governs(&db, "g0", "r0", &ids[0], "", "t").unwrap();

        // Uninspected GOVERNS is quality work.
        let qc = quality_candidates(&db).unwrap();
        assert_eq!(qc.len(), 1);
        assert_eq!(qc[0].0.inspection_status, "uninspected");

        // No edge between an unknown pair → verdict reports not-found.
        assert!(!update_governs_verdict(
            &db, "r0", "nope", "passing", "criterion text long enough", "evidence", 0.9,
            "llm:quality", "t1",
        ).unwrap());

        // Record the verdict and read it back via a scan.
        assert!(update_governs_verdict(
            &db, "r0", &ids[0], "passing",
            "each module owns exactly one concern",
            "reviewed src/x.rs: single concern per unit",
            0.85, "llm:quality", "t1",
        ).unwrap());
        let g = get_governs_between(&db, "r0", &ids[0]).unwrap().unwrap();
        assert_eq!(g.inspection_status, "passing");
        assert_eq!(g.criterion, "each module owns exactly one concern");
        assert_eq!(g.evidence, "reviewed src/x.rs: single concern per unit");
        assert_eq!(g.inspected_by, "llm:quality");
        assert!((g.confidence - 0.85).abs() < 1e-9);

        // Green earned → off the quality queue.
        assert!(quality_candidates(&db).unwrap().is_empty());
    }

    /// The validator queue: an implemented leaf with no proof surfaces (a parent
    /// does not — children prove it); unrun proofs surface lower than failing
    /// ones; all-passed drops off the queue.
    #[test]
    fn validate_candidates_missing_unrun_failing() {
        use crate::types::Validation;
        let (db, ids) = db_inited(3); // 0: parent, 1: leaf-no-proof, 2: leaf-with-proof
        insert_hierarchy(&db, "h0", &ids[0], &ids[1], "", "t").unwrap();
        insert_hierarchy(&db, "h1", &ids[0], &ids[2], "", "t").unwrap();
        insert_validation(&db, &Validation {
            id: "v0".into(), name: "smoke".into(), description: String::new(),
            validation_type: "test".into(), command: "true".into(),
            last_run: String::new(), last_result: "not_run".into(),
        }).unwrap();
        insert_validates(&db, "ve0", "v0", &ids[2], "", "t").unwrap();

        let vc = validate_candidates(&db).unwrap();
        let by_id: std::collections::HashMap<&str, &ValidateCandidate> =
            vc.iter().map(|c| (c.intent.id.as_str(), c)).collect();
        assert!(by_id.contains_key(ids[1].as_str()), "leaf without proof must surface: {vc:?}");
        assert!(by_id[ids[1].as_str()].reason.contains("no proof"));
        assert!(by_id.contains_key(ids[2].as_str()), "unrun proof must surface");
        assert!(!by_id.contains_key(ids[0].as_str()), "parents are proven via children");

        // Failing proof outranks everything.
        update_validation_result(&db, "v0", "failed", "t1").unwrap();
        let vc = validate_candidates(&db).unwrap();
        assert_eq!(vc[0].intent.id, ids[2]);
        assert!(vc[0].reason.contains("failing"));

        // Passed proof drops the intent off the queue.
        update_validation_result(&db, "v0", "passed", "t2").unwrap();
        let vc = validate_candidates(&db).unwrap();
        assert!(vc.iter().all(|c| c.intent.id != ids[2]), "{vc:?}");
    }

    /// Build altitude: a planned parent never surfaces while a child is still
    /// pending (children first); once all children are implemented it surfaces
    /// as a roll-up, not a code-writing task.
    #[test]
    fn build_queue_defers_planned_parents_until_children_done() {
        let (db, ids) = db_inited(3); // 0: parent, 1+2: children
        insert_hierarchy(&db, "h0", &ids[0], &ids[1], "", "t").unwrap();
        insert_hierarchy(&db, "h1", &ids[0], &ids[2], "", "t").unwrap();
        for id in &ids {
            set_intent_lifecycle(&db, id, "planned", "t").unwrap();
        }

        let bc = build_candidates(&db).unwrap();
        let surfaced: Vec<&str> = bc.iter().map(|c| c.intent.id.as_str()).collect();
        assert!(!surfaced.contains(&ids[0].as_str()), "parent must wait for children: {surfaced:?}");
        assert_eq!(surfaced.len(), 2, "both leaf children queue");
        assert!(bc.iter().all(|c| !c.rollup), "leaves are real build work");

        set_intent_lifecycle(&db, &ids[1], "implemented", "t").unwrap();
        set_intent_lifecycle(&db, &ids[2], "implemented", "t").unwrap();
        let bc = build_candidates(&db).unwrap();
        assert_eq!(bc.len(), 1);
        assert_eq!(bc[0].intent.id, ids[0]);
        assert!(bc[0].rollup, "parent with implemented children is a roll-up");

        // needs_change surfaces at ANY altitude (component refactors are real).
        set_intent_lifecycle(&db, &ids[0], "needs_change", "t").unwrap();
        set_intent_lifecycle(&db, &ids[1], "planned", "t").unwrap();
        let bc = build_candidates(&db).unwrap();
        let surfaced: Vec<&str> = bc.iter().map(|c| c.intent.id.as_str()).collect();
        assert!(surfaced.contains(&ids[0].as_str()), "needs_change parent must surface: {surfaced:?}");
    }

    /// Quality ripple: when code implementing an intent changes, its *passing*
    /// GOVERNS verdicts go needs_reverification (green is re-earned via the
    /// quality queue); failing/uninspected ones are untouched (already open).
    #[test]
    fn governs_ripple_invalidates_passing_verdicts() {
        let (db, ids) = db_inited(1);
        for (rid, name) in [("r0", "no_eval"), ("r1", "no_uncaught")] {
            insert_rule(&db, &QualityRule {
                id: rid.into(), name: name.into(), description: "d".into(),
                detection_logic: "dl".into(), severity: "error".into(),
            }).unwrap();
            insert_governs(&db, &format!("g-{rid}"), rid, &ids[0], "", "t").unwrap();
        }
        update_governs_verdict(
            &db, "r0", &ids[0], "passing", "no dynamic evaluation anywhere",
            "grep: no eval usage", 0.9, "llm:quality", "t",
        ).unwrap();
        update_governs_verdict(
            &db, "r1", &ids[0], "failing", "no uncaught exceptions escape",
            "bare JSON.parse at parser.js:1", 0.9, "llm:quality", "t",
        ).unwrap();

        let flagged = flag_governs_for_intent(&db, &ids[0], "src/x.rs changed", "t2").unwrap();
        assert_eq!(flagged, 1, "only the passing verdict goes stale");
        let g0 = get_governs_between(&db, "r0", &ids[0]).unwrap().unwrap();
        let g1 = get_governs_between(&db, "r1", &ids[0]).unwrap().unwrap();
        assert_eq!(g0.inspection_status, "needs_reverification");
        assert_eq!(g1.inspection_status, "failing", "open work stays open");
        // The stale verdict is back on the quality queue.
        assert!(quality_candidates(&db).unwrap().iter().any(|(g, _)|
            g.rule_id == "r0" && g.inspection_status == "needs_reverification"));
    }

    /// IMPLEMENTS is unique per (intent, codefile) pair — re-grounding the same
    /// pair is a no-op, so endpoint-matched updates stay unambiguous.
    #[test]
    fn insert_implements_is_idempotent_per_pair() {
        let (db, ids) = db_inited(1);
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, "im0", &ids[0], "cf", "fn x", "", "t").unwrap();
        insert_implements(&db, "im1", &ids[0], "cf", "fn y", "other", "t2").unwrap();
        let imps = list_implements_for_intent(&db, &ids[0]).unwrap();
        assert_eq!(imps.len(), 1, "duplicate IMPLEMENTS must not be created");
        assert_eq!(imps[0].id, "im0", "first grounding wins");
    }

    /// delete_implements is the ungrounding half of insert: endpoint-matched,
    /// false when absent, and the intent honestly regresses to unrealized.
    #[test]
    fn delete_implements_ungrounds() {
        let (db, ids) = db_inited(1);
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, "im", &ids[0], "cf", "fn x", "", "t").unwrap();
        assert!(vertical_completeness(&db).unwrap().complete);
        assert!(delete_implements(&db, &ids[0], "cf").unwrap());
        assert!(!delete_implements(&db, &ids[0], "cf").unwrap(), "second delete is a no-op");
        assert!(list_implements_for_intent(&db, &ids[0]).unwrap().is_empty());
        assert!(!vertical_completeness(&db).unwrap().complete);
    }

    /// Removing a CodeFile (phantom after delete/rename on disk) kills its
    /// IMPLEMENTS edges; intents grounded only there become unrealized leaves
    /// again, so vertical completeness regresses honestly.
    #[test]
    fn codefile_remove_unrealizes_intents() {
        let (db, ids) = db_inited(1);
        insert_codefile(&db, &codefile("cf", "src/gone.rs")).unwrap();
        insert_implements(&db, "im", &ids[0], "cf", "fn x", "", "t").unwrap();
        assert!(vertical_completeness(&db).unwrap().complete);

        // By path, then by id for a missing key.
        let removed = delete_codefile(&db, "src/gone.rs").unwrap().unwrap();
        assert_eq!(removed.id, "cf");
        assert!(delete_codefile(&db, "src/gone.rs").unwrap().is_none());

        assert!(list_codefiles(&db).unwrap().is_empty());
        assert!(list_implements_for_intent(&db, &ids[0]).unwrap().is_empty());
        let vc = vertical_completeness(&db).unwrap();
        assert!(!vc.complete, "intent is unrealized again: {vc:?}");
        assert_eq!(vc.unrealized_leaves.len(), 1);
    }

    /// Smells: twin intents (similar wording, no edge), overlapping ownership
    /// (shared file, no edge), unmeasured intents (rule never held against an
    /// intent with code), unused rules — each disappears once the relationship
    /// is recorded or the rule considered.
    #[test]
    fn smells_surface_twins_overlap_and_unmeasured() {
        let (db, _) = db_inited(0);
        // Twins: same level, near-identical wording, no edge.
        insert_intent(&db, &Intent {
            id: "t1".into(), name: "parse markdown input".into(),
            description: "turns markdown text into an AST for rendering".into(),
            ..intent("t1", "x")
        }).unwrap();
        insert_intent(&db, &Intent {
            id: "t2".into(), name: "markdown input parsing".into(),
            description: "turns markdown text into an AST tree".into(),
            ..intent("t2", "x")
        }).unwrap();
        // Overlap: two unrelated intents grounded in the same file.
        insert_intent(&db, &intent("o1", "alpha responsibility")).unwrap();
        insert_intent(&db, &intent("o2", "beta duty")).unwrap();
        insert_codefile(&db, &codefile("cf", "src/shared.rs")).unwrap();
        insert_implements(&db, "im1", "o1", "cf", "", "", "t").unwrap();
        insert_implements(&db, "im2", "o2", "cf", "", "", "t").unwrap();
        // A rule that has never been considered against o1/o2, and an unused one.
        insert_rule(&db, &QualityRule {
            id: "r0".into(), name: "no_panics".into(), description: "d".into(),
            detection_logic: "dl".into(), severity: "error".into(),
        }).unwrap();

        let smells = compute_smells(&db).unwrap();
        let kinds: Vec<&str> = smells.iter().map(|s| s.kind.as_str()).collect();
        assert!(kinds.contains(&"twin_intents"), "{kinds:?}");
        assert!(kinds.contains(&"overlapping_ownership"), "{kinds:?}");
        assert!(kinds.contains(&"unmeasured_intents"), "{kinds:?}");
        assert!(kinds.contains(&"unused_rule"), "{kinds:?}");

        // Recording the relationships/verdicts silences the smells.
        get_or_create_relates_to(&db, "e0", "t1", "t2", "t").unwrap();
        update_relates_to_independent(&db, "t1", "t2", "twin in name only — verified distinct", "llm:analyzer", "t").unwrap();
        get_or_create_relates_to(&db, "e1", "o1", "o2", "t").unwrap();
        insert_governs(&db, "g1", "r0", "o1", "", "t").unwrap();
        insert_governs(&db, "g2", "r0", "o2", "", "t").unwrap();
        update_governs_verdict(&db, "r0", "o2", "independent",
            "panic-freedom criterion does not constrain beta",
            "beta duty has no execution path that can panic", 0.9, "llm:quality", "t").unwrap();
        let kinds: Vec<String> = compute_smells(&db).unwrap().iter().map(|s| s.kind.clone()).collect();
        assert!(!kinds.contains(&"twin_intents".to_string()), "{kinds:?}");
        assert!(!kinds.contains(&"overlapping_ownership".to_string()), "{kinds:?}");
        assert!(!kinds.contains(&"unmeasured_intents".to_string()), "{kinds:?}");
        assert!(!kinds.contains(&"unused_rule".to_string()), "{kinds:?}");
    }

    /// Discovery suspicion: a pair sharing an implemented file outranks an
    /// unrelated pair of equal degree, and the why travels in the notes.
    #[test]
    fn unexplored_pairs_ranked_by_suspicion() {
        let (db, _) = db_inited(0);
        insert_intent(&db, &intent("a", "alpha parsing engine")).unwrap();
        insert_intent(&db, &intent("b", "beta rendering surface")).unwrap();
        insert_intent(&db, &intent("c", "gamma unrelated thing")).unwrap();
        insert_codefile(&db, &codefile("cf", "src/shared.rs")).unwrap();
        insert_implements(&db, "im1", "a", "cf", "", "", "t").unwrap();
        insert_implements(&db, "im2", "b", "cf", "", "", "t").unwrap();

        let pairs = unexplored_pairs_scored(&db).unwrap();
        let top = &pairs[0].0;
        let pair_ids = [top.from_id.as_str(), top.to_id.as_str()];
        assert!(pair_ids.contains(&"a") && pair_ids.contains(&"b"),
            "shared-file pair should rank first: {} × {}", top.from_name, top.to_name);
        assert!(top.notes.contains("share 1 implemented file"), "{}", top.notes);
    }

    /// GOVERNS `independent` = measured, rule does not apply: a valid verdict
    /// that is not quality work and passes doctor (with evidence recorded).
    #[test]
    fn governs_independent_verdict_is_terminal_and_audited() {
        let (db, ids) = db_inited(1);
        insert_rule(&db, &QualityRule {
            id: "r0".into(), name: "no_sql".into(), description: "d".into(),
            detection_logic: "dl".into(), severity: "warning".into(),
        }).unwrap();
        insert_governs(&db, "g0", "r0", &ids[0], "", "t").unwrap();
        update_governs_verdict(&db, "r0", &ids[0], "independent",
            "criterion would be: no raw SQL strings constructed",
            "this intent touches no datastore at all — the rule has no surface here",
            0.9, "llm:quality", "t").unwrap();

        assert!(quality_candidates(&db).unwrap().is_empty(), "independent is not open work");
        let rep = check_graph(&db).unwrap();
        assert!(rep.issues.iter().all(|i| !i.contains("invalid inspection_status")), "{:?}", rep.issues);
        assert!(rep.issues.iter().all(|i| !i.contains("records no why")), "{:?}", rep.issues);
    }

    /// Doctor audits the trust layer: a verdict recorded by an out-of-lane role,
    /// a confidence outside [0,1], and an independence claim with no recorded
    /// why are all integrity issues.
    #[test]
    fn doctor_flags_provenance_and_evidence_violations() {
        let (db, ids) = db_inited(3);
        get_or_create_relates_to(&db, "e0", &ids[0], &ids[1], "t").unwrap();
        // Builder green-lighting its own work + absurd confidence.
        update_relates_to_ground(
            &db, &ids[0], &ids[1], "a perfectly substantive criterion", 7.3, "llm:builder", "t",
        ).unwrap();
        // Independence with no why (empty notes).
        get_or_create_relates_to(&db, "e1", &ids[0], &ids[2], "t").unwrap();
        update_relates_to_independent(&db, &ids[0], &ids[2], "", "llm:analyzer", "t").unwrap();

        let rep = check_graph(&db).unwrap();
        assert!(rep.issues.iter().any(|i| i.contains("out of lane")), "{:?}", rep.issues);
        assert!(rep.issues.iter().any(|i| i.contains("outside [0.0, 1.0]")), "{:?}", rep.issues);
        assert!(rep.issues.iter().any(|i| i.contains("records no why")), "{:?}", rep.issues);
        assert!(!rep.healthy());
    }
}

/// Proves the value-escaping path (`schema::esc` + string interpolation) is
/// safe against injection AND lossless for arbitrary input. This is why loom
/// keeps the readable interpolated queries instead of migrating every query to
/// parameter binding: the escaping path is verified correct here, so a rewrite
/// onto grafeo's separate parameter execution path would add risk without
/// removing any bug. See also `parameterized_queries` for the proven fallback.
#[cfg(test)]
mod escaping {
    use super::*;
    use crate::db::GrafeoDb;
    use crate::types::Intent;

    fn mk(id: &str, desc: &str) -> Intent {
        Intent { id: id.into(), name: "n".into(), description: desc.into(),
            abstraction_level: "feature".into(), domain: "d".into(), source_refs: "[]".into(),
            status: "proposed".into(), aspect: String::new(), lifecycle: "implemented".into(), created_at: "t".into(), updated_at: "t".into() }
    }

    #[test]
    fn esc_round_trips_adversarial_input() {
        let db = GrafeoDb::in_memory();
        let nasty = [
            "O'Brien",
            "back\\slash",            // a literal backslash
            "quote'and\\back",
            "'; MATCH (n) DETACH DELETE n; //",
            "café 日本語 — ünîcödé",
            "tab\tand\nnewline-literal",
            "actual\nnewline:\n<-",
            "",
        ];
        for (k, d) in nasty.iter().enumerate() {
            let id = format!("n{k}");
            insert_intent(&db, &mk(&id, d)).unwrap();
            let got = get_intent(&db, &id).unwrap().expect("must read back");
            assert_eq!(&got.description, d, "round-trip mismatch for input {k:?}");
        }
        // also a real newline byte and a real tab byte
        let id = "real_ctrl";
        let d = "line1
line2	end";
        insert_intent(&db, &mk(id, d)).unwrap();
        let got = get_intent(&db, id).unwrap().unwrap();
        assert_eq!(got.description, d, "real control chars must round-trip");
        println!("ESC_ROUNDTRIP ok for {} adversarial inputs", nasty.len() + 1);
    }
}
/// Proves grafeo's parameter-binding path (`execute_with_params`, `$name`
/// placeholders) is reliable for loom's exact query shapes — node match by
/// param, INSERT, endpoint-keyed SET, endpoint-keyed read. Kept as the
/// documented fallback if the escaping path ever proves insufficient.
#[cfg(test)]
mod parameterized_queries {
    use grafeo::{GrafeoDB, Value};
    use std::collections::HashMap;
    fn p(pairs: &[(&str, &str)]) -> HashMap<String, Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), Value::from(*v))).collect()
    }
    #[test]
    fn params_reliable_for_loom_shapes() {
        let db = GrafeoDB::new_in_memory();
        let s = db.session();
        let n = 8;
        let ids: Vec<String> = (0..n).map(|i| format!("i{i}")).collect();
        for id in &ids {
            s.execute_with_params("INSERT (:Intent {id: $id, status: $st})",
                p(&[("id", id), ("st", "proposed")])).unwrap();
        }
        let (mut k, mut fails) = (0, 0);
        for i in 0..n {
            for j in (i + 1)..n {
                let eid = format!("e{k}"); k += 1;
                s.execute_with_params(
                    "MATCH (a:Intent {id:$from}),(b:Intent {id:$to}) \
                     INSERT (a)-[:RELATES_TO {id:$eid, inspection_status:$st}]->(b)",
                    p(&[("from", &ids[i]), ("to", &ids[j]), ("eid", &eid), ("st", "uninspected")])).unwrap();
                s.execute_with_params(
                    "MATCH (a:Intent {id:$from})-[r:RELATES_TO]->(b:Intent {id:$to}) SET r.inspection_status=$st",
                    p(&[("from", &ids[i]), ("to", &ids[j]), ("st", "passing")])).unwrap();
                let r = s.execute_with_params(
                    "MATCH (a:Intent {id:$from})-[r:RELATES_TO]->(b:Intent {id:$to}) RETURN r.id AS x, r.inspection_status AS st",
                    p(&[("from", &ids[i]), ("to", &ids[j])])).unwrap();
                let ok = r.rows().first().map(|row| matches!(&row[1], Value::String(s) if s.to_string()=="passing")).unwrap_or(false);
                if !ok { fails += 1; }
            }
        }
        println!("PARAM_SPIKE edges={k} fails={fails}");
        assert_eq!(fails, 0, "param path unreliable");
    }
}
