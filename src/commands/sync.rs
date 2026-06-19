use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::db::ensure_initialized;
use crate::output::Printer;
use crate::types::SyncReport;

pub fn run(path: &str, printer: &Printer) -> Result<()> {
    let base = if path == "." {
        crate::db::resolve_root()?
    } else {
        Path::new(path)
            .canonicalize()
            .unwrap_or_else(|_| Path::new(path).to_path_buf())
    };

    ensure_initialized(&base)?;
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&base))?;
    run_with_sqlite(&mut store, &base, printer)
}

fn run_with_sqlite(
    store: &mut crate::db::sqlite::SqliteGraphStore,
    base: &std::path::Path,
    printer: &Printer,
) -> Result<()> {
    let codefiles = store.list_codefiles()?;
    let files_checked = codefiles.len();
    let now = chrono::Utc::now().to_rfc3339();

    let snapshot = store.query_snapshot()?;
    let active: HashSet<String> = snapshot
        .intents
        .iter()
        .map(|intent| intent.id.clone())
        .collect();
    let all_implements = snapshot.implements.clone();
    let mut intents_by_codefile: HashMap<&str, Vec<String>> = HashMap::new();
    for im in &all_implements {
        intents_by_codefile
            .entry(im.codefile_id.as_str())
            .or_default()
            .push(im.intent_id.clone());
    }
    let all_relates = snapshot.relates.clone();
    let all_governs = snapshot.governs.clone();
    let all_validates = snapshot.validates.clone();
    let all_validations = snapshot.validations.clone();
    let all_targets = store.list_all_targets()?;
    let all_serves = store.list_all_serves()?;

    let mut files_changed = 0usize;
    let mut targets_flagged = 0usize;
    let mut relates_to_flagged = 0usize;
    let mut governs_flagged = 0usize;
    let mut serves_flagged = 0usize;
    let mut validations_invalidated = 0usize;
    let mut changes: Vec<String> = Vec::new();
    let mut missing_files: Vec<String> = Vec::new();
    let mut escaped_files: Vec<String> = Vec::new();
    let mut text_contents: HashMap<String, String> = HashMap::new();
    let mut non_utf8_files: HashSet<String> = HashSet::new();
    let mut related_edges_flagged: HashSet<String> = HashSet::new();
    let mut governs_edges_flagged_ids: HashSet<String> = HashSet::new();
    let mut targets_edges_flagged_ids: HashSet<String> = HashSet::new();
    let mut serves_edges_flagged_ids: HashSet<String> = HashSet::new();
    let mut invalidated_validation_ids: HashSet<String> = HashSet::new();
    // Crash-safety (atomic-enough sync): a changed file's content hash is the
    // signal that suppresses re-processing on the next sync. Writing it BEFORE
    // the file's ripple (edge flips / validation invalidation / fact updates)
    // means a kill in between would advance the hash while leaving dependent
    // edges green-but-stale, and the file would never be re-detected. So we
    // COLLECT hash updates here and flush them LAST, after every file's ripple
    // has landed. A kill anywhere before the flush leaves the hash unadvanced;
    // the next `loom sync` re-detects the file and re-applies the (idempotent)
    // ripple. (cf_id, new_hash, Some(mtime) for a content change | None to adopt
    // a legacy hash without touching mtime.)
    let mut pending_hash_updates: Vec<(String, String, Option<String>)> = Vec::new();

    for cf in &codefiles {
        let Some(rel) = crate::repo::confine(base, Path::new(&cf.path)) else {
            escaped_files.push(cf.path.clone());
            continue;
        };
        let abs_path = base.join(rel);
        let meta = match fs::metadata(&abs_path) {
            Ok(m) => m,
            Err(_) => {
                missing_files.push(cf.path.clone());
                continue;
            }
        };
        let mtime = meta.modified().map_err(|e| {
            anyhow::anyhow!(
                "Cannot read mtime for {}: {} — restore the file (or `loom codefile remove <path>` if it is intentionally gone), then re-run `loom sync`.",
                abs_path.display(), e
            )
        })?;
        let mtime_str = {
            let dt: chrono::DateTime<chrono::Utc> = mtime.into();
            dt.to_rfc3339()
        };
        let bytes = fs::read(&abs_path).map_err(|e| {
            anyhow::anyhow!(
                "Cannot read bytes for {}: {} — restore the file (or `loom codefile remove <path>` if it is intentionally gone), then re-run `loom sync`.",
                abs_path.display(), e
            )
        })?;
        let new_hash = crate::repo::content_hash(&bytes);
        match String::from_utf8(bytes) {
            Ok(content) => {
                text_contents.insert(cf.path.clone(), content);
            }
            Err(_) => {
                non_utf8_files.insert(cf.path.clone());
            }
        }
        let changed = if !cf.content_hash.is_empty() {
            new_hash != cf.content_hash
        } else if cf.last_modified.is_empty() {
            true
        } else {
            match chrono::DateTime::parse_from_rfc3339(&cf.last_modified) {
                Ok(stored) => {
                    let stored_utc = stored.with_timezone(&chrono::Utc);
                    let disk_utc: chrono::DateTime<chrono::Utc> = mtime.into();
                    disk_utc > stored_utc
                }
                Err(_) => true,
            }
        };

        let hash_updated = new_hash != cf.content_hash;
        if hash_updated && !changed {
            // Legacy graph (no stored content_hash): adopt the content hash so
            // future syncs are content-addressed. Deliberately does NOT touch
            // last_modified — mtime is not a content signal, and a no-op sync
            // must never drift the committed graph (so `loom export --check`
            // stays a reliable gate and smell adjudication only re-opens on
            // real content change). last_modified now moves ONLY with content.
            pending_hash_updates.push((cf.id.clone(), new_hash.clone(), None));
        }
        if !changed {
            continue;
        }

        files_changed += 1;
        changes.push(cf.path.clone());
        pending_hash_updates.push((cf.id.clone(), new_hash.clone(), Some(mtime_str.clone())));

        let cause = format!("{} changed", cf.path);
        let intent_ids = intents_by_codefile
            .get(cf.id.as_str())
            .cloned()
            .unwrap_or_default();
        let affected = affected_intents(base, cf, text_contents.get(&cf.path), &all_implements);
        let effective_ids: Vec<String> = match &affected {
            None => intent_ids.clone(),
            Some(set) => intent_ids
                .iter()
                .filter(|intent_id| set.contains(intent_id.as_str()))
                .cloned()
                .collect(),
        };

        for iid in effective_ids
            .iter()
            .filter(|intent_id| active.contains(*intent_id))
        {
            for edge in all_relates
                .iter()
                .filter(|edge| edge.from_id == *iid || edge.to_id == *iid)
                // Kind-aware staleness: a meaning-only edge (every kind is
                // shares_vocab/same_domain/doc_reference) tracks concept overlap,
                // not this file's code — a code change must not re-open it. A
                // `stable` edge is a settled coupling the analyst exempted from
                // code-change churn (`loom edge stable`), so skip it too.
                .filter(|edge| {
                    !edge.stable && crate::types::relates_stales_on_code_change(&edge.kinds)
                })
            {
                if related_edges_flagged.insert(edge.id.clone())
                    && store.flag_relates_to_needs_reverification(edge, &cause, &now)?
                {
                    relates_to_flagged += 1;
                }
            }
            for edge in all_governs.iter().filter(|edge| edge.intent_id == *iid) {
                if governs_edges_flagged_ids.insert(edge.id.clone())
                    && store.flag_governs_needs_reverification(edge, &cause, &now)?
                {
                    governs_flagged += 1;
                }
            }
            for edge in all_targets.iter().filter(|edge| edge.intent_id == *iid) {
                if targets_edges_flagged_ids.insert(edge.id.clone())
                    && store.flag_targets_needs_reverification(edge, &cause, &now)?
                {
                    targets_flagged += 1;
                }
            }
            for edge in all_serves.iter().filter(|edge| edge.intent_id == *iid) {
                if serves_edges_flagged_ids.insert(edge.id.clone())
                    && store.flag_serves_needs_reverification(edge, &cause, &now)?
                {
                    serves_flagged += 1;
                }
            }
        }

        for edge in all_validates
            .iter()
            .filter(|edge| effective_ids.iter().any(|iid| iid == &edge.intent_id))
        {
            if !invalidated_validation_ids.insert(edge.validation_id.clone()) {
                continue;
            }
            if all_validations.iter().any(|validation| {
                validation.id == edge.validation_id
                    && validation.last_result != "not_run"
                    && validation.last_result != "blocked"
                    && !validation.last_result.is_empty()
            }) && store.invalidate_validation(&edge.validation_id)?
            {
                validations_invalidated += 1;
            }
        }
    }

    let mut locators_stale: Vec<String> = Vec::new();
    for cf in &codefiles {
        if let Some(content) = text_contents.get(&cf.path) {
            let facts = crate::repo::extract_physical_facts(base, &cf.path, content);
            if facts.imports != cf.imports {
                store.update_codefile_imports(&cf.id, &facts.imports)?;
            }
            if facts.symbols != cf.symbols {
                store.update_codefile_symbols(&cf.id, &facts.symbols)?;
            }
            if facts.symbol_facts != cf.symbol_facts {
                store.update_codefile_symbol_facts(&cf.id, &facts.symbol_facts)?;
            }
        } else if non_utf8_files.contains(&cf.path) {
            for im in &all_implements {
                if im.codefile_path == cf.path && !im.locator.trim().is_empty() {
                    locators_stale.push(format!(
                        "{} @ '{}' (intent '{}') — file is not readable as text; locator unverifiable",
                        im.codefile_path, im.locator, im.intent_name
                    ));
                }
            }
        }
    }
    for im in &all_implements {
        let Some(content) = text_contents.get(&im.codefile_path) else {
            continue;
        };
        if !crate::repo::locator_present(content, &im.locator) {
            locators_stale.push(format!(
                "{} @ '{}' (intent '{}')",
                im.codefile_path, im.locator, im.intent_name
            ));
            store.flag_implements_needs_reverification(&im.intent_id, &im.codefile_id)?;
        }
    }

    // Unverifiable files: a registered file that is gone (missing), outside the
    // graph root (escaped), or unreadable as text (non-UTF8) cannot prove the
    // claims grounded in it, so those claims must not stay green. There is no
    // symbol narrowing possible (the content is unavailable), so EVERY intent
    // grounding such a file is affected: flag its IMPLEMENTS grounding and
    // ripple one hop (relates/governs/targets/serves), and invalidate linked
    // validations — mirroring the changed-file path above. Without this, an
    // intent reads fully realized/proven while its code is missing.
    let unverifiable: HashSet<&str> = missing_files
        .iter()
        .chain(escaped_files.iter())
        .chain(non_utf8_files.iter())
        .map(String::as_str)
        .collect();
    for cf in &codefiles {
        if !unverifiable.contains(cf.path.as_str()) {
            continue;
        }
        let cause = format!("{} unverifiable (missing/escaped/unreadable)", cf.path);
        let intent_ids = intents_by_codefile
            .get(cf.id.as_str())
            .cloned()
            .unwrap_or_default();
        for iid in intent_ids
            .iter()
            .filter(|intent_id| active.contains(*intent_id))
        {
            store.flag_implements_needs_reverification(iid, &cf.id)?;
            for edge in all_relates
                .iter()
                .filter(|edge| edge.from_id == *iid || edge.to_id == *iid)
                // Kind-aware staleness: a meaning-only edge (every kind is
                // shares_vocab/same_domain/doc_reference) tracks concept overlap,
                // not this file's code — a code change must not re-open it. A
                // `stable` edge is a settled coupling the analyst exempted from
                // code-change churn (`loom edge stable`), so skip it too.
                .filter(|edge| {
                    !edge.stable && crate::types::relates_stales_on_code_change(&edge.kinds)
                })
            {
                if related_edges_flagged.insert(edge.id.clone())
                    && store.flag_relates_to_needs_reverification(edge, &cause, &now)?
                {
                    relates_to_flagged += 1;
                }
            }
            for edge in all_governs.iter().filter(|edge| edge.intent_id == *iid) {
                if governs_edges_flagged_ids.insert(edge.id.clone())
                    && store.flag_governs_needs_reverification(edge, &cause, &now)?
                {
                    governs_flagged += 1;
                }
            }
            for edge in all_targets.iter().filter(|edge| edge.intent_id == *iid) {
                if targets_edges_flagged_ids.insert(edge.id.clone())
                    && store.flag_targets_needs_reverification(edge, &cause, &now)?
                {
                    targets_flagged += 1;
                }
            }
            for edge in all_serves.iter().filter(|edge| edge.intent_id == *iid) {
                if serves_edges_flagged_ids.insert(edge.id.clone())
                    && store.flag_serves_needs_reverification(edge, &cause, &now)?
                {
                    serves_flagged += 1;
                }
            }
        }
        for edge in all_validates
            .iter()
            .filter(|edge| intent_ids.iter().any(|iid| iid == &edge.intent_id))
        {
            if !invalidated_validation_ids.insert(edge.validation_id.clone()) {
                continue;
            }
            if all_validations.iter().any(|validation| {
                validation.id == edge.validation_id
                    && validation.last_result != "not_run"
                    && validation.last_result != "blocked"
                    && !validation.last_result.is_empty()
            }) && store.invalidate_validation(&edge.validation_id)?
            {
                validations_invalidated += 1;
            }
        }
    }

    // Cross-service (federation) ripple. A delegation watches a child graph's
    // committed export; when that export's content hash changes the child's
    // contract may have moved, so re-open the seam intents that depend on it.
    // First observation just records the baseline (no ripple). The baseline is
    // advanced AFTER the seam edges are flagged so a crash leaves it
    // re-detectable (same discipline as the codefile hashes). No delegations →
    // this whole block is a no-op (the single-repo case, e.g. loom's own graph).
    let delegations = store.list_delegations()?;
    for delegation in &delegations {
        let Some(rel) = crate::repo::confine(base, Path::new(&delegation.target)) else {
            continue;
        };
        let Ok(bytes) = fs::read(base.join(rel)) else {
            continue; // missing child export — `loom coverage`/`delegate list` report it
        };
        let new_hash = crate::repo::content_hash(&bytes);
        if new_hash == delegation.export_hash {
            continue;
        }
        if !delegation.export_hash.is_empty() {
            // Ripple BEFORE advancing the baseline (crash-safety).
            let cause = format!("child export {} changed", delegation.target);
            for iid in delegation
                .seam_intents
                .iter()
                .filter(|i| active.contains(*i))
            {
                for edge in all_relates
                    .iter()
                    .filter(|edge| edge.from_id == *iid || edge.to_id == *iid)
                {
                    if related_edges_flagged.insert(edge.id.clone())
                        && store.flag_relates_to_needs_reverification(edge, &cause, &now)?
                    {
                        relates_to_flagged += 1;
                    }
                }
                for edge in all_validates.iter().filter(|edge| &edge.intent_id == iid) {
                    if invalidated_validation_ids.insert(edge.validation_id.clone())
                        && store.invalidate_validation(&edge.validation_id)?
                    {
                        validations_invalidated += 1;
                    }
                }
            }
        }
        store.set_delegation_export_hash(&delegation.id, &new_hash)?;
    }

    // Flush deferred content-hash updates LAST: every file's ripple above has
    // now landed, so advancing the hashes here can no longer leave a torn graph
    // (see pending_hash_updates above). A crash mid-flush is still safe — the
    // unflushed files simply re-process next sync (idempotently).
    for (cf_id, hash, mtime) in &pending_hash_updates {
        match mtime {
            Some(mtime_str) => store.update_codefile_hash_and_mtime(cf_id, hash, mtime_str)?,
            None => store.update_codefile_hash(cf_id, hash)?,
        };
    }

    store.set_last_synced(&chrono::Utc::now().to_rfc3339())?;

    // Enforce the transition-note cap that the status nudge and `loom guide`
    // promise: drop routine transition churn beyond `cap` newest per target
    // (regression markers `-> failing`/`-> needs_change` are always preserved by
    // prunable_transition_notes). cap == 0 is the explicit uncapped opt-out.
    // Behavior now matches the words — long runs no longer leave five-digit
    // routine note counts dragging the read path.
    let transition_cap = store.transition_cap()?;
    let transitions_compacted = if transition_cap > 0 {
        let prunable = store.prunable_transition_notes(transition_cap)?;
        for note in &prunable {
            store.delete_note_by_id(&note.id)?;
        }
        prunable.len()
    } else {
        0
    };

    // Graded ripple: the one-hop flips above produced the stale frontier; count
    // the intents two/three hops out that now carry a decaying priority bump
    // (status untouched). Reuse this post-sync snapshot for the closing pulse.
    let post_snapshot = store.query_snapshot()?;
    let intents_priority_bumped = crate::db::queries::ripple_bump_by_intent(&post_snapshot).len();

    let report = SyncReport {
        files_checked,
        files_changed,
        relates_to_edges_flagged: relates_to_flagged,
        intents_priority_bumped,
        targets_edges_flagged: targets_flagged,
        governs_edges_flagged: governs_flagged,
        serves_edges_flagged: serves_flagged,
        validations_invalidated,
        missing_files,
        escaped_files,
        locators_stale,
        changes,
        transitions_compacted,
    };

    const REPORT_CAP: usize = 20;
    let next_step = if report.files_changed == 0
        && report.missing_files.is_empty()
        && report.escaped_files.is_empty()
        && report.locators_stale.is_empty()
    {
        "`loom status` (or `loom next --all` for closeout)".to_string()
    } else if report.files_changed == 0
        && report.missing_files.is_empty()
        && report.escaped_files.is_empty()
        && !report.locators_stale.is_empty()
    {
        "`loom next --mode fix` to re-inspect IMPLEMENTS edges with stale locators.".to_string()
    } else {
        format!(
            "`loom next --mode fix{}` to re-inspect flagged edges{}",
            if report.relates_to_edges_flagged > 10 {
                " --take 20"
            } else {
                ""
            },
            if report.governs_edges_flagged > 0 {
                ", and `loom next --mode quality` to re-earn flagged quality green."
            } else {
                "."
            }
        )
    };

    if printer.json {
        let mut v = serde_json::to_value(&report)?;
        let Some(obj) = v.as_object_mut() else {
            anyhow::bail!("SyncReport did not serialize to a JSON object");
        };
        for (key, total_key) in [
            ("changes", "changes_total"),
            ("missing_files", "missing_files_total"),
            ("escaped_files", "escaped_files_total"),
            ("locators_stale", "locators_stale_total"),
        ] {
            let total = obj
                .get(key)
                .and_then(|a| a.as_array())
                .map_or(0, |a| a.len());
            if let Some(arr) = obj.get_mut(key).and_then(|a| a.as_array_mut()) {
                arr.truncate(REPORT_CAP);
            }
            obj.insert(total_key.to_string(), total.into());
        }
        printer.print_json(&crate::output::with_read_anchor(v, store, &next_step)?);
    } else {
        println!("── loom sync ────────────────────────────────────────────────────────");
        println!("  Files checked:                 {}", report.files_checked);
        println!("  Files changed since last sync: {}", report.files_changed);
        println!(
            "  RELATES_TO edges flagged:      {}",
            report.relates_to_edges_flagged
        );
        if report.intents_priority_bumped > 0 {
            println!(
                "  Intents priority-bumped (2-3 hop): {} (graded ripple — no status change)",
                report.intents_priority_bumped
            );
        }
        println!(
            "  GOVERNS verdicts flagged:      {}",
            report.governs_edges_flagged
        );
        println!(
            "  TARGETS edges flagged:         {}",
            report.targets_edges_flagged
        );
        println!(
            "  SERVES edges flagged:          {}",
            report.serves_edges_flagged
        );
        println!(
            "  Validations invalidated:       {}",
            report.validations_invalidated
        );
        if report.transitions_compacted > 0 {
            println!(
                "  Transition notes compacted:    {} (routine churn beyond the cap; regressions kept)",
                report.transitions_compacted
            );
        }
        if !report.changes.is_empty() {
            println!();
            println!("  Changed files ({}):", report.changes.len());
            for c in report.changes.iter().take(REPORT_CAP) {
                println!("    {c}");
            }
        }
        if !report.missing_files.is_empty() {
            println!();
            println!(
                "  ⚠ Registered files MISSING on disk ({}):",
                report.missing_files.len()
            );
            for p in report.missing_files.iter().take(REPORT_CAP) {
                println!("    {}", p);
            }
            println!("    → `loom codefile remove <path>` to drop a phantom, or restore the file.");
        }
        if !report.escaped_files.is_empty() {
            println!();
            println!(
                "  ⚠ Registered paths ESCAPING the graph root ({}):",
                report.escaped_files.len()
            );
            for p in report.escaped_files.iter().take(REPORT_CAP) {
                println!("    {}", p);
            }
            println!("    → `loom codefile remove <path>` — files outside the repository cannot be tracked.");
        }
        if !report.locators_stale.is_empty() {
            println!();
            println!(
                "  ⚠ STALE locators ({} — symbol renamed/moved? grounding flipped to needs_reverification):",
                report.locators_stale.len()
            );
            for l in report.locators_stale.iter().take(REPORT_CAP) {
                println!("    {}", l);
            }
        }
        println!();
        if report.files_changed == 0
            && report.missing_files.is_empty()
            && report.escaped_files.is_empty()
            && report.locators_stale.is_empty()
        {
            println!("  ✓ All files up to date — no edges need reverification.");
        } else if report.relates_to_edges_flagged + report.governs_edges_flagged > 0 {
            println!("  Each flagged edge carries a transition note naming the changed file (`loom edge show <id>`).");
        }
        let graph_state = store.graph_state(&post_snapshot)?;
        println!("  → Next: {next_step}");
        println!("  {}", crate::output::fmt_pulse(&graph_state));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helper: which intents grounded in a changed file are ACTUALLY affected —
// the symbol-level narrowing. Returns `None` to mean "can't attribute, flip the
// whole file" (the conservative default that never under-flags), or `Some(set)`
// of intent ids whose IMPLEMENTS locator on this file is file-level (empty) or
// names a symbol whose body hash changed.
// ---------------------------------------------------------------------------
fn affected_intents(
    base: &Path,
    cf: &crate::types::CodeFile,
    content: Option<&String>,
    all_implements: &[crate::types::Implements],
) -> Option<HashSet<String>> {
    // No readable text (binary/non-UTF8) → can't diff symbols.
    let content = content?;
    // No prior symbol facts (never extracted) → nothing to diff against.
    if cf.symbol_facts.is_empty() {
        return None;
    }
    let facts = crate::repo::extract_physical_facts(base, &cf.path, content);
    // Unsupported language / feature-light build (no tree-sitter) → whole-file.
    if facts.symbol_facts.is_empty() {
        return None;
    }
    // Need body hashes on BOTH sides; a pre-upgrade graph (or feature-light
    // extraction) lacks them → fall back rather than mis-diff.
    if cf
        .symbol_facts
        .iter()
        .chain(facts.symbol_facts.iter())
        .any(|f| f.body_hash.is_empty())
    {
        return None;
    }
    let old: HashMap<&str, &str> = cf
        .symbol_facts
        .iter()
        .map(|f| (f.label.as_str(), f.body_hash.as_str()))
        .collect();
    let name_of: HashMap<&str, &str> = cf
        .symbol_facts
        .iter()
        .chain(facts.symbol_facts.iter())
        .map(|f| (f.label.as_str(), f.name.as_str()))
        .collect();
    // Changed = added, removed, or body hash differs (matched by label).
    let mut changed: HashSet<&str> = HashSet::new();
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
    for lbl in old.keys() {
        if !new_labels.contains(lbl) {
            changed.insert(lbl);
        }
    }
    // Content changed but NO symbol changed → the edit is outside every symbol
    // (comments / whitespace / imports / module-level). Conservative: fall back
    // to whole-file rather than risk missing a real behavior change.
    if changed.is_empty() {
        return None;
    }
    let changed_names: Vec<&str> = changed
        .iter()
        .filter_map(|lbl| name_of.get(lbl).copied())
        .filter(|n| !n.is_empty())
        .collect();
    // An intent is affected iff one of its IMPLEMENTS edges on THIS file is
    // file-level (empty locator) or names a changed symbol. Substring match
    // mirrors `locator_present`; it over-flags rather than under-flags.
    let mut affected = HashSet::new();
    for im in all_implements.iter().filter(|im| im.codefile_id == cf.id) {
        let loc = im.locator.trim();
        if loc.is_empty() || changed_names.iter().any(|n| loc.contains(n)) {
            affected.insert(im.intent_id.clone());
        }
    }
    Some(affected)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "treesitter")]
    use crate::types::Implements;
    use crate::types::{CodeFile, SymbolFact};

    fn cf(symbol_facts: Vec<SymbolFact>) -> CodeFile {
        CodeFile {
            id: "cf1".into(),
            path: "src/foo.rs".into(),
            language: "rust".into(),
            last_modified: String::new(),
            imports: vec![],
            symbols: vec![],
            symbol_facts,
            content_hash: String::new(),
        }
    }
    #[cfg(feature = "treesitter")]
    fn imp(intent: &str, locator: &str) -> Implements {
        Implements {
            id: format!("imp:{intent}"),
            intent_id: intent.into(),
            codefile_id: "cf1".into(),
            intent_name: intent.into(),
            codefile_path: "src/foo.rs".into(),
            inspection_status: "passing".into(),
            criterion: String::new(),
            confidence: 0.0,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: String::new(),
            locator: locator.into(),
            notes: String::new(),
            created_at: String::new(),
        }
    }

    // `None` (whole-file fallback) whenever the change can't be attributed to
    // symbols — holds in EVERY build. This is the --no-default-features safety
    // net: feature-light extraction yields no symbol facts → fall back.
    #[test]
    fn affected_falls_back_without_prior_facts() {
        let base = std::env::temp_dir();
        let c = "fn a() {}\n".to_string();
        assert!(affected_intents(&base, &cf(vec![]), Some(&c), &[]).is_none());
    }
    #[test]
    fn affected_falls_back_without_content() {
        assert!(affected_intents(&std::env::temp_dir(), &cf(vec![]), None, &[]).is_none());
    }

    // The narrowing itself needs tree-sitter to extract symbols.
    #[cfg(feature = "treesitter")]
    #[test]
    fn affected_narrows_to_the_changed_symbol_only() {
        let base = std::env::temp_dir();
        let old = "fn a() {\n    1\n}\nfn b() {\n    2\n}\n";
        let new = "fn a() {\n    999\n}\nfn b() {\n    2\n}\n"; // only a's body changed
        let old_facts = crate::repo::extract_physical_facts(&base, "src/foo.rs", old).symbol_facts;
        assert!(!old_facts.is_empty(), "tree-sitter extracted symbols");
        let codefile = cf(old_facts);
        let impls = vec![imp("ia", "fn a"), imp("ib", "fn b"), imp("ifile", "")];
        let affected = affected_intents(&base, &codefile, Some(&new.to_string()), &impls)
            .expect("symbol-level diff, not the whole-file fallback");
        assert!(
            affected.contains("ia"),
            "intent on the changed symbol flips"
        );
        assert!(
            affected.contains("ifile"),
            "file-level grounding always flips"
        );
        assert!(
            !affected.contains("ib"),
            "intent on the UNCHANGED symbol must NOT flip — this is the win"
        );
    }

    // Content changed but every symbol body is identical (a comment shifted the
    // lines) → conservative whole-file fallback, never a silent miss.
    #[cfg(feature = "treesitter")]
    #[test]
    fn affected_falls_back_when_change_is_outside_symbols() {
        let base = std::env::temp_dir();
        let old = "fn a() {\n    1\n}\n";
        let new = "// added comment\nfn a() {\n    1\n}\n"; // a moves but its body is identical
        let old_facts = crate::repo::extract_physical_facts(&base, "src/foo.rs", old).symbol_facts;
        let codefile = cf(old_facts);
        let impls = vec![imp("ia", "fn a")];
        assert!(
            affected_intents(&base, &codefile, Some(&new.to_string()), &impls).is_none(),
            "no symbol body changed → fall back to whole-file (conservative)"
        );
    }
}
