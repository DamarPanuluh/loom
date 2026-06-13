//! Database query layer, split by concern.
//!
//! Each submodule owns the queries for exactly one node or edge type (plus
//! `row` for shared value extraction, `scoring` for `loom next`, and `stats`
//! for reports). `mod.rs` only wires them together and re-exports a flat API so
//! the rest of the crate keeps importing `crate::db::queries::<fn>` unchanged.
//!
//! Reliability rules that shaped this layer (probed — tests/grafeo_probe.rs):
//! edge identity is DERIVED from the endpoint pair (schema v4), so edges are
//! matched via their endpoint nodes; edge-property STATUS filters live in the
//! query (deterministic), but never `WHERE r.id` (that name resolves to the
//! internal edge id in filter position). Free-text values go through $params.
//! See the project memory `grafeo-relationship-matching`.

mod row;

pub mod codefile;
pub mod completeness;
pub mod delegation;
pub mod find;
pub mod governs;
pub mod hierarchy;
pub mod hypothesis;
pub mod ignore;
pub mod implements;
pub mod integrity;
pub mod intent;
pub mod journeys;
pub mod meta;
pub mod note;
pub mod persona;
pub mod portability;
pub mod relates_to;
pub mod rule;
pub mod scoring;
pub mod serves;
pub mod smells;
pub mod snapshot;
pub mod stats;
pub mod symbol_accountability;
pub mod targets;
pub mod validates;
pub mod validation;
pub mod vocab;

