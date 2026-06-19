//! `loom wiki` — the DOCUMENT projection of the graph. Generates a human-readable
//! Markdown wiki (overview + architecture tree + components-by-domain + quality
//! bars) deterministically from the intent graph. Same shape as `loom export`:
//! same graph → identical bytes, so `--check` is a byte comparison (pre-commit/CI
//! freshness). The graph is the source of truth; this file is a regenerable VIEW —
//! never hand-edited, and not a second teacher (agents drive the graph, humans
//! read the wiki).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::db::queries::{GraphMeta, QuerySnapshot};
use crate::output::Printer;
use crate::types::Intent;

pub fn run(out: &str, check: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
    let snap = store.query_snapshot()?;
    let meta = store.graph_meta()?;
    let md = render_wiki(&snap, meta.as_ref());
    emit(&cwd, out, check, &md, printer)
}

// ---------------------------------------------------------------------------
// File write / freshness check — mirrors `loom export` (deterministic bytes).
// ---------------------------------------------------------------------------

fn emit(root: &Path, out: &str, check: bool, md: &str, printer: &Printer) -> Result<()> {
    if check {
        if out == "-" {
            anyhow::bail!("--check needs a file to compare against (not '-').");
        }
        let confined = crate::repo::confine(root, Path::new(out))
            .ok_or_else(|| anyhow::anyhow!("wiki path escapes graph root: {out}"))?;
        let on_disk = fs::read_to_string(root.join(confined)).ok();
        let fresh = on_disk.as_deref() == Some(md);
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": if fresh { "ok" } else if on_disk.is_none() { "missing" } else { "stale" },
                "out": out,
                "next_step": if fresh { format!("commit {out}") } else { format!("run `loom wiki` and commit {out}") },
            }));
        } else if fresh {
            println!("✓ {out} is up to date with the graph.");
        } else if on_disk.is_none() {
            println!("✗ {out} does not exist — run `loom wiki` and commit it.");
        } else {
            println!("✗ {out} is STALE — the graph changed since it was written. Run `loom wiki`.");
        }
        if !fresh {
            anyhow::bail!("wiki is stale or missing — run `loom wiki` and commit the result.");
        }
        return Ok(());
    }

    if out == "-" {
        println!("{md}");
        return Ok(());
    }
    let confined = crate::repo::confine(root, Path::new(out))
        .ok_or_else(|| anyhow::anyhow!("wiki path escapes graph root: {out}"))?;
    let target = root.join(confined);
    let mut tmp = target.as_os_str().to_os_string();
    tmp.push(".tmp");
    fs::write(&tmp, md)?;
    fs::rename(&tmp, &target)?;
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "out": out,
            "bytes": md.len(),
            "next_step": format!("commit {out} so the wiki travels with the repo"),
        }));
    } else {
        println!("✓ Wrote {out}  ({} bytes)", md.len());
        println!("  → It's a projection — regenerate after graph changes; `loom wiki --check` guards freshness.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering — deterministic (everything sorted, no timestamps).
// ---------------------------------------------------------------------------

const NO_DOMAIN: &str = "(uncategorized)";

fn render_wiki(snap: &QuerySnapshot, meta: Option<&GraphMeta>) -> String {
    let name = meta
        .map(|m| m.graph_name.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "loom graph".to_string());

    let mut s = String::new();
    s.push_str(&format!("# {name} — loom wiki\n\n"));
    s.push_str("> Generated from the loom intent graph by `loom wiki` — do not edit by hand.\n");
    s.push_str(
        "> Regenerate after graph changes (`loom wiki`); `loom wiki --check` verifies freshness.\n",
    );
    s.push_str("> The graph is the source of truth; this file is a projection of it.\n\n");

    render_overview(&mut s, snap);
    render_architecture(&mut s, snap);
    render_components(&mut s, snap);
    render_quality(&mut s, snap);

    s
}

fn sorted_unique<'a>(vals: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut v: Vec<&str> = vals.filter(|x| !x.is_empty()).collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn render_overview(s: &mut String, snap: &QuerySnapshot) {
    let level = |lvl: &str| {
        snap.intents
            .iter()
            .filter(|i| i.abstraction_level == lvl)
            .count()
    };
    s.push_str("## Overview\n\n");
    s.push_str(&format!(
        "- **Intents:** {} (system: {}, component: {}, feature: {})\n",
        snap.intents.len(),
        level("system"),
        level("component"),
        level("feature"),
    ));
    let domains = sorted_unique(snap.intents.iter().map(|i| i.domain.as_str()));
    if !domains.is_empty() {
        s.push_str(&format!("- **Domains:** {}\n", domains.join(", ")));
    }
    let layers = sorted_unique(snap.intents.iter().map(|i| i.layer.as_str()));
    if !layers.is_empty() {
        s.push_str(&format!("- **Layers:** {}\n", layers.join(", ")));
    }
    s.push_str(&format!(
        "- **Code files mapped:** {}\n",
        snap.codefiles.len()
    ));
    s.push_str(&format!("- **Quality rules:** {}\n\n", snap.rules.len()));
}

fn render_architecture(s: &mut String, snap: &QuerySnapshot) {
    s.push_str("## Architecture\n\n");
    s.push_str("The intent hierarchy — what the system is, decomposed top-down.\n\n");

    let by_id: HashMap<&str, &Intent> = snap.intents.iter().map(|i| (i.id.as_str(), i)).collect();
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for (p, c) in &snap.hierarchy {
        children.entry(p.as_str()).or_default().push(c.as_str());
    }
    let child_set: HashSet<&str> = snap.hierarchy.iter().map(|(_, c)| c.as_str()).collect();
    let mut roots: Vec<&Intent> = snap
        .intents
        .iter()
        .filter(|i| !child_set.contains(i.id.as_str()))
        .collect();
    roots.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));

    if roots.is_empty() {
        s.push_str("_(no intents yet)_\n\n");
        return;
    }
    let mut visited = HashSet::new();
    for r in roots {
        render_node(s, r, &by_id, &children, 0, &mut visited);
    }
    s.push('\n');
}

