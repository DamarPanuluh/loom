//! `loom impact` — preview the graph work caused by changed files. Given a set
//! of changed files (explicit, or auto-detected from git), report the
//! INTENT-level fallout: which intents' groundings go stale, which proofs must
//! re-run, and which downstream RELATES_TO/GOVERNS edges ripple one hop. For
//! files already changed on disk it reuses the same symbol/import narrowing as
//! `loom sync`; for hypothetical future edits it stays conservative. Read-only.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use anyhow::Result;

use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;
use crate::types::{relates_is_import_only_coupling, relates_stales_on_code_change, CodeFile};

pub fn run(files: Vec<String>, staged: bool, printer: &Printer) -> Result<()> {
    let root = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&root)?;
    let snap = store.query_snapshot()?;
    let auto_detect = files.is_empty();

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
    let all_changed_registered: BTreeSet<&str> = changed_set
        .iter()
        .copied()
        .filter(|p| registered.contains(p))
        .collect();
    let codefile_by_path: BTreeMap<&str, &CodeFile> = snap
        .codefiles
        .iter()
        .map(|cf| (cf.path.as_str(), cf))
        .collect();
    let already_synced: BTreeSet<&str> = if auto_detect {
        all_changed_registered
            .iter()
            .copied()
            .filter(|p| {
                codefile_by_path
                    .get(p)
                    .is_some_and(|cf| !codefile_has_content_delta(&root, cf))
            })
            .collect()
    } else {
        BTreeSet::new()
    };
    let changed_registered: BTreeSet<&str> = all_changed_registered
        .iter()
        .copied()
        .filter(|p| !already_synced.contains(p))
        .collect();
    // Changed SOURCE files not in the graph — a coverage gap the change widens.
    let unregistered: BTreeSet<&str> = changed_set
        .iter()
        .copied()
        .filter(|p| {
            !registered.contains(p)
                && *p != "loom.graph.json"
                && !crate::repo::lang_of(p).is_empty()
        })
        .collect();

    let codefiles_for_coupling =
        codefiles_with_current_changed_facts(&root, &snap.codefiles, &changed_registered);
    let coupled = compute_coupled_intent_pairs(&codefiles_for_coupling, &snap.implements);

    // Directly affected: every intent grounded in a changed (registered) file —
    // narrowed to the changed symbols when the file is already edited on disk and
    // tree-sitter can attribute the delta. A hypothetical path with unchanged
    // content remains conservative (all groundings in the file).
    let mut affected: BTreeMap<&str, (&str, BTreeSet<&str>)> = BTreeMap::new();
    let mut affected_sets_by_file = BTreeMap::<&str, Option<HashSet<String>>>::new();
    for cf in &snap.codefiles {
        if changed_registered.contains(cf.path.as_str()) {
            affected_sets_by_file.insert(
                cf.id.as_str(),
                affected_intents_for_current_file(&root, cf, &snap.implements),
            );
        }
    }
    for im in &snap.implements {
        if let Some(narrowed) = affected_sets_by_file.get(im.codefile_id.as_str()) {
            if narrowed
                .as_ref()
                .is_some_and(|set| !set.contains(im.intent_id.as_str()))
            {
                continue;
            }
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
    // plus GOVERNS verdicts that must be re-measured. Mirrors sync's staling
    // predicate: meaning-only/stable edges do not stale; independent edges stale
    // only if a live structural coupling now exists; import-only passing edges
    // stay green while the live import coupling still exists.
    let mut downstream: BTreeSet<(&str, &str, &str)> = BTreeSet::new(); // (neighbor, via, edge-kind)
    for e in &snap.relates {
        if !relates_edge_would_stale(e, &coupled) {
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
            &already_synced,
            printer,
        );
    } else {
        render_human(
            &changed_registered,
            &affected,
            &proofs,
            &downstream,
            &unregistered,
            &already_synced,
        );
    }
    Ok(())
}

fn codefile_has_content_delta(root: &Path, cf: &CodeFile) -> bool {
    if cf.content_hash.is_empty() {
        return true;
    }
    let Some(rel) = crate::repo::confine(root, Path::new(&cf.path)) else {
        return true;
    };
    let Ok(bytes) = std::fs::read(root.join(rel)) else {
        return true;
    };
    crate::repo::content_hash(&bytes) != cf.content_hash
}

fn codefiles_with_current_changed_facts(
    root: &Path,
    codefiles: &[CodeFile],
    changed: &BTreeSet<&str>,
) -> Vec<CodeFile> {
    codefiles
        .iter()
        .cloned()
        .map(|mut cf| {
            if changed.contains(cf.path.as_str()) {
                if let Some(content) = read_current_text(root, &cf.path) {
                    let facts = crate::repo::extract_physical_facts(root, &cf.path, &content);
                    if !facts.imports.is_empty() || !facts.symbol_facts.is_empty() {
                        cf.imports = facts.imports;
                        cf.symbols = facts.symbols;
                        cf.symbol_facts = facts.symbol_facts;
                        cf.extractor_grade = facts.extractor_grade;
                    }
                }
            }
            cf
        })
        .collect()
}

fn read_current_text(root: &Path, path: &str) -> Option<String> {
    let rel = crate::repo::confine(root, Path::new(path))?;
    let bytes = std::fs::read(root.join(rel)).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn affected_intents_for_current_file(
    root: &Path,
    cf: &CodeFile,
    implements: &[crate::types::Implements],
) -> Option<HashSet<String>> {
    let content = read_current_text(root, &cf.path)?;
    let new_hash = crate::repo::content_hash(content.as_bytes());
    if !cf.content_hash.is_empty() && new_hash == cf.content_hash {
        return None;
    }
    if cf.symbol_facts.is_empty() {
        return None;
    }
    let facts = crate::repo::extract_physical_facts(root, &cf.path, &content);
    if facts.symbol_facts.is_empty()
        || cf
            .symbol_facts
            .iter()
            .chain(facts.symbol_facts.iter())
            .any(|f| f.body_hash.is_empty())
    {
        return None;
    }
    let old: std::collections::HashMap<&str, &str> = cf
        .symbol_facts
        .iter()
        .map(|f| (f.label.as_str(), f.body_hash.as_str()))
        .collect();
    let name_of: std::collections::HashMap<&str, &str> = cf
        .symbol_facts
        .iter()
        .chain(facts.symbol_facts.iter())
        .map(|f| (f.label.as_str(), f.name.as_str()))
        .collect();
    let mut changed = HashSet::<&str>::new();
    for f in &facts.symbol_facts {
        match old.get(f.label.as_str()) {
            Some(h) if *h == f.body_hash.as_str() => {}
            _ => {
                changed.insert(f.label.as_str());
            }
        }
    }
    let new_labels: HashSet<&str> = facts
        .symbol_facts
        .iter()
        .map(|f| f.label.as_str())
        .collect();
    for label in old.keys() {
        if !new_labels.contains(label) {
            changed.insert(label);
        }
    }
    if changed.is_empty() {
        return None;
    }
    let changed_names: Vec<&str> = changed
        .iter()
        .filter_map(|label| name_of.get(label).copied())
        .filter(|name| !name.is_empty())
        .collect();
    let tracked_names: Vec<&str> = name_of
        .values()
        .copied()
        .filter(|name| !name.is_empty())
        .collect();
    let mut affected = HashSet::new();
    for im in implements.iter().filter(|im| im.codefile_id == cf.id) {
        let loc = im.locator.trim();
        let names_changed = changed_names
            .iter()
            .any(|name| crate::db::queries::symbol_match::contains_identifier_word(loc, name));
        let names_tracked = tracked_names
            .iter()
            .any(|name| crate::db::queries::symbol_match::contains_identifier_word(loc, name));
        if loc.is_empty() || names_changed || !names_tracked {
            affected.insert(im.intent_id.clone());
        }
    }
    Some(affected)
}

fn compute_coupled_intent_pairs(
    codefiles: &[CodeFile],
    implements: &[crate::types::Implements],
) -> HashSet<(String, String)> {
    let mut intents_on_file: std::collections::HashMap<&str, HashSet<&str>> =
        std::collections::HashMap::new();
    for im in implements {
        intents_on_file
            .entry(im.codefile_path.as_str())
            .or_default()
            .insert(im.intent_id.as_str());
    }
    let mut coupled = HashSet::new();
    for cf in codefiles {
        let Some(owners_a) = intents_on_file.get(cf.path.as_str()) else {
            continue;
        };
        for target in &cf.imports {
            let Some(owners_b) = intents_on_file.get(target.as_str()) else {
                continue;
            };
            for a in owners_a {
                for b in owners_b {
                    if a != b {
                        coupled.insert(super::sorted_pair(a, b));
                    }
                }
            }
        }
    }
    coupled
}

fn relates_edge_would_stale(
    edge: &crate::types::RelatesTo,
    coupled: &HashSet<(String, String)>,
) -> bool {
    if edge.stable {
        return false;
    }
    let pair = super::sorted_pair(&edge.from_id, &edge.to_id);
    if edge.inspection_status == "independent" {
        return coupled.contains(&pair);
    }
    if !relates_stales_on_code_change(&edge.kinds) {
        return false;
    }
    if edge.inspection_status == "passing" && relates_is_import_only_coupling(&edge.kinds) {
        return !coupled.contains(&pair);
    }
    true
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
    already_synced: &BTreeSet<&str>,
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
    if !already_synced.is_empty() {
        println!();
        println!("  git-dirty but already synced into the graph (no impact work):");
        for p in already_synced {
            println!("    • {p}");
        }
    }
    println!();
    println!(
        "  → actual edited files are narrowed with sync's symbol/import rules; hypothetical \
         unchanged paths stay conservative. Reconcile with `loom sync`, then re-verify only \
         the queued work via `loom next --mode fix` / `--mode validate`."
    );
}

#[allow(clippy::type_complexity)]
fn render_json(
    changed: &BTreeSet<&str>,
    affected: &BTreeMap<&str, (&str, BTreeSet<&str>)>,
    proofs: &BTreeSet<(&str, &str)>,
    downstream: &BTreeSet<(&str, &str, &str)>,
    unregistered: &BTreeSet<&str>,
    already_synced: &BTreeSet<&str>,
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
        "already_synced_changed_files": already_synced.iter().collect::<Vec<_>>(),
        "next_step": "actual edited files are narrowed with sync's symbol/import rules; hypothetical unchanged paths stay conservative. Reconcile with `loom sync`, then re-verify only the queued work via `loom next --mode fix` / `--mode validate`.",
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::sorted_pair;
    use crate::types::{DiscoveryCentrality, RelatesTo};

    fn rel(status: &str, kinds: &[&str]) -> RelatesTo {
        RelatesTo {
            id: "rt:a:b".into(),
            from_id: "a".into(),
            to_id: "b".into(),
            from_name: "a".into(),
            to_name: "b".into(),
            inspection_status: status.into(),
            criterion: String::new(),
            confidence: 0.9,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: String::new(),
            priority_score: 0.0,
            notes: String::new(),
            kinds: kinds.iter().map(|k| k.to_string()).collect(),
            stable: false,
            discovery_class: String::new(),
            discovery_signals: Vec::new(),
            discovery_centrality: DiscoveryCentrality::default(),
        }
    }

    fn coupled() -> HashSet<(String, String)> {
        [sorted_pair("a", "b")].into_iter().collect()
    }

    #[test]
    fn impact_predicate_keeps_import_only_edges_green_while_import_still_exists() {
        for edge in [rel("passing", &[]), rel("passing", &["imports"])] {
            assert!(
                !relates_edge_would_stale(&edge, &coupled()),
                "mechanically re-derived import-only edges should not become manual recheck churn"
            );
        }
        assert!(
            relates_edge_would_stale(&rel("passing", &["imports"]), &HashSet::new()),
            "when the live import coupling disappears, the old import claim must be rechecked"
        );
    }

    #[test]
    fn impact_predicate_reopens_independent_only_when_coupling_appears() {
        assert!(
            !relates_edge_would_stale(&rel("independent", &[]), &HashSet::new()),
            "editing one side does not falsify independence by itself"
        );
        assert!(
            relates_edge_would_stale(&rel("independent", &[]), &coupled()),
            "a newly observed structural coupling falsifies independence"
        );
    }

    #[test]
    fn impact_predicate_still_stales_judgment_couplings() {
        assert!(
            relates_edge_would_stale(&rel("passing", &["calls"]), &coupled()),
            "judgment couplings can be invalidated by behavior edits and still require review"
        );
        assert!(
            !relates_edge_would_stale(&rel("passing", &["same_domain"]), &coupled()),
            "meaning-only edges are not code-change work"
        );
    }
}
