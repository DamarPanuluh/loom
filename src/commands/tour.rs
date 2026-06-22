//! `loom tour` — a guided comprehension walkthrough of the intent graph in
//! dependency/decomposition order, so an agent or human understands the system
//! FAST. Each stop reads back what a part is SUPPOSED to do (its criterion),
//! where it's realized, what it depends on, and — uniquely to loom — whether it
//! is PROVEN. A pure read-only projection of the graph (like `wiki`/`explain`).

use std::cmp::Reverse;
use std::collections::HashMap;

use anyhow::Result;

use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;
use crate::types::Intent;

struct Stop {
    id: String,
    name: String,
    level: String,
    domain: String,
    layer: String,
    supposed_to: String,
    grounded: Vec<(String, String, String)>, // path, locator, status
    proven: Option<bool>,                    // None = non-leaf (proven via leaves)
    proof_detail: String,
    depends_on: Vec<(String, String)>, // other intent name, kinds joined
    children: Vec<String>,
    read_files: Vec<String>,
}

pub fn run(target: Option<&str>, limit: usize, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&cwd)?;
    let snap = store.query_snapshot()?;

    let by_id: HashMap<&str, &Intent> = snap.intents.iter().map(|i| (i.id.as_str(), i)).collect();

    // Hierarchy maps.
    let mut parent_of: HashMap<&str, &str> = HashMap::new();
    let mut children_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for (parent, child) in &snap.hierarchy {
        parent_of.insert(child.as_str(), parent.as_str());
        children_of
            .entry(parent.as_str())
            .or_default()
            .push(child.as_str());
    }

    // The set of intents to tour: a subtree if a target is given, else all.
    let ordering: Vec<&str> = if let Some(t) = target {
        let root = resolve(&snap.intents, t)?;
        let mut subtree: Vec<&str> = Vec::new();
        collect_subtree(root, &children_of, &mut subtree);
        comprehension_order(&subtree, &by_id, &parent_of, &snap.degrees)
    } else {
        let all: Vec<&str> = snap.intents.iter().map(|i| i.id.as_str()).collect();
        comprehension_order(&all, &by_id, &parent_of, &snap.degrees)
    };

    if ordering.is_empty() {
        let msg = "No intents to tour yet — seed the graph first (`loom seed --suggest`, then \
                   `loom intent add`).";
        if printer.json {
            printer.print_json(&serde_json::json!({ "stops": [], "total": 0, "next_step": msg }));
        } else {
            println!("{msg}");
        }
        return Ok(());
    }

    let total = ordering.len();
    let shown_ids: Vec<&str> = if limit == 0 {
        ordering
    } else {
        ordering.into_iter().take(limit).collect()
    };

    // Per-intent passing-proof lookup.
    let result_by_vid: HashMap<&str, &str> = snap
        .validations
        .iter()
        .map(|v| (v.id.as_str(), v.last_result.as_str()))
        .collect();
    let mut passing_proofs: HashMap<&str, usize> = HashMap::new();
    for ve in &snap.validates {
        if result_by_vid.get(ve.validation_id.as_str()) == Some(&"passed") {
            *passing_proofs.entry(ve.intent_id.as_str()).or_default() += 1;
        }
    }

    let stops: Vec<Stop> = shown_ids
        .iter()
        .filter_map(|id| {
            by_id
                .get(id)
                .map(|intent| build_stop(intent, &snap, &children_of, &passing_proofs))
        })
        .collect();

    let graph_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("graph")
        .to_string();

    if printer.json {
        render_json(&stops, &graph_name, total, printer);
    } else {
        render_human(&stops, &graph_name, total);
    }
    Ok(())
}