fn render_node<'a>(
    s: &mut String,
    intent: &'a Intent,
    by_id: &HashMap<&'a str, &'a Intent>,
    children: &HashMap<&'a str, Vec<&'a str>>,
    depth: usize,
    visited: &mut HashSet<&'a str>,
) {
    // Tree guard — a HIERARCHY should be acyclic, but never loop on bad data.
    if depth > 12 || !visited.insert(intent.id.as_str()) {
        return;
    }
    let indent = "  ".repeat(depth);
    let desc = if intent.description.is_empty() {
        String::new()
    } else {
        format!(" — {}", intent.description)
    };
    let dep = if intent.status == "deprecated" {
        " _(deprecated)_"
    } else {
        ""
    };
    s.push_str(&format!("{indent}- **{}**{dep}{desc}\n", intent.name));

    if let Some(kids) = children.get(intent.id.as_str()) {
        let mut kids: Vec<&Intent> = kids
            .iter()
            .filter_map(|id| by_id.get(id).copied())
            .collect();
        kids.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        for k in kids {
            render_node(s, k, by_id, children, depth + 1, visited);
        }
    }
}

fn render_components(s: &mut String, snap: &QuerySnapshot) {
    s.push_str("## Components & code\n\n");
    s.push_str("Intents grouped by domain, with where each is grounded in code.\n\n");

    // intent id → sorted unique grounded file paths.
    let mut files_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for im in &snap.implements {
        files_of
            .entry(im.intent_id.as_str())
            .or_default()
            .push(im.codefile_path.as_str());
    }
    for v in files_of.values_mut() {
        v.sort_unstable();
        v.dedup();
    }

    // domains in deterministic order, uncategorized last.
    let mut domains = sorted_unique(snap.intents.iter().map(|i| i.domain.as_str()));
    let has_uncat = snap.intents.iter().any(|i| i.domain.is_empty());
    if has_uncat {
        domains.push(NO_DOMAIN);
    }

    for d in domains {
        s.push_str(&format!("### {d}\n\n"));
        let mut members: Vec<&Intent> = snap
            .intents
            .iter()
            .filter(|i| {
                if d == NO_DOMAIN {
                    i.domain.is_empty()
                } else {
                    i.domain == d
                }
            })
            .collect();
        members.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        for i in members {
            let files = files_of
                .get(i.id.as_str())
                .map(|f| format!("  `{}`", f.join("`, `")))
                .unwrap_or_default();
            let desc = if i.description.is_empty() {
                String::new()
            } else {
                format!(" — {}", i.description)
            };
            s.push_str(&format!("- **{}**{desc}{files}\n", i.name));
        }
        s.push('\n');
    }
}

fn render_quality(s: &mut String, snap: &QuerySnapshot) {
    if snap.rules.is_empty() {
        return;
    }
    s.push_str("## Quality bars\n\n");
    s.push_str("The norms loom holds the code to, by category.\n\n");

    let mut rules: Vec<&crate::types::QualityRule> = snap.rules.iter().collect();
    rules.sort_by(|a, b| {
        (a.kind.as_str(), a.name.as_str()).cmp(&(b.kind.as_str(), b.name.as_str()))
    });

    let mut categories = sorted_unique(snap.rules.iter().map(|r| r.kind.as_str()));
    let has_uncat = snap.rules.iter().any(|r| r.kind.is_empty());
    if has_uncat {
        categories.push(NO_DOMAIN);
    }

    for cat in categories {
        s.push_str(&format!("### {cat}\n\n"));
        for r in rules.iter().filter(|r| {
            if cat == NO_DOMAIN {
                r.kind.is_empty()
            } else {
                r.kind == cat
            }
        }) {
            let desc = if r.description.is_empty() {
                String::new()
            } else {
                format!(" — {}", r.description)
            };
            s.push_str(&format!("- **{}** ({}){desc}\n", r.name, r.severity));
        }
        s.push('\n');
    }
}
