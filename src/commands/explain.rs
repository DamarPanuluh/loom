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
use crate::types::{relates_stales_on_code_change, Intent, QualityRule, RelationKind, Validation};

pub fn run(target: &str, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&cwd)?;
    run_with_db(&store, target, printer)
}

pub fn run_with_db(db: &dyn GraphReadRepository, target: &str, printer: &Printer) -> Result<()> {
    let snap = db.query_snapshot()?;
    let resolved = resolve_target(&snap, target);

    let ids: Vec<String> = match &resolved {
        Resolved::Intent(id) => vec![id.clone()],
        Resolved::File { intents, .. } => intents.clone(),
        Resolved::None => anyhow::bail!(
            "Nothing matches '{target}' as an intent (id / exact name / unique fragment) or a \
             registered file path. Try `loom find \"{target}\"` or `loom intent list`."
        ),
    };

    // A registered file with no grounding intents is itself a useful answer.
    if let Resolved::File { path, intents } = &resolved {
        if intents.is_empty() {
            return render_uncovered_file(path, printer);
        }
    }

    let by_id: HashMap<&str, &Intent> = snap.intents.iter().map(|i| (i.id.as_str(), i)).collect();
    let explanations: Vec<Explanation> = ids
        .iter()
        .filter_map(|id| by_id.get(id.as_str()).map(|i| build_explanation(&snap, i)))
        .collect();

    let file_ctx = match &resolved {
        Resolved::File { path, .. } => Some(path.as_str()),
        _ => None,
    };

    if printer.json {
        render_json(&explanations, file_ctx, printer);
    } else {
        render_human(&explanations, file_ctx);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Target resolution: intent first (loom's primary node), then file.
// ---------------------------------------------------------------------------

enum Resolved {
    Intent(String),
    File { path: String, intents: Vec<String> },
    None,
}

fn resolve_target(snap: &QuerySnapshot, target: &str) -> Resolved {
    if snap.intents.iter().any(|i| i.id == target) {
        return Resolved::Intent(target.to_string());
    }
    let lower = target.to_lowercase();
    let exact: Vec<&Intent> = snap
        .intents
        .iter()
        .filter(|i| i.name.to_lowercase() == lower)
        .collect();
    if exact.len() == 1 {
        return Resolved::Intent(exact[0].id.clone());
    }
    let frag: Vec<&Intent> = snap
        .intents
        .iter()
        .filter(|i| i.name.to_lowercase().contains(&lower))
        .collect();
    if frag.len() == 1 {
        return Resolved::Intent(frag[0].id.clone());
    }
    // Fall through to file: exact path, then path suffix, then substring.
    let file = snap
        .codefiles
        .iter()
        .find(|c| c.path == target)
        .or_else(|| snap.codefiles.iter().find(|c| c.path.ends_with(target)))
        .or_else(|| {
            let hits: Vec<_> = snap
                .codefiles
                .iter()
                .filter(|c| c.path.contains(target))
                .collect();
            if hits.len() == 1 {
                Some(hits[0])
            } else {
                None
            }
        });
    if let Some(cf) = file {
        let mut intents: Vec<String> = Vec::new();
        for im in &snap.implements {
            if im.codefile_path == cf.path && !intents.contains(&im.intent_id) {
                intents.push(im.intent_id.clone());
            }
        }
        return Resolved::File {
            path: cf.path.clone(),
            intents,
        };
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

fn build_explanation(snap: &QuerySnapshot, intent: &Intent) -> Explanation {
    let id = intent.id.as_str();

    let groundings: Vec<(String, String, String)> = snap
        .implements
        .iter()
        .filter(|im| im.intent_id == id)
        .map(|im| {
            (
                im.codefile_path.clone(),
                im.locator.clone(),
                im.inspection_status.clone(),
            )
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

    let by_id: HashMap<&str, &Intent> = snap.intents.iter().map(|i| (i.id.as_str(), i)).collect();
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

    let validation_by_id: HashMap<&str, &Validation> = snap
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
                "domain": e.domain,
                "layer": e.layer,
                "visibility": e.visibility,
                "grounded_in": e.groundings.iter().map(|(p, l, s)| serde_json::json!({
                    "path": p, "locator": l, "status": s
                })).collect::<Vec<_>>(),
                "coupled_to": e.couplings.iter().map(|c| serde_json::json!({
                    "intent": c.other_name, "kinds": c.kinds, "status": c.status,
                    "ripples_on_code_change": c.ripples,
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
                    "meaning_only_links": e.couplings.len() - rippled.len(),
                },
            })
        })
        .collect();
    printer.print_json(&serde_json::json!({
        "target_file": file_ctx,
        "intents": intents,
        "next_step": "loom edge explore <a> <b> ground …  (inspect a coupling)  ·  loom intent show <id>  (raw node)",
    }));
}

fn render_human(explanations: &[Explanation], file_ctx: Option<&str>) {
    if let Some(f) = file_ctx {
        println!(
            "══ explain: {f}  (file → {} grounding intent(s)) ══",
            explanations.len()
        );
    }
    for e in explanations {
        println!();
        println!("══ {}  [{} · {}] ══", e.name, e.level, e.lifecycle);
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
                let ripple = if c.ripples {
                    "ripples on code change"
                } else {
                    "meaning-only — won't ripple"
                };
                println!(
                    "    {:<40} {:<22} {}  · {ripple}",
                    c.other_name, kinds, c.status
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

        // Impact = asserted relationships that stale on code change. Independent
        // edges (confirmed unrelated) and meaning-only kinds don't count.
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