fn build_stop(
    intent: &Intent,
    snap: &crate::db::queries::QuerySnapshot,
    children_of: &HashMap<&str, Vec<&str>>,
    passing_proofs: &HashMap<&str, usize>,
) -> Stop {
    let mut grounded = Vec::new();
    let mut read_files = Vec::new();
    for im in &snap.implements {
        if im.intent_id == intent.id {
            grounded.push((
                im.codefile_path.clone(),
                im.locator.clone(),
                im.inspection_status.clone(),
            ));
            if !read_files.contains(&im.codefile_path) {
                read_files.push(im.codefile_path.clone());
            }
        }
    }
    let mut depends_on = Vec::new();
    for e in &snap.relates {
        if e.inspection_status == "independent" {
            continue;
        }
        let other = if e.from_id == intent.id {
            Some((&e.to_name, &e.kinds))
        } else if e.to_id == intent.id {
            Some((&e.from_name, &e.kinds))
        } else {
            None
        };
        if let Some((name, kinds)) = other {
            depends_on.push((name.clone(), kinds.join(", ")));
        }
    }
    let children: Vec<String> = children_of
        .get(intent.id.as_str())
        .map(|cs| {
            cs.iter()
                .filter_map(|c| {
                    snap.intents
                        .iter()
                        .find(|i| i.id == **c)
                        .map(|i| i.name.clone())
                })
                .collect()
        })
        .unwrap_or_default();

    let is_leaf = children.is_empty();
    let passing = passing_proofs.get(intent.id.as_str()).copied().unwrap_or(0);
    let (proven, proof_detail) = if !is_leaf {
        (None, "proven through its leaves".to_string())
    } else if passing > 0 {
        (Some(true), format!("{passing} passing validation(s)"))
    } else {
        (Some(false), "no passing proof yet".to_string())
    };

    let supposed_to = if !intent.criterion.trim().is_empty() {
        intent.criterion.clone()
    } else {
        intent.description.clone()
    };

    Stop {
        id: intent.id.clone(),
        name: intent.name.clone(),
        level: intent.abstraction_level.clone(),
        domain: intent.domain.clone(),
        layer: intent.layer.clone(),
        supposed_to,
        grounded,
        proven,
        proof_detail,
        depends_on,
        children,
        read_files,
    }
}

/// Comprehension order: hierarchy depth (roots first → drill in), then
/// abstraction altitude (system before feature), then RELATES_TO centrality
/// (most-connected/foundational first), then name. A parent always precedes its
/// children, so the tour reads big-picture → detail.
fn comprehension_order<'a>(
    ids: &[&'a str],
    by_id: &HashMap<&str, &Intent>,
    parent_of: &HashMap<&str, &str>,
    degrees: &HashMap<String, i64>,
) -> Vec<&'a str> {
    let depth = |id: &str| -> usize {
        let mut d = 0;
        let mut cur = id;
        // Acyclic by HIERARCHY invariant; bound the walk defensively anyway.
        for _ in 0..1024 {
            match parent_of.get(cur) {
                Some(p) => {
                    d += 1;
                    cur = p;
                }
                None => break,
            }
        }
        d
    };
    let mut out: Vec<&str> = ids.to_vec();
    out.sort_by_cached_key(|id| {
        let intent = by_id.get(id);
        let level_rank = match intent.map(|i| i.abstraction_level.as_str()) {
            Some("system") => 0,
            Some("component") => 1,
            Some("feature") => 2,
            Some("cross_cutting") => 3,
            _ => 4,
        };
        let deg = degrees.get(*id).copied().unwrap_or(0);
        let name = intent.map(|i| i.name.clone()).unwrap_or_default();
        (depth(id), level_rank, Reverse(deg), name)
    });
    out
}

fn collect_subtree<'a>(
    root: &'a str,
    children_of: &HashMap<&'a str, Vec<&'a str>>,
    out: &mut Vec<&'a str>,
) {
    if out.contains(&root) {
        return; // defensive against a malformed cycle
    }
    out.push(root);
    if let Some(children) = children_of.get(root) {
        for c in children {
            collect_subtree(c, children_of, out);
        }
    }
}

/// Resolve a tour target (exact id / exact name / unique case-insensitive
/// fragment) to an intent id.
fn resolve<'a>(intents: &'a [Intent], target: &str) -> Result<&'a str> {
    if let Some(i) = intents.iter().find(|i| i.id == target || i.name == target) {
        return Ok(i.id.as_str());
    }
    let needle = target.to_lowercase();
    let hits: Vec<&Intent> = intents
        .iter()
        .filter(|i| i.name.to_lowercase().contains(&needle))
        .collect();
    match hits.as_slice() {
        [one] => Ok(one.id.as_str()),
        [] => anyhow::bail!(
            "Nothing matches '{target}' as an intent (id / name / unique fragment). \
             Try `loom find \"{target}\"` or `loom tour` for the whole graph."
        ),
        many => {
            let names: Vec<&str> = many.iter().take(6).map(|i| i.name.as_str()).collect();
            anyhow::bail!(
                "'{target}' matches {} intents: {} — refine the fragment or pass an id.",
                many.len(),
                names.join(", ")
            )
        }
    }
}

