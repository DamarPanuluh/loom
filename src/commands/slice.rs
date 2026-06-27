//! `loom slice plan` — horizontal work territories (the scheduling-advice fact).
//!
//! The vertical axis (role lanes) answers "what DISCIPLINE is this agent
//! using?". This command computes the horizontal axis — "what TERRITORY may it
//! touch?". A slice is a conservative cluster of related intents (one top-level
//! intent subtree) plus the codefile footprint they ground to. Two slices that
//! share a codefile, or are joined by a RELATES_TO edge, CONFLICT: an
//! orchestrator must never hand overlapping territory to parallel code-editing
//! agents.
//!
//! This is a FACT command — idempotent, read-only, a pure function of the graph
//! snapshot. It ADVISES territory; it never spawns, never executes, and carries
//! no imperative. The DECISION to dispatch belongs to the orchestrator hat
//! (`loom guide --mode orchestrate`), never to this output.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::Result;
use serde::Serialize;

use crate::cli::SliceCmd;
use crate::db::queries::QuerySnapshot;
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;
use crate::types::{Implements, Intent, RelatesTo};

/// Read-only graph-fact lanes: they record verdicts/proofs and never edit code,
/// so they are safe to run in parallel on disjoint slices (intra-slice work —
/// honor `conflicts_with` for the cross-slice edges).
const SAFE_LANES: &[&str] = &["discovery", "quality", "validate", "review", "prove"];
/// Code-editing lanes: only one agent may hold a slice at a time.
const EXCLUSIVE_LANES: &[&str] = &["build", "fix", "refactor"];

