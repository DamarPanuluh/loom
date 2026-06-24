//! `loom explain` — intent intelligence. Given a file or an intent, answer what
//! it IS, what it's FOR (its code groundings), what it's coupled to and BY WHAT
//! KIND (the relationship taxonomy), what governs it, and what RIPPLES if you
//! change it. A read-only, answer-shaped projection of the graph centered on one
//! node — the "why/intent" layer grep and LSP can't give.

use std::collections::HashMap;

use anyhow::Result;

use crate::db::queries::QuerySnapshot;
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;
use crate::types::{relates_stales_on_code_change, CodeFile, Intent, QualityRule, RelationKind};

pub fn run(target: &str, impact: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&cwd)?;
    run_with_db(&store, target, impact, printer)
}

pub fn run_with_db(
    db: &dyn GraphReadRepository,
    target: &str,
    impact: bool,
    printer: &Printer,
) -> Result<()> {
    let snap = db.query_snapshot()?;
    // Resolve + name against the UNFILTERED intent set (query_snapshot drops
    // deprecated intents; `intent show`/`list` keep them, so explain must too —
    // otherwise a deprecated target is wrongly "not found").
    let all_intents = db.list_intents(None, None)?;

    let resolved = resolve_target(&all_intents, &snap.codefiles, target);
    let (ids, file_ctx): (Vec<String>, Option<String>) = match resolved {
        Resolved::Intent(id) => (vec![id], None),
        Resolved::File(path) => {
            let mut intents: Vec<String> = Vec::new();
            for im in &snap.implements {
                if im.codefile_path == path && !intents.contains(&im.intent_id) {
                    intents.push(im.intent_id.clone());
                }
            }
            if intents.is_empty() {
                return render_uncovered_file(&path, printer);
            }
            (intents, Some(path))
        }
        Resolved::Ambiguous(names) => {
            let shown = names.iter().take(6).cloned().collect::<Vec<_>>().join(", ");
            let more = names.len().saturating_sub(6);
            anyhow::bail!(
                "'{target}' matches {} intents: {shown}{} — refine the fragment or pass an id \
                 (`loom intent list`).",
                names.len(),
                if more > 0 {
                    format!(", …+{more}")
                } else {
                    String::new()
                }
            );
        }
        Resolved::None => anyhow::bail!(
            "Nothing matches '{target}' as an intent (id / exact name / unique fragment) or a \
             registered file path. Try `loom find \"{target}\"` or `loom intent list`."
        ),
    };

    let by_id: HashMap<&str, &Intent> = all_intents.iter().map(|i| (i.id.as_str(), i)).collect();
    let explanations: Vec<Explanation> = ids
        .iter()
        .filter_map(|id| {
            by_id
                .get(id.as_str())
                .map(|i| build_explanation(&snap, &by_id, i))
        })
        .collect();

    let label = file_ctx.clone().unwrap_or_else(|| {
        explanations
            .first()
            .map(|e| e.name.clone())
            .unwrap_or_default()
    });

    if impact {
        render_impact(&explanations, &label, printer);
    } else if printer.json {
        render_json(&explanations, file_ctx.as_deref(), printer);
    } else {
        render_human(&explanations, file_ctx.as_deref());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Target resolution: intent first (loom's primary node), then file. Distinguishes
// not-found from ambiguous, and never silently picks a first match.
// ---------------------------------------------------------------------------

enum Resolved {
    Intent(String),
    File(String),
    Ambiguous(Vec<String>),
    None,
}

fn resolve_target(intents: &[Intent], codefiles: &[CodeFile], target: &str) -> Resolved {
    if target.trim().is_empty() {
        return Resolved::None;
    }
    if let Some(i) = intents.iter().find(|i| i.id == target) {
        return Resolved::Intent(i.id.clone());
    }
    let lower = target.to_lowercase();

    let exact: Vec<&Intent> = intents
        .iter()
        .filter(|i| i.name.to_lowercase() == lower)
        .collect();
    if exact.len() == 1 {
        return Resolved::Intent(exact[0].id.clone());
    }
    if exact.len() > 1 {
        return Resolved::Ambiguous(exact.iter().map(|i| i.name.clone()).collect());
    }

    // An exact file path is unambiguous — let it win before fuzzy intent matching.
    if let Some(cf) = codefiles.iter().find(|c| c.path == target) {
        return Resolved::File(cf.path.clone());
    }

    let frag: Vec<&Intent> = intents
        .iter()
        .filter(|i| i.name.to_lowercase().contains(&lower))
        .collect();
    if frag.len() == 1 {
        return Resolved::Intent(frag[0].id.clone());
    }
    if frag.len() > 1 {
        return Resolved::Ambiguous(frag.iter().map(|i| i.name.clone()).collect());
    }

    // File by suffix, then substring — UNIQUE matches only (no order-dependent
    // first-match: an empty/loose needle must not silently grab a file).
    let suffix: Vec<&CodeFile> = codefiles
        .iter()
        .filter(|c| c.path.ends_with(target))
        .collect();
    if suffix.len() == 1 {
        return Resolved::File(suffix[0].path.clone());
    }
    let sub: Vec<&CodeFile> = codefiles
        .iter()
        .filter(|c| c.path.contains(target))
        .collect();
    if sub.len() == 1 {
        return Resolved::File(sub[0].path.clone());
    }
    Resolved::None
}

// ---------------------------------------------------------------------------
// The explanation: everything the graph knows about one intent, synthesized.
// ---------------------------------------------------------------------------

struct Coupling {
    other_name: String,
    kinds: Vec<String>,
    status: String,
    ripples: bool,
    trust: u8,
    /// An asserted relationship (passing/failing/stale) — vs `independent`
    /// (confirmed unrelated) or `unexplored`/`uninspected` (not yet judged).
    active: bool,
}

struct Explanation {
    id: String,
    name: String,
    description: String,
    level: String,
    lifecycle: String,
    domain: String,
    layer: String,
    visibility: String,
    deprecated: bool,
    groundings: Vec<(String, String, String)>, // path, locator, status
    couplings: Vec<Coupling>,
    governs: Vec<(String, String, String)>, // rule name, kind, status
    parent: Option<String>,
    children: Vec<String>,
    validations: Vec<(String, String)>, // name, last_result
}

fn trust_rank(kinds: &[String]) -> u8 {
    kinds
        .iter()
        .filter_map(|k| k.parse::<RelationKind>().ok())
        .map(|rk| match rk.trust_weight() {
            "strong" => 3,
            "medium" => 2,
            _ => 1,
        })
        .max()
        .unwrap_or(0)
}

fn build_explanation(
    snap: &QuerySnapshot,
    by_id: &HashMap<&str, &Intent>,
    intent: &Intent,
) -> Explanation {
    let id = intent.id.as_str();

    let groundings: Vec<(String, String, String)> = snap
        .implements
        .iter()
        .filter(|im| im.intent_id == id)
        .map(|im| {
            let display_status =
                crate::output::grounding_status_label(&im.inspection_status, &im.criterion);
            (im.codefile_path.clone(), im.locator.clone(), display_status)
        })
        .collect();

    let mut couplings: Vec<Coupling> = snap
        .relates
        .iter()
        .filter(|e| e.from_id == id || e.to_id == id)
        .map(|e| {
            let other_name = if e.from_id == id {
                e.to_name.clone()
            } else {
                e.from_name.clone()
            };
            Coupling {
                other_name,
                kinds: e.kinds.clone(),
                ripples: relates_stales_on_code_change(&e.kinds),
                trust: trust_rank(&e.kinds),
                active: matches!(
                    e.inspection_status.as_str(),
                    "passing" | "failing" | "needs_reverification"
                ),
                status: e.inspection_status.clone(),
            }
        })
        .collect();
    // Real relationships first (independent/unexplored sink), then strongest
    // trust, then code-coupling — the links that matter most to a change on top.
    couplings.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then(b.trust.cmp(&a.trust))
            .then(b.ripples.cmp(&a.ripples))
            .then(a.other_name.cmp(&b.other_name))
    });

    let rule_by_id: HashMap<&str, &QualityRule> =
        snap.rules.iter().map(|r| (r.id.as_str(), r)).collect();
    let governs: Vec<(String, String, String)> = snap
        .governs
        .iter()
        .filter(|g| g.intent_id == id)
        .map(|g| {
            let kind = rule_by_id
                .get(g.rule_id.as_str())
                .map(|r| r.kind.clone())
                .unwrap_or_default();
            (g.rule_name.clone(), kind, g.inspection_status.clone())
        })
        .collect();

    let parent = snap
        .hierarchy
        .iter()
        .find(|(_, child)| child == id)
        .and_then(|(p, _)| by_id.get(p.as_str()))
        .map(|i| i.name.clone());
    let children: Vec<String> = snap
        .hierarchy
        .iter()
        .filter(|(parent, _)| parent == id)
        .filter_map(|(_, c)| by_id.get(c.as_str()).map(|i| i.name.clone()))
        .collect();

    let validation_by_id: HashMap<&str, &crate::types::Validation> = snap
        .validations
        .iter()
        .map(|v| (v.id.as_str(), v))
        .collect();
    let validations: Vec<(String, String)> = snap
        .validates
        .iter()
        .filter(|ve| ve.intent_id == id)
        .map(|ve| {
            let last = validation_by_id
                .get(ve.validation_id.as_str())
                .map(|v| v.last_result.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "not_run".to_string());
            (ve.validation_name.clone(), last)
        })
        .collect();

    Explanation {
        id: intent.id.clone(),
        name: intent.name.clone(),
        description: intent.description.clone(),
        level: intent.abstraction_level.clone(),
        lifecycle: intent.lifecycle.clone(),
        domain: intent.domain.clone(),
        layer: intent.layer.clone(),
        visibility: intent.visibility.clone(),
        deprecated: intent.status == "deprecated",
        groundings,
        couplings,
        governs,
        parent,
        children,
        validations,
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn coupling_tag(c: &Coupling) -> &'static str {
    // The ripple tag is only meaningful for ASSERTED relationships; an
    // `independent`/`unexplored` edge must not claim to "ripple" (it contradicts
    // the impact summary, which counts only active edges).
    if !c.active {
        "not an asserted relationship — won't re-open"
    } else if c.ripples {
        "ripples on code change"
    } else {
        "meaning-only — won't ripple"
    }
}

fn render_json(explanations: &[Explanation], file_ctx: Option<&str>, printer: &Printer) {
    let intents: Vec<serde_json::Value> = explanations
        .iter()
        .map(|e| {
            let rippled: Vec<&str> = e
                .couplings
                .iter()
                .filter(|c| c.ripples && c.active)
                .map(|c| c.other_name.as_str())
                .collect();
            serde_json::json!({
                "id": e.id,
                "name": e.name,
                "description": e.description,
                "level": e.level,
                "lifecycle": e.lifecycle,
                "deprecated": e.deprecated,
                "domain": e.domain,
                "layer": e.layer,
                "visibility": e.visibility,
                "grounded_in": e.groundings.iter().map(|(p, l, s)| serde_json::json!({
                    "path": p, "locator": l, "status": s
                })).collect::<Vec<_>>(),
                "coupled_to": e.couplings.iter().map(|c| serde_json::json!({
                    "intent": c.other_name, "kinds": c.kinds, "status": c.status,
                    "asserted": c.active,
                    "ripples_on_code_change": c.ripples && c.active,
                })).collect::<Vec<_>>(),
                "governed_by": e.governs.iter().map(|(n, k, s)| serde_json::json!({
                    "rule": n, "kind": k, "status": s
                })).collect::<Vec<_>>(),
                "parent": e.parent,
                "children": e.children,
                "proven_by": e.validations.iter().map(|(n, r)| serde_json::json!({
                    "validation": n, "last_result": r
                })).collect::<Vec<_>>(),
                "impact": {
                    "ripples_to": rippled,
                    "meaning_only_links": e.couplings.iter().filter(|c| c.active && !c.ripples).count(),
                },
            })
        })
        .collect();
    printer.print_json(&serde_json::json!({
        "target_file": file_ctx,
        "intents": intents,
        "next_step": "loom intent show <id>  (raw node)  ·  loom explain <coupled intent>  (follow a link)  ·  loom explain <file> --impact  (before editing)",
    }));
}

fn render_human(explanations: &[Explanation], file_ctx: Option<&str>) {
    if let Some(f) = file_ctx {
        println!(
            "══ explain: {f}  (file → {} grounding intent(s)) ══",
            explanations.len()
        );
    }
    let mut last_id = String::new();
    for e in explanations {
        last_id = e.id.clone();
        let dep = if e.deprecated { "  [DEPRECATED]" } else { "" };
        println!();
        println!("══ {}  [{} · {}]{dep} ══", e.name, e.level, e.lifecycle);
        if !e.description.is_empty() {
            println!("  {}", e.description);
        }
        let mut meta = Vec::new();
        if !e.domain.is_empty() {
            meta.push(format!("domain: {}", e.domain));
        }
        if !e.layer.is_empty() {
            meta.push(format!("layer: {}", e.layer));
        }
        if !e.visibility.is_empty() {
            meta.push(format!("visibility: {}", e.visibility));
        }
        if !meta.is_empty() {
            println!("  {}", meta.join(" · "));
        }

        if !e.groundings.is_empty() {
            println!();
            println!("  Grounded in ({} file(s)):", e.groundings.len());
            for (p, l, s) in &e.groundings {
                let loc = if l.is_empty() {
                    String::new()
                } else {
                    format!(":{l}")
                };
                println!("    {p}{loc}  [{s}]");
            }
        }

        if !e.couplings.is_empty() {
            const SHOW: usize = 15;
            let active = e.couplings.iter().filter(|c| c.active).count();
            println!();
            println!(
                "  Coupled to ({} total · {active} asserted, real relationships first):",
                e.couplings.len()
            );
            for c in e.couplings.iter().take(SHOW) {
                let kinds = if c.kinds.is_empty() {
                    "(un-kinded)".to_string()
                } else {
                    format!("[{}]", c.kinds.join(", "))
                };
                println!(
                    "    {:<40} {:<22} {}  · {}",
                    c.other_name,
                    kinds,
                    c.status,
                    coupling_tag(c)
                );
            }
            if e.couplings.len() > SHOW {
                println!(
                    "    … +{} more (mostly mechanical/independent links — use --json for all)",
                    e.couplings.len() - SHOW
                );
            }
        }

        if !e.governs.is_empty() {
            println!();
            println!("  Governed by:");
            for (n, k, s) in &e.governs {
                let kind = if k.is_empty() {
                    String::new()
                } else {
                    format!(" ({k})")
                };
                println!("    {n}{kind}  {s}");
            }
        }

        if e.parent.is_some() || !e.children.is_empty() {
            println!();
            println!("  Hierarchy:");
            if let Some(p) = &e.parent {
                println!("    parent: {p}");
            }
            if !e.children.is_empty() {
                println!("    children: {}", e.children.join(", "));
            }
        }

        if !e.validations.is_empty() {
            println!();
            println!("  Proven by:");
            for (n, r) in &e.validations {
                println!("    {n}  {r}");
            }
        }

        let rippled: Vec<&str> = e
            .couplings
            .iter()
            .filter(|c| c.ripples && c.active)
            .map(|c| c.other_name.as_str())
            .collect();
        let meaning_only = e
            .couplings
            .iter()
            .filter(|c| c.active && !c.ripples)
            .count();
        println!();
        if rippled.is_empty() {
            println!("  Impact — no asserted code-coupled relationships; a change here ripples to nothing tracked.");
        } else {
            let shown = rippled
                .iter()
                .take(12)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let more = rippled.len().saturating_sub(12);
            println!(
                "  Impact — changing this ripples to {} relationship(s): {}{}{}",
                rippled.len(),
                shown,
                if more > 0 {
                    format!(" … +{more}")
                } else {
                    String::new()
                },
                if meaning_only > 0 {
                    format!("  ({meaning_only} meaning-only link(s) won't re-open)")
                } else {
                    String::new()
                }
            );
        }
    }
    // Anchor (human parity with the json next_step).
    if !last_id.is_empty() {
        println!();
        println!(
            "  → Next: `loom intent show {last_id}` (raw node) · `loom explain <coupled intent>` to follow a link · `loom explain <file> --impact` before editing."
        );
    }
}

fn render_impact(explanations: &[Explanation], label: &str, printer: &Printer) {
    let affected: Vec<&str> = explanations.iter().map(|e| e.name.as_str()).collect();
    let mut reopens: Vec<(String, Vec<String>)> = Vec::new();
    let mut seen_r = std::collections::HashSet::new();
    let mut rerun: Vec<(String, String)> = Vec::new();
    let mut seen_v = std::collections::HashSet::new();
    for e in explanations {
        for c in &e.couplings {
            // Co-intents of the same file are "directly affected", not ripple.
            if c.active
                && c.ripples
                && !affected.contains(&c.other_name.as_str())
                && seen_r.insert(c.other_name.clone())
            {
                reopens.push((c.other_name.clone(), c.kinds.clone()));
            }
        }
        for (n, r) in &e.validations {
            if seen_v.insert(n.clone()) {
                rerun.push((n.clone(), r.clone()));
            }
        }
    }

    if printer.json {
        printer.print_json(&serde_json::json!({
            "target": label,
            "directly_affected": affected,
            "reopens_relationships": reopens.iter().map(|(n, k)| serde_json::json!({
                "intent": n, "kinds": k
            })).collect::<Vec<_>>(),
            "rerun_validations": rerun.iter().map(|(n, r)| serde_json::json!({
                "validation": n, "last_result": r
            })).collect::<Vec<_>>(),
            "summary": {
                "directly_affected": affected.len(),
                "relationships_reopened": reopens.len(),
                "validations_to_rerun": rerun.len(),
            },
            "next_step": "after the change: `loom sync`, then `loom next --mode fix` / `--mode validate`",
        }));
        return;
    }

    println!("══ Blast radius: {label} ══");
    println!(
        "  Directly affected ({} intent(s)): {}",
        affected.len(),
        if affected.is_empty() {
            "(none)".to_string()
        } else {
            affected.join(", ")
        }
    );
    println!();
    if reopens.is_empty() {
        println!("  Re-opens 0 relationships — no asserted code-coupling ripples from here.");
    } else {
        println!(
            "  Re-opens {} relationship(s) (re-verify after the change):",
            reopens.len()
        );
        for (n, k) in reopens.iter().take(20) {
            let kinds = if k.is_empty() {
                "(un-kinded)".to_string()
            } else {
                format!("[{}]", k.join(", "))
            };
            println!("    {n:<44} {kinds}");
        }
        if reopens.len() > 20 {
            println!("    … +{} more (use --json for all)", reopens.len() - 20);
        }
    }
    if !rerun.is_empty() {
        println!();
        println!("  Re-run {} validation(s):", rerun.len());
        for (n, r) in rerun.iter().take(20) {
            println!("    {n}  (last: {r})");
        }
        if rerun.len() > 20 {
            println!("    … +{} more (use --json for all)", rerun.len() - 20);
        }
    }
    println!();
    println!(
        "  → After changing this: `loom sync`, then `loom next --mode fix` / `--mode validate`."
    );
}

fn render_uncovered_file(path: &str, printer: &Printer) -> Result<()> {
    if printer.json {
        printer.print_json(&serde_json::json!({
            "target_file": path,
            "intents": [],
            "message": "This file is registered but no intent grounds it (no IMPLEMENTS edge).",
            "next_step": format!("loom edge implement <intent> {path} --locator <symbol>  (ground it), or `loom coverage` to see the gap"),
        }));
    } else {
        println!("══ explain: {path} ══");
        println!("  Registered, but NO intent grounds this file (no IMPLEMENTS edge).");
        println!("  → It's uncovered: `loom edge implement <intent> {path} --locator <symbol>`, or `loom coverage`.");
    }
    Ok(())
}
