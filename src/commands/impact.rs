//! `loom impact` — the pre-change blast radius. Given a set of changed files
//! (explicit, or auto-detected from git), report the INTENT-level fallout BEFORE
//! the edit lands: which intents' groundings go stale, which proofs must re-run,
//! and which downstream RELATES_TO/GOVERNS edges ripple one hop. It's what
//! `loom sync` WOULD flag, computed ahead of time — so an agent knows the cost of
//! a change before making it. Read-only.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use anyhow::Result;

use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;
use crate::types::relates_stales_on_code_change;

pub fn run(files: Vec<String>, staged: bool, printer: &Printer) -> Result<()> {
    let root = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&root)?;
    let snap = store.query_snapshot()?;

    let registered: HashSet<&str> = snap.codefiles.iter().map(|c| c.path.as_str()).collect();

    // Changed files: explicit args (confined to repo-relative), else git.
    let changed: Vec<String> = if files.is_empty() {
        git_changed(&root, staged)
    } else {
        files
            .iter()
            .filter_map(|f| crate::repo::confine(&root, Path::new(f)))
            .collect()
    };

    if changed.is_empty() {
        let msg = "No changed files detected (clean working tree). Pass paths explicitly to \
                   preview: `loom impact <path>…`.";
        if printer.json {
            printer.print_json(&serde_json::json!({ "changed_files": [], "next_step": msg }));
        } else {
            println!("{msg}");
        }
        return Ok(());
    }

    let changed_set: HashSet<&str> = changed.iter().map(String::as_str).collect();
    let changed_registered: BTreeSet<&str> = changed_set
        .iter()
        .copied()
        .filter(|p| registered.contains(p))
        .collect();
    // Changed SOURCE files not in the graph — a coverage gap the change widens.
    let unregistered: BTreeSet<&str> = changed_set
        .iter()
        .copied()
        .filter(|p| !registered.contains(p) && !crate::repo::lang_of(p).is_empty())
        .collect();

    // Directly affected: every intent grounded in a changed (registered) file —
    // its IMPLEMENTS locator must be re-verified once the file changes.
    let mut affected: BTreeMap<&str, (&str, BTreeSet<&str>)> = BTreeMap::new();
    for im in &snap.implements {
        if changed_registered.contains(im.codefile_path.as_str()) {
            affected
                .entry(im.intent_id.as_str())
                .or_insert_with(|| (im.intent_name.as_str(), BTreeSet::new()))
                .1
                .insert(im.codefile_path.as_str());
        }
    }
    let affected_ids: HashSet<&str> = affected.keys().copied().collect();

    // Proofs to re-run: validations linked to an affected intent.
    let mut proofs: BTreeSet<(&str, &str)> = BTreeSet::new(); // (validation, intent)
    for ve in &snap.validates {
        if affected_ids.contains(ve.intent_id.as_str()) {
            proofs.insert((ve.validation_name.as_str(), ve.intent_name.as_str()));
        }
    }

    // Downstream ripple (one hop): RELATES_TO edges that stale on a code change,
    // plus GOVERNS verdicts that must be re-measured.
    let mut downstream: BTreeSet<(&str, &str, &str)> = BTreeSet::new(); // (neighbor, via, edge-kind)
    for e in &snap.relates {
        if e.stable
            || e.inspection_status == "independent"
            || !relates_stales_on_code_change(&e.kinds)
        {
            continue;
        }
        if affected_ids.contains(e.from_id.as_str()) {
            downstream.insert((e.to_name.as_str(), e.from_name.as_str(), "relates"));
        }
        if affected_ids.contains(e.to_id.as_str()) {
            downstream.insert((e.from_name.as_str(), e.to_name.as_str(), "relates"));
        }
    }
    for g in &snap.governs {
        if affected_ids.contains(g.intent_id.as_str()) && g.inspection_status != "uninspected" {
            downstream.insert((g.rule_name.as_str(), g.intent_name.as_str(), "governs"));
        }
    }

    if printer.json {
        render_json(
            &changed_registered,
            &affected,
            &proofs,
            &downstream,
            &unregistered,
            printer,
        );
    } else {
        render_human(
            &changed_registered,
            &affected,
            &proofs,
            &downstream,
            &unregistered,
        );
    }
    Ok(())
}