// Flat re-export: callers use `crate::db::queries::<fn>` regardless of which
// submodule a query lives in. `row` stays internal — only the submodules need
// its helpers (via `super::row`).
pub use codefile::*;
pub use completeness::*;
pub use delegation::*;
pub use find::*;
pub use governs::*;
pub use hierarchy::*;
pub use hypothesis::*;
pub use ignore::*;
pub use implements::*;
pub use integrity::*;
pub use intent::*;
pub use journeys::*;
pub use meta::*;
pub use note::*;
pub use persona::*;
pub use portability::*;
pub use relates_to::*;
pub use rule::*;
pub use scoring::*;
pub use serves::*;
pub use smells::*;
pub use snapshot::*;
pub use stats::*;
pub use symbol_accountability::*;
pub use targets::*;
pub use validates::*;
pub use validation::*;
pub use vocab::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{GrafeoDb, LoomDb};
    use crate::types::{CodeFile, Ignore, Intent, Note, QualityRule, SymbolFact, Validation};

    fn assert_smell_teaching(s: &Smell) {
        assert!(!s.teaching.principle.trim().is_empty(), "{s:?}");
        assert!(!s.teaching.inspect.is_empty(), "{s:?}");
        assert!(
            s.teaching.inspect.iter().all(|i| !i.trim().is_empty()),
            "{s:?}"
        );
        assert!(!s.teaching.avoid.is_empty(), "{s:?}");
        assert!(
            s.teaching.avoid.iter().all(|i| !i.trim().is_empty()),
            "{s:?}"
        );
        assert!(!s.teaching.done_when.trim().is_empty(), "{s:?}");
    }

    fn assert_adjudicated_teaching(s: &AdjudicatedSmell) {
        assert!(!s.teaching.principle.trim().is_empty(), "{s:?}");
        assert!(!s.teaching.inspect.is_empty(), "{s:?}");
        assert!(
            s.teaching.inspect.iter().all(|i| !i.trim().is_empty()),
            "{s:?}"
        );
        assert!(!s.teaching.avoid.is_empty(), "{s:?}");
        assert!(
            s.teaching.avoid.iter().all(|i| !i.trim().is_empty()),
            "{s:?}"
        );
        assert!(!s.teaching.done_when.trim().is_empty(), "{s:?}");
    }

    fn intent(id: &str, name: &str) -> Intent {
        Intent {
            id: id.to_string(),
            name: name.to_string(),
            description: "d".to_string(),
            abstraction_level: "feature".to_string(),
            domain: "test".to_string(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "proposed".to_string(),
            aspect: String::new(),
            tags: Vec::new(),
            visibility: String::new(),
            lifecycle: "implemented".to_string(),
            created_at: "t0".to_string(),
            updated_at: "t0".to_string(),
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
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                let e = get_or_create_relates_to(&db, &ids[i], &ids[j], "t").unwrap();
                assert_eq!(e.from_id, ids[i]);
                assert_eq!(e.to_id, ids[j]);
                assert_eq!(e.inspection_status, "uninspected");
                created.push((e.id.clone(), ids[i].clone(), ids[j].clone()));
            }
        }
        let all = list_relates_to(&db, None).unwrap();
        assert_eq!(all.len(), created.len(), "list lost edges");
        for (eid, from, to) in &created {
            assert!(
                get_relates_to(&db, eid).unwrap().is_some(),
                "edge {eid} missing by id"
            );
            assert!(
                get_relates_to_between(&db, from, to).unwrap().is_some(),
                "edge {from}->{to} missing by endpoints"
            );
        }
    }

    /// get_or_create is idempotent: re-requesting the same pair returns the
    /// existing edge and never creates a duplicate.
    #[test]
    fn get_or_create_is_idempotent() {
        let (db, ids) = db_with_intents(2);
        let first = get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        for _ in 0..10 {
            let again = get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
            assert_eq!(again.id, first.id);
        }
        assert_eq!(list_relates_to(&db, None).unwrap().len(), 1);
    }

    /// ground / issue persist the new state and meta, and the status filter
    /// (done in Rust) reflects it.
    #[test]
    fn ground_and_issue_persist() {
        let (db, ids) = db_with_intents(3);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        get_or_create_relates_to(&db, &ids[0], &ids[2], "t").unwrap();

        assert!(update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "crit",
            "checked at src/x.rs:1-9",
            0.9,
            "llm",
            "t"
        )
        .unwrap());
        let e0 = get_relates_to_between(&db, &ids[0], &ids[1])
            .unwrap()
            .unwrap();
        assert_eq!(e0.inspection_status, "passing");
        assert_eq!(e0.criterion, "crit");
        assert_eq!(
            e0.evidence, "checked at src/x.rs:1-9",
            "ground records what was found"
        );
        assert!((e0.confidence - 0.9).abs() < 1e-9);

        assert!(
            update_relates_to_issue(&db, &ids[0], &ids[2], "c", "ev", 0.9, "llm", "t").unwrap()
        );
        let failing = list_relates_to(&db, Some("failing")).unwrap();
        assert_eq!(failing.len(), 1);
        assert_eq!(failing[0].evidence, "ev");
        assert_eq!(list_relates_to(&db, Some("passing")).unwrap().len(), 1);

        // Re-grounding a previously-failing edge must not leave the old
        // failure evidence behind the new green (evidence belongs to the
        // verdict that recorded it).
        assert!(update_relates_to_ground(&db, &ids[0], &ids[2], "c", "", 0.9, "llm", "t").unwrap());
        let regrounded = get_relates_to_between(&db, &ids[0], &ids[2])
            .unwrap()
            .unwrap();
        assert_eq!(regrounded.inspection_status, "passing");
        assert_eq!(
            regrounded.evidence, "",
            "stale failing evidence cleared on re-ground"
        );
    }

    /// independent is a state on RELATES_TO, not a separate edge.
    #[test]
    fn independent_persists() {
        let (db, ids) = db_with_intents(2);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        assert!(
            update_relates_to_independent(&db, &ids[0], &ids[1], "unrelated", "llm", "t").unwrap()
        );
        let e = get_relates_to_between(&db, &ids[0], &ids[1])
            .unwrap()
            .unwrap();
        assert_eq!(e.inspection_status, "independent");
        assert_eq!(e.notes, "unrelated");
    }

    /// fix marks the edge passing and ripples needs_reverification to passing
    /// neighbours that share an endpoint.
    #[test]
    fn fix_edge_ripples_to_neighbours() {
        let (db, ids) = db_with_intents(3);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        get_or_create_relates_to(&db, &ids[0], &ids[2], "t").unwrap();
        // e0 is passing (shares node 0 with e1); e1 is failing
        update_relates_to_ground(&db, &ids[0], &ids[1], "c", "", 0.9, "llm", "t").unwrap();
        update_relates_to_issue(&db, &ids[0], &ids[2], "c", "ev", 0.9, "llm", "t").unwrap();

        let e1 = get_relates_to_between(&db, &ids[0], &ids[2])
            .unwrap()
            .unwrap();
        assert!(fix_edge(&db, &e1.id, "fixed", "llm:fixer", "t").unwrap());

        assert_eq!(
            get_relates_to_between(&db, &ids[0], &ids[2])
                .unwrap()
                .unwrap()
                .inspection_status,
            "passing"
        );
        assert_eq!(
            get_relates_to_between(&db, &ids[0], &ids[1])
                .unwrap()
                .unwrap()
                .inspection_status,
            "needs_reverification"
        );
    }

    /// Discovery surfaces existing uninspected edges; when none remain it falls
    /// back to unexplored intent pairs.
    #[test]
    fn discovery_seeds_unexplored_pairs() {
        let (db, ids) = db_with_intents(3);
        assert!(scored_candidates(&db, "discovery").unwrap().is_empty());

        let pairs = unexplored_pairs_scored(&db).unwrap();
        assert_eq!(pairs.len(), 3); // C(3,2)
        assert!(pairs
            .iter()
            .all(|(e, _)| e.inspection_status == "unexplored" && e.id.is_empty()));

        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        assert_eq!(unexplored_pairs_scored(&db).unwrap().len(), 2);
        assert_eq!(scored_candidates(&db, "discovery").unwrap().len(), 1);
    }

    // --- Note layer ---

    fn note(id: &str, kind: &str, tk: &str, tid: &str) -> Note {
        Note {
            id: id.to_string(),
            kind: kind.to_string(),
            text: "t".to_string(),
            author: "llm".to_string(),
            target_kind: tk.to_string(),
            target_id: tid.to_string(),
            audience: String::new(),
            created_at: "t0".to_string(),
        }
    }

    /// `note()` with an explicit timestamp — adjudication tests compare a
    /// decision's created_at against the structure's newest change.
    fn note_at(id: &str, kind: &str, tk: &str, tid: &str, at: &str) -> Note {
        let mut n = note(id, kind, tk, tid);
        n.created_at = at.to_string();
        n
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

    /// `parse_sync_cause` is the read side of `record_sync_flip`'s text
    /// contract — if the formats drift apart, `loom next --take` silently
    /// loses its by-file grouping, so the round-trip is pinned here.
    #[test]
    fn sync_flip_cause_round_trips() {
        let db = GrafeoDb::in_memory();
        record_sync_flip(
            &db,
            "edge",
            "e1",
            "passing",
            "needs_reverification",
            "src/db/mod.rs changed",
            "t1",
        )
        .unwrap();
        let n = &notes_for_target(&db, "e1").unwrap()[0];
        assert_eq!(parse_sync_cause(&n.text), Some("src/db/mod.rs"));
        // Verdict transitions and free-form causes are NOT file groups.
        assert_eq!(parse_sync_cause("passing → failing"), None);
        assert_eq!(
            parse_sync_cause("? → needs_reverification (sync: locator missing)"),
            None
        );
    }

    // --- doctor / integrity ---

    /// Init helper that also writes the LoomMeta sentinel doctor expects.
    fn db_inited(n: usize) -> (GrafeoDb, Vec<String>) {
        let (db, ids) = db_with_intents(n);
        db.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-test",
            "testgraph",
            "owned",
        ))
        .unwrap();
        (db, ids)
    }

    #[test]
    fn ignore_round_trip() {
        let db = GrafeoDb::in_memory();
        insert_ignore(
            &db,
            &Ignore {
                id: "ig1".into(),
                pattern: "fixtures/**".into(),
                reason: "fixtures".into(),
                author: "llm".into(),
                created_at: "t".into(),
            },
        )
        .unwrap();
        let list = list_ignores(&db).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].pattern, "fixtures/**");
        assert_eq!(list[0].reason, "fixtures");
    }

    #[test]
    fn doctor_passes_on_well_formed_graph() {
        let (db, ids) = db_inited(3);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        // The criterion must be substantive — doctor audits verdicts for
        // vacuous criteria (the write-time gate enforces the same rule).
        update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "both intents persist via the same session",
            "",
            0.9,
            "llm",
            "t",
        )
        .unwrap();
        insert_note(&db, &note("n1", "idea", "intent", &ids[0])).unwrap();
        insert_ignore(
            &db,
            &Ignore {
                id: "ig1".into(),
                pattern: "fixtures/**".into(),
                reason: "fixtures".into(),
                author: "llm".into(),
                created_at: "t".into(),
            },
        )
        .unwrap();
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
            rep.issues
                .iter()
                .any(|i| i.contains("missing property 'status'")),
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
            rep.issues
                .iter()
                .any(|i| i.contains("missing property 'inspection_status'")),
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
        CodeFile {
            id: id.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: "".into(),
            imports: Vec::new(),
            symbols: Vec::new(),
            symbol_facts: Vec::new(),
            content_hash: "".into(),
        }
    }

    /// IMPLEMENTS is a structural grounding assertion → defaults to `passing`,
    /// not `uninspected` (so it never sits as perpetual unresolved work).
    #[test]
    fn implements_defaults_passing() {
        let (db, ids) = db_with_intents(1);
        db.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-test",
            "testgraph",
            "owned",
        ))
        .unwrap();
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, &ids[0], "cf", "fn x", "", "t").unwrap();
        let imps = list_implements_for_intent(&db, &ids[0]).unwrap();
        assert_eq!(imps[0].inspection_status, "passing");
    }

    /// Coherence regression: the status compass must never say "discovery" when
    /// `loom next` has no discovery work — `graph_state` and the next-loop use the
    /// same candidate computation (incl. hierarchy-pair exclusion).
    #[test]
    fn compass_agrees_with_next_when_complete() {
        let (db, ids) = db_inited(2);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        update_relates_to_ground(&db, &ids[0], &ids[1], "c", "", 0.9, "llm", "t").unwrap();
        // Both intents are implemented leaves, so BOTH must be grounded for the
        // vertical spine to be complete (the stricter completeness model).
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, &ids[0], "cf", "fn x", "", "t").unwrap();
        insert_implements(&db, &ids[1], "cf", "fn y", "", "t").unwrap();
        insert_vocab_term(&db, &term("core", "complete-graph fixture responsibility")).unwrap();
        for id in &ids {
            tag_intent(&db, id, &["core"]);
        }

        assert!(
            scored_candidates(&db, "discovery").unwrap().is_empty(),
            "next has discovery work"
        );
        assert!(
            unexplored_pairs_scored(&db).unwrap().is_empty(),
            "unexplored pairs remain"
        );
        let gs = graph_state(&db).unwrap();
        assert!(
            gs.vertically_complete,
            "spine should be complete: {:?}",
            vertical_completeness(&db).unwrap()
        );
        assert!(gs.horizontally_explored, "grid should be explored");
        // Unproven implemented leaves route to validate first (handoff order).
        assert_eq!(
            gs.phase, "validate",
            "unproven leaves route to validate, got '{}'",
            gs.phase
        );
        use crate::types::Validation;
        insert_validation(
            &db,
            &Validation {
                id: "v0".into(),
                name: "smoke".into(),
                description: String::new(),
                validation_type: "test".into(),
                command: "true".into(),
                last_run: "t".into(),
                last_result: "passed".into(),
            },
        )
        .unwrap();
        for id in ids.iter() {
            insert_validates(&db, "v0", id, "", "t").unwrap();
        }

        // 360°: an EMPTY normative plane blocks `complete` — coded intents with
        // zero measuring sticks route to quality (seed a pack).
        let gs = graph_state(&db).unwrap();
        assert_eq!(
            gs.phase, "quality",
            "no rules + coded intents should route to quality, got '{}'",
            gs.phase
        );
        assert_eq!(
            gs.coverage.measured_pairs.total, 0,
            "no rules → no measuring surface"
        );

        // Seed one rule and measure BOTH coded intents (verdict creates the
        // edge — the one-command path) → now genuinely complete.
        insert_rule(
            &db,
            &QualityRule {
                id: "r0".into(),
                name: "stick".into(),
                description: "d".into(),
                detection_logic: "dl".into(),
                severity: "warning".into(),
                inspection_effort: String::new(),
            },
        )
        .unwrap();
        let gs = graph_state(&db).unwrap();
        assert_eq!(
            gs.phase, "quality",
            "unmeasured pairs should route to quality"
        );
        assert_eq!(gs.coverage.measured_pairs.total, 2);
        for id in &ids {
            insert_governs(&db, "r0", id, "", "t").unwrap();
            update_governs_verdict(
                &db,
                "r0",
                id,
                "passing",
                "criterion text long enough",
                "evidence text long enough",
                0.9,
                "llm:quality",
                "t",
            )
            .unwrap();
        }
        // Measured + proven + grounded + explored → genuinely complete.
        let gs = graph_state(&db).unwrap();
        assert_eq!(gs.coverage.measured_pairs.covered, 2);
        assert_eq!(
            gs.phase, "complete",
            "compass said '{}' but next is empty",
            gs.phase
        );
    }

    /// The stricter completeness model: an implemented leaf intent with no code
    /// is an unrealized gap → vertically incomplete → compass routes to `ground`,
    /// not `complete`. Grounding it (or marking it `planned`) clears the gap.
    #[test]
    fn unrealized_leaf_blocks_vertical_completeness() {
        let (db, ids) = db_inited(2);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        update_relates_to_ground(&db, &ids[0], &ids[1], "c", "", 0.9, "llm", "t").unwrap();
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, &ids[0], "cf", "fn x", "", "t").unwrap();
        // ids[1] is an implemented leaf with no IMPLEMENTS → unrealized.

        let vc = vertical_completeness(&db).unwrap();
        assert_eq!(vc.unrealized_leaves.len(), 1, "{vc:?}");
        assert!(!vc.complete);
        let gs = graph_state(&db).unwrap();
        assert!(!gs.vertically_complete);
        assert_eq!(gs.phase, "ground", "expected ground, got '{}'", gs.phase);

        // Grounding it closes the spine.
        insert_implements(&db, &ids[1], "cf", "fn y", "", "t").unwrap();
        assert!(vertical_completeness(&db).unwrap().complete);
    }

    /// An orphan CodeFile (no IMPLEMENTS reaches it) breaks the physical→semantic
    /// join and blocks vertical completeness.
    #[test]
    fn unreached_codefile_blocks_vertical_completeness() {
        let (db, ids) = db_inited(1);
        insert_codefile(&db, &codefile("cf0", "src/used.rs")).unwrap();
        insert_codefile(&db, &codefile("cf1", "src/orphan.rs")).unwrap();
        insert_implements(&db, &ids[0], "cf0", "fn x", "", "t").unwrap();

        let vc = vertical_completeness(&db).unwrap();
        assert_eq!(
            vc.unreached_codefiles,
            vec!["src/orphan.rs".to_string()],
            "{vc:?}"
        );
        assert!(!vc.complete);
    }

    /// HIERARCHY is enforced as a tree at insert time: a second parent and a
    /// cycle are both rejected.
    #[test]
    fn hierarchy_enforces_tree_shape() {
        let (db, ids) = db_with_intents(3); // a, b, c
                                            // a -> b is fine.
        insert_hierarchy(&db, &ids[0], &ids[1], "", "t").unwrap();
        // a -> b again: duplicate, rejected.
        assert!(insert_hierarchy(&db, &ids[0], &ids[1], "", "t").is_err());
        // c -> b: b would get a second parent, rejected.
        assert!(insert_hierarchy(&db, &ids[2], &ids[1], "", "t").is_err());
        // b -> a: would create a cycle (a is already an ancestor of b), rejected.
        assert!(insert_hierarchy(&db, &ids[1], &ids[0], "", "t").is_err());
        // b -> c is fine (extends the chain a -> b -> c).
        insert_hierarchy(&db, &ids[1], &ids[2], "", "t").unwrap();
        // c -> a: would close the cycle a -> b -> c -> a, rejected.
        assert!(insert_hierarchy(&db, &ids[2], &ids[0], "", "t").is_err());

        let all = list_all_hierarchy(&db).unwrap();
        assert_eq!(
            all.len(),
            2,
            "only the two valid edges should exist: {all:?}"
        );
        // Tree is well-formed → doctor sees no hierarchy issues.
        db.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-test",
            "testgraph",
            "owned",
        ))
        .unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(
            !rep.issues
                .iter()
                .any(|i| i.contains("HIERARCHY") || i.contains("parent")),
            "{:?}",
            rep.issues
        );
    }

    /// Non-leaf intents (have children) are realized through their children, so
    /// they don't themselves need IMPLEMENTS — only leaves do.
    #[test]
    fn non_leaf_intents_need_no_direct_grounding() {
        let (db, ids) = db_inited(2); // parent, child
        insert_hierarchy(&db, &ids[0], &ids[1], "", "t").unwrap();
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        // Ground only the leaf (child); the parent is realized via the child.
        insert_implements(&db, &ids[1], "cf", "fn x", "", "t").unwrap();

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
        assert_eq!(
            bc[0].intent.lifecycle, "needs_change",
            "needs_change should outrank planned"
        );
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
            id: "v0".into(),
            name: "smoke".into(),
            description: String::new(),
            validation_type: "manual_check".into(),
            command: "true".into(),
            last_run: String::new(),
            last_result: "not_run".into(),
        };
        insert_validation(&db, &v).unwrap();
        insert_validates(&db, "v0", &ids[0], "", "t").unwrap();

        let linked = validations_for_intent(&db, &ids[0]).unwrap();
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].id, "v0");
        // ids[0] now has a validation; ids[1] still doesn't.
        let no_val: Vec<_> = intents_without_validations(&db)
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert!(!no_val.contains(&ids[0]));
        assert!(no_val.contains(&ids[1]));
    }

    /// GOVERNS is the green gate: applying a rule defaults to `uninspected`
    /// (green is earned, not assumed), carries a `confidence` field, and once the
    /// vertical spine is complete it drives phase=quality (`loom rule check`).
    #[test]
    fn governs_default_uninspected_drives_quality_phase() {
        use crate::types::Validation;
        let (db, ids) = db_inited(1);
        // Realize the single leaf so the vertical spine is complete and the
        // compass can advance past ground; prove it so it can advance past
        // validate (missing proof routes there first — handoff order).
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, &ids[0], "cf", "fn x", "", "t").unwrap();
        insert_validation(
            &db,
            &Validation {
                id: "v0".into(),
                name: "smoke".into(),
                description: String::new(),
                validation_type: "test".into(),
                command: "true".into(),
                last_run: "t".into(),
                last_result: "passed".into(),
            },
        )
        .unwrap();
        insert_validates(&db, "v0", &ids[0], "", "t").unwrap();
        insert_rule(
            &db,
            &QualityRule {
                id: "r0".into(),
                name: "no_god_objects".into(),
                description: "d".into(),
                detection_logic: "many concerns in one unit".into(),
                severity: "warning".into(),
                inspection_effort: String::new(),
            },
        )
        .unwrap();
        insert_governs(&db, "r0", &ids[0], "no god objects", "t").unwrap();

        let gov = list_governs_for_intent(&db, &ids[0]).unwrap();
        assert_eq!(gov.len(), 1);
        assert_eq!(
            gov[0].inspection_status, "uninspected",
            "green must be earned"
        );
        assert_eq!(gov[0].confidence, 0.0);
        let gs = graph_state(&db).unwrap();
        assert!(
            gs.vertically_complete,
            "spine should be complete: {:?}",
            vertical_completeness(&db).unwrap()
        );
        assert_eq!(
            gs.phase, "quality",
            "uninspected quality gate should drive the quality lane"
        );
    }

    /// THE COHERENCE INVARIANT: whenever the compass names an actionable
    /// phase, that phase's queue is non-empty — `loom status` may never send
    /// an agent to a `loom next` that answers "nothing to do". Walks one graph
    /// through every phase, asserting the invariant at each step. (The two
    /// historical coherence bugs — phase=validate with an empty validator
    /// queue, and stale GOVERNS driving the queue but not the compass — are
    /// both covered by the walk.)
    #[test]
    fn compass_phase_always_has_a_nonempty_queue() {
        use crate::types::Validation;
        fn assert_coherent(db: &GrafeoDb, step: &str) {
            let gs = graph_state(db).unwrap();
            match gs.phase.as_str() {
                "build" => assert!(
                    !build_candidates(db).unwrap().is_empty(),
                    "[{step}] phase=build but build queue empty"
                ),
                "fix" => assert!(
                    !scored_candidates(db, "fix").unwrap().is_empty(),
                    "[{step}] phase=fix but fix queue empty"
                ),
                "validate" => assert!(
                    !validate_candidates(db).unwrap().is_empty(),
                    "[{step}] phase=validate but validator queue empty"
                ),
                "quality" => {
                    // phase=quality with an empty queue is legal ONLY as the
                    // "normative plane empty — seed a pack" prompt.
                    let q = quality_candidates(db).unwrap();
                    let rules = list_rules(db).unwrap();
                    assert!(
                        !q.is_empty() || rules.is_empty(),
                        "[{step}] phase=quality, rules exist, but quality queue empty"
                    );
                }
                "discovery" => assert!(
                    !scored_candidates(db, "discovery").unwrap().is_empty()
                        || !unexplored_pairs_scored(db).unwrap().is_empty(),
                    "[{step}] phase=discovery but nothing to discover"
                ),
                "audit" => assert!(
                    !compute_smells(db).unwrap().open.is_empty(),
                    "[{step}] phase=audit but no open findings"
                ),
                "complete" => {
                    assert!(
                        build_candidates(db).unwrap().is_empty(),
                        "[{step}] complete with build work"
                    );
                    assert!(
                        scored_candidates(db, "fix").unwrap().is_empty(),
                        "[{step}] complete with fix work"
                    );
                    assert!(
                        validate_candidates(db).unwrap().is_empty(),
                        "[{step}] complete with validate work"
                    );
                    assert!(
                        quality_candidates(db).unwrap().is_empty(),
                        "[{step}] complete with quality work"
                    );
                    assert!(
                        compute_smells(db).unwrap().open.is_empty(),
                        "[{step}] complete with open findings"
                    );
                }
                _ => {}
            }
        }

        let (db, ids) = db_inited(2);
        assert_coherent(&db, "fresh graph");

        // planned → build
        set_intent_lifecycle(&db, &ids[0], "planned", "t").unwrap();
        assert_coherent(&db, "planned intent");
        set_intent_lifecycle(&db, &ids[0], "implemented", "t").unwrap();

        // unrealized leaves → ground (structural; no queue to check) → realize
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, &ids[0], "cf", "fn x", "", "t").unwrap();
        insert_implements(&db, &ids[1], "cf", "fn y", "", "t").unwrap();
        insert_vocab_term(&db, &term("core", "complete-graph fixture responsibility")).unwrap();
        tag_intent(&db, &ids[0], &["core"]);
        tag_intent(&db, &ids[1], &["core"]);
        assert_coherent(&db, "realized, unproven");

        // failing relationship → fix
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        update_relates_to_issue(
            &db,
            &ids[0],
            &ids[1],
            "criterion long enough",
            "evidence long enough",
            0.9,
            "llm:analyzer",
            "t",
        )
        .unwrap();
        assert_coherent(&db, "failing edge");
        update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "criterion long enough",
            "",
            0.9,
            "llm",
            "t",
        )
        .unwrap();

        // unproven leaves → validate
        assert_coherent(&db, "grounded, unproven");
        insert_validation(
            &db,
            &Validation {
                id: "v0".into(),
                name: "smoke".into(),
                description: String::new(),
                validation_type: "test".into(),
                command: "true".into(),
                last_run: "t".into(),
                last_result: "passed".into(),
            },
        )
        .unwrap();
        insert_validates(&db, "v0", &ids[0], "", "t").unwrap();
        insert_validates(&db, "v0", &ids[1], "", "t").unwrap();

        // empty normative plane → quality (seed prompt; queue legally empty)
        assert_coherent(&db, "proven, no rules");

        // unmeasured pairs → quality with a non-empty queue
        insert_rule(
            &db,
            &QualityRule {
                id: "r0".into(),
                name: "stick".into(),
                description: "d".into(),
                detection_logic: "dl".into(),
                severity: "warning".into(),
                inspection_effort: String::new(),
            },
        )
        .unwrap();
        assert_coherent(&db, "unmeasured rule");
        for id in &ids {
            insert_governs(&db, "r0", id, "", "t").unwrap();
            update_governs_verdict(
                &db,
                "r0",
                id,
                "passing",
                "criterion text long enough",
                "evidence text long enough",
                0.9,
                "llm:quality",
                "t",
            )
            .unwrap();
        }

        // stale GOVERNS → quality (historically: queue had it, compass didn't)
        let flagged = flag_governs_for_intent(&db, &ids[0], "src/x.rs changed", "t2").unwrap();
        assert_eq!(flagged, 1);
        let gs = graph_state(&db).unwrap();
        assert_eq!(
            gs.phase, "quality",
            "stale GOVERNS green must drive the compass, got '{}'",
            gs.phase
        );
        assert_coherent(&db, "stale GOVERNS");
        update_governs_verdict(
            &db,
            "r0",
            &ids[0],
            "passing",
            "criterion text long enough",
            "evidence text long enough",
            0.9,
            "llm:quality",
            "t3",
        )
        .unwrap();

        // everything addressed → complete
        let gs = graph_state(&db).unwrap();
        assert_eq!(gs.phase, "complete", "got '{}'", gs.phase);
        assert_coherent(&db, "complete");

        // a third intent on the same file (tangle threshold) → audit: green is
        // gated on zero OPEN findings, not just on empty queues.
        let id2 = "intent-2";
        insert_intent(&db, &intent(id2, "I2")).unwrap();
        get_or_create_relates_to(&db, &ids[0], id2, "t4").unwrap();
        update_relates_to_ground(
            &db,
            &ids[0],
            id2,
            "criterion long enough",
            "",
            0.9,
            "llm",
            "t4",
        )
        .unwrap();
        get_or_create_relates_to(&db, &ids[1], id2, "t4").unwrap();
        update_relates_to_ground(
            &db,
            &ids[1],
            id2,
            "criterion long enough",
            "",
            0.9,
            "llm",
            "t4",
        )
        .unwrap();
        insert_implements(&db, id2, "cf", "fn z", "", "t4").unwrap();
        tag_intent(&db, id2, &["core"]);
        insert_validates(&db, "v0", id2, "", "t4").unwrap();
        insert_governs(&db, "r0", id2, "", "t4").unwrap();
        update_governs_verdict(
            &db,
            "r0",
            id2,
            "passing",
            "criterion text long enough",
            "evidence text long enough",
            0.9,
            "llm:quality",
            "t4",
        )
        .unwrap();
        let gs = graph_state(&db).unwrap();
        assert_eq!(
            gs.phase, "audit",
            "an open finding must gate green, got '{}'",
            gs.phase
        );
        assert_coherent(&db, "open finding");

        // Adjudicate it: a decision note on the FILE, newer than its newest
        // claim — refuting a suspicion is as valuable as fixing it.
        insert_note(&db, &note_at("nd0", "decision", "codefile", "cf", "t9")).unwrap();
        let gs = graph_state(&db).unwrap();
        assert_eq!(
            gs.phase, "complete",
            "adjudicated finding must clear the gate, got '{}'",
            gs.phase
        );
        assert_coherent(&db, "adjudicated complete");

        // The pre-decision plane never gates green, but rotting proposals must
        // surface at the gate everyone reads: the complete message discloses
        // the prove queue (the only push surface the plane has).
        insert_hypothesis(&db, &hypothesis("h-pending", "split the scoring module")).unwrap();
        let gs = graph_state(&db).unwrap();
        assert_eq!(
            gs.phase, "complete",
            "a proposed hypothesis must NOT gate green, got '{}'",
            gs.phase
        );
        assert!(
            gs.next_action.contains("1 proposed hypothesis"),
            "complete message must disclose pending proofs: {}",
            gs.next_action
        );
        assert!(
            gs.next_action.contains("--mode prove"),
            "{}",
            gs.next_action
        );
        assert_coherent(&db, "complete with pending proposal");
    }

    /// Recurrence memory: verdict transitions are auto-recorded as transition
    /// notes, and a target that keeps regressing surfaces as recurrent_trouble.
    #[test]
    fn transition_history_feeds_recurrent_smell() {
        let (db, ids) = db_inited(2);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        // fail → fix → fail again: two regressions.
        update_relates_to_issue(
            &db,
            &ids[0],
            &ids[1],
            "criterion long enough",
            "evidence one",
            0.9,
            "llm:analyzer",
            "t1",
        )
        .unwrap();
        let e = get_relates_to_between(&db, &ids[0], &ids[1])
            .unwrap()
            .unwrap();
        fix_edge(&db, &e.id, "patched once", "llm:fixer", "t2").unwrap();
        update_relates_to_issue(
            &db,
            &ids[0],
            &ids[1],
            "criterion long enough",
            "evidence two",
            0.9,
            "llm:analyzer",
            "t3",
        )
        .unwrap();

        let transitions = list_notes(&db, Some(&e.id), Some("transition")).unwrap();
        assert!(
            transitions.len() >= 3,
            "every verdict change recorded: {transitions:?}"
        );

        let smells = compute_smells(&db).unwrap().open;
        let rec: Vec<_> = smells
            .iter()
            .filter(|s| s.kind == "recurrent_trouble")
            .collect();
        assert_eq!(rec.len(), 1, "{smells:?}");
        assert!(
            rec[0].summary.contains("regressed 2 times"),
            "{}",
            rec[0].summary
        );
        assert!(
            rec[0].evidence.contains("2 transition(s)"),
            "{}",
            rec[0].evidence
        );
        assert!(
            rec[0].evidence.contains("the last at t3"),
            "evidence must carry the last regression timestamp: {}",
            rec[0].evidence
        );
        assert!(
            rec[0]
                .evidence
                .contains("recent regressions: t3 passing → failing by llm:analyzer"),
            "{}",
            rec[0].evidence
        );
        assert!(
            rec[0].evidence.contains(&format!(
                "loom note list --edge {} --kind transition --limit 0",
                e.id
            )),
            "{}",
            rec[0].evidence
        );
        assert_smell_teaching(rec[0]);
        assert!(
            rec[0]
                .teaching
                .principle
                .contains("patching again is suspect"),
            "{:?}",
            rec[0].teaching
        );
        assert!(
            rec[0].teaching.inspect.contains(&format!(
                "loom note list --edge {} --kind transition --limit 0",
                e.id
            )),
            "{:?}",
            rec[0].teaching
        );

        // Terminal state: a decision note NEWER than the last regression marks
        // the recurrence addressed — finding resolves, history stays intact.
        let mut decision = note("nd", "decision", "edge", &e.id);
        decision.text = "redesigned the criterion; root cause was X".into();
        decision.created_at = "t4".into(); // after the t3 regression
        insert_note(&db, &decision).unwrap();
        let report = compute_smells(&db).unwrap();
        let smells = &report.open;
        assert!(
            !smells.iter().any(|s| s.kind == "recurrent_trouble"),
            "a decision newer than the last regression must resolve the finding: {smells:?}"
        );
        let adj = report
            .adjudicated
            .iter()
            .find(|s| s.kind == "recurrent_trouble")
            .expect("resolved recurrent trouble should retain its ruling");
        assert_adjudicated_teaching(adj);
        assert!(
            adj.teaching.inspect.contains(&format!(
                "loom note list --edge {} --kind transition --limit 0",
                e.id
            )),
            "{:?}",
            adj.teaching
        );

        // …but a NEW regression after the decision re-flags it.
        fix_edge(&db, &e.id, "patched again", "llm:fixer", "t5").unwrap();
        update_relates_to_issue(
            &db,
            &ids[0],
            &ids[1],
            "criterion long enough",
            "evidence three",
            0.9,
            "llm:analyzer",
            "t6",
        )
        .unwrap();
        let smells = compute_smells(&db).unwrap().open;
        assert!(
            smells.iter().any(|s| s.kind == "recurrent_trouble"),
            "a regression after the decision must re-flag: {smells:?}"
        );
    }

    /// The smells report discloses high-signal coverage for
    /// duplicated_responsibility: tag collisions are stronger than the lexical
    /// fallback, so callers can show how much coded surface is tagged.
    #[test]
    fn smell_report_counts_tag_coverage() {
        let (db, _) = db_inited(0);
        let mut a = intent("a", "alpha engine");
        a.tags = vec!["authz".into()];
        insert_intent(&db, &a).unwrap();
        let mut b = intent("b", "beta surface");
        b.domain = "storage".into(); // helper default is "test" → two coded domains
        b.layer = "storage".into();
        insert_intent(&db, &b).unwrap();
        insert_intent(&db, &intent("c", "gamma uncoded")).unwrap(); // no code → outside the pair-space
        insert_codefile(&db, &codefile("cfa", "src/a.rs")).unwrap();
        insert_codefile(&db, &codefile("cfb", "src/b.rs")).unwrap();
        insert_implements(&db, "a", "cfa", "", "", "t").unwrap();
        insert_implements(&db, "b", "cfb", "", "", "t").unwrap();

        let report = compute_smells(&db).unwrap();
        assert_eq!(
            report.coded_intents, 2,
            "only intents with IMPLEMENTS count"
        );
        assert_eq!(
            report.tagged_coded_intents, 1,
            "only the tagged coded intent counts"
        );
        assert_eq!(
            report.coded_layers, 1,
            "distinct layers across coded intents"
        );
        assert_eq!(report.declared_layers, 0, "no order declared yet");

        set_layer_order(&db, &["presentation".into(), "storage".into()]).unwrap();
        let report = compute_smells(&db).unwrap();
        assert_eq!(
            report.declared_layers, 2,
            "the report reflects the armed instrument"
        );
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
        insert_implements(&db, "a", "cfa", "", "", "t").unwrap();
        insert_implements(&db, "b", "cfb", "", "", "t").unwrap();
        update_codefile_imports(&db, "cfa", &["src/b.rs".to_string()]).unwrap();

        let smells = compute_smells(&db).unwrap().open;
        assert!(
            smells
                .iter()
                .any(|s| s.kind == "undeclared_coupling"
                    && s.evidence.contains("src/a.rs → src/b.rs")),
            "{smells:?}"
        );
        let pairs = unexplored_pairs_scored(&db).unwrap();
        assert!(
            pairs[0].0.notes.contains("imports each other"),
            "{}",
            pairs[0].0.notes
        );

        get_or_create_relates_to(&db, "a", "b", "t").unwrap();
        update_relates_to_ground(
            &db,
            "a",
            "b",
            "alpha calls beta through its public surface",
            "",
            0.9,
            "llm:analyzer",
            "t",
        )
        .unwrap();
        assert!(!compute_smells(&db)
            .unwrap()
            .open
            .iter()
            .any(|s| s.kind == "undeclared_coupling"));
    }

    /// Portability: export is deterministic, and an import into a fresh graph
    /// reproduces every node and edge with its meta intact.
    #[test]
    fn export_import_round_trip() {
        use crate::types::Validation;
        let (db, ids) = db_inited(2);
        insert_hierarchy(&db, &ids[0], &ids[1], "", "t").unwrap();
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        update_codefile_imports(&db, "cf", &["src/y.rs".to_string()]).unwrap();
        update_codefile_symbols(
            &db,
            "cf",
            &["fn x".to_string(), "struct Worker".to_string()],
        )
        .unwrap();
        update_codefile_symbol_facts(
            &db,
            "cf",
            &[
                SymbolFact {
                    label: "pub fn x".into(),
                    name: "x".into(),
                    kind: "fn".into(),
                    visibility: "public".into(),
                    line_start: 10,
                    line_end: 12,
                    is_test: false,
                },
                SymbolFact {
                    label: "struct Worker".into(),
                    name: "Worker".into(),
                    kind: "struct".into(),
                    visibility: "private".into(),
                    line_start: 20,
                    line_end: 24,
                    is_test: false,
                },
            ],
        )
        .unwrap();
        insert_implements(&db, &ids[1], "cf", "fn x", "", "t").unwrap();
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "parent and child coexist by design",
            "",
            0.8,
            "llm:analyzer",
            "t",
        )
        .unwrap();
        insert_rule(
            &db,
            &QualityRule {
                id: "r0".into(),
                name: "no_sql".into(),
                description: "d".into(),
                detection_logic: "dl".into(),
                severity: "warning".into(),
                inspection_effort: String::new(),
            },
        )
        .unwrap();
        insert_governs(&db, "r0", &ids[1], "", "t").unwrap();
        insert_validation(
            &db,
            &Validation {
                id: "v0".into(),
                name: "smoke".into(),
                description: String::new(),
                validation_type: "test".into(),
                command: "true".into(),
                last_run: String::new(),
                last_result: "not_run".into(),
            },
        )
        .unwrap();
        insert_validates(&db, "v0", &ids[1], "", "t").unwrap();
        set_intent_visibility(&db, &ids[0], "internal", "t2").unwrap();

        let export = export_graph(&db).unwrap();
        let again = export_graph(&db).unwrap();
        assert_eq!(
            serde_json::to_string(&export).unwrap(),
            serde_json::to_string(&again).unwrap(),
            "export must be deterministic"
        );

        let db2 = GrafeoDb::in_memory();
        let report = import_graph(&db2, &export, false).unwrap();
        assert!(report.nodes >= 5 && report.edges >= 5, "{report:?}");
        // Spot-check fidelity: verdict meta and imports survive the trip.
        let e = get_relates_to_between(&db2, &ids[0], &ids[1])
            .unwrap()
            .unwrap();
        assert_eq!(e.inspection_status, "passing");
        assert_eq!(e.criterion, "parent and child coexist by design");
        assert!((e.confidence - 0.8).abs() < 1e-9);
        let cf = list_codefiles(&db2).unwrap();
        assert_eq!(cf[0].imports, vec!["src/y.rs".to_string()]);
        assert_eq!(
            cf[0].symbols,
            vec!["fn x".to_string(), "struct Worker".to_string()]
        );
        assert_eq!(cf[0].symbol_facts.len(), 2);
        assert_eq!(cf[0].symbol_facts[0].label, "pub fn x");
        assert_eq!(cf[0].symbol_facts[0].visibility, "public");
        let i0 = get_intent(&db2, &ids[0]).unwrap().unwrap();
        assert_eq!(
            i0.visibility, "internal",
            "the audience ruling survives the trip"
        );
        // Re-import into the same graph must refuse (restoration, not merge).
        assert!(import_graph(&db2, &export, false).is_err());

        // A v6 export without symbols/facts imports forward; both default empty.
        let mut old = export.clone();
        old["schema_version"] = serde_json::json!("6");
        for cf in old["nodes"]["CodeFile"].as_array_mut().unwrap() {
            cf.as_object_mut().unwrap().remove("symbols");
            cf.as_object_mut().unwrap().remove("symbol_facts");
        }
        let db3 = GrafeoDb::in_memory();
        db3.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-old",
            "old",
            "owned",
        ))
        .unwrap();
        import_graph(&db3, &old, false).unwrap();
        let imported = list_codefiles(&db3).unwrap();
        assert!(imported[0].symbols.is_empty());
        assert!(
            imported[0].symbol_facts.is_empty(),
            "absent symbol facts are additive, not a malformed export"
        );

        // A v7 export may have compact symbols but no rich facts.
        let mut v7 = export.clone();
        v7["schema_version"] = serde_json::json!("7");
        for cf in v7["nodes"]["CodeFile"].as_array_mut().unwrap() {
            cf.as_object_mut().unwrap().remove("symbol_facts");
        }
        let db4 = GrafeoDb::in_memory();
        db4.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-v7",
            "old-v7",
            "owned",
        ))
        .unwrap();
        import_graph(&db4, &v7, false).unwrap();
        let imported = list_codefiles(&db4).unwrap();
        assert_eq!(
            imported[0].symbols,
            vec!["fn x".to_string(), "struct Worker".to_string()]
        );
        assert!(imported[0].symbol_facts.is_empty());
    }

    /// Hard-deleting a node prunes the notes on its edges too (derived keys
    /// embed endpoint ids), and `dangling_notes` finds any pre-existing
    /// orphans for `loom note prune`.
    #[test]
    fn delete_prunes_edge_notes_and_prune_finds_orphans() {
        let (db, ids) = db_inited(2);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        // Grounding records a transition note targeting the edge.
        update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "pair coexists by design here",
            "",
            0.9,
            "llm:analyzer",
            "t",
        )
        .unwrap();
        let key =
            crate::db::schema::edge_key(crate::db::schema::edge::RELATES_TO, &ids[0], &ids[1]);
        assert!(
            !notes_for_target(&db, &key).unwrap().is_empty(),
            "transition note exists"
        );

        // A pre-existing orphan (simulates v3-era damage).
        insert_note(
            &db,
            &note("orphan", "question", "edge", "rt:ghost-a:ghost-b"),
        )
        .unwrap();

        delete_intent(&db, &ids[0]).unwrap();
        assert!(
            notes_for_target(&db, &key).unwrap().is_empty(),
            "edge notes must die with the edge"
        );

        let dangling = dangling_notes(&db).unwrap();
        assert_eq!(dangling.len(), 1, "{dangling:?}");
        assert_eq!(dangling[0].id, "orphan");
        delete_note_by_id(&db, "orphan").unwrap();
        assert!(dangling_notes(&db).unwrap().is_empty());
    }

    /// A v3 export (stored edge uuids; notes referencing them) upgrades in
    /// flight: the legacy edge id is dropped and edge-targeted notes are
    /// remapped to the derived v4 edge key.
    #[test]
    fn import_upgrades_v3_export() {
        let (db, ids) = db_inited(2);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        insert_note(&db, &note("n-edge", "question", "edge", "PLACEHOLDER")).unwrap();
        let mut export = export_graph(&db).unwrap();

        // Rewrite the export back into v3 shape: stored uuid on the edge,
        // note targeting that uuid, list props as JSON-encoded STRINGS.
        export["schema_version"] = serde_json::json!("3");
        for item in export["nodes"]["Intent"].as_array_mut().unwrap() {
            for key in ["source_refs", "tags"] {
                let encoded = serde_json::to_string(&item[key]).unwrap();
                item[key] = serde_json::json!(encoded);
            }
        }
        export["edges"]["RELATES_TO"][0]
            .as_object_mut()
            .unwrap()
            .insert("id".into(), serde_json::json!("legacy-uuid-e0"));
        for item in export["nodes"]["Note"].as_array_mut().unwrap() {
            if item["target_kind"] == "edge" {
                item["target_id"] = serde_json::json!("legacy-uuid-e0");
            }
        }

        let db2 = GrafeoDb::in_memory();
        import_graph(&db2, &export, false).unwrap();
        let derived =
            crate::db::schema::edge_key(crate::db::schema::edge::RELATES_TO, &ids[0], &ids[1]);
        let notes = list_notes(&db2, None, None).unwrap();
        let n = notes.iter().find(|n| n.target_kind == "edge").unwrap();
        assert_eq!(
            n.target_id, derived,
            "edge-targeted note must remap to the derived key"
        );
        // v5 upgrade: stringified list props arrive as native lists.
        let re_export = export_graph(&db2).unwrap();
        assert!(
            re_export["nodes"]["Intent"][0]["source_refs"].is_array(),
            "list props must re-export as real arrays after the upgrade"
        );

        // A version with no upgrade path still rejects loudly.
        export["schema_version"] = serde_json::json!("2");
        let db3 = GrafeoDb::in_memory();
        assert!(import_graph(&db3, &export, false).is_err());
    }

    #[test]
    fn import_rejects_malformed_export_shape() {
        let db = GrafeoDb::in_memory();
        let malformed = serde_json::json!({
            "loom_export": 1,
            "schema_version": crate::db::schema::SCHEMA_VERSION,
            "nodes": {},
            "edges": {},
        });

        let err = import_graph(&db, &malformed, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing `nodes.Intent` array"), "{err}");
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

        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        // Reverse direction of the same pair — must not double-count.
        get_or_create_relates_to(&db, &ids[1], &ids[0], "t").unwrap();
        insert_hierarchy(&db, &ids[2], &ids[3], "", "t").unwrap();

        let counted = count_unexplored_pairs(&db).unwrap();
        let enumerated = unexplored_pairs_scored(&db).unwrap().len() as i64;
        assert_eq!(
            counted, enumerated,
            "cheap count must equal full enumeration"
        );
        assert_eq!(counted, 8); // 10 − {0,1} − {2,3}
    }
    /// The discovery numbers must ADD UP: explored_pairs.total ==
    /// covered + pending(uninspected pairs) + unexplored_pairs. Regression:
    /// hierarchy edges touching RETIRED intents leaked into the axis
    /// denominator (and inspected edges with retired endpoints into covered),
    /// so `loom status --json` reported covered/total/unexplored that
    /// disagreed by exactly the number of retired-touching links.
    #[test]
    fn explored_axis_agrees_with_unexplored_count() {
        let (db, ids) = db_with_intents(4);
        // A hierarchy link and an inspected edge that BOTH touch the soon-
        // retired intent: neither may count anywhere after retirement.
        insert_hierarchy(&db, &ids[0], &ids[3], "", "t").unwrap();
        get_or_create_relates_to(&db, &ids[2], &ids[3], "t").unwrap();
        update_relates_to_ground(&db, &ids[2], &ids[3], "criterion", "", 0.9, "llm", "t").unwrap();
        assert!(retire_intent(&db, &ids[3], "superseded in test", None, "t2").unwrap());

        // Active grid: {0,1,2} → 3 pairs. One covered, one pending, one unexplored.
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        update_relates_to_ground(&db, &ids[0], &ids[1], "criterion", "", 0.9, "llm", "t").unwrap();
        get_or_create_relates_to(&db, &ids[1], &ids[2], "t").unwrap(); // uninspected

        let gs = graph_state(&db).unwrap();
        let ax = &gs.coverage.explored_pairs;
        assert_eq!((ax.covered, ax.total), (1, 3), "active-only grid");
        assert_eq!(
            gs.unexplored_pairs, 1,
            "pair 0×2 is the only unexplored one"
        );
        // The identity an agent reconciles by hand: total = covered + pending + unexplored.
        assert_eq!(ax.total, ax.covered + 1 + gs.unexplored_pairs);
    }

    /// `blocked` is a recorded "can't run yet": it leaves the validator queue
    /// (not nagging about work nobody can do), the compass stops routing to it,
    /// and a later code-change sync does NOT flip it back to not_run.
    #[test]
    fn blocked_validation_leaves_queue_and_survives_sync() {
        use crate::types::Validation;
        let (db, ids) = db_inited(1);
        insert_validation(
            &db,
            &Validation {
                id: "v0".into(),
                name: "external smoke".into(),
                description: String::new(),
                validation_type: "manual_check".into(),
                command: String::new(),
                last_run: String::new(),
                last_result: "not_run".into(),
            },
        )
        .unwrap();
        insert_validates(&db, "v0", &ids[0], "", "t").unwrap();
        // not_run → in the validator queue
        assert!(validate_candidates(&db)
            .unwrap()
            .iter()
            .any(|c| c.intent.id == ids[0]));

        update_validation_result(&db, "v0", "blocked", "t1").unwrap();
        set_validates_status_for_validation(
            &db,
            "v0",
            "uninspected",
            "blocked: needs a live target URL",
        )
        .unwrap();
        // blocked → out of the queue, compass no longer routes to validate
        assert!(!validate_candidates(&db)
            .unwrap()
            .iter()
            .any(|c| c.intent.id == ids[0]));
        assert_ne!(graph_state(&db).unwrap().phase, "validate");

        // a code change doesn't unblock it (and doesn't erase the state)
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, &ids[0], "cf", "", "", "t").unwrap();
        let n = invalidate_validations_for_codefile(&db, "cf").unwrap();
        assert_eq!(n, 0, "blocked proofs are not flipped to not_run");
        assert_eq!(
            get_validation(&db, "v0").unwrap().unwrap().last_result,
            "blocked"
        );
    }

    /// `intent source add/remove` — source_refs is editable after creation
    /// (docs and code alike), idempotent on add, honest on a missing remove.
    #[test]
    fn source_refs_add_remove_roundtrip() {
        let (db, ids) = db_with_intents(1);
        let refs = |db: &GrafeoDb| -> Vec<String> {
            get_intent(db, &ids[0]).unwrap().unwrap().source_refs
        };
        assert!(add_source_ref(&db, &ids[0], "docs/CONTRACT.md", "t1").unwrap());
        assert!(add_source_ref(&db, &ids[0], "src/main.rs", "t2").unwrap());
        assert!(add_source_ref(&db, &ids[0], "docs/CONTRACT.md", "t3").unwrap()); // idempotent
        assert_eq!(
            refs(&db),
            vec!["docs/CONTRACT.md".to_string(), "src/main.rs".to_string()]
        );
        assert_eq!(
            remove_source_ref(&db, &ids[0], "src/main.rs", "t4").unwrap(),
            Some(true)
        );
        assert_eq!(
            remove_source_ref(&db, &ids[0], "src/main.rs", "t5").unwrap(),
            Some(false)
        );
        assert_eq!(refs(&db), vec!["docs/CONTRACT.md".to_string()]);
        assert!(remove_source_ref(&db, "ghost", "x", "t6")
            .unwrap()
            .is_none());
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

    /// CodeFile.symbols is an additive native-list physical fact populated by
    /// sync and preserved in the store.
    #[test]
    fn codefile_symbols_roundtrip() {
        let (db, _) = db_with_intents(0);
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        assert!(list_codefiles(&db).unwrap()[0].symbols.is_empty());
        update_codefile_symbols(
            &db,
            "cf",
            &["class User".to_string(), "function build".to_string()],
        )
        .unwrap();
        assert_eq!(
            list_codefiles(&db).unwrap()[0].symbols,
            vec!["class User".to_string(), "function build".to_string()]
        );
        update_codefile_symbol_facts(
            &db,
            "cf",
            &[SymbolFact {
                label: "export class User".into(),
                name: "User".into(),
                kind: "class".into(),
                visibility: "public".into(),
                line_start: 3,
                line_end: 8,
                is_test: false,
            }],
        )
        .unwrap();
        let cf = list_codefiles(&db).unwrap();
        assert_eq!(cf[0].symbol_facts[0].name, "User");
        assert_eq!(cf[0].symbol_facts[0].line_start, 3);
    }

    /// A sync flip explains itself: the transition note names the changed file.
    #[test]
    fn sync_flip_note_names_the_cause() {
        let (db, ids) = db_with_intents(1);
        record_sync_flip(
            &db,
            "edge",
            "e0",
            "passing",
            "needs_reverification",
            "src/db/mod.rs changed",
            "t",
        )
        .unwrap();
        let notes = notes_for_target(&db, "e0").unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, "transition");
        assert!(
            notes[0].text.contains("(sync: src/db/mod.rs changed)"),
            "{}",
            notes[0].text
        );
        // …and it never reads as a verdict regression to the recurrence smell.
        assert!(!notes[0].text.ends_with("→ failing"));
        let _ = ids;
    }

    /// Doctor catches verdicts that read as inspected without having been
    /// inspected: confidence still 0.0, or no last_inspected timestamp.
    #[test]
    fn doctor_flags_defaulted_verdicts() {
        let (db, ids) = db_inited(2);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        // Verdict recorded with the 0.0 default — query layer permits it; the
        // command-layer gate normally prevents it; doctor must catch it.
        update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "a real, falsifiable criterion",
            "",
            0.0,
            "llm",
            "t1",
        )
        .unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(
            rep.issues.iter().any(|i| i.contains("confidence 0.0")),
            "{:?}",
            rep.issues
        );

        // Erase the timestamp behind the verdict → second flavour of the same lie.
        db.execute(&format!(
            "MATCH (a:Intent {{id: '{}'}})-[r:RELATES_TO]->(b:Intent {{id: '{}'}}) \
             SET r.last_inspected = '', r.confidence = 0.9",
            ids[0], ids[1]
        ))
        .unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(
            rep.issues
                .iter()
                .any(|i| i.contains("last_inspected is empty")),
            "{:?}",
            rep.issues
        );
    }

    /// Solo-mode provenance is legal but worth a nudge: all-bare verdicts →
    /// hint; a declared role anywhere → no hint.
    #[test]
    fn doctor_hints_solo_mode_provenance() {
        let (db, ids) = db_inited(2);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "a real, falsifiable criterion",
            "",
            0.9,
            "llm",
            "t1",
        )
        .unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(
            rep.hints.iter().any(|h| h.contains("solo mode")),
            "{:?}",
            rep.hints
        );
        assert!(rep.healthy(), "hints never fail doctor: {:?}", rep.issues);

        update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "a real, falsifiable criterion",
            "",
            0.9,
            "llm:analyzer",
            "t2",
        )
        .unwrap();
        let rep = check_graph(&db).unwrap();
        assert!(
            !rep.hints.iter().any(|h| h.contains("solo mode")),
            "{:?}",
            rep.hints
        );
    }

    /// Federation: a graph has an identity that travels with its export, and a
    /// restore ADOPTS it (the imported graph IS that graph, not a new one).
    #[test]
    fn graph_identity_travels_through_export_import() {
        let (db, _) = db_with_intents(1);
        db.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-grid",
            "grid",
            "observed",
        ))
        .unwrap();
        let m = get_meta(&db).unwrap().unwrap();
        assert_eq!(
            (
                m.graph_id.as_str(),
                m.graph_name.as_str(),
                m.custody.as_str()
            ),
            ("g-grid", "grid", "observed")
        );
        assert!(m.observed());

        let export = export_graph(&db).unwrap();
        assert_eq!(export["graph_id"], "g-grid");
        assert_eq!(export["graph_name"], "grid");
        assert_eq!(export["custody"], "observed");

        // Fresh init elsewhere gets a placeholder identity; import adopts.
        let db2 = GrafeoDb::in_memory();
        db2.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-fresh",
            "fresh",
            "owned",
        ))
        .unwrap();
        import_graph(&db2, &export, false).unwrap();
        let m2 = get_meta(&db2).unwrap().unwrap();
        assert_eq!(
            m2.graph_id, "g-grid",
            "restore adopts the exported identity"
        );
        assert_eq!(m2.custody, "observed");
    }

    /// PORTING (`loom import --as-planned`): the semantic plane travels — the
    /// physical plane is rebuilt. Intents arrive planned, criteria intact;
    /// codefiles/groundings dropped; verdicts reset to uninspected with
    /// evidence cleared; proofs not_run; identity NOT adopted.
    #[test]
    fn import_as_planned_ports_the_semantic_plane() {
        use crate::types::Validation;
        let (db, ids) = db_inited(2);
        insert_hierarchy(&db, &ids[0], &ids[1], "", "t").unwrap();
        insert_codefile(&db, &codefile("cf", "src/old_lang.rs")).unwrap();
        insert_implements(&db, &ids[1], "cf", "fn old", "", "t").unwrap();
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "parent rolls up the child's contract",
            "",
            0.9,
            "llm",
            "t",
        )
        .unwrap();
        insert_rule(
            &db,
            &QualityRule {
                id: "r0".into(),
                name: "stick".into(),
                description: "d".into(),
                detection_logic: "dl".into(),
                severity: "warning".into(),
                inspection_effort: String::new(),
            },
        )
        .unwrap();
        insert_governs(&db, "r0", &ids[1], "", "t").unwrap();
        update_governs_verdict(
            &db,
            "r0",
            &ids[1],
            "passing",
            "criterion text long enough",
            "evidence from the OLD code",
            0.9,
            "llm:quality",
            "t",
        )
        .unwrap();
        insert_validation(
            &db,
            &Validation {
                id: "v0".into(),
                name: "smoke".into(),
                description: String::new(),
                validation_type: "test".into(),
                command: "cargo test old".into(),
                last_run: "t".into(),
                last_result: "passed".into(),
            },
        )
        .unwrap();
        insert_validates(&db, "v0", &ids[1], "", "t").unwrap();

        let export = export_graph(&db).unwrap();
        let db2 = GrafeoDb::in_memory();
        db2.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-target",
            "target",
            "owned",
        ))
        .unwrap();
        let report = import_graph(&db2, &export, true).unwrap();
        assert!(report.skipped_nodes >= 1, "codefile dropped: {report:?}");
        assert!(report.skipped_edges >= 1, "grounding dropped: {report:?}");

        // Identity stays the TARGET's — a port is a new graph.
        assert_eq!(get_meta(&db2).unwrap().unwrap().graph_id, "g-target");
        // Physical plane gone; semantic plane planned.
        assert!(list_codefiles(&db2).unwrap().is_empty());
        for i in list_intents(&db2, None, None).unwrap() {
            assert_eq!(i.lifecycle, "planned", "{}", i.name);
            assert!(list_implements_for_intent(&db2, &i.id).unwrap().is_empty());
        }
        // The contract travels; the old proof does not.
        let e = get_relates_to_between(&db2, &ids[0], &ids[1])
            .unwrap()
            .unwrap();
        assert_eq!(e.inspection_status, "uninspected");
        assert_eq!(e.criterion, "parent rolls up the child's contract");
        assert!(e.evidence.is_empty(), "old-code evidence must not travel");
        let g = get_governs_between(&db2, "r0", &ids[1]).unwrap().unwrap();
        assert_eq!(g.inspection_status, "uninspected");
        assert!(g.evidence.is_empty());
        let v = get_validation(&db2, "v0").unwrap().unwrap();
        assert_eq!(
            v.last_result, "not_run",
            "the proof is a spec to re-express"
        );
        assert_eq!(
            v.command, "cargo test old",
            "the command text travels as the spec"
        );
        // The port lands as a buildable design: the build queue is full.
        assert!(!build_candidates(&db2).unwrap().is_empty());
    }

    /// THE RETIREMENT CONTRACT: a retired intent is invisible to computation,
    /// visible to history. Fallout is reported (orphans, solely-owned files,
    /// dangling proofs), and every queue/axis/centrality stops counting it —
    /// including the trigger: a file owned only by retired design reads
    /// UNREACHED again.
    #[test]
    fn retire_is_invisible_to_computation_visible_to_history() {
        use crate::types::Validation;
        let (db, ids) = db_inited(3); // 0: parent, 1: child (to retire), 2: sibling
        insert_hierarchy(&db, &ids[0], &ids[1], "", "t").unwrap();
        insert_hierarchy(&db, &ids[0], &ids[2], "", "t").unwrap();
        insert_codefile(&db, &codefile("cf-solo", "src/only_old.rs")).unwrap();
        insert_codefile(&db, &codefile("cf-shared", "src/shared.rs")).unwrap();
        insert_implements(&db, &ids[1], "cf-solo", "fn old", "", "t").unwrap();
        insert_implements(&db, &ids[1], "cf-shared", "fn a", "", "t").unwrap();
        insert_implements(&db, &ids[2], "cf-shared", "fn b", "", "t").unwrap();
        get_or_create_relates_to(&db, &ids[1], &ids[2], "t").unwrap();
        insert_validation(
            &db,
            &Validation {
                id: "v0".into(),
                name: "old-proof".into(),
                description: String::new(),
                validation_type: "test".into(),
                command: "true".into(),
                last_run: String::new(),
                last_result: "not_run".into(),
            },
        )
        .unwrap();
        insert_validates(&db, "v0", &ids[1], "", "t").unwrap();

        // Fallout names exactly the triggered work.
        let f = retire_fallout(&db, &ids[1]).unwrap();
        assert_eq!(f.solely_grounded_files, vec!["src/only_old.rs".to_string()]);
        assert_eq!(f.dangling_validations, vec!["old-proof".to_string()]);
        assert_eq!(f.edges_leaving_computation, 1);
        assert!(f.orphaned_children.is_empty());

        assert!(retire_intent(
            &db,
            &ids[1],
            "superseded by a new decomposition",
            Some(&ids[2]),
            "t2"
        )
        .unwrap());

        // History: node + edges + a decision note naming the successor remain.
        let i = get_intent(&db, &ids[1]).unwrap().unwrap();
        assert_eq!(i.status, "deprecated");
        let notes = list_notes(&db, Some(&ids[1]), Some("decision")).unwrap();
        assert!(
            notes.iter().any(|n| n.text.contains("replaced by")),
            "{notes:?}"
        );

        // Computation: the retired intent is gone from every number.
        assert!(list_active_intents(&db)
            .unwrap()
            .iter()
            .all(|i| i.id != ids[1]));
        assert!(
            scored_candidates(&db, "discovery")
                .unwrap()
                .iter()
                .all(|(e, _)| e.from_id != ids[1] && e.to_id != ids[1]),
            "queues drop its edges"
        );
        assert!(
            !all_intent_degrees(&db).unwrap().contains_key(&ids[1]),
            "centrality drops it"
        );
        assert!(
            validate_selection(&db)
                .unwrap()
                .iter()
                .all(|(i, _, _)| i.id != ids[1]),
            "its proofs stop nagging the validator"
        );
        let vc = vertical_completeness(&db).unwrap();
        assert!(
            vc.unreached_codefiles
                .contains(&"src/only_old.rs".to_string()),
            "the solely-owned file surfaces as a gap: {vc:?}"
        );
        assert!(
            !vc.unreached_codefiles
                .contains(&"src/shared.rs".to_string()),
            "the shared file stays reached via the sibling"
        );
        let gs = graph_state(&db).unwrap();
        assert_eq!(gs.intents, 2, "pulse counts active intents only");
    }

    /// Redefinition is the semantic twin of the sync ripple: every verdict
    /// earned against the OLD wording goes stale — including the IMPLEMENTS
    /// grounding (code is byte-identical, but "does it do what this says?"
    /// changed meaning) and `independent` claims (verified absence was judged
    /// against the old meaning too). Blocked proofs keep their reason.
    #[test]
    fn redefinition_ripples_one_hop() {
        use crate::types::Validation;
        let (db, ids) = db_with_intents(3);
        // RELATES_TO: one passing, one independent — both flip.
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "criterion long enough",
            "",
            0.9,
            "llm",
            "t",
        )
        .unwrap();
        get_or_create_relates_to(&db, &ids[0], &ids[2], "t").unwrap();
        update_relates_to_independent(
            &db,
            &ids[0],
            &ids[2],
            "verified: nothing shared",
            "llm",
            "t",
        )
        .unwrap();
        // IMPLEMENTS: passing by construction.
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, &ids[0], "cf", "fn x", "", "t").unwrap();
        // GOVERNS: passing verdict.
        insert_rule(
            &db,
            &QualityRule {
                id: "r0".into(),
                name: "stick".into(),
                description: "d".into(),
                detection_logic: "dl".into(),
                severity: "warning".into(),
                inspection_effort: String::new(),
            },
        )
        .unwrap();
        insert_governs(&db, "r0", &ids[0], "", "t").unwrap();
        update_governs_verdict(
            &db,
            "r0",
            &ids[0],
            "passing",
            "criterion text long enough",
            "evidence text long enough",
            0.9,
            "llm:quality",
            "t",
        )
        .unwrap();
        // Proofs: one passed (flips), one blocked (keeps its reason).
        insert_validation(
            &db,
            &Validation {
                id: "v0".into(),
                name: "proof".into(),
                description: String::new(),
                validation_type: "test".into(),
                command: "true".into(),
                last_run: "t".into(),
                last_result: "passed".into(),
            },
        )
        .unwrap();
        insert_validates(&db, "v0", &ids[0], "", "t").unwrap();
        insert_validation(
            &db,
            &Validation {
                id: "v1".into(),
                name: "blocked-proof".into(),
                description: String::new(),
                validation_type: "manual_check".into(),
                command: String::new(),
                last_run: String::new(),
                last_result: "blocked".into(),
            },
        )
        .unwrap();
        insert_validates(&db, "v1", &ids[0], "", "t").unwrap();

        assert!(update_intent_meaning(
            &db,
            &ids[0],
            None,
            Some("routing now includes host matching"),
            "t2"
        )
        .unwrap());
        let r = ripple_intent_redefinition(&db, &ids[0], "I0", "t2").unwrap();

        assert_eq!(
            get_intent(&db, &ids[0]).unwrap().unwrap().description,
            "routing now includes host matching"
        );
        assert_eq!(r.relates_to_flagged, 2, "passing AND independent flip");
        assert_eq!(r.implements_flagged, 1);
        assert_eq!(r.governs_flagged, 1);
        assert_eq!(r.validations_invalidated, 1, "blocked proof untouched");
        assert_eq!(
            get_relates_to_between(&db, &ids[0], &ids[1])
                .unwrap()
                .unwrap()
                .inspection_status,
            "needs_reverification"
        );
        assert_eq!(
            get_relates_to_between(&db, &ids[0], &ids[2])
                .unwrap()
                .unwrap()
                .inspection_status,
            "needs_reverification"
        );
        assert_eq!(
            list_implements_for_intent(&db, &ids[0]).unwrap()[0].inspection_status,
            "needs_reverification"
        );
        assert_eq!(
            list_governs_for_intent(&db, &ids[0]).unwrap()[0].inspection_status,
            "needs_reverification"
        );
        assert_eq!(
            get_validation(&db, "v0").unwrap().unwrap().last_result,
            "not_run"
        );
        assert_eq!(
            get_validation(&db, "v1").unwrap().unwrap().last_result,
            "blocked"
        );
        // The flip notes carry the redefinition cause, but never pollute the
        // hot-FILE grouping (`parse_sync_cause` is for "<path> changed" only).
        let edge_id = get_relates_to_between(&db, &ids[0], &ids[1])
            .unwrap()
            .unwrap()
            .id;
        let n = &notes_for_target(&db, &edge_id).unwrap().pop().unwrap();
        assert!(n.text.contains("intent 'I0' redefined"), "{}", n.text);
        assert_eq!(parse_sync_cause(&n.text), None);
        // A neighbour's OWN claims are untouched (one hop, not transitive).
        let d1 = get_intent(&db, &ids[1]).unwrap().unwrap();
        assert_eq!(d1.description, "d", "neighbour intents are not rewritten");
    }

    /// Confirmation is an append-only freshness EVENT: the newest confirm note
    /// is the stamp the align queue ranks by; never confirmed = None.
    #[test]
    fn confirm_stamps_are_append_only_freshness() {
        let (db, ids) = db_with_intents(1);
        assert_eq!(last_confirmed_at(&db, &ids[0]).unwrap(), None);
        record_confirmation(&db, &ids[0], "human", "t1").unwrap();
        assert_eq!(
            last_confirmed_at(&db, &ids[0]).unwrap().as_deref(),
            Some("t1")
        );
        record_confirmation(&db, &ids[0], "human", "t3").unwrap();
        assert_eq!(
            last_confirmed_at(&db, &ids[0]).unwrap().as_deref(),
            Some("t3"),
            "newest stamp wins"
        );
        // Both events remain — alignment history, not a mutable field.
        assert_eq!(
            list_notes(&db, Some(&ids[0]), Some("confirm"))
                .unwrap()
                .len(),
            2
        );
    }

    /// Centrality counts REAL relationships only: `independent` edges give the
    /// grid closure but contribute nothing to blast radius.
    #[test]
    fn degree_excludes_independent_edges() {
        let (db, ids) = db_inited(3);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "criterion long enough",
            "",
            0.9,
            "llm",
            "t",
        )
        .unwrap();
        get_or_create_relates_to(&db, &ids[0], &ids[2], "t").unwrap();
        update_relates_to_independent(
            &db,
            &ids[0],
            &ids[2],
            "verified: no shared surface at all between these",
            "llm",
            "t",
        )
        .unwrap();

        let d = all_intent_degrees(&db).unwrap();
        assert_eq!(
            *d.get(&ids[0]).unwrap_or(&0),
            1,
            "independent edge must not count"
        );
        assert_eq!(*d.get(&ids[1]).unwrap_or(&0), 1);
        assert!(
            !d.contains_key(&ids[2]),
            "only an independent edge → zero centrality"
        );
    }

    /// The review queue — confidence is the coordination channel between
    /// tiers: an honest low-confidence verdict surfaces for re-inspection,
    /// ranked by (1−conf)×centrality; re-recording at/above the threshold
    /// resolves it. Both RELATES_TO and GOVERNS verdicts participate.
    #[test]
    fn low_confidence_verdicts_feed_the_review_queue() {
        let (db, ids) = db_inited(2);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        // A scout grounds with HONEST uncertainty.
        update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "names overlap but the call path was not traced",
            "",
            0.45,
            "llm:analyzer",
            "t",
        )
        .unwrap();
        insert_rule(
            &db,
            &QualityRule {
                id: "r0".into(),
                name: "stick".into(),
                description: "d".into(),
                detection_logic: "dl".into(),
                severity: "warning".into(),
                inspection_effort: "high".into(),
            },
        )
        .unwrap();
        insert_governs(&db, "r0", &ids[0], "", "t").unwrap();
        update_governs_verdict(
            &db,
            "r0",
            &ids[0],
            "passing",
            "criterion text long enough",
            "evidence text long enough",
            0.5,
            "llm:quality",
            "t",
        )
        .unwrap();

        let rc = review_candidates(&db).unwrap();
        assert_eq!(rc.len(), 2, "both uncertain verdicts surface: {}", rc.len());

        // Reviewer confirms the edge with real confidence → off the queue.
        update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "traced: a calls b's parser in fn run",
            "",
            0.9,
            "llm:analyzer",
            "t2",
        )
        .unwrap();
        let rc = review_candidates(&db).unwrap();
        assert_eq!(rc.len(), 1, "confirmed edge resolved");
        assert!(matches!(rc[0].0, ReviewCandidate::Governs(_)));
        update_governs_verdict(
            &db,
            "r0",
            &ids[0],
            "passing",
            "criterion text long enough",
            "re-inspected: holds with specifics",
            0.85,
            "llm:quality",
            "t3",
        )
        .unwrap();
        assert!(review_candidates(&db).unwrap().is_empty(), "queue drains");
    }

    /// Notes carry an optional audience — the directed-handoff channel — and
    /// rules carry inspection_effort; both round-trip through export/import,
    /// and exports WITHOUT the optional fields still import (additive schema).
    #[test]
    fn audience_and_effort_round_trip_and_stay_optional() {
        let (db, ids) = db_inited(1);
        insert_note(
            &db,
            &Note {
                id: "n0".into(),
                kind: "todo".into(),
                text: "locator broke in src/x.rs — re-ground it".into(),
                author: "llm:analyzer".into(),
                target_kind: "intent".into(),
                target_id: ids[0].clone(),
                audience: "builder".into(),
                created_at: "t".into(),
            },
        )
        .unwrap();
        insert_rule(
            &db,
            &QualityRule {
                id: "r0".into(),
                name: "stick".into(),
                description: "d".into(),
                detection_logic: "dl".into(),
                severity: "warning".into(),
                inspection_effort: "low".into(),
            },
        )
        .unwrap();

        let export = export_graph(&db).unwrap();
        let db2 = GrafeoDb::in_memory();
        db2.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-2",
            "two",
            "owned",
        ))
        .unwrap();
        import_graph(&db2, &export, false).unwrap();
        let n = list_notes(&db2, Some(&ids[0]), None).unwrap();
        assert!(n.iter().any(|x| x.audience == "builder"), "{n:?}");
        let r = list_rules(&db2).unwrap();
        assert_eq!(r[0].inspection_effort, "low");

        // An export missing the optional fields (older binary) still imports.
        let mut old = export.clone();
        for note in old["nodes"]["Note"].as_array_mut().unwrap() {
            note.as_object_mut().unwrap().remove("audience");
        }
        for rule in old["nodes"]["QualityRule"].as_array_mut().unwrap() {
            rule.as_object_mut().unwrap().remove("inspection_effort");
        }
        let db3 = GrafeoDb::in_memory();
        db3.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-3",
            "three",
            "owned",
        ))
        .unwrap();
        import_graph(&db3, &old, false).unwrap();
        assert_eq!(list_rules(&db3).unwrap()[0].inspection_effort, "");
    }

    /// The custody gate: an observed graph (someone else's code) rejects
    /// actions that claim building/fixing; an owned graph passes.
    #[test]
    fn custody_gate_blocks_observed_graphs() {
        let (db, _) = db_with_intents(0);
        db.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-x",
            "vendor-sdk",
            "observed",
        ))
        .unwrap();
        let err = ensure_owned(&db, "mark an edge fixed")
            .unwrap_err()
            .to_string();
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
        insert_delegation(
            &db,
            &Delegation {
                id: "d0".into(),
                pattern: "services/grid/**".into(),
                target: "services/grid/loom.graph.json".into(),
                author: "llm:builder".into(),
                created_at: "t".into(),
            },
        )
        .unwrap();
        let ds = list_delegations(&db).unwrap();
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].pattern, "services/grid/**");
        assert_eq!(ds[0].target, "services/grid/loom.graph.json");
        let export = export_graph(&db).unwrap();
        assert_eq!(export["nodes"]["Delegation"].as_array().unwrap().len(), 1);
    }

    fn hypothesis(id: &str, name: &str) -> crate::types::Hypothesis {
        crate::types::Hypothesis {
            id: id.into(),
            name: name.into(),
            claim: "scoring.rs serves four unrelated intents".into(),
            proposal: "extract discovery ranking into its own module".into(),
            predicted_outcome: "scoring.rs under 300 lines, tangled-file smell gone".into(),
            status: "proposed".into(),
            author: "llm:quality".into(),
            evidence: String::new(),
            inspected_by: String::new(),
            last_inspected: String::new(),
            created_at: "t0".into(),
            updated_at: "t0".into(),
        }
    }

    /// Hypothesis round trip + the state machine writes: a proof verdict stamps
    /// status/evidence/provenance and records a transition note; a decision
    /// stamps status and records a transition note. TARGETS edges resolve by
    /// endpoints (the reliable key).
    #[test]
    fn hypothesis_roundtrip_and_state_machine() {
        let (db, ids) = db_with_intents(2);
        insert_hypothesis(&db, &hypothesis("h0", "split the scoring module")).unwrap();

        // Retrievable by id, by exact name, by unique fragment.
        assert!(get_hypothesis(&db, "h0").unwrap().is_some());
        assert_eq!(
            resolve_hypothesis(&db, "split the scoring module").unwrap(),
            "h0"
        );
        assert_eq!(resolve_hypothesis(&db, "scoring").unwrap(), "h0");

        // TARGETS edges, endpoint-matched.
        insert_targets(&db, "h0", &ids[0], "t").unwrap();
        insert_targets(&db, "h0", &ids[1], "t").unwrap();
        let ts = list_targets_for_hypothesis(&db, "h0").unwrap();
        assert_eq!(ts.len(), 2);
        assert!(ts.iter().all(|t| t.inspection_status == "uninspected"));
        assert!(get_targets_between(&db, "h0", &ids[0]).unwrap().is_some());

        // Proof verdict: status + evidence + provenance + transition note.
        update_hypothesis_verdict(
            &db,
            "h0",
            "supported",
            "read scoring.rs: ranking shares no types with priority scoring",
            "llm:analyzer",
            "t1",
        )
        .unwrap();
        let h = get_hypothesis(&db, "h0").unwrap().unwrap();
        assert_eq!(h.status, "supported");
        assert_eq!(h.inspected_by, "llm:analyzer");
        assert_eq!(h.last_inspected, "t1");
        assert!(!h.evidence.is_empty());

        // Decision: adopted, with its own transition note.
        set_hypothesis_status(&db, "h0", "adopted", "llm:builder", "t2").unwrap();
        assert_eq!(
            get_hypothesis(&db, "h0").unwrap().unwrap().status,
            "adopted"
        );
        let notes = notes_for_target(&db, "h0").unwrap();
        let transitions: Vec<_> = notes.iter().filter(|n| n.kind == "transition").collect();
        assert_eq!(transitions.len(), 2, "{notes:?}");
        assert!(transitions
            .iter()
            .any(|n| n.text.contains("proposed → supported")));
        assert!(transitions
            .iter()
            .any(|n| n.text.contains("supported → adopted")));

        // Status filter.
        assert_eq!(list_hypotheses(&db, Some("adopted")).unwrap().len(), 1);
        assert_eq!(list_hypotheses(&db, Some("proposed")).unwrap().len(), 0);
    }

    /// The prove queue serves only PROPOSED hypotheses, highest combined
    /// target-centrality (blast radius) first; proven/decided ones leave the
    /// queue. An untargeted proposal still surfaces, last.
    #[test]
    fn prove_queue_ranks_proposed_hypotheses_by_target_centrality() {
        let (db, ids) = db_with_intents(4);
        // Make intent 0 central: real RELATES_TO edges to the other three.
        for j in 1..4 {
            get_or_create_relates_to(&db, &ids[0], &ids[j], "t").unwrap();
            update_relates_to_ground(
                &db,
                &ids[0],
                &ids[j],
                "they cooperate via a stable contract",
                "",
                0.9,
                "llm",
                "t",
            )
            .unwrap();
        }
        let mut h_central = hypothesis("h-central", "touches the hub");
        h_central.created_at = "t2".into();
        insert_hypothesis(&db, &h_central).unwrap();
        insert_targets(&db, "h-central", &ids[0], "t").unwrap();
        let mut h_leaf = hypothesis("h-leaf", "touches a leaf");
        h_leaf.created_at = "t1".into();
        insert_hypothesis(&db, &h_leaf).unwrap();
        insert_targets(&db, "h-leaf", &ids[3], "t").unwrap();
        insert_hypothesis(&db, &hypothesis("h-untargeted", "floats free")).unwrap();

        let q = prove_candidates(&db).unwrap();
        assert_eq!(q.len(), 3);
        assert_eq!(
            q[0].0.id, "h-central",
            "hub-targeting proposal first: {q:?}"
        );
        assert!(q[0].1 > q[1].1);
        assert_eq!(q[2].0.id, "h-untargeted", "untargeted still surfaces, last");

        // A proven hypothesis leaves the prove queue.
        update_hypothesis_verdict(
            &db,
            "h-central",
            "supported",
            "checked: the hub is real",
            "llm:analyzer",
            "t3",
        )
        .unwrap();
        let q = prove_candidates(&db).unwrap();
        assert_eq!(q.len(), 2);
        assert!(q.iter().all(|(h, _)| h.status == "proposed"));
    }

    /// The v3 staleness loop: sync flips passing TARGETS edges when target
    /// code changes, the prove queue then serves the supported hypothesis as
    /// a RE-PROVE item (its support was earned against old code), and
    /// re-proving re-stamps the edges, clearing the staleness.
    #[test]
    fn stale_target_support_routes_back_to_prove_queue() {
        let (db, ids) = db_with_intents(2);
        insert_hypothesis(&db, &hypothesis("h0", "split the scoring module")).unwrap();
        insert_targets(&db, "h0", &ids[0], "t").unwrap();

        // Prove it (what `loom hypothesis prove` does): node verdict + stamp.
        update_hypothesis_verdict(
            &db,
            "h0",
            "supported",
            "checked against the code",
            "llm:analyzer",
            "t1",
        )
        .unwrap();
        set_targets_status_for_hypothesis(
            &db,
            "h0",
            "passing",
            "hypothesis proof establishes whether this target is affected",
            "checked against the code",
            "llm:analyzer",
            "t1",
        )
        .unwrap();
        assert!(
            prove_candidates(&db).unwrap().is_empty(),
            "fresh support is not prove-queue work"
        );

        // Target code changes — the ripple flips the passing TARGETS edge.
        let flipped =
            targets::flag_targets_for_intent(&db, &ids[0], "src/x.rs changed", "t2").unwrap();
        assert_eq!(flipped, 1);
        let ts = list_targets_for_hypothesis(&db, "h0").unwrap();
        assert_eq!(ts[0].inspection_status, "needs_reverification");

        // Stale support routes back: the supported hypothesis is due again.
        let q = prove_candidates(&db).unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].0.status, "supported");

        // Re-proving re-stamps the edges and clears the queue.
        update_hypothesis_verdict(
            &db,
            "h0",
            "supported",
            "still holds after the change",
            "llm:analyzer",
            "t3",
        )
        .unwrap();
        set_targets_status_for_hypothesis(
            &db,
            "h0",
            "passing",
            "hypothesis proof establishes whether this target is affected",
            "still holds after the change",
            "llm:analyzer",
            "t3",
        )
        .unwrap();
        assert!(prove_candidates(&db).unwrap().is_empty());
    }

    /// Hypotheses are a proof queue, not a parking lot. A single old proposal
    /// or a swollen fresh queue surfaces as a teaching smell; proving/refuting
    /// the stale item clears the finding.
    #[test]
    fn hypothesis_accumulation_smell_flags_stale_and_teaches_drain() {
        let (db, _) = db_inited(0);
        let mut h = hypothesis("h-old", "old redesign idea");
        h.created_at =
            (chrono::Utc::now() - chrono::Duration::days(HYPOTHESIS_STALE_DAYS + 1)).to_rfc3339();
        h.updated_at = h.created_at.clone();
        insert_hypothesis(&db, &h).unwrap();

        let smells = compute_smells(&db).unwrap().open;
        let finding = smells
            .iter()
            .find(|s| s.kind == "hypothesis_accumulation")
            .expect("stale proposed hypothesis should surface");
        assert!(finding.summary.contains("1 proposed"), "{finding:?}");
        assert!(
            finding
                .evidence
                .contains(&format!("older than {}d", HYPOTHESIS_STALE_DAYS)),
            "{}",
            finding.evidence
        );
        assert!(
            finding.remedy.contains("loom next --mode prove"),
            "{}",
            finding.remedy
        );
        assert_smell_teaching(finding);
        assert!(
            finding
                .teaching
                .principle
                .contains("proof item, not long-term memory"),
            "{:?}",
            finding.teaching
        );

        update_hypothesis_verdict(
            &db,
            "h-old",
            "refuted",
            "read the code: the claimed split is no longer real",
            "llm:analyzer",
            "t1",
        )
        .unwrap();
        let smells = compute_smells(&db).unwrap().open;
        assert!(
            !smells.iter().any(|s| s.kind == "hypothesis_accumulation"),
            "refuted hypotheses leave the proposed backlog: {smells:?}"
        );
    }

    #[test]
    fn hypothesis_accumulation_smell_flags_bulk_proposed_queue() {
        let (db, _) = db_inited(0);
        let now = chrono::Utc::now().to_rfc3339();
        for idx in 0..(HYPOTHESIS_BACKLOG_LIMIT - 1) {
            let mut h = hypothesis(&format!("h-{idx}"), &format!("fresh idea {idx}"));
            h.created_at = now.clone();
            h.updated_at = now.clone();
            insert_hypothesis(&db, &h).unwrap();
        }
        assert!(
            !compute_smells(&db)
                .unwrap()
                .open
                .iter()
                .any(|s| s.kind == "hypothesis_accumulation"),
            "below the fresh backlog threshold should stay quiet"
        );

        let mut h = hypothesis("h-limit", "fresh idea at threshold");
        h.created_at = now.clone();
        h.updated_at = now;
        insert_hypothesis(&db, &h).unwrap();

        let smells = compute_smells(&db).unwrap().open;
        let finding = smells
            .iter()
            .find(|s| s.kind == "hypothesis_accumulation")
            .expect("fresh queue at threshold should surface");
        assert!(
            finding
                .summary
                .contains(&format!("{} proposed", HYPOTHESIS_BACKLOG_LIMIT)),
            "{}",
            finding.summary
        );
        assert!(
            finding.evidence.contains("without TARGETS"),
            "{}",
            finding.evidence
        );
        assert!(
            finding
                .teaching
                .inspect
                .iter()
                .any(|i| i.contains("hypothesis list")),
            "{:?}",
            finding.teaching
        );
    }

    /// The hypothesis plane travels with the export, and exports from OLDER
    /// binaries (no Hypothesis/TARGETS sections at all) still import — the
    /// sections are additive, same contract as optional props.
    #[test]
    fn hypothesis_travels_and_old_exports_still_import() {
        let (db, ids) = db_inited(1);
        insert_hypothesis(&db, &hypothesis("h0", "split the scoring module")).unwrap();
        insert_targets(&db, "h0", &ids[0], "t").unwrap();

        let export = export_graph(&db).unwrap();
        assert_eq!(export["nodes"]["Hypothesis"].as_array().unwrap().len(), 1);
        assert_eq!(export["edges"]["TARGETS"].as_array().unwrap().len(), 1);

        let db2 = GrafeoDb::in_memory();
        db2.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-2",
            "two",
            "owned",
        ))
        .unwrap();
        import_graph(&db2, &export, false).unwrap();
        let h = get_hypothesis(&db2, "h0").unwrap().unwrap();
        assert_eq!(h.claim, "scoring.rs serves four unrelated intents");
        assert_eq!(list_targets_for_hypothesis(&db2, "h0").unwrap().len(), 1);

        // An older export has NO hypothesis sections — import reads them empty.
        let mut old = export.clone();
        old["nodes"].as_object_mut().unwrap().remove("Hypothesis");
        old["edges"].as_object_mut().unwrap().remove("TARGETS");
        let db3 = GrafeoDb::in_memory();
        db3.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-3",
            "three",
            "owned",
        ))
        .unwrap();
        import_graph(&db3, &old, false).unwrap();
        assert!(list_hypotheses(&db3, None).unwrap().is_empty());
    }

    /// PORTING: a hypothesis travels as design lineage, but a supported/refuted
    /// proof was earned against the OLD code — it arrives `proposed` with the
    /// proof meta cleared. Decisions (adopted/rejected) stay history.
    #[test]
    fn import_as_planned_resets_hypothesis_proofs() {
        let (db, ids) = db_inited(1);
        insert_hypothesis(&db, &hypothesis("h0", "split the scoring module")).unwrap();
        update_hypothesis_verdict(
            &db,
            "h0",
            "supported",
            "checked against old repo",
            "llm:analyzer",
            "t1",
        )
        .unwrap();
        insert_hypothesis(&db, &hypothesis("h1", "kill the cd fallback")).unwrap();
        set_hypothesis_status(&db, "h1", "rejected", "llm:builder", "t1").unwrap();
        insert_hypothesis(&db, &hypothesis("h2", "thread graph snapshots")).unwrap();
        set_hypothesis_status(&db, "h2", "confirmed", "llm:validator", "t1").unwrap();
        insert_targets(&db, "h0", &ids[0], "t").unwrap();

        let export = export_graph(&db).unwrap();
        let db2 = GrafeoDb::in_memory();
        db2.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-2",
            "port",
            "owned",
        ))
        .unwrap();
        import_graph(&db2, &export, true).unwrap();

        let h0 = get_hypothesis(&db2, "h0").unwrap().unwrap();
        assert_eq!(h0.status, "proposed", "earned proof must not travel");
        assert_eq!(h0.evidence, "");
        assert_eq!(h0.last_inspected, "");
        let h1 = get_hypothesis(&db2, "h1").unwrap().unwrap();
        assert_eq!(h1.status, "rejected", "decisions are lineage and stay");
        let h2 = get_hypothesis(&db2, "h2").unwrap().unwrap();
        assert_eq!(
            h2.status, "adopted",
            "confirmed resets to adopted: the outcome was verified against OLD code"
        );
        // TARGETS edges travel (intents travel) but arrive uninspected.
        let ts = list_targets_for_hypothesis(&db2, "h0").unwrap();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].inspection_status, "uninspected");
    }

    /// A GOVERNS verdict inherits DOWN the hierarchy: a rule held against a
    /// component covers its coded descendants in the unmeasured-intents smell,
    /// so measuring at the honest altitude clears the smell instead of leaving
    /// a wall of per-leaf busywork. Uncovered siblings still flag.
    #[test]
    fn unmeasured_smell_respects_ancestor_verdicts() {
        let (db, ids) = db_with_intents(3); // 0 = component, 1 = its child, 2 = uncovered
        insert_hierarchy(&db, &ids[0], &ids[1], "", "t").unwrap();
        // All three have code (the smell only measures coded intents).
        for (k, iid) in ids.iter().enumerate() {
            insert_codefile(&db, &codefile(&format!("cf{k}"), &format!("src/f{k}.rs"))).unwrap();
            insert_implements(&db, iid, &format!("cf{k}"), "", "", "t").unwrap();
        }
        insert_rule(
            &db,
            &QualityRule {
                id: "r0".into(),
                name: "no-eval".into(),
                description: "d".into(),
                detection_logic: "dl".into(),
                severity: "error".into(),
                inspection_effort: String::new(),
            },
        )
        .unwrap();
        insert_governs(&db, "r0", &ids[0], "", "t").unwrap();
        update_governs_verdict(
            &db,
            "r0",
            &ids[0],
            "passing",
            "no dynamic evaluation in this component",
            "workspace clippy denial covers the whole subtree",
            0.9,
            "llm:quality",
            "t",
        )
        .unwrap();

        let unmeasured: Vec<_> = compute_smells(&db)
            .unwrap()
            .open
            .into_iter()
            .filter(|s| s.kind == "unmeasured_intents")
            .collect();
        assert_eq!(unmeasured.len(), 1, "one rule → one finding");
        let f = &unmeasured[0];
        assert!(
            f.summary.contains("1 intent(s)"),
            "child covered by ancestor: {}",
            f.summary
        );
        assert!(
            f.evidence.contains("I2"),
            "only the uncovered sibling flags: {}",
            f.evidence
        );
    }

    /// `loom validation update`: a corrected command resets the proof (the old
    /// result proved a different command); `loom validation delete` removes the
    /// node + edges so the intent is provably unproven again.
    #[test]
    fn validation_update_resets_proof_and_delete_removes_it() {
        use crate::types::Validation;
        let (db, ids) = db_with_intents(1);
        insert_validation(
            &db,
            &Validation {
                id: "v0".into(),
                name: "ledger write".into(),
                description: String::new(),
                validation_type: "test".into(),
                command: "cargo test -p wrong-pkg".into(),
                last_run: "t".into(),
                last_result: "passed".into(),
            },
        )
        .unwrap();
        insert_validates(&db, "v0", &ids[0], "", "t").unwrap();
        set_validates_status_for_validation(&db, "v0", "passing", "ran green").unwrap();

        // The command-layer flow: definition updated, then proof reset.
        assert!(
            update_validation_definition(&db, "v0", Some("cargo test -p right-pkg"), None).unwrap()
        );
        update_validation_result(&db, "v0", "not_run", "").unwrap();
        set_validates_status_for_validation(
            &db,
            "v0",
            "uninspected",
            "command updated — proof must be re-run",
        )
        .unwrap();
        let v = get_validation(&db, "v0").unwrap().unwrap();
        assert_eq!(v.command, "cargo test -p right-pkg");
        assert_eq!(v.last_result, "not_run");
        assert_eq!(
            list_validates_for_intent(&db, &ids[0]).unwrap()[0].inspection_status,
            "uninspected"
        );
        // …and the intent is back on the validator queue.
        assert!(validate_candidates(&db)
            .unwrap()
            .iter()
            .any(|c| c.intent.id == ids[0]));

        assert!(delete_validation(&db, "v0").unwrap());
        assert!(get_validation(&db, "v0").unwrap().is_none());
        assert!(list_validates_for_intent(&db, &ids[0]).unwrap().is_empty());
        assert!(
            !delete_validation(&db, "v0").unwrap(),
            "second delete reports not-found"
        );
    }

    /// `loom validation mark` path: a manual_check with no command can be given a
    /// verdict by hand — node last_result + the per-intent VALIDATES edge both move.
    #[test]
    fn validation_mark_records_manual_verdict() {
        use crate::types::Validation;
        let (db, ids) = db_inited(1);
        insert_validation(
            &db,
            &Validation {
                id: "v0".into(),
                name: "manual smoke".into(),
                description: String::new(),
                validation_type: "manual_check".into(),
                command: String::new(),
                last_run: String::new(),
                last_result: "not_run".into(),
            },
        )
        .unwrap();
        insert_validates(&db, "v0", &ids[0], "", "t").unwrap();

        update_validation_result(&db, "v0", "passed", "t").unwrap();
        let n =
            set_validates_status_for_validation(&db, "v0", "passing", "checked by hand").unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            get_validation(&db, "v0").unwrap().unwrap().last_result,
            "passed"
        );
        let edges = list_validates_for_intent(&db, &ids[0]).unwrap();
        assert_eq!(edges[0].inspection_status, "passing");
        assert_eq!(edges[0].notes, "checked by hand");
        assert_eq!(resolve_validation(&db, "manual smoke").unwrap(), "v0");
    }

    #[test]
    fn doctor_flags_version_mismatch() {
        let (db, _) = db_with_intents(0);
        db.execute(&crate::db::schema::insert_meta(
            "999",
            "t",
            "g-test",
            "testgraph",
            "owned",
        ))
        .unwrap();
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
        insert_rule(
            &db,
            &QualityRule {
                id: "r0".into(),
                name: "no_god_objects".into(),
                description: "d".into(),
                detection_logic: "many concerns in one unit".into(),
                severity: "warning".into(),
                inspection_effort: String::new(),
            },
        )
        .unwrap();
        insert_governs(&db, "r0", &ids[0], "", "t").unwrap();

        // Uninspected GOVERNS is quality work.
        let qc = quality_candidates(&db).unwrap();
        assert_eq!(qc.len(), 1);
        assert_eq!(qc[0].0.inspection_status, "uninspected");

        // No edge between an unknown pair → verdict reports not-found.
        assert!(!update_governs_verdict(
            &db,
            "r0",
            "nope",
            "passing",
            "criterion text long enough",
            "evidence",
            0.9,
            "llm:quality",
            "t1",
        )
        .unwrap());

        // Record the verdict and read it back via a scan.
        assert!(update_governs_verdict(
            &db,
            "r0",
            &ids[0],
            "passing",
            "each module owns exactly one concern",
            "reviewed src/x.rs: single concern per unit",
            0.85,
            "llm:quality",
            "t1",
        )
        .unwrap());
        let g = get_governs_between(&db, "r0", &ids[0]).unwrap().unwrap();
        assert_eq!(g.inspection_status, "passing");
        assert_eq!(g.criterion, "each module owns exactly one concern");
        assert_eq!(g.evidence, "reviewed src/x.rs: single concern per unit");
        assert_eq!(g.inspected_by, "llm:quality");
        assert!((g.confidence - 0.85).abs() < 1e-9);

        // Green earned → off the quality queue.
        assert!(quality_candidates(&db).unwrap().is_empty());
    }

    /// 360° normative queue: a rule × intent-with-code pair nobody considered
    /// is quality work (synthetic `unmeasured`, no edge yet), surfaced at the
    /// HIGHEST altitude only — an unmeasured child under an unmeasured parent
    /// is shadowed (one verdict up there covers it). A verdict on the parent
    /// silences the whole subtree; intents with no code are never nagged.
    #[test]
    fn unmeasured_pairs_feed_quality_queue_at_highest_altitude() {
        let (db, ids) = db_inited(3); // 0: parent, 1: child (both coded); 2: no code
        insert_hierarchy(&db, &ids[0], &ids[1], "", "t").unwrap();
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, &ids[0], "cf", "fn x", "", "t").unwrap();
        insert_implements(&db, &ids[1], "cf", "fn y", "", "t").unwrap();
        insert_rule(
            &db,
            &QualityRule {
                id: "r0".into(),
                name: "stick".into(),
                description: "d".into(),
                detection_logic: "what to look for".into(),
                severity: "warning".into(),
                inspection_effort: String::new(),
            },
        )
        .unwrap();

        let qc = quality_candidates(&db).unwrap();
        let unmeasured: Vec<_> = qc
            .iter()
            .filter(|(g, _)| g.inspection_status == "unmeasured")
            .collect();
        assert_eq!(
            unmeasured.len(),
            1,
            "only the subtree top surfaces: {:?}",
            qc.iter()
                .map(|(g, _)| (g.intent_id.clone(), g.inspection_status.clone()))
                .collect::<Vec<_>>()
        );
        assert_eq!(unmeasured[0].0.intent_id, ids[0]);
        assert!(
            unmeasured[0].0.id.is_empty(),
            "no edge yet — the verdict creates it"
        );
        assert!(
            unmeasured[0].0.notes.contains("what to look for"),
            "detection logic travels with the item"
        );

        let nc = normative_coverage(&db).unwrap();
        assert_eq!(nc.intents_with_code, 2);
        assert_eq!((nc.measured_pairs, nc.total_pairs), (0, 2));

        // A verdict at the parent covers the child by inheritance → queue dry.
        insert_governs(&db, "r0", &ids[0], "", "t").unwrap();
        update_governs_verdict(
            &db,
            "r0",
            &ids[0],
            "passing",
            "criterion text long enough",
            "evidence text long enough",
            0.9,
            "llm:quality",
            "t",
        )
        .unwrap();
        assert!(
            quality_candidates(&db).unwrap().is_empty(),
            "component verdict covers descendants"
        );
        assert_eq!(normative_coverage(&db).unwrap().measured_pairs, 2);
    }

    /// The behavioral vantage point: a parent whose children declare a happy
    /// aspect but no realized+proven sad/fallback sibling is a happy_path_only
    /// smell; adding bare aspect children is not enough.
    #[test]
    fn happy_path_only_smell_flags_and_clears() {
        let (db, ids) = db_inited(1);
        let mut happy = intent("happy-child", "login succeeds");
        happy.aspect = "happy".into();
        insert_intent(&db, &happy).unwrap();
        insert_hierarchy(&db, &ids[0], "happy-child", "", "t").unwrap();

        let missing = |needle: &str| {
            let smells = compute_smells(&db).unwrap().open;
            let found = smells.iter().find(|s| s.kind == "happy_path_only").cloned();
            assert!(found.is_some(), "{smells:?}");
            let found = found.unwrap();
            assert!(
                found.summary.contains(needle),
                "expected '{needle}' in {:?}",
                found.summary
            );
            found
        };

        missing("sad/fallback");

        let mut sad = intent("sad-child", "login fails cleanly");
        sad.aspect = "sad".into();
        sad.lifecycle = "planned".into();
        insert_intent(&db, &sad).unwrap();
        insert_hierarchy(&db, &ids[0], "sad-child", "", "t").unwrap();
        let mut fb = intent("fb-child", "login degrades gracefully");
        fb.aspect = "fallback".into();
        insert_intent(&db, &fb).unwrap();
        insert_hierarchy(&db, &ids[0], "fb-child", "", "t").unwrap();
        let finding = missing("sad/fallback");
        assert!(
            finding
                .evidence
                .contains("realized+proven sad/fallback aspects {}"),
            "{}",
            finding.evidence
        );

        set_intent_lifecycle(&db, "sad-child", "implemented", "t1").unwrap();
        insert_codefile(&db, &codefile("sad-cf", "src/sad.rs")).unwrap();
        insert_implements(&db, "sad-child", "sad-cf", "fn sad", "", "t2").unwrap();
        missing("sad/fallback");

        insert_validation(
            &db,
            &Validation {
                id: "sad-v".into(),
                name: "sad proof".into(),
                description: String::new(),
                validation_type: "test".into(),
                command: "true".into(),
                last_run: String::new(),
                last_result: "not_run".into(),
            },
        )
        .unwrap();
        insert_validates(&db, "sad-v", "sad-child", "", "t3").unwrap();
        missing("sad/fallback");
        update_validation_result(&db, "sad-v", "failed", "t4").unwrap();
        missing("sad/fallback");
        update_validation_result(&db, "sad-v", "passed", "t5").unwrap();
        let finding = missing("fallback");
        assert!(
            !finding.summary.contains("sad/fallback"),
            "sad is satisfied once implemented, grounded, and directly proven: {}",
            finding.summary
        );

        insert_codefile(&db, &codefile("fb-cf", "src/fallback.rs")).unwrap();
        insert_implements(&db, "fb-child", "fb-cf", "fn fallback", "", "t6").unwrap();
        insert_validation(
            &db,
            &Validation {
                id: "fb-v".into(),
                name: "fallback proof".into(),
                description: String::new(),
                validation_type: "test".into(),
                command: "true".into(),
                last_run: String::new(),
                last_result: "passed".into(),
            },
        )
        .unwrap();
        insert_validates(&db, "fb-v", "fb-child", "", "t7").unwrap();
        let smells = compute_smells(&db).unwrap().open;
        assert!(
            !smells.iter().any(|s| s.kind == "happy_path_only"),
            "{smells:?}"
        );
    }

    /// Adjudication terminal state for tangled_file: a decision note on the
    /// FILE newer than its newest claim resolves the finding; a claim added
    /// after the decision re-opens it.
    #[test]
    fn tangled_file_decision_note_resolves_and_reflags() {
        let (db, ids) = db_inited(4);
        insert_codefile(&db, &codefile("cf", "src/hub.rs")).unwrap();
        for id in ids.iter().take(3) {
            insert_implements(&db, id, "cf", "fn f", "", "t1").unwrap();
        }
        assert!(compute_smells(&db)
            .unwrap()
            .open
            .iter()
            .any(|s| s.kind == "tangled_file"));

        // Decision newer than every claim → adjudicated: the finding leaves
        // `open` but surfaces in `adjudicated` WITH its ruling — "no findings"
        // and "findings ruled deliberate" must never look alike (dogfood: five
        // godfiles vanished behind batch-stamped notes and nothing said so).
        insert_note(&db, &note_at("nd", "decision", "codefile", "cf", "t2")).unwrap();
        let report = compute_smells(&db).unwrap();
        assert!(!report.open.iter().any(|s| s.kind == "tangled_file"));
        let adj = report
            .adjudicated
            .iter()
            .find(|a| a.kind == "tangled_file")
            .expect("suppressed finding must surface with its ruling");
        assert_adjudicated_teaching(adj);
        assert_eq!(adj.ruled_at, "t2");
        assert!(
            adj.summary.contains("3 distinct intents"),
            "{}",
            adj.summary
        );
        assert!(
            adj.reopens_when.contains("IMPLEMENTS claim"),
            "{}",
            adj.reopens_when
        );

        // A NEW claim after the decision re-opens the question (and the
        // adjudication entry disappears — it no longer suppresses anything).
        insert_implements(&db, &ids[3], "cf", "fn g", "", "t3").unwrap();
        let report = compute_smells(&db).unwrap();
        assert!(report.open.iter().any(|s| s.kind == "tangled_file"));
        assert!(!report.adjudicated.iter().any(|a| a.kind == "tangled_file"));
    }

    /// Adjudication terminal state for scattered_intent: a decision note on
    /// the intent newer than its newest grounding resolves it; a grounding
    /// added after the decision re-opens it.
    #[test]
    fn scattered_intent_decision_note_resolves_and_reflags() {
        let (db, ids) = db_inited(1);
        for k in 0..4 {
            insert_codefile(&db, &codefile(&format!("cf{k}"), &format!("src/f{k}.rs"))).unwrap();
            insert_implements(&db, &ids[0], &format!("cf{k}"), "fn f", "", "t1").unwrap();
        }
        assert!(compute_smells(&db)
            .unwrap()
            .open
            .iter()
            .any(|s| s.kind == "scattered_intent"));

        insert_note(&db, &note_at("nd", "decision", "intent", &ids[0], "t2")).unwrap();
        assert!(!compute_smells(&db)
            .unwrap()
            .open
            .iter()
            .any(|s| s.kind == "scattered_intent"));

        insert_codefile(&db, &codefile("cf4", "src/f4.rs")).unwrap();
        insert_implements(&db, &ids[0], "cf4", "fn f", "", "t3").unwrap();
        assert!(compute_smells(&db)
            .unwrap()
            .open
            .iter()
            .any(|s| s.kind == "scattered_intent"));
    }

    /// Symbol accountability is an audit smell, not a raw coverage gate: it
    /// opens for behavior-significant symbols without precise ownership and a
    /// current file/intent decision can accept broad ownership until structure
    /// changes again.
    #[test]
    fn symbol_accountability_gap_decision_note_resolves_and_reflags() {
        let (db, ids) = db_inited(2);
        let mut cf = codefile("cf", "pkg/a.py");
        cf.symbol_facts = vec![
            SymbolFact {
                label: "pub fn run".into(),
                name: "run".into(),
                kind: "fn".into(),
                visibility: "public".into(),
                line_start: 1,
                line_end: 3,
                is_test: false,
            },
            SymbolFact {
                label: "pub fn stop".into(),
                name: "stop".into(),
                kind: "fn".into(),
                visibility: "public".into(),
                line_start: 5,
                line_end: 7,
                is_test: false,
            },
        ];
        cf.symbols = cf
            .symbol_facts
            .iter()
            .map(|fact| fact.label.clone())
            .collect();
        insert_codefile(&db, &cf).unwrap();
        insert_implements(&db, &ids[0], "cf", "", "", "t1").unwrap();

        assert!(compute_smells(&db)
            .unwrap()
            .open
            .iter()
            .any(|s| s.kind == "symbol_accountability_gap"));

        insert_note(&db, &note_at("nd", "decision", "codefile", "cf", "t2")).unwrap();
        let report = compute_smells(&db).unwrap();
        assert!(!report
            .open
            .iter()
            .any(|s| s.kind == "symbol_accountability_gap"));
        assert!(report
            .adjudicated
            .iter()
            .any(|s| s.kind == "symbol_accountability_gap"));

        insert_implements(&db, &ids[1], "cf", "pub fn run", "", "t3").unwrap();
        assert!(compute_smells(&db)
            .unwrap()
            .open
            .iter()
            .any(|s| s.kind == "symbol_accountability_gap"));
    }

    /// Adjudication terminal state for happy_path_only: a decision note on
    /// the parent newer than its newest aspect-tagged child records why the
    /// missing path is N/A; a new aspect-tagged child re-opens the question.
    #[test]
    fn happy_path_decision_note_resolves_and_reflags() {
        let (db, ids) = db_inited(1);
        let mut happy = intent("happy-child", "login succeeds");
        happy.aspect = "happy".into();
        happy.created_at = "t1".into();
        insert_intent(&db, &happy).unwrap();
        insert_hierarchy(&db, &ids[0], "happy-child", "", "t").unwrap();
        assert!(compute_smells(&db)
            .unwrap()
            .open
            .iter()
            .any(|s| s.kind == "happy_path_only"));

        insert_note(&db, &note_at("nd", "decision", "intent", &ids[0], "t2")).unwrap();
        assert!(!compute_smells(&db)
            .unwrap()
            .open
            .iter()
            .any(|s| s.kind == "happy_path_only"));

        let mut edge_case = intent("edge-child", "login rejects malformed input");
        edge_case.aspect = "edge_case".into();
        edge_case.created_at = "t3".into();
        insert_intent(&db, &edge_case).unwrap();
        insert_hierarchy(&db, &ids[0], "edge-child", "", "t").unwrap();
        assert!(compute_smells(&db)
            .unwrap()
            .open
            .iter()
            .any(|s| s.kind == "happy_path_only"));
    }

    /// The consumer plane's completeness check (unjourneyed_surface), both
    /// regimes: zero passed sagas → ONE aggregate finding on the root,
    /// adjudicated by a decision note there; ≥1 passed saga → per-intent findings with tree-aware
    /// coverage (a journeyed sibling never covers its sibling), adjudicated
    /// per intent, re-opened by a redefinition. Untriaged visibility never
    /// fires — the smell is what makes the user_visible ruling load-bearing.
    #[test]
    fn unjourneyed_surface_flags_aggregate_then_per_intent() {
        use crate::types::Validation;
        let (db, ids) = db_inited(3);
        insert_hierarchy(&db, &ids[0], &ids[1], "", "t").unwrap();
        insert_hierarchy(&db, &ids[0], &ids[2], "", "t").unwrap();
        insert_codefile(&db, &codefile("cf1", "src/f1.rs")).unwrap();
        insert_codefile(&db, &codefile("cf2", "src/f2.rs")).unwrap();
        insert_implements(&db, &ids[1], "cf1", "fn a", "", "t").unwrap();
        insert_implements(&db, &ids[2], "cf2", "fn b", "", "t").unwrap();

        // Untriaged visibility → silent.
        assert!(!compute_smells(&db)
            .unwrap()
            .open
            .iter()
            .any(|s| s.kind == "unjourneyed_surface"));

        set_intent_visibility(&db, &ids[1], "user_visible", "t1").unwrap();
        set_intent_visibility(&db, &ids[2], "user_visible", "t1").unwrap();

        // Zero passed sagas → exactly ONE aggregate finding, targeting the root.
        let smells = compute_smells(&db).unwrap().open;
        let uj: Vec<&Smell> = smells
            .iter()
            .filter(|s| s.kind == "unjourneyed_surface")
            .collect();
        assert_eq!(uj.len(), 1, "{smells:?}");
        assert!(
            uj[0].summary.contains("no passed consumer journey"),
            "{}",
            uj[0].summary
        );
        assert!(
            uj[0].remedy.contains(&ids[0]),
            "aggregate remedy targets the root: {}",
            uj[0].remedy
        );

        // A decision note on the root (newer than the newest user_visible
        // intent) adjudicates the aggregate — visible WITH its ruling.
        insert_note(&db, &note_at("nd", "decision", "intent", &ids[0], "t8")).unwrap();
        let report = compute_smells(&db).unwrap();
        assert!(!report.open.iter().any(|s| s.kind == "unjourneyed_surface"));
        assert!(report
            .adjudicated
            .iter()
            .any(|a| a.kind == "unjourneyed_surface"));

        // A not-run saga arrives but proves nothing yet: it must NOT cover any
        // intent or switch into the per-intent regime.
        insert_validation(
            &db,
            &Validation {
                id: "sg0".into(),
                name: "first journey".into(),
                description: String::new(),
                validation_type: "saga".into(),
                command: "loom saga run j.yaml".into(),
                last_run: String::new(),
                last_result: "not_run".into(),
            },
        )
        .unwrap();
        insert_validates(&db, "sg0", &ids[1], "", "t9").unwrap();
        let smells = compute_smells(&db).unwrap().open;
        let uj: Vec<&Smell> = smells
            .iter()
            .filter(|s| s.kind == "unjourneyed_surface")
            .collect();
        assert_eq!(uj.len(), 1, "{smells:?}");
        assert!(
            uj[0].summary.contains("no passed consumer journey"),
            "a declared but unrun saga must not satisfy consumer journey coverage: {}",
            uj[0].summary
        );

        // Once the saga passes, it journeys ids[1] only: ids[1] is covered
        // (direct), ids[0] via ancestor roll-up, ids[2] — the unjourneyed
        // SIBLING — fires individually.
        update_validation_result(&db, "sg0", "passed", "t10").unwrap();
        let smells = compute_smells(&db).unwrap().open;
        let uj: Vec<&Smell> = smells
            .iter()
            .filter(|s| s.kind == "unjourneyed_surface")
            .collect();
        assert_eq!(uj.len(), 1, "{smells:?}");
        assert!(uj[0].summary.contains("I2"), "{}", uj[0].summary);

        // A decision note on the intent (newer than its updated_at) adjudicates.
        insert_note(&db, &note_at("nd2", "decision", "intent", &ids[2], "t9")).unwrap();
        let report = compute_smells(&db).unwrap();
        assert!(!report.open.iter().any(|s| s.kind == "unjourneyed_surface"));
        assert!(report
            .adjudicated
            .iter()
            .any(|a| a.kind == "unjourneyed_surface" && a.summary.contains("I2")));

        // A redefinition after the ruling re-opens the question.
        assert!(update_intent_meaning(
            &db,
            &ids[2],
            None,
            Some("checkout now also handles refunds"),
            "t9b"
        )
        .unwrap());
        assert!(compute_smells(&db)
            .unwrap()
            .open
            .iter()
            .any(|s| s.kind == "unjourneyed_surface"));
    }

    /// `try_resolve_intent` is the spawn guard for the journey-first saga
    /// entrance: Ok(None) ONLY when nothing matches; an ambiguous fragment is
    /// still an error (spawning on ambiguity would mint a twin).
    #[test]
    fn try_resolve_distinguishes_missing_from_ambiguous() {
        let (db, _) = db_with_intents(2); // names I0, I1 — fragment "I" is ambiguous
        assert_eq!(try_resolve_intent(&db, "no-such-intent").unwrap(), None);
        assert_eq!(
            try_resolve_intent(&db, "I0").unwrap(),
            Some("intent-0".to_string())
        );
        let err = try_resolve_intent(&db, "I").unwrap_err().to_string();
        assert!(err.contains("ambiguous"), "got: {err}");
    }

    /// The 360° coverage vector counts every axis honestly: an axis with no
    /// surface is total=0 (rendered "—", never a vacuous 100%); proven counts
    /// only PASSED validations on implemented leaves.
    #[test]
    fn coverage_vector_counts_every_axis() {
        use crate::types::Validation;
        let (db, ids) = db_inited(2);
        insert_codefile(&db, &codefile("cf0", "src/x.rs")).unwrap();
        insert_codefile(&db, &codefile("cf1", "src/y.rs")).unwrap();
        insert_implements(&db, &ids[0], "cf0", "fn x", "", "t").unwrap();

        let c = graph_state(&db).unwrap().coverage;
        assert_eq!((c.grounded_files.covered, c.grounded_files.total), (1, 2));
        assert_eq!((c.realized_leaves.covered, c.realized_leaves.total), (1, 2));
        assert_eq!((c.explored_pairs.covered, c.explored_pairs.total), (0, 1));
        assert_eq!(c.measured_pairs.total, 0, "no rules → no measuring surface");
        assert_eq!((c.proven_leaves.covered, c.proven_leaves.total), (0, 2));

        // Explore the pair, prove one leaf — the axes move.
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        update_relates_to_ground(&db, &ids[0], &ids[1], "c", "", 0.9, "llm", "t").unwrap();
        insert_validation(
            &db,
            &Validation {
                id: "v0".into(),
                name: "smoke".into(),
                description: String::new(),
                validation_type: "test".into(),
                command: "true".into(),
                last_run: String::new(),
                last_result: "not_run".into(),
            },
        )
        .unwrap();
        insert_validates(&db, "v0", &ids[0], "", "t").unwrap();
        update_validation_result(&db, "v0", "passed", "t1").unwrap();

        let c = graph_state(&db).unwrap().coverage;
        assert_eq!((c.explored_pairs.covered, c.explored_pairs.total), (1, 1));
        assert!(c.explored_pairs.done());
        assert_eq!((c.proven_leaves.covered, c.proven_leaves.total), (1, 2));
    }

    /// The validator queue: an implemented leaf with no proof surfaces (a parent
    /// does not — children prove it); unrun proofs surface lower than failing
    /// ones; all-passed drops off the queue.
    #[test]
    fn validate_candidates_missing_unrun_failing() {
        use crate::types::Validation;
        let (db, ids) = db_inited(3); // 0: parent, 1: leaf-no-proof, 2: leaf-with-proof
        insert_hierarchy(&db, &ids[0], &ids[1], "", "t").unwrap();
        insert_hierarchy(&db, &ids[0], &ids[2], "", "t").unwrap();
        insert_validation(
            &db,
            &Validation {
                id: "v0".into(),
                name: "smoke".into(),
                description: String::new(),
                validation_type: "test".into(),
                command: "true".into(),
                last_run: String::new(),
                last_result: "not_run".into(),
            },
        )
        .unwrap();
        insert_validates(&db, "v0", &ids[2], "", "t").unwrap();

        let vc = validate_candidates(&db).unwrap();
        let by_id: std::collections::HashMap<&str, &ValidateCandidate> =
            vc.iter().map(|c| (c.intent.id.as_str(), c)).collect();
        assert!(
            by_id.contains_key(ids[1].as_str()),
            "leaf without proof must surface: {vc:?}"
        );
        assert!(by_id[ids[1].as_str()].reason.contains("no proof"));
        assert!(
            by_id.contains_key(ids[2].as_str()),
            "unrun proof must surface"
        );
        assert!(
            !by_id.contains_key(ids[0].as_str()),
            "parents are proven via children"
        );

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
        insert_hierarchy(&db, &ids[0], &ids[1], "", "t").unwrap();
        insert_hierarchy(&db, &ids[0], &ids[2], "", "t").unwrap();
        for id in &ids {
            set_intent_lifecycle(&db, id, "planned", "t").unwrap();
        }

        let bc = build_candidates(&db).unwrap();
        let surfaced: Vec<&str> = bc.iter().map(|c| c.intent.id.as_str()).collect();
        assert!(
            !surfaced.contains(&ids[0].as_str()),
            "parent must wait for children: {surfaced:?}"
        );
        assert_eq!(surfaced.len(), 2, "both leaf children queue");
        assert!(bc.iter().all(|c| !c.rollup), "leaves are real build work");

        set_intent_lifecycle(&db, &ids[1], "implemented", "t").unwrap();
        set_intent_lifecycle(&db, &ids[2], "implemented", "t").unwrap();
        let bc = build_candidates(&db).unwrap();
        assert_eq!(bc.len(), 1);
        assert_eq!(bc[0].intent.id, ids[0]);
        assert!(
            bc[0].rollup,
            "parent with implemented children is a roll-up"
        );

        // needs_change surfaces at ANY altitude (component refactors are real).
        set_intent_lifecycle(&db, &ids[0], "needs_change", "t").unwrap();
        set_intent_lifecycle(&db, &ids[1], "planned", "t").unwrap();
        let bc = build_candidates(&db).unwrap();
        let surfaced: Vec<&str> = bc.iter().map(|c| c.intent.id.as_str()).collect();
        assert!(
            surfaced.contains(&ids[0].as_str()),
            "needs_change parent must surface: {surfaced:?}"
        );
    }

    /// Quality ripple: when code implementing an intent changes, its *passing*
    /// GOVERNS verdicts go needs_reverification (green is re-earned via the
    /// quality queue); failing/uninspected ones are untouched (already open).
    #[test]
    fn governs_ripple_invalidates_passing_verdicts() {
        let (db, ids) = db_inited(1);
        for (rid, name) in [("r0", "no_eval"), ("r1", "no_uncaught")] {
            insert_rule(
                &db,
                &QualityRule {
                    id: rid.into(),
                    name: name.into(),
                    description: "d".into(),
                    detection_logic: "dl".into(),
                    severity: "error".into(),
                    inspection_effort: String::new(),
                },
            )
            .unwrap();
            insert_governs(&db, rid, &ids[0], "", "t").unwrap();
        }
        update_governs_verdict(
            &db,
            "r0",
            &ids[0],
            "passing",
            "no dynamic evaluation anywhere",
            "grep: no eval usage",
            0.9,
            "llm:quality",
            "t",
        )
        .unwrap();
        update_governs_verdict(
            &db,
            "r1",
            &ids[0],
            "failing",
            "no uncaught exceptions escape",
            "bare JSON.parse at parser.js:1",
            0.9,
            "llm:quality",
            "t",
        )
        .unwrap();

        let flagged = flag_governs_for_intent(&db, &ids[0], "src/x.rs changed", "t2").unwrap();
        assert_eq!(flagged, 1, "only the passing verdict goes stale");
        let g0 = get_governs_between(&db, "r0", &ids[0]).unwrap().unwrap();
        let g1 = get_governs_between(&db, "r1", &ids[0]).unwrap().unwrap();
        assert_eq!(g0.inspection_status, "needs_reverification");
        assert_eq!(g1.inspection_status, "failing", "open work stays open");
        // The stale verdict is back on the quality queue.
        assert!(quality_candidates(&db)
            .unwrap()
            .iter()
            .any(|(g, _)| g.rule_id == "r0" && g.inspection_status == "needs_reverification"));
    }

    /// IMPLEMENTS is unique per (intent, codefile) pair — re-grounding the same
    /// pair is a no-op, so endpoint-matched updates stay unambiguous.
    #[test]
    fn insert_implements_is_idempotent_per_pair() {
        let (db, ids) = db_inited(1);
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, &ids[0], "cf", "fn x", "", "t").unwrap();
        insert_implements(&db, &ids[0], "cf", "fn y", "other", "t2").unwrap();
        let imps = list_implements_for_intent(&db, &ids[0]).unwrap();
        assert_eq!(imps.len(), 1, "duplicate IMPLEMENTS must not be created");
        assert_eq!(
            imps[0].id,
            format!("imp:{}:cf", ids[0]),
            "first grounding wins"
        );
    }

    /// delete_implements is the ungrounding half of insert: endpoint-matched,
    /// false when absent, and the intent honestly regresses to unrealized.
    #[test]
    fn delete_implements_ungrounds() {
        let (db, ids) = db_inited(1);
        insert_codefile(&db, &codefile("cf", "src/x.rs")).unwrap();
        insert_implements(&db, &ids[0], "cf", "fn x", "", "t").unwrap();
        assert!(vertical_completeness(&db).unwrap().complete);
        assert!(delete_implements(&db, &ids[0], "cf").unwrap());
        assert!(
            !delete_implements(&db, &ids[0], "cf").unwrap(),
            "second delete is a no-op"
        );
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
        insert_implements(&db, &ids[0], "cf", "fn x", "", "t").unwrap();
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
        insert_intent(
            &db,
            &Intent {
                id: "t1".into(),
                name: "parse markdown input".into(),
                description: "turns markdown text into an AST for rendering".into(),
                ..intent("t1", "x")
            },
        )
        .unwrap();
        insert_intent(
            &db,
            &Intent {
                id: "t2".into(),
                name: "markdown input parsing".into(),
                description: "turns markdown text into an AST tree".into(),
                ..intent("t2", "x")
            },
        )
        .unwrap();
        // Overlap: two unrelated intents grounded in the same file.
        insert_intent(&db, &intent("o1", "alpha responsibility")).unwrap();
        insert_intent(&db, &intent("o2", "beta duty")).unwrap();
        insert_codefile(&db, &codefile("cf", "src/shared.rs")).unwrap();
        insert_implements(&db, "o1", "cf", "", "", "t").unwrap();
        insert_implements(&db, "o2", "cf", "", "", "t").unwrap();
        // A rule that has never been considered against o1/o2, and an unused one.
        insert_rule(
            &db,
            &QualityRule {
                id: "r0".into(),
                name: "no_panics".into(),
                description: "d".into(),
                detection_logic: "dl".into(),
                severity: "error".into(),
                inspection_effort: String::new(),
            },
        )
        .unwrap();

        let smells = compute_smells(&db).unwrap().open;
        let kinds: Vec<&str> = smells.iter().map(|s| s.kind.as_str()).collect();
        assert!(kinds.contains(&"twin_intents"), "{kinds:?}");
        assert!(kinds.contains(&"overlapping_ownership"), "{kinds:?}");
        assert!(kinds.contains(&"unmeasured_intents"), "{kinds:?}");
        assert!(kinds.contains(&"unused_rule"), "{kinds:?}");
        for smell in &smells {
            assert_smell_teaching(smell);
        }

        // Recording the relationships/verdicts silences the smells.
        get_or_create_relates_to(&db, "t1", "t2", "t").unwrap();
        update_relates_to_independent(
            &db,
            "t1",
            "t2",
            "twin in name only — verified distinct",
            "llm:analyzer",
            "t",
        )
        .unwrap();
        get_or_create_relates_to(&db, "o1", "o2", "t").unwrap();
        insert_governs(&db, "r0", "o1", "", "t").unwrap();
        insert_governs(&db, "r0", "o2", "", "t").unwrap();
        update_governs_verdict(
            &db,
            "r0",
            "o2",
            "independent",
            "panic-freedom criterion does not constrain beta",
            "beta duty has no execution path that can panic",
            0.9,
            "llm:quality",
            "t",
        )
        .unwrap();
        let kinds: Vec<String> = compute_smells(&db)
            .unwrap()
            .open
            .iter()
            .map(|s| s.kind.clone())
            .collect();
        assert!(!kinds.contains(&"twin_intents".to_string()), "{kinds:?}");
        assert!(
            !kinds.contains(&"overlapping_ownership".to_string()),
            "{kinds:?}"
        );
        assert!(
            !kinds.contains(&"unmeasured_intents".to_string()),
            "{kinds:?}"
        );
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
        insert_implements(&db, "a", "cf", "", "", "t").unwrap();
        insert_implements(&db, "b", "cf", "", "", "t").unwrap();

        let pairs = unexplored_pairs_scored(&db).unwrap();
        let top = &pairs[0].0;
        let pair_ids = [top.from_id.as_str(), top.to_id.as_str()];
        assert!(
            pair_ids.contains(&"a") && pair_ids.contains(&"b"),
            "shared-file pair should rank first: {} × {}",
            top.from_name,
            top.to_name
        );
        assert!(
            top.notes.contains("share 1 implemented file"),
            "{}",
            top.notes
        );
    }

    /// GOVERNS `independent` = measured, rule does not apply: a valid verdict
    /// that is not quality work and passes doctor (with evidence recorded).
    #[test]
    fn governs_independent_verdict_is_terminal_and_audited() {
        let (db, ids) = db_inited(1);
        insert_rule(
            &db,
            &QualityRule {
                id: "r0".into(),
                name: "no_sql".into(),
                description: "d".into(),
                detection_logic: "dl".into(),
                severity: "warning".into(),
                inspection_effort: String::new(),
            },
        )
        .unwrap();
        insert_governs(&db, "r0", &ids[0], "", "t").unwrap();
        update_governs_verdict(
            &db,
            "r0",
            &ids[0],
            "independent",
            "criterion would be: no raw SQL strings constructed",
            "this intent touches no datastore at all — the rule has no surface here",
            0.9,
            "llm:quality",
            "t",
        )
        .unwrap();

        assert!(
            quality_candidates(&db).unwrap().is_empty(),
            "independent is not open work"
        );
        let rep = check_graph(&db).unwrap();
        assert!(
            rep.issues
                .iter()
                .all(|i| !i.contains("invalid inspection_status")),
            "{:?}",
            rep.issues
        );
        assert!(
            rep.issues.iter().all(|i| !i.contains("records no why")),
            "{:?}",
            rep.issues
        );
    }

    /// Doctor audits the trust layer: a verdict recorded by an out-of-lane role,
    /// a confidence outside [0,1], and an independence claim with no recorded
    /// why are all integrity issues.
    #[test]
    fn doctor_flags_provenance_and_evidence_violations() {
        let (db, ids) = db_inited(3);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        // Builder green-lighting its own work + absurd confidence.
        update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "a perfectly substantive criterion",
            "",
            7.3,
            "llm:builder",
            "t",
        )
        .unwrap();
        // Independence with no why (empty notes).
        get_or_create_relates_to(&db, &ids[0], &ids[2], "t").unwrap();
        update_relates_to_independent(&db, &ids[0], &ids[2], "", "llm:analyzer", "t").unwrap();

        let rep = check_graph(&db).unwrap();
        assert!(
            rep.issues.iter().any(|i| i.contains("out of lane")),
            "{:?}",
            rep.issues
        );
        assert!(
            rep.issues.iter().any(|i| i.contains("outside [0.0, 1.0]")),
            "{:?}",
            rep.issues
        );
        assert!(
            rep.issues.iter().any(|i| i.contains("records no why")),
            "{:?}",
            rep.issues
        );
        assert!(!rep.healthy());
    }

    /// `loom find` is the ask-the-map entry point: BM25 over names +
    /// descriptions, deprecated intents invisible, each hit hydrated with
    /// parent chain, groundings, and a freshness count — and a miss is an
    /// empty result, never an error.
    #[test]
    fn find_ranks_by_relevance_and_skips_deprecated() {
        let db = GrafeoDb::in_memory();
        let mk = |id: &str, name: &str, desc: &str| {
            let mut i = intent(id, name);
            i.description = desc.to_string();
            i
        };
        insert_intent(&db, &mk("root", "loom core", "the whole system")).unwrap();
        insert_intent(
            &db,
            &mk(
                "sync",
                "sync ripple engine",
                "detects content changes and propagates staleness to neighbor edges",
            ),
        )
        .unwrap();
        insert_intent(
            &db,
            &mk(
                "queue",
                "priority work queue",
                "returns the highest priority work item with full context",
            ),
        )
        .unwrap();
        insert_intent(
            &db,
            &mk(
                "old",
                "legacy ripple walker",
                "ripple ripple ripple — superseded design",
            ),
        )
        .unwrap();
        retire_intent(
            &db,
            "old",
            "superseded by the sync ripple engine",
            Some("sync"),
            "t1",
        )
        .unwrap();
        insert_hierarchy(&db, "root", "sync", "", "t0").unwrap();
        insert_codefile(&db, &codefile("cf", "src/sync.rs")).unwrap();
        insert_implements(&db, "sync", "cf", "fn run", "", "t0").unwrap();
        get_or_create_relates_to(&db, "sync", "queue", "t0").unwrap();
        db.execute(
            "MATCH (a:Intent {id: 'sync'})-[r:RELATES_TO]->(b:Intent {id: 'queue'}) \
             SET r.inspection_status = 'needs_reverification'",
        )
        .unwrap();

        let (hits, match_total) = find_intents(&db, "ripple staleness", 5).unwrap();
        assert_eq!(
            match_total,
            hits.len(),
            "fewer than limit matches → total equals shown (not truncated)"
        );
        assert_eq!(
            hits[0].intent.id, "sync",
            "most relevant intent must rank first"
        );
        assert!(
            hits.iter().all(|h| h.intent.id != "old"),
            "deprecated intents must be invisible to find"
        );
        let top = &hits[0];
        assert_eq!(top.parent_chain, vec!["loom core".to_string()]);
        assert_eq!(
            top.groundings,
            vec![("src/sync.rs".to_string(), "fn run".to_string())]
        );
        assert_eq!(top.stale_edges, 1, "freshness must count the stale claim");
        assert!(
            find_intents(&db, "qwertyuiop zxcvbn", 5)
                .unwrap()
                .0
                .is_empty(),
            "a miss is an empty result, not an error"
        );
    }

    // -----------------------------------------------------------------------
    // The bounded tag vocabulary (vocab.rs + the smells/scoring it feeds)
    // -----------------------------------------------------------------------

    fn term(name: &str, desc: &str) -> crate::types::VocabTerm {
        crate::types::VocabTerm {
            id: format!("vt-{name}"),
            name: name.to_string(),
            description: desc.to_string(),
            author: "llm".to_string(),
            created_at: "t0".to_string(),
        }
    }

    fn tag_intent(db: &GrafeoDb, id: &str, tags: &[&str]) {
        set_intent_tags(db, id, tags.iter().map(|s| s.to_string()).collect(), "t1").unwrap();
    }

    #[test]
    fn vocab_terms_round_trip_and_merge_retags() {
        let (db, ids) = db_with_intents(3);
        insert_vocab_term(&db, &term("retry", "re-attempt after failure")).unwrap();
        insert_vocab_term(&db, &term("retries", "duplicate of retry")).unwrap();
        assert_eq!(
            get_vocab_term(&db, "retry").unwrap().unwrap().description,
            "re-attempt after failure"
        );

        tag_intent(&db, &ids[0], &["retries"]);
        tag_intent(&db, &ids[1], &["retries", "retry"]); // merge must dedupe
        tag_intent(&db, &ids[2], &["retry"]);

        let retagged = merge_vocab_terms(&db, "retries", "retry", "t2").unwrap();
        assert_eq!(
            retagged, 2,
            "only carriers of the dissolved term are touched"
        );
        assert!(
            get_vocab_term(&db, "retries").unwrap().is_none(),
            "dissolved term deleted"
        );
        for id in &ids {
            let tags = parse_tags(&get_intent(&db, id).unwrap().unwrap()).unwrap();
            assert_eq!(tags, vec!["retry".to_string()], "intent {id}: {tags:?}");
        }
        let counts = tag_counts(&list_active_intents(&db).unwrap()).unwrap();
        assert_eq!(counts.get("retry"), Some(&3));
    }

    #[test]
    fn term_keys_are_normalized_and_drift_is_recognizable() {
        assert_eq!(normalize_term("  AuthN ").unwrap(), "authn");
        assert!(
            normalize_term("two words").is_err(),
            "whitespace is not a key"
        );
        assert!(normalize_term("").is_err());

        assert!(terms_look_alike("auth", "authn"), "containment");
        assert!(terms_look_alike("retry", "retries"), "plural via stemming");
        assert!(terms_look_alike("color", "colour"), "small edit distance");
        assert!(!terms_look_alike("retry", "retry"), "identity is not drift");
        assert!(
            !terms_look_alike("authz", "cache"),
            "distinct keys stay distinct"
        );
        // Deliberate limitation: morphological drift only — semantic synonyms
        // ('authn'/'authentication') are caught by the inlined registry at tag
        // time and the agent's judgment, not by string distance.
        assert!(!terms_look_alike("authn", "authentication"));
    }

    #[test]
    fn shared_tag_weight_is_rarity_weighted() {
        let counts: std::collections::HashMap<String, usize> =
            [("rare".to_string(), 2), ("broad".to_string(), 20)].into();
        let a = vec!["rare".to_string(), "broad".to_string()];
        let b = vec!["rare".to_string(), "broad".to_string()];
        let (w, shared) = shared_tag_weight(&a, &b, &counts);
        assert_eq!(shared, vec!["broad".to_string(), "rare".to_string()]);
        // 1/2 + 1/20 — the near-unique term dominates; a spammed broad term
        // contributes almost nothing (the over-tagging defense).
        assert!((w - 0.55).abs() < 1e-9, "{w}");
        let (w0, _) = shared_tag_weight(&a, &[], &counts);
        assert_eq!(w0, 0.0, "untagged never collides");
    }

    #[test]
    fn duplicated_responsibility_fires_only_for_unlinked_disjoint_pairs() {
        let (db, ids) = db_with_intents(4);
        insert_vocab_term(&db, &term("backoff", "delay growth between attempts")).unwrap();
        // Phrased so token jaccard stays below TWIN_SIMILARITY — the lexical
        // detector must NOT be the one catching this pair.
        tag_intent(&db, &ids[0], &["backoff"]);
        tag_intent(&db, &ids[1], &["backoff"]);
        // Disjoint groundings, no import between them.
        insert_codefile(&db, &codefile("f0", "src/fetch.rs")).unwrap();
        insert_codefile(&db, &codefile("f1", "src/jobs.rs")).unwrap();
        insert_implements(&db, &ids[0], "f0", "", "", "t").unwrap();
        insert_implements(&db, &ids[1], "f1", "", "", "t").unwrap();

        let smells = compute_smells(&db).unwrap().open;
        let dup: Vec<&Smell> = smells
            .iter()
            .filter(|s| s.kind == "duplicated_responsibility")
            .collect();
        assert_eq!(dup.len(), 1, "{smells:?}");
        assert!(dup[0].evidence.contains("backoff"), "{}", dup[0].evidence);
        assert!(dup[0].remedy.contains(&ids[0]) && dup[0].remedy.contains(&ids[1]));

        // A recorded relationship silences it — the suspicion did its job.
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        let smells = compute_smells(&db).unwrap().open;
        assert!(
            !smells.iter().any(|s| s.kind == "duplicated_responsibility"),
            "{smells:?}"
        );

        // A shared file belongs to overlapping_ownership, not this detector.
        tag_intent(&db, &ids[2], &["backoff"]);
        tag_intent(&db, &ids[3], &["backoff"]);
        insert_implements(&db, &ids[2], "f0", "", "", "t").unwrap();
        insert_implements(&db, &ids[3], "f0", "", "", "t").unwrap();
        let smells = compute_smells(&db).unwrap().open;
        let dup_pairs: Vec<&Smell> = smells
            .iter()
            .filter(|s| {
                s.kind == "duplicated_responsibility"
                    && s.remedy.contains(&ids[2])
                    && s.remedy.contains(&ids[3])
            })
            .collect();
        assert!(
            dup_pairs.is_empty(),
            "shared-file pair must route to overlapping_ownership: {smells:?}"
        );
    }

    #[test]
    fn duplicated_responsibility_falls_back_for_untagged_disjoint_code() {
        let (db, ids) = db_with_intents(2);
        update_intent_meaning(
            &db,
            &ids[0],
            Some("mail retry backoff"),
            Some("retry delivery with exponential backoff after transient failure"),
            "t0",
        )
        .unwrap();
        update_intent_meaning(
            &db,
            &ids[1],
            Some("job retry backoff"),
            Some("retry delivery with exponential backoff after transient failure"),
            "t0",
        )
        .unwrap();
        insert_codefile(&db, &codefile("f0", "src/mail_retry.rs")).unwrap();
        insert_codefile(&db, &codefile("f1", "src/job_retry.rs")).unwrap();
        insert_implements(&db, &ids[0], "f0", "", "", "t").unwrap();
        insert_implements(&db, &ids[1], "f1", "", "", "t").unwrap();

        let smells = compute_smells(&db).unwrap().open;
        let dup: Vec<&Smell> = smells
            .iter()
            .filter(|s| s.kind == "duplicated_responsibility")
            .collect();
        assert_eq!(dup.len(), 1, "{smells:?}");
        assert!(
            dup[0].evidence.contains("untagged lexical fallback"),
            "{}",
            dup[0].evidence
        );
        assert!(dup[0].evidence.contains("retry"), "{}", dup[0].evidence);
        assert!(dup[0].evidence.contains("backoff"), "{}", dup[0].evidence);

        get_or_create_relates_to(&db, &ids[0], &ids[1], "t").unwrap();
        let smells = compute_smells(&db).unwrap().open;
        assert!(
            !smells.iter().any(|s| s.kind == "duplicated_responsibility"),
            "{smells:?}"
        );
    }

    #[test]
    fn duplicated_responsibility_fallback_requires_coded_disjoint_unimported_pairs() {
        let (db, ids) = db_with_intents(6);
        update_intent_meaning(
            &db,
            &ids[0],
            Some("orphan reserve rebuild"),
            Some("reserve rebuild orphan queue"),
            "t",
        )
        .unwrap();
        update_intent_meaning(
            &db,
            &ids[1],
            Some("orphan reserve rebuild"),
            Some("reserve rebuild orphan queue"),
            "t",
        )
        .unwrap();
        update_intent_meaning(
            &db,
            &ids[2],
            Some("shared export render"),
            Some("shared export render stream"),
            "t",
        )
        .unwrap();
        update_intent_meaning(
            &db,
            &ids[3],
            Some("shared export render"),
            Some("shared export render stream"),
            "t",
        )
        .unwrap();
        update_intent_meaning(
            &db,
            &ids[4],
            Some("cache hydrate loader"),
            Some("cache hydrate loader refresh"),
            "t",
        )
        .unwrap();
        update_intent_meaning(
            &db,
            &ids[5],
            Some("cache hydrate loader"),
            Some("cache hydrate loader refresh"),
            "t",
        )
        .unwrap();
        insert_codefile(&db, &codefile("shared", "src/shared.rs")).unwrap();
        insert_codefile(&db, &codefile("from", "src/from.rs")).unwrap();
        insert_codefile(&db, &codefile("to", "src/to.rs")).unwrap();
        insert_implements(&db, &ids[2], "shared", "", "", "t").unwrap();
        insert_implements(&db, &ids[3], "shared", "", "", "t").unwrap();
        insert_implements(&db, &ids[4], "from", "", "", "t").unwrap();
        insert_implements(&db, &ids[5], "to", "", "", "t").unwrap();
        update_codefile_imports(&db, "from", &["src/to.rs".to_string()]).unwrap();

        let smells = compute_smells(&db).unwrap().open;
        for pair in [(&ids[0], &ids[1]), (&ids[2], &ids[3]), (&ids[4], &ids[5])] {
            assert!(
                !smells.iter().any(|s| {
                    s.kind == "duplicated_responsibility"
                        && s.remedy.contains(pair.0)
                        && s.remedy.contains(pair.1)
                }),
                "pair {pair:?} must not route to duplicated_responsibility: {smells:?}"
            );
        }
    }

    #[test]
    fn duplicate_detection_unarmed_fires_clears_and_adjudicates() {
        let (db, ids) = db_with_intents(3);
        insert_vocab_term(&db, &term("alpha", "first responsibility")).unwrap();
        insert_vocab_term(&db, &term("beta", "second responsibility")).unwrap();
        for (idx, id) in ids.iter().enumerate() {
            let cfid = format!("cf{idx}");
            insert_codefile(&db, &codefile(&cfid, &format!("src/f{idx}.rs"))).unwrap();
            insert_implements(&db, id, &cfid, "", "", "t1").unwrap();
        }

        let report = compute_smells(&db).unwrap();
        let unarmed: Vec<&Smell> = report
            .open
            .iter()
            .filter(|s| s.kind == "duplicate_detection_unarmed")
            .collect();
        assert_eq!(unarmed.len(), 1, "{:?}", report.open);
        assert!(
            unarmed[0].evidence.contains("3 of 3 coded intent(s)"),
            "{}",
            unarmed[0].evidence
        );

        for id in &ids {
            tag_intent(&db, id, &["alpha"]);
        }
        let report = compute_smells(&db).unwrap();
        assert!(
            !report
                .open
                .iter()
                .any(|s| s.kind == "duplicate_detection_unarmed"),
            "{:?}",
            report.open
        );

        set_intent_tags(&db, &ids[2], Vec::new(), "t2").unwrap();
        let report = compute_smells(&db).unwrap();
        assert!(
            report
                .open
                .iter()
                .any(|s| s.kind == "duplicate_detection_unarmed"),
            "{:?}",
            report.open
        );

        insert_note(
            &db,
            &note_at("n-dupe-coverage", "decision", "intent", &ids[0], "t3"),
        )
        .unwrap();
        let report = compute_smells(&db).unwrap();
        assert!(
            !report
                .open
                .iter()
                .any(|s| s.kind == "duplicate_detection_unarmed"),
            "{:?}",
            report.open
        );
        assert!(
            report
                .adjudicated
                .iter()
                .any(|s| s.kind == "duplicate_detection_unarmed"),
            "{:?}",
            report.adjudicated
        );

        insert_codefile(&db, &codefile("cf-new", "src/new.rs")).unwrap();
        insert_implements(&db, &ids[2], "cf-new", "", "", "t4").unwrap();
        let report = compute_smells(&db).unwrap();
        assert!(
            report
                .open
                .iter()
                .any(|s| s.kind == "duplicate_detection_unarmed"),
            "{:?}",
            report.open
        );
    }

    #[test]
    fn vocab_drift_smell_names_the_merge() {
        let (db, ids) = db_with_intents(2);
        insert_vocab_term(&db, &term("auth", "login, sessions")).unwrap();
        insert_vocab_term(&db, &term("authn", "who are you")).unwrap();
        insert_vocab_term(&db, &term("cache", "stored derived data")).unwrap();
        tag_intent(&db, &ids[0], &["auth"]);
        tag_intent(&db, &ids[1], &["authn"]);

        let smells = compute_smells(&db).unwrap().open;
        let drift: Vec<&Smell> = smells.iter().filter(|s| s.kind == "vocab_drift").collect();
        assert_eq!(drift.len(), 1, "{smells:?}");
        // Equal usage (1 vs 1): the tie keeps the first-ranked ('authn' here by
        // ua >= ub) — what matters is the remedy is a concrete merge command.
        assert!(
            drift[0].remedy.contains("loom vocab merge"),
            "{}",
            drift[0].remedy
        );
        assert!(!smells
            .iter()
            .any(|s| s.kind == "vocab_drift" && s.summary.contains("cache")));
    }

    #[test]
    fn discovery_ranking_rewards_tag_collisions() {
        let (db, ids) = db_with_intents(3);
        insert_vocab_term(&db, &term("lineage", "where a decision came from")).unwrap();
        tag_intent(&db, &ids[0], &["lineage"]);
        tag_intent(&db, &ids[1], &["lineage"]);

        let pairs = unexplored_pairs_scored(&db).unwrap();
        let top = &pairs[0].0;
        let tagged_pair = (top.from_id == ids[0] && top.to_id == ids[1])
            || (top.from_id == ids[1] && top.to_id == ids[0]);
        assert!(
            tagged_pair,
            "tag collision must outrank untagged pairs: {pairs:?}"
        );
        assert!(
            top.notes.contains("lineage"),
            "the why must travel: {}",
            top.notes
        );
    }

    #[test]
    fn vocab_travels_in_export_and_old_exports_still_import() {
        let (db, ids) = db_inited(2);
        insert_vocab_term(&db, &term("custody", "who may change the code")).unwrap();
        tag_intent(&db, &ids[0], &["custody"]);

        let export = export_graph(&db).unwrap();
        assert_eq!(export["nodes"]["VocabTerm"].as_array().unwrap().len(), 1);

        let db2 = GrafeoDb::in_memory();
        db2.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-2",
            "two",
            "owned",
        ))
        .unwrap();
        import_graph(&db2, &export, false).unwrap();
        assert_eq!(list_vocab_terms(&db2).unwrap().len(), 1);
        let tags = parse_tags(&get_intent(&db2, &ids[0]).unwrap().unwrap()).unwrap();
        assert_eq!(tags, vec!["custody".to_string()], "tags survive the trip");

        // An older export has no VocabTerm section and no tags property.
        let mut old = export.clone();
        old["nodes"].as_object_mut().unwrap().remove("VocabTerm");
        for i in old["nodes"]["Intent"].as_array_mut().unwrap() {
            i.as_object_mut().unwrap().remove("tags");
        }
        let db3 = GrafeoDb::in_memory();
        db3.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-3",
            "three",
            "owned",
        ))
        .unwrap();
        import_graph(&db3, &old, false).unwrap();
        assert!(list_vocab_terms(&db3).unwrap().is_empty());
        let tags = parse_tags(&get_intent(&db3, &ids[0]).unwrap().unwrap()).unwrap();
        assert!(
            tags.is_empty(),
            "absent tags read as untagged, never an error"
        );
    }

    /// The declared layer order is one atomic list on the meta sentinel:
    /// empty until declared, replaced whole, cleared with `&[]`.
    #[test]
    fn layer_order_round_trip_replace_and_clear() {
        let (db, _) = db_inited(0);
        assert!(
            get_layer_order(&db).unwrap().is_empty(),
            "fresh graph has no order"
        );
        set_layer_order(&db, &["cli".into(), "app".into(), "db".into()]).unwrap();
        assert_eq!(get_layer_order(&db).unwrap(), vec!["cli", "app", "db"]);
        set_layer_order(&db, &["ui".into(), "core".into()]).unwrap();
        assert_eq!(
            get_layer_order(&db).unwrap(),
            vec!["ui", "core"],
            "order replaces whole"
        );
        set_layer_order(&db, &[]).unwrap();
        assert!(
            get_layer_order(&db).unwrap().is_empty(),
            "clear empties the order"
        );
    }

    /// Layering: an import pointing UP the declared order is flagged; down
    /// imports, undeclared layers, and no-order graphs are silent. A recorded
    /// RELATES_TO edge does NOT excuse direction (that is the whole point —
    /// undeclared_coupling asks "declared?", this asks "right way?"). The
    /// terminal state is a decision note on the importing intent newer than
    /// its newest grounding; a new grounding re-opens.
    #[test]
    fn layering_violation_flags_imports_up_the_declared_order() {
        let (db, _) = db_inited(0);
        let mut ui = intent("ui", "panel rendering");
        ui.domain = "product-ui".into();
        ui.layer = "presentation".into();
        insert_intent(&db, &ui).unwrap();
        let mut infra = intent("infra", "storage adapters");
        infra.domain = "persistence".into();
        infra.layer = "storage".into();
        insert_intent(&db, &infra).unwrap();
        let mut misc = intent("misc", "scratch tools");
        misc.domain = "tools".into();
        misc.layer = "tools".into(); // never declared in the order
        insert_intent(&db, &misc).unwrap();
        insert_codefile(&db, &codefile("cfu", "src/ui.rs")).unwrap();
        insert_codefile(&db, &codefile("cfi", "src/infra.rs")).unwrap();
        insert_codefile(&db, &codefile("cfm", "src/misc.rs")).unwrap();
        insert_implements(&db, "ui", "cfu", "", "", "t").unwrap();
        insert_implements(&db, "infra", "cfi", "", "", "t").unwrap();
        insert_implements(&db, "misc", "cfm", "", "", "t").unwrap();
        update_codefile_imports(&db, "cfi", &["src/ui.rs".to_string()]).unwrap(); // UP
        update_codefile_imports(&db, "cfu", &["src/infra.rs".to_string()]).unwrap(); // down
        update_codefile_imports(&db, "cfm", &["src/ui.rs".to_string()]).unwrap(); // exempt

        let lv = |r: &SmellReport| {
            r.open
                .iter()
                .filter(|s| s.kind == "layering_violation")
                .count()
        };
        assert_eq!(
            lv(&compute_smells(&db).unwrap()),
            0,
            "no order declared → silent"
        );

        set_layer_order(&db, &["presentation".into(), "storage".into()]).unwrap();
        let report = compute_smells(&db).unwrap();
        assert_eq!(
            lv(&report),
            1,
            "only the up-import is flagged: {:?}",
            report.open
        );
        let s = report
            .open
            .iter()
            .find(|s| s.kind == "layering_violation")
            .unwrap();
        assert!(
            s.summary.contains("'storage adapters' (storage)"),
            "{}",
            s.summary
        );
        assert!(
            s.evidence.contains("src/infra.rs → src/ui.rs"),
            "{}",
            s.evidence
        );

        // A recorded relationship does not excuse direction.
        get_or_create_relates_to(&db, "infra", "ui", "t").unwrap();
        update_relates_to_ground(
            &db,
            "infra",
            "ui",
            "criterion long enough",
            "",
            0.9,
            "llm",
            "t",
        )
        .unwrap();
        assert_eq!(
            lv(&compute_smells(&db).unwrap()),
            1,
            "a RELATES_TO edge must not silence it"
        );

        // Terminal state: decision on the importing intent, newer than its
        // newest grounding — the finding moves to the adjudicated surface.
        insert_note(&db, &note_at("nd", "decision", "intent", "infra", "t2")).unwrap();
        let report = compute_smells(&db).unwrap();
        assert_eq!(
            lv(&report),
            0,
            "a decision newer than the grounding resolves it"
        );
        assert!(
            report
                .adjudicated
                .iter()
                .any(|a| a.kind == "layering_violation"),
            "suppressed finding must surface WITH its ruling: {:?}",
            report.adjudicated
        );

        // A new grounding on the importing intent re-opens the question.
        insert_codefile(&db, &codefile("cfi2", "src/infra2.rs")).unwrap();
        insert_implements(&db, "infra", "cfi2", "", "", "t3").unwrap();
        assert_eq!(
            lv(&compute_smells(&db).unwrap()),
            1,
            "a new grounding must re-open it"
        );

        set_layer_order(&db, &[]).unwrap();
        assert_eq!(
            lv(&compute_smells(&db).unwrap()),
            0,
            "clearing the order silences it"
        );
    }

    #[test]
    fn product_domain_order_does_not_arm_layering_violation() {
        let (db, _) = db_inited(0);
        let mut ui = intent("ui", "panel rendering");
        ui.domain = "presentation".into();
        insert_intent(&db, &ui).unwrap();
        let mut infra = intent("infra", "storage adapters");
        infra.domain = "storage".into();
        insert_intent(&db, &infra).unwrap();
        insert_codefile(&db, &codefile("cfu", "src/ui.rs")).unwrap();
        insert_codefile(&db, &codefile("cfi", "src/infra.rs")).unwrap();
        insert_implements(&db, "ui", "cfu", "", "", "t").unwrap();
        insert_implements(&db, "infra", "cfi", "", "", "t").unwrap();
        update_codefile_imports(&db, "cfi", &["src/ui.rs".to_string()]).unwrap();

        set_layer_order(&db, &["presentation".into(), "storage".into()]).unwrap();
        let report = compute_smells(&db).unwrap();
        assert!(
            report.open.iter().all(|s| s.kind != "layering_violation"),
            "product domain labels alone must not arm layering: {:?}",
            report.open
        );
    }

    /// The declared order travels: in restores AND in ports (it is design,
    /// not evidence earned against old code); absent on older exports.
    #[test]
    fn layer_order_travels_in_export_and_ports() {
        let (db, _) = db_inited(1);
        set_layer_order(&db, &["app".into(), "db".into()]).unwrap();
        let export = export_graph(&db).unwrap();
        assert_eq!(export["layer_order"], serde_json::json!(["app", "db"]));

        let db2 = GrafeoDb::in_memory();
        db2.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-2",
            "two",
            "owned",
        ))
        .unwrap();
        import_graph(&db2, &export, false).unwrap();
        assert_eq!(
            get_layer_order(&db2).unwrap(),
            vec!["app", "db"],
            "restore adopts the order"
        );

        let db3 = GrafeoDb::in_memory();
        db3.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-3",
            "three",
            "owned",
        ))
        .unwrap();
        import_graph(&db3, &export, true).unwrap();
        assert_eq!(
            get_layer_order(&db3).unwrap(),
            vec!["app", "db"],
            "a port keeps the design"
        );

        let mut old = export.clone();
        old.as_object_mut().unwrap().remove("layer_order");
        let db4 = GrafeoDb::in_memory();
        db4.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-4",
            "four",
            "owned",
        ))
        .unwrap();
        import_graph(&db4, &old, false).unwrap();
        assert!(
            get_layer_order(&db4).unwrap().is_empty(),
            "older exports read as no order"
        );
    }

    #[test]
    fn v5_export_domain_order_imports_as_layer_order_and_layers() {
        let (db, ids) = db_inited(1);
        let id = ids[0].clone();
        let now = "t1";
        db.execute("MATCH (n:Intent) SET n.domain = 'storage', n.layer = ''")
            .unwrap();
        let mut export = export_graph(&db).unwrap();
        let obj = export.as_object_mut().unwrap();
        obj.insert("schema_version".into(), serde_json::json!("5"));
        obj.remove("layer_order");
        obj.insert(
            "domain_order".into(),
            serde_json::json!(["presentation", "storage"]),
        );
        for intent in obj["nodes"]["Intent"].as_array_mut().unwrap() {
            intent.as_object_mut().unwrap().remove("layer");
        }

        let db2 = GrafeoDb::in_memory();
        db2.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            now,
            "g-legacy",
            "legacy",
            "owned",
        ))
        .unwrap();
        import_graph(&db2, &export, false).unwrap();
        assert_eq!(
            get_layer_order(&db2).unwrap(),
            vec!["presentation", "storage"],
            "legacy domain_order becomes canonical layer_order"
        );
        let imported = get_intent(&db2, &id).unwrap().unwrap();
        assert_eq!(imported.domain, "storage", "product domain is preserved");
        assert_eq!(
            imported.layer, "storage",
            "v5 domain copies into layer only when domain_order proved layer semantics"
        );
    }

    #[test]
    fn doctor_flags_unregistered_and_spammed_tags() {
        let (db, ids) = db_inited(2);
        insert_vocab_term(&db, &term("good", "a registered term")).unwrap();
        tag_intent(&db, &ids[0], &["good", "ghost"]); // ghost is unregistered
        let report = check_graph(&db).unwrap();
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.contains("ghost") && i.contains("VocabTerm")),
            "{:?}",
            report.issues
        );
        // Over the cap: bypasses the command gate by writing directly — doctor
        // must still catch it (defense in depth, like every other audit).
        set_intent_tags(
            &db,
            &ids[1],
            vec!["good".into(), "a1".into(), "a2".into(), "a3".into()],
            "t",
        )
        .unwrap();
        let report = check_graph(&db).unwrap();
        assert!(
            report.issues.iter().any(|i| i.contains("4 tags")),
            "{:?}",
            report.issues
        );
    }

    #[test]
    fn align_ranks_churned_unconfirmed_intent_first() {
        let (db, ids) = db_with_intents(2);
        let edge = get_or_create_relates_to(&db, &ids[0], &ids[1], "t0").unwrap();
        let confirm_a = note_at(
            "confirm-a",
            "confirm",
            "intent",
            &ids[0],
            "2026-01-01T00:00:00Z",
        );
        let confirm_b = note_at(
            "confirm-b",
            "confirm",
            "intent",
            &ids[1],
            "2026-01-03T00:00:00Z",
        );
        insert_note(&db, &confirm_a).unwrap();
        insert_note(&db, &confirm_b).unwrap();
        record_sync_flip(
            &db,
            "edge",
            &edge.id,
            "passing",
            "needs_reverification",
            "src/lib.rs changed",
            "2026-01-02T00:00:00Z",
        )
        .unwrap();

        let candidates = align_candidates(&db).unwrap();
        assert_eq!(candidates[0].intent.id, ids[0]);
        assert_eq!(candidates[0].churn_since_confirm, 1);
        assert!(candidates[0].score > candidates[1].score);
    }

    #[test]
    fn align_ignores_retired_intents() {
        let (db, ids) = db_with_intents(2);
        // An old confirm admits ids[1] through the grace sweep — a quiet,
        // freshly-stated intent is (correctly) not a drift suspect at all.
        insert_note(
            &db,
            &note_at(
                "confirm-b",
                "confirm",
                "intent",
                &ids[1],
                "2026-01-01T00:00:00Z",
            ),
        )
        .unwrap();
        retire_intent(&db, &ids[0], "superseded", None, "2026-01-02T00:00:00Z").unwrap();

        let candidates = align_candidates(&db).unwrap();
        assert!(candidates.iter().all(|c| c.intent.id != ids[0]));
        assert!(candidates.iter().any(|c| c.intent.id == ids[1]));
    }

    /// The align queue ADMITS only drift suspects: fresh-confirmed, unchurned
    /// meanings stay out — which is what lets the queue drain to empty, the
    /// interview's stopping condition.
    #[test]
    fn align_queue_drains_when_fresh_and_quiet() {
        let (db, ids) = db_with_intents(2);
        let edge = get_or_create_relates_to(&db, &ids[0], &ids[1], "t0").unwrap();
        let yesterday = (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339();
        insert_note(
            &db,
            &note_at("c-a", "confirm", "intent", &ids[0], &yesterday),
        )
        .unwrap();
        insert_note(
            &db,
            &note_at("c-b", "confirm", "intent", &ids[1], &yesterday),
        )
        .unwrap();
        assert!(
            align_candidates(&db).unwrap().is_empty(),
            "fresh + quiet = empty queue"
        );

        // Code churns under the shared edge → both meanings are suspects again.
        let now = chrono::Utc::now().to_rfc3339();
        record_sync_flip(
            &db,
            "edge",
            &edge.id,
            "passing",
            "needs_reverification",
            "src/lib.rs changed",
            &now,
        )
        .unwrap();
        let candidates = align_candidates(&db).unwrap();
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().all(|c| c.churn_since_confirm == 1));
    }

    /// Retirement ripples like a redefinition: verified RELATES_TO verdicts
    /// flip, and the flip is churn on the LIVING neighbour — superseding a
    /// design makes the meanings confirmed around it drift suspects.
    #[test]
    fn retirement_ripples_drift_suspicion_to_neighbours() {
        let (db, ids) = db_with_intents(2);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t0").unwrap();
        update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "criterion long enough",
            "",
            0.9,
            "llm",
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        insert_note(
            &db,
            &note_at("c-b", "confirm", "intent", &ids[1], "2026-01-01T00:00:00Z"),
        )
        .unwrap();

        retire_intent(&db, &ids[0], "superseded", None, "2026-01-02T00:00:00Z").unwrap();

        let edge = get_relates_to_between(&db, &ids[0], &ids[1])
            .unwrap()
            .unwrap();
        assert_eq!(edge.inspection_status, "needs_reverification");
        let candidates = align_candidates(&db).unwrap();
        let neighbour = candidates.iter().find(|c| c.intent.id == ids[1]).unwrap();
        assert_eq!(
            neighbour.churn_since_confirm, 1,
            "the retirement flip counts as churn"
        );
        // The cause is on record, but never pollutes the hot-FILE grouping.
        let n = notes_for_target(&db, &edge.id).unwrap().pop().unwrap();
        assert!(n.text.contains("intent 'I0' retired"), "{}", n.text);
        assert_eq!(parse_sync_cause(&n.text), None);
    }

    /// A redefinition resets the intent's OWN drift clock — its ripple flips
    /// share the redefinition timestamp, so an align-driven `intent update`
    /// must not bounce the just-rewritten meaning back to the top of the
    /// queue — while the same flips ARE churn for the neighbour, whose
    /// confirmed meaning predates the new wording.
    #[test]
    fn redefinition_resets_own_align_clock_but_churns_neighbours() {
        let (db, ids) = db_with_intents(2);
        get_or_create_relates_to(&db, &ids[0], &ids[1], "t0").unwrap();
        update_relates_to_ground(
            &db,
            &ids[0],
            &ids[1],
            "criterion long enough",
            "",
            0.9,
            "llm",
            "t1",
        )
        .unwrap();
        insert_note(
            &db,
            &note_at("c-a", "confirm", "intent", &ids[0], "2026-01-01T00:00:00Z"),
        )
        .unwrap();
        insert_note(
            &db,
            &note_at("c-b", "confirm", "intent", &ids[1], "2026-01-01T00:00:00Z"),
        )
        .unwrap();

        // The command transaction lands the decision note and the ripple with
        // ONE timestamp; mirror that here.
        let t = "2026-01-05T00:00:00Z";
        let mut redef = note_at("redef", "decision", "intent", &ids[0], t);
        redef.text = "redefined: user evolved the meaning\nwas: d".to_string();
        insert_note(&db, &redef).unwrap();
        ripple_intent_redefinition(&db, &ids[0], "I0", t).unwrap();

        let candidates = align_candidates(&db).unwrap();
        let own = candidates.iter().find(|c| c.intent.id == ids[0]).unwrap();
        assert_eq!(
            own.churn_since_confirm, 0,
            "own ripple must not count against the new wording"
        );
        let neighbour = candidates.iter().find(|c| c.intent.id == ids[1]).unwrap();
        assert_eq!(
            neighbour.churn_since_confirm, 1,
            "neighbour's confirm predates the redefinition"
        );
    }

    #[test]
    fn align_churn_before_confirm_not_counted() {
        let (db, ids) = db_with_intents(2);
        let edge = get_or_create_relates_to(&db, &ids[0], &ids[1], "t0").unwrap();
        record_sync_flip(
            &db,
            "edge",
            &edge.id,
            "passing",
            "needs_reverification",
            "src/lib.rs changed",
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        let confirm = note_at(
            "confirm-after",
            "confirm",
            "intent",
            &ids[0],
            "2026-01-02T00:00:00Z",
        );
        insert_note(&db, &confirm).unwrap();

        let candidates = align_candidates(&db).unwrap();
        let candidate = candidates.iter().find(|c| c.intent.id == ids[0]).unwrap();
        assert_eq!(candidate.churn_since_confirm, 0);
    }
    /// "This is internal, don't ask the user again": a visibility=internal
    /// ruling takes the intent OUT of the interview queue regardless of
    /// churn; clearing the ruling (what a redefinition does) re-admits it.
    #[test]
    fn align_skips_internal_until_ruling_cleared() {
        let (db, ids) = db_with_intents(2);
        let edge = get_or_create_relates_to(&db, &ids[0], &ids[1], "t0").unwrap();
        insert_note(
            &db,
            &note_at("c-a", "confirm", "intent", &ids[0], "2026-01-01T00:00:00Z"),
        )
        .unwrap();
        insert_note(
            &db,
            &note_at("c-b", "confirm", "intent", &ids[1], "2026-01-01T00:00:00Z"),
        )
        .unwrap();
        record_sync_flip(
            &db,
            "edge",
            &edge.id,
            "passing",
            "needs_reverification",
            "src/lib.rs changed",
            "2026-01-02T00:00:00Z",
        )
        .unwrap();
        assert_eq!(
            align_candidates(&db).unwrap().len(),
            2,
            "both churned → both suspects"
        );

        set_intent_visibility(&db, &ids[0], "internal", "2026-01-03T00:00:00Z").unwrap();
        let candidates = align_candidates(&db).unwrap();
        assert!(
            candidates.iter().all(|c| c.intent.id != ids[0]),
            "internal leaves the interview"
        );
        assert!(candidates.iter().any(|c| c.intent.id == ids[1]));

        // A redefinition clears the ruling ("" = untriaged) — back in the queue.
        set_intent_visibility(&db, &ids[0], "", "2026-01-04T00:00:00Z").unwrap();
        assert!(align_candidates(&db)
            .unwrap()
            .iter()
            .any(|c| c.intent.id == ids[0]));
    }

    /// The "terminology confusing, keep concept" outcome: a `--reword` stamp
    /// resets the align clock exactly like a redefinition (the meaning
    /// statement was just deliberately restated) — churn predating it stops
    /// counting and the fresh wording sits inside the grace window.
    #[test]
    fn reworded_stamp_resets_align_clock() {
        let (db, ids) = db_with_intents(2);
        let edge = get_or_create_relates_to(&db, &ids[0], &ids[1], "t0").unwrap();
        insert_note(
            &db,
            &note_at("c-a", "confirm", "intent", &ids[0], "2026-01-01T00:00:00Z"),
        )
        .unwrap();
        record_sync_flip(
            &db,
            "edge",
            &edge.id,
            "passing",
            "needs_reverification",
            "src/lib.rs changed",
            "2026-01-02T00:00:00Z",
        )
        .unwrap();
        assert!(
            align_candidates(&db)
                .unwrap()
                .iter()
                .any(|c| c.intent.id == ids[0] && c.churn_since_confirm == 1),
            "churned under an old confirm → suspect"
        );

        let now = chrono::Utc::now().to_rfc3339();
        let mut reword = note_at("rw", "decision", "intent", &ids[0], &now);
        reword.text = "reworded: clearer words for the same concept\nwas: d".to_string();
        insert_note(&db, &reword).unwrap();

        assert!(
            align_candidates(&db)
                .unwrap()
                .iter()
                .all(|c| c.intent.id != ids[0]),
            "fresh reword + no churn since = out of the queue"
        );
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
    use crate::db::{GrafeoDb, LoomDb};
    use crate::types::Intent;

    fn mk(id: &str, desc: &str) -> Intent {
        Intent {
            id: id.into(),
            name: "n".into(),
            description: desc.into(),
            abstraction_level: "feature".into(),
            domain: "d".into(),
            source_refs: Vec::new(),
            layer: String::new(),
            status: "proposed".into(),
            aspect: String::new(),
            tags: Vec::new(),
            visibility: String::new(),
            lifecycle: "implemented".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }

    #[test]
    fn esc_round_trips_adversarial_input() {
        let db = GrafeoDb::in_memory();
        db.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "g-test",
            "testgraph",
            "owned",
        ))
        .unwrap();
        let nasty = [
            "O'Brien",
            "\"double\" quote",
            "back\\slash", // a literal backslash
            "trailing\\",
            "quote'and\\back",
            "'; MATCH (n) DETACH DELETE n; //",
            "'}) DETACH DELETE (n) //",
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
            set_identity(&db, "g-test", d, "owned").unwrap();
            let meta = get_meta(&db).unwrap().expect("must read meta back");
            assert_eq!(
                &meta.graph_name, d,
                "esc/interpolation round-trip mismatch for input {k:?}"
            );
        }
        // also a real newline byte and a real tab byte
        let id = "real_ctrl";
        let d = "line1
line2	end";
        insert_intent(&db, &mk(id, d)).unwrap();
        let got = get_intent(&db, id).unwrap().unwrap();
        assert_eq!(got.description, d, "real control chars must round-trip");
        println!(
            "ESC_ROUNDTRIP ok for {} adversarial inputs",
            nasty.len() + 1
        );
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
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::from(*v)))
            .collect()
    }
    #[test]
    fn params_reliable_for_loom_shapes() {
        let db = GrafeoDB::new_in_memory();
        let s = db.session();
        let n = 8;
        let ids: Vec<String> = (0..n).map(|i| format!("i{i}")).collect();
        for id in &ids {
            s.execute_with_params(
                "INSERT (:Intent {id: $id, status: $st})",
                p(&[("id", id), ("st", "proposed")]),
            )
            .unwrap();
        }
        let (mut k, mut fails) = (0, 0);
        for i in 0..n {
            for j in (i + 1)..n {
                let eid = format!("e{k}");
                k += 1;
                s.execute_with_params(
                    "MATCH (a:Intent {id:$from}),(b:Intent {id:$to}) \
                     INSERT (a)-[:RELATES_TO {id:$eid, inspection_status:$st}]->(b)",
                    p(&[
                        ("from", &ids[i]),
                        ("to", &ids[j]),
                        ("eid", &eid),
                        ("st", "uninspected"),
                    ]),
                )
                .unwrap();
                s.execute_with_params(
                    "MATCH (a:Intent {id:$from})-[r:RELATES_TO]->(b:Intent {id:$to}) SET r.inspection_status=$st",
                    p(&[("from", &ids[i]), ("to", &ids[j]), ("st", "passing")])).unwrap();
                let r = s.execute_with_params(
                    "MATCH (a:Intent {id:$from})-[r:RELATES_TO]->(b:Intent {id:$to}) RETURN r.id AS x, r.inspection_status AS st",
                    p(&[("from", &ids[i]), ("to", &ids[j])])).unwrap();
                let ok = r
                    .rows()
                    .first()
                    .map(|row| matches!(&row[1], Value::String(s) if s.to_string()=="passing"))
                    .unwrap_or(false);
                if !ok {
                    fails += 1;
                }
            }
        }
        println!("PARAM_SPIKE edges={k} fails={fails}");
        assert_eq!(fails, 0, "param path unreliable");
    }
}