fn proof_glyph(stop: &Stop) -> String {
    match stop.proven {
        Some(true) => format!("✓ proven ({})", stop.proof_detail),
        Some(false) => format!("✗ {}", stop.proof_detail),
        None => format!("— {}", stop.proof_detail),
    }
}

fn render_human(stops: &[Stop], graph_name: &str, total: usize) {
    println!(
        "── loom tour: {graph_name} ({} of {total} intents, comprehension order) ──",
        stops.len()
    );
    println!("  Read top-down: systems first, then the parts they decompose into. Each stop =");
    println!("  what it's SUPPOSED to do · where it's realized · whether it's PROVEN.");
    println!();
    for (n, s) in stops.iter().enumerate() {
        let facet = [
            (!s.domain.is_empty()).then(|| format!("domain: {}", s.domain)),
            (!s.layer.is_empty()).then(|| format!("layer: {}", s.layer)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let facet = if facet.is_empty() {
            String::new()
        } else {
            format!("   · {facet}")
        };
        println!("  {}. [{}] {}{}", n + 1, s.level, s.name, facet);
        if !s.supposed_to.trim().is_empty() {
            println!("     supposed to: {}", s.supposed_to);
        }
        if !s.grounded.is_empty() {
            let g = s
                .grounded
                .iter()
                .take(3)
                .map(|(p, l, st)| {
                    if l.is_empty() {
                        format!("{p} [{st}]")
                    } else {
                        format!("{p} @ {l} [{st}]")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            println!("     grounded:    {g}");
        }
        println!("     proof:       {}", proof_glyph(s));
        if !s.depends_on.is_empty() {
            let d = s
                .depends_on
                .iter()
                .take(4)
                .map(|(name, kinds)| {
                    if kinds.is_empty() {
                        name.clone()
                    } else {
                        format!("{name} ({kinds})")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            println!("     depends on:  {d}");
        }
        if !s.children.is_empty() {
            println!("     decomposes:  {}", s.children.join(", "));
        }
        println!();
    }
    if stops.len() < total {
        println!(
            "  … +{} more — `loom tour <intent>` to drill into one subtree, or `--limit 0` for all.",
            total - stops.len()
        );
    }
    println!(
        "  → Next: read the files named under each stop; `loom explain <intent>` for full detail."
    );
    println!("  Terminal state = the MATURITY LADDER at Production-ready (`loom status` / `loom complete`):");
    println!(
        "  Seeded → Realized → Proven → Hardened → Production-ready; focus = the lowest unmet rung. RECORD ≠ DISCHARGE."
    );
}

fn render_json(stops: &[Stop], graph_name: &str, total: usize, printer: &Printer) {
    let items: Vec<serde_json::Value> = stops
        .iter()
        .enumerate()
        .map(|(n, s)| {
            serde_json::json!({
                "order": n + 1,
                "id": s.id,
                "name": s.name,
                "level": s.level,
                "domain": s.domain,
                "layer": s.layer,
                "supposed_to": s.supposed_to,
                "grounded_in": s.grounded.iter().map(|(p, l, st)| serde_json::json!({
                    "path": p, "locator": l, "status": st,
                })).collect::<Vec<_>>(),
                "proven": s.proven,
                "proof": s.proof_detail,
                "depends_on": s.depends_on.iter().map(|(name, kinds)| serde_json::json!({
                    "intent": name, "kinds": kinds,
                })).collect::<Vec<_>>(),
                "decomposes_into": s.children,
                "read_files": s.read_files,
            })
        })
        .collect();
    printer.print_json(&serde_json::json!({
        "graph": graph_name,
        "stops": items,
        "shown": stops.len(),
        "total": total,
        "truncated": stops.len() < total,
        "next_step": "Read the files named in each stop's read_files in order; `loom tour <intent>` drills into a subtree, `loom explain <intent>` gives full detail. Terminal state = the maturity ladder at Production-ready (Seeded → Realized → Proven → Hardened → Production-ready; focus = the lowest unmet rung) — RECORD ≠ DISCHARGE.",
    }));
}