/// Files changed vs HEAD (staged + unstaged + untracked), or staged-only.
/// Newline-separated `git` output; best-effort (empty outside a git repo).
fn git_changed(root: &Path, staged_only: bool) -> Vec<String> {
    let runs: &[&[&str]] = if staged_only {
        &[&["diff", "--cached", "--name-only"]]
    } else {
        &[
            &["diff", "--name-only", "HEAD"],
            &["ls-files", "--others", "--exclude-standard"],
        ]
    };
    let mut out: Vec<String> = Vec::new();
    for args in runs {
        let Ok(output) = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(*args)
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let p = line.trim().replace('\\', "/");
            if !p.is_empty() && !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

#[allow(clippy::type_complexity)]
fn render_human(
    changed: &BTreeSet<&str>,
    affected: &BTreeMap<&str, (&str, BTreeSet<&str>)>,
    proofs: &BTreeSet<(&str, &str)>,
    downstream: &BTreeSet<(&str, &str, &str)>,
    unregistered: &BTreeSet<&str>,
) {
    println!("── loom impact ──────────────────────────────────────────────────────");
    println!(
        "  {} changed file(s) → {} intent(s) directly affected · {} proof(s) to re-run · {} downstream edge(s)",
        changed.len(),
        affected.len(),
        proofs.len(),
        downstream.len()
    );
    if !affected.is_empty() {
        println!();
        println!("  directly affected (groundings go stale — re-verify):");
        for (name, files) in affected.values() {
            println!(
                "    • {name}   ({})",
                files.iter().copied().collect::<Vec<_>>().join(", ")
            );
        }
    }
    if !proofs.is_empty() {
        println!();
        println!("  proofs to re-run:");
        for (v, intent) in proofs {
            println!("    • {v}  ({intent})");
        }
    }
    if !downstream.is_empty() {
        println!();
        println!("  downstream (these stale one hop on the change):");
        for (neighbor, via, kind) in downstream {
            println!("    • {neighbor}  ← {via}  [{kind}]");
        }
    }
    if !unregistered.is_empty() {
        println!();
        println!("  changed source files NOT in the graph (coverage gap):");
        for p in unregistered {
            println!("    • {p}   — `loom codefile add {p}`");
        }
    }
    println!();
    println!(
        "  → `loom sync` will flag exactly these after the change; re-verify with \
         `loom next --mode fix` / `--mode validate`."
    );
}

#[allow(clippy::type_complexity)]
fn render_json(
    changed: &BTreeSet<&str>,
    affected: &BTreeMap<&str, (&str, BTreeSet<&str>)>,
    proofs: &BTreeSet<(&str, &str)>,
    downstream: &BTreeSet<(&str, &str, &str)>,
    unregistered: &BTreeSet<&str>,
    printer: &Printer,
) {
    printer.print_json(&serde_json::json!({
        "changed_files": changed.iter().collect::<Vec<_>>(),
        "directly_affected": affected.iter().map(|(id, (name, files))| serde_json::json!({
            "intent_id": id,
            "intent": name,
            "files": files.iter().collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "proofs_to_rerun": proofs.iter().map(|(v, intent)| serde_json::json!({
            "validation": v, "intent": intent,
        })).collect::<Vec<_>>(),
        "downstream": downstream.iter().map(|(neighbor, via, kind)| serde_json::json!({
            "ripples_to": neighbor, "via": via, "kind": kind,
        })).collect::<Vec<_>>(),
        "unregistered_changed_source": unregistered.iter().collect::<Vec<_>>(),
        "next_step": "`loom sync` flags exactly these after the change; re-verify via `loom next --mode fix` / `--mode validate`.",
    }));
}