#[derive(Debug, Clone, Serialize)]
pub struct SliceIntent {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SliceConflict {
    /// The conflicting slice's id (a slug, e.g. `slice:auth`).
    pub slice: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Slice {
    pub id: String,
    /// The top-level intent subtree this slice is anchored on (its name).
    pub anchor: String,
    pub kind: &'static str,
    pub intents: Vec<SliceIntent>,
    pub intent_count: usize,
    /// Codefiles grounded by this slice's intents (its exclusive territory).
    pub codefiles: Vec<String>,
    /// Codefiles ALSO touched by another slice — the central/shared files that
    /// force a conflict (and `serial` code-editing).
    pub shared_codefiles: Vec<String>,
    pub safe_lanes: Vec<&'static str>,
    pub exclusive_lanes: Vec<&'static str>,
    /// Conservative class for CODE-EDITING work on this slice:
    /// `exclusive_slice` (disjoint — one writer, parallel with other slices) or
    /// `serial` (shares territory — no safe partition provable). Read-only lanes
    /// are always safe; see `safe_lanes`.
    pub parallel_safety: &'static str,
    /// Model-neutral capability tier — a statement about the WORK, never a model.
    pub effort: &'static str,
    pub risk: &'static str,
    pub conflicts_with: Vec<SliceConflict>,
}

// ---------------------------------------------------------------------------
// Slice computation — a pure function of plain graph parts (testable without a
// live store). Anchored on top-level intent subtrees rather than raw connected
// components: loom's own graph is one dense component, so components alone would
// collapse into a single useless mega-slice.
// ---------------------------------------------------------------------------

/// Every intent's slice ANCHOR: the top-level subtree it belongs to — the
/// highest ancestor that is still a direct child of an absolute root, or the
/// intent itself when it is a root or an orphan.
fn anchor_of(id: &str, parent: &HashMap<String, String>) -> String {
    let mut cur = id.to_string();
    for _ in 0..10_000 {
        match parent.get(&cur) {
            // parent has its own parent → keep climbing
            Some(p) if parent.contains_key(p) => cur = p.clone(),
            // parent is a root → `cur` is the top-level subtree anchor
            Some(_) => return cur,
            // no parent → `cur` is a root/orphan = its own anchor
            None => return cur,
        }
    }
    cur
}

fn slugify(name: &str) -> String {
    let mut s = String::new();
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            s.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !s.is_empty() {
            s.push('-');
            prev_dash = true;
        }
    }
    while s.ends_with('-') {
        s.pop();
    }
    if s.is_empty() {
        s.push_str("slice");
    }
    s
}

fn unique_slice_id(name: &str, used: &mut HashSet<String>) -> String {
    let base = format!("slice:{}", slugify(name));
    if used.insert(base.clone()) {
        return base;
    }
    let mut n = 2;
    loop {
        let cand = format!("{base}-{n}");
        if used.insert(cand.clone()) {
            return cand;
        }
        n += 1;
    }
}

pub fn compute_slices(snap: &QuerySnapshot) -> Vec<Slice> {
    compute_slices_from_parts(
        &snap.intents,
        &snap.implements,
        &snap.hierarchy,
        &snap.relates,
    )
}

pub fn compute_slices_from_parts(
    intents: &[Intent],
    implements: &[Implements],
    hierarchy: &[(String, String)],
    relates: &[RelatesTo],
) -> Vec<Slice> {
    let intent_ids: HashSet<&str> = intents.iter().map(|i| i.id.as_str()).collect();

    // parent map (child -> parent), restricted to edges between active intents.
    let mut parent: HashMap<String, String> = HashMap::new();
    for (p, c) in hierarchy {
        if intent_ids.contains(p.as_str()) && intent_ids.contains(c.as_str()) {
            parent.insert(c.clone(), p.clone());
        }
    }

    // intent -> anchor, and anchor -> members (BTreeMap for deterministic order)
    let anchor_of_intent: HashMap<&str, String> = intents
        .iter()
        .map(|i| (i.id.as_str(), anchor_of(&i.id, &parent)))
        .collect();
    let mut members: BTreeMap<String, Vec<&Intent>> = BTreeMap::new();
    for i in intents {
        members
            .entry(anchor_of_intent[i.id.as_str()].clone())
            .or_default()
            .push(i);
    }

    // codefile footprint per anchor
    let mut footprint: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for im in implements {
        if let Some(anchor) = anchor_of_intent.get(im.intent_id.as_str()) {
            footprint
                .entry(anchor.clone())
                .or_default()
                .insert(im.codefile_path.clone());
        }
    }
    // central files: path -> the anchors that touch it. BTreeMap (not HashMap)
    // so the shared-file conflict reason is deterministic — the loop below picks
    // the lexicographically-smallest shared path, making `slice plan` idempotent.
    let mut file_anchors: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (anchor, files) in &footprint {
        for f in files {
            file_anchors
                .entry(f.clone())
                .or_default()
                .insert(anchor.clone());
        }
    }

    // conflicts keyed by anchor -> (other anchor -> reason)
    let mut conflicts: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (path, anchors) in &file_anchors {
        if anchors.len() < 2 {
            continue;
        }
        for a in anchors {
            for b in anchors {
                if a != b {
                    conflicts
                        .entry(a.clone())
                        .or_default()
                        .entry(b.clone())
                        .or_insert_with(|| format!("shares codefile {path}"));
                }
            }
        }
    }
    for e in relates {
        if e.inspection_status == "independent" {
            continue;
        }
        let (Some(a), Some(b)) = (
            anchor_of_intent.get(e.from_id.as_str()),
            anchor_of_intent.get(e.to_id.as_str()),
        ) else {
            continue;
        };
        if a == b {
            continue;
        }
        let reason = "joined by a RELATES_TO edge".to_string();
        conflicts
            .entry(a.clone())
            .or_default()
            .entry(b.clone())
            .or_insert_with(|| reason.clone());
        conflicts
            .entry(b.clone())
            .or_default()
            .entry(a.clone())
            .or_insert(reason);
    }

    // Assign slice ids first (anchor -> slug) so conflicts can reference them.
    let mut used_ids: HashSet<String> = HashSet::new();
    let mut slice_id_of: BTreeMap<String, String> = BTreeMap::new();
    for (anchor, mem) in &members {
        let anchor_name = mem
            .iter()
            .find(|i| i.id == *anchor)
            .map(|i| i.name.as_str())
            .unwrap_or(anchor.as_str());
        slice_id_of.insert(anchor.clone(), unique_slice_id(anchor_name, &mut used_ids));
    }

    let empty_files = BTreeSet::new();
    let mut slices: Vec<Slice> = Vec::new();
    for (anchor, mem) in &members {
        let files = footprint.get(anchor).unwrap_or(&empty_files);
        let shared: Vec<String> = files
            .iter()
            .filter(|f| file_anchors.get(*f).map(|s| s.len() > 1).unwrap_or(false))
            .cloned()
            .collect();
        let conf = conflicts.get(anchor).cloned().unwrap_or_default();
        let central = !shared.is_empty();
        let has_conflict = !conf.is_empty();
        let has_cross_cutting = mem.iter().any(|i| i.abstraction_level == "cross_cutting");

        let parallel_safety = if central || has_conflict {
            "serial"
        } else {
            "exclusive_slice"
        };
        let effort = if has_cross_cutting || central || mem.len() >= 8 {
            "high"
        } else if mem
            .iter()
            .any(|i| i.abstraction_level == "component" || i.abstraction_level == "system")
            || mem.len() >= 3
        {
            "mid"
        } else {
            "low"
        };
        let risk = if central || has_conflict || has_cross_cutting {
            "cross_cutting"
        } else {
            "local"
        };

        let anchor_name = mem
            .iter()
            .find(|i| i.id == *anchor)
            .map(|i| i.name.clone())
            .unwrap_or_else(|| anchor.clone());

        let mut intents_v: Vec<SliceIntent> = mem
            .iter()
            .map(|i| SliceIntent {
                id: i.id.clone(),
                name: i.name.clone(),
            })
            .collect();
        intents_v.sort_by(|a, b| a.id.cmp(&b.id));

        let mut conflicts_with: Vec<SliceConflict> = conf
            .into_iter()
            .filter_map(|(other_anchor, reason)| {
                slice_id_of.get(&other_anchor).map(|slice| SliceConflict {
                    slice: slice.clone(),
                    reason,
                })
            })
            .collect();
        conflicts_with.sort_by(|a, b| a.slice.cmp(&b.slice));

        slices.push(Slice {
            id: slice_id_of[anchor].clone(),
            anchor: anchor_name,
            kind: "intent_subtree",
            intent_count: intents_v.len(),
            intents: intents_v,
            codefiles: files.iter().cloned().collect(),
            shared_codefiles: shared,
            safe_lanes: SAFE_LANES.to_vec(),
            exclusive_lanes: EXCLUSIVE_LANES.to_vec(),
            parallel_safety,
            effort,
            risk,
            conflicts_with,
        });
    }

    slices.sort_by(|a, b| a.id.cmp(&b.id));
    slices
}

/// The intent-id territory of one slice — the chokepoint `loom next --slice`
/// restricts its queue to. Errors (listing the available slice ids) when the id
/// is unknown, so a typo fails loudly instead of silently serving the global
/// queue.
pub fn slice_intent_ids(snap: &QuerySnapshot, slice_id: &str) -> Result<HashSet<String>> {
    let slices = compute_slices(snap);
    if let Some(s) = slices.iter().find(|s| s.id == slice_id) {
        return Ok(s.intents.iter().map(|i| i.id.clone()).collect());
    }
    let available: Vec<&str> = slices.iter().map(|s| s.id.as_str()).collect();
    anyhow::bail!(
        "Unknown slice '{slice_id}'. Run `loom slice plan` for the territory map. Available: {}",
        available.join(", ")
    )
}

// ---------------------------------------------------------------------------
// Command surface
// ---------------------------------------------------------------------------

pub fn run(cmd: SliceCmd, printer: &Printer) -> Result<()> {
    match cmd {
        SliceCmd::Plan => {
            let cwd = crate::db::resolve_root()?;
            let store = GraphReadHandle::open(&cwd)?;
            run_plan_with_db(&store, printer)
        }
    }
}

const PLAN_NOTE: &str = "Territory advice only — loom never spawns. safe_lanes are safe for INTRA-slice \
    work on DISJOINT slices; honor conflicts_with (serialize code-editing across conflicting slices). \
    The DECISION to dispatch is the orchestrator hat's: loom guide --mode orchestrate. \
    Queue a slice with: loom next --mode <lane> --slice <id>.";

fn run_plan_with_db(db: &dyn GraphReadRepository, printer: &Printer) -> Result<()> {
    let snap = db.query_snapshot()?;
    let slices = compute_slices(&snap);

    if printer.json {
        printer.print_json(&serde_json::json!({
            "graph": {
                "intents": snap.intents.len(),
                "codefiles": snap.codefiles.len(),
                "slices": slices.len(),
            },
            "note": PLAN_NOTE,
            "slices": slices,
        }));
        return Ok(());
    }

    println!("── loom slice plan ───────────────────────────────────────────────────");
    println!(
        "  {} slice(s) over {} intents · {} codefiles",
        slices.len(),
        snap.intents.len(),
        snap.codefiles.len()
    );
    println!();
    for s in &slices {
        println!("  {}  [{}]", s.id, s.parallel_safety);
        println!(
            "    anchor: {}  ({} intent(s) · effort {} · risk {})",
            s.anchor, s.intent_count, s.effort, s.risk
        );
        if !s.codefiles.is_empty() {
            println!("    codefiles: {}", s.codefiles.join(", "));
        }
        if !s.shared_codefiles.is_empty() {
            println!("    shared:    {}", s.shared_codefiles.join(", "));
        }
        for c in &s.conflicts_with {
            println!("    conflicts: {} ({})", c.slice, c.reason);
        }
        println!();
    }
    println!("  {PLAN_NOTE}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(id: &str, name: &str, level: &str) -> Intent {
        Intent {
            id: id.into(),
            name: name.into(),
            description: name.into(),
            criterion: String::new(),
            abstraction_level: level.into(),
            domain: String::new(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "active".into(),
            aspect: String::new(),
            tags: Vec::new(),
            visibility: "internal".into(),
            boundary: String::new(),
            lifecycle: "implemented".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }

    fn grounding(intent_id: &str, path: &str) -> Implements {
        Implements {
            id: format!("im:{intent_id}:{path}"),
            intent_id: intent_id.into(),
            codefile_id: path.into(),
            intent_name: intent_id.into(),
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

    fn relates(from: &str, to: &str, status: &str) -> RelatesTo {
        RelatesTo {
            id: format!("rel:{from}:{to}"),
            from_id: from.into(),
            to_id: to.into(),
            from_name: from.into(),
            to_name: to.into(),
            inspection_status: status.into(),
            criterion: String::new(),
            confidence: 0.0,
            evidence: String::new(),
            last_inspected: String::new(),
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

    /// (intents, implements, hierarchy, relates) — the plain parts a slice test
    /// feeds `compute_slices_from_parts`.
    type GraphParts = (
        Vec<Intent>,
        Vec<Implements>,
        Vec<(String, String)>,
        Vec<RelatesTo>,
    );

    /// root → two component subtrees, disjoint codefiles → two disjoint,
    /// conflict-free, parallel-safe slices.
    fn disjoint_graph() -> GraphParts {
        let intents = vec![
            intent("root", "loom", "system"),
            intent("auth", "auth", "component"),
            intent("auth_login", "login", "feature"),
            intent("pay", "payments", "component"),
            intent("pay_charge", "charge", "feature"),
        ];
        let implements = vec![
            grounding("auth_login", "src/auth.rs"),
            grounding("pay_charge", "src/pay.rs"),
        ];
        let hierarchy = vec![
            ("root".into(), "auth".into()),
            ("auth".into(), "auth_login".into()),
            ("root".into(), "pay".into()),
            ("pay".into(), "pay_charge".into()),
        ];
        (intents, implements, hierarchy, Vec::new())
    }

    #[test]
    fn slice_plan_emits_disjoint_slices() {
        let (i, im, h, r) = disjoint_graph();
        let slices = compute_slices_from_parts(&i, &im, &h, &r);
        // root is its own singleton anchor; auth and pay are the two components.
        let ids: Vec<&str> = slices.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"slice:auth"), "auth slice present: {ids:?}");
        assert!(
            ids.contains(&"slice:payments"),
            "payments slice present: {ids:?}"
        );

        let auth = slices.iter().find(|s| s.id == "slice:auth").unwrap();
        let pay = slices.iter().find(|s| s.id == "slice:payments").unwrap();
        // disjoint codefile footprints
        let inter: Vec<&String> = auth
            .codefiles
            .iter()
            .filter(|f| pay.codefiles.contains(f))
            .collect();
        assert!(inter.is_empty(), "footprints disjoint");
        // no conflict, code-editing is exclusive_slice (parallel across slices)
        assert!(auth.conflicts_with.is_empty());
        assert_eq!(auth.parallel_safety, "exclusive_slice");
        assert_eq!(pay.parallel_safety, "exclusive_slice");
    }

    #[test]
    fn slice_plan_marks_shared_file_serial() {
        let (i, mut im, h, _r) = disjoint_graph();
        // make pay_charge also ground the auth file → shared/central → conflict
        im.push(grounding("pay_charge", "src/auth.rs"));
        let slices = compute_slices_from_parts(&i, &im, &h, &Vec::new());
        let auth = slices.iter().find(|s| s.id == "slice:auth").unwrap();
        let pay = slices.iter().find(|s| s.id == "slice:payments").unwrap();
        assert_eq!(auth.parallel_safety, "serial", "shared file forces serial");
        assert_eq!(pay.parallel_safety, "serial");
        assert!(auth
            .conflicts_with
            .iter()
            .any(|c| c.slice == "slice:payments"));
        assert!(auth.shared_codefiles.contains(&"src/auth.rs".to_string()));
    }

    #[test]
    fn slice_plan_cross_edge_conflicts() {
        let (i, im, h, _r) = disjoint_graph();
        let r = vec![relates("auth_login", "pay_charge", "passing")];
        let slices = compute_slices_from_parts(&i, &im, &h, &r);
        let auth = slices.iter().find(|s| s.id == "slice:auth").unwrap();
        assert!(
            auth.conflicts_with
                .iter()
                .any(|c| c.slice == "slice:payments"),
            "cross-slice RELATES_TO is a conflict"
        );
        assert_eq!(auth.parallel_safety, "serial");
    }

    #[test]
    fn slice_plan_is_idempotent() {
        let (i, im, h, r) = disjoint_graph();
        let a = compute_slices_from_parts(&i, &im, &h, &r);
        let b = compute_slices_from_parts(&i, &im, &h, &r);
        let aj = serde_json::to_string(&a).unwrap();
        let bj = serde_json::to_string(&b).unwrap();
        assert_eq!(aj, bj, "identical graph yields identical slices");
    }

    #[test]
    fn next_slice_filters_to_territory() {
        let (i, im, h, r) = disjoint_graph();
        // build a snapshot from parts to exercise the real lookup + filter path
        let snap = QuerySnapshot::from_parts(
            i,
            h,
            r,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            im,
            Vec::new(),
            None,
        );
        let auth = slice_intent_ids(&snap, "slice:auth").unwrap();
        assert!(auth.contains("auth_login"));
        assert!(!auth.contains("pay_charge"), "other slice excluded");
        assert!(slice_intent_ids(&snap, "slice:nope").is_err());

        // the next-filter chokepoint: restricted_to keeps only the slice's
        // intents (and their groundings) — everything else is gone.
        let scoped = snap.restricted_to(&auth);
        let ids: Vec<&str> = scoped.intents.iter().map(|i| i.id.as_str()).collect();
        assert!(ids.contains(&"auth_login"));
        assert!(
            !ids.contains(&"pay_charge"),
            "restricted snapshot excludes other territory"
        );
        assert!(
            scoped
                .implements
                .iter()
                .all(|im| auth.contains(&im.intent_id)),
            "groundings restricted to the slice"
        );
    }
}
