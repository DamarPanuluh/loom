use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::db::ensure_initialized;
use crate::output::Printer;
use crate::types::{
    CodeFile, Governs, Implements, RelatesTo, ServesEdge, SyncReport, TargetsEdge, ValidatesEdge,
    Validation,
};

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

type SqliteStore = crate::db::sqlite::SqliteGraphStore;

const REPORT_CAP: usize = 20;

fn run_with_sqlite(
    store: &mut SqliteStore,
    base: &std::path::Path,
    printer: &Printer,
) -> Result<()> {
    let codefiles = store.list_codefiles()?;
    let files_checked = codefiles.len();
    let now = chrono::Utc::now().to_rfc3339();

    let sync_data = load_sync_data(store)?;
    let intents_by_codefile = group_intents_by_codefile(&sync_data.all_implements);
    let ctx = SyncContext {
        now: &now,
        active: &sync_data.active,
        intents_by_codefile: &intents_by_codefile,
        all_implements: &sync_data.all_implements,
        all_relates: &sync_data.all_relates,
        all_governs: &sync_data.all_governs,
        all_targets: &sync_data.all_targets,
        all_serves: &sync_data.all_serves,
        all_validates: &sync_data.all_validates,
        all_validations: &sync_data.all_validations,
    };
    let mut state = SyncState::default();

    scan_files_and_flag_changes(store, base, &codefiles, &ctx, &mut state)?;
    update_physical_facts_and_flag_locators(store, base, &codefiles, &ctx, &mut state)?;
    flag_unverifiable_files(store, &codefiles, &ctx, &mut state)?;
    ripple_delegations(store, base, &ctx, &mut state)?;
    flush_pending_hash_updates(store, &state.pending_hash_updates)?;

    store.set_last_synced(&chrono::Utc::now().to_rfc3339())?;

    let transitions_compacted = compact_transitions(store)?;
    let post_snapshot = store.query_snapshot()?;
    let intents_priority_bumped = crate::db::queries::ripple_bump_by_intent(&post_snapshot).len();
    let report = build_sync_report(
        files_checked,
        intents_priority_bumped,
        transitions_compacted,
        state,
    );

    print_sync_report(store, printer, &post_snapshot, report)
}

struct SyncData {
    active: HashSet<String>,
    all_implements: Vec<Implements>,
    all_relates: Vec<RelatesTo>,
    all_governs: Vec<Governs>,
    all_validates: Vec<ValidatesEdge>,
    all_validations: Vec<Validation>,
    all_targets: Vec<TargetsEdge>,
    all_serves: Vec<ServesEdge>,
}

struct SyncContext<'a> {
    now: &'a str,
    active: &'a HashSet<String>,
    intents_by_codefile: &'a HashMap<&'a str, Vec<String>>,
    all_implements: &'a [Implements],
    all_relates: &'a [RelatesTo],
    all_governs: &'a [Governs],
    all_validates: &'a [ValidatesEdge],
    all_validations: &'a [Validation],
    all_targets: &'a [TargetsEdge],
    all_serves: &'a [ServesEdge],
}

#[derive(Default)]
struct SyncState {
    files_changed: usize,
    targets_flagged: usize,
    relates_to_flagged: usize,
    governs_flagged: usize,
    serves_flagged: usize,
    validations_invalidated: usize,
    changes: Vec<String>,
    missing_files: Vec<String>,
    escaped_files: Vec<String>,
    locators_stale: Vec<String>,
    text_contents: HashMap<String, String>,
    non_utf8_files: HashSet<String>,
    related_edges_flagged: HashSet<String>,
    governs_edges_flagged_ids: HashSet<String>,
    targets_edges_flagged_ids: HashSet<String>,
    serves_edges_flagged_ids: HashSet<String>,
    invalidated_validation_ids: HashSet<String>,
    pending_hash_updates: Vec<(String, String, Option<String>)>,
}

struct ScannedCodeFile {
    mtime_str: String,
    disk_utc: chrono::DateTime<chrono::Utc>,
    new_hash: String,
}

fn load_sync_data(store: &mut SqliteStore) -> Result<SyncData> {
    let snapshot = store.query_snapshot()?;
    let active = snapshot
        .intents
        .iter()
        .map(|intent| intent.id.clone())
        .collect();
    Ok(SyncData {
        active,
        all_implements: snapshot.implements.clone(),
        all_relates: snapshot.relates.clone(),
        all_governs: snapshot.governs.clone(),
        all_validates: snapshot.validates.clone(),
        all_validations: snapshot.validations.clone(),
        all_targets: store.list_all_targets()?,
        all_serves: store.list_all_serves()?,
    })
}

fn group_intents_by_codefile(all_implements: &[Implements]) -> HashMap<&str, Vec<String>> {
    let mut intents_by_codefile: HashMap<&str, Vec<String>> = HashMap::new();
    for im in all_implements {
        intents_by_codefile
            .entry(im.codefile_id.as_str())
            .or_default()
            .push(im.intent_id.clone());
    }
    intents_by_codefile
}

fn scan_files_and_flag_changes(
    store: &mut SqliteStore,
    base: &Path,
    codefiles: &[CodeFile],
    ctx: &SyncContext<'_>,
    state: &mut SyncState,
) -> Result<()> {
    for cf in codefiles {
        let Some(scanned) = scan_codefile(base, cf, state)? else {
            continue;
        };
        let changed = codefile_changed(cf, &scanned);
        let hash_updated = scanned.new_hash != cf.content_hash;
        if hash_updated && !changed {
            // Legacy graph (no stored content_hash): adopt the content hash so
            // future syncs are content-addressed. Deliberately does NOT touch
            // last_modified — mtime is not a content signal, and a no-op sync
            // must never drift the committed graph (so `loom export --check`
            // stays a reliable gate and smell adjudication only re-opens on
            // real content change). last_modified now moves ONLY with content.
            state
                .pending_hash_updates
                .push((cf.id.clone(), scanned.new_hash.clone(), None));
        }
        if !changed {
            continue;
        }

        state.files_changed += 1;
        state.changes.push(cf.path.clone());
        state.pending_hash_updates.push((
            cf.id.clone(),
            scanned.new_hash.clone(),
            Some(scanned.mtime_str.clone()),
        ));

        let cause = format!("{} changed", cf.path);
        let intent_ids = ctx
            .intents_by_codefile
            .get(cf.id.as_str())
            .cloned()
            .unwrap_or_default();
        let affected = affected_intents(
            base,
            cf,
            state.text_contents.get(&cf.path),
            ctx.all_implements,
        );
        let effective_ids: Vec<String> = match &affected {
            None => intent_ids.clone(),
            Some(set) => intent_ids
                .iter()
                .filter(|intent_id| set.contains(intent_id.as_str()))
                .cloned()
                .collect(),
        };

        flag_code_ripple_for_intents(store, ctx, state, &effective_ids, &cause)?;
        invalidate_validations(store, ctx, state, &effective_ids)?;
    }
    Ok(())
}

fn scan_codefile(
    base: &Path,
    cf: &CodeFile,
    state: &mut SyncState,
) -> Result<Option<ScannedCodeFile>> {
    let Some(rel) = crate::repo::confine(base, Path::new(&cf.path)) else {
        state.escaped_files.push(cf.path.clone());
        return Ok(None);
    };
    let abs_path = base.join(rel);
    let meta = match fs::metadata(&abs_path) {
        Ok(m) => m,
        Err(_) => {
            state.missing_files.push(cf.path.clone());
            return Ok(None);
        }
    };
    let mtime = meta.modified().map_err(|e| {
        anyhow::anyhow!(
            "Cannot read mtime for {}: {} — restore the file (or `loom codefile remove <path>` if it is intentionally gone), then re-run `loom sync`.",
            abs_path.display(), e
        )
    })?;
    let disk_utc: chrono::DateTime<chrono::Utc> = mtime.into();
    let mtime_str = disk_utc.to_rfc3339();
    let bytes = fs::read(&abs_path).map_err(|e| {
        anyhow::anyhow!(
            "Cannot read bytes for {}: {} — restore the file (or `loom codefile remove <path>` if it is intentionally gone), then re-run `loom sync`.",
            abs_path.display(), e
        )
    })?;
    let new_hash = crate::repo::content_hash(&bytes);
    match String::from_utf8(bytes) {
        Ok(content) => {
            state.text_contents.insert(cf.path.clone(), content);
        }
        Err(_) => {
            state.non_utf8_files.insert(cf.path.clone());
        }
    }
    Ok(Some(ScannedCodeFile {
        mtime_str,
        disk_utc,
        new_hash,
    }))
}

fn codefile_changed(cf: &CodeFile, scanned: &ScannedCodeFile) -> bool {
    if !cf.content_hash.is_empty() {
        scanned.new_hash != cf.content_hash
    } else if cf.last_modified.is_empty() {
        true
    } else {
        match chrono::DateTime::parse_from_rfc3339(&cf.last_modified) {
            Ok(stored) => scanned.disk_utc > stored.with_timezone(&chrono::Utc),
            Err(_) => true,
        }
    }
}

fn flag_code_ripple_for_intents(
    store: &mut SqliteStore,
    ctx: &SyncContext<'_>,
    state: &mut SyncState,
    intent_ids: &[String],
    cause: &str,
) -> Result<()> {
    for iid in intent_ids
        .iter()
        .filter(|intent_id| ctx.active.contains(*intent_id))
    {
        flag_relates(store, ctx, state, iid, cause, true)?;
        flag_governs(store, ctx, state, iid, cause)?;
        flag_targets(store, ctx, state, iid, cause)?;
        flag_serves(store, ctx, state, iid, cause)?;
    }
    Ok(())
}

fn flag_relates(
    store: &mut SqliteStore,
    ctx: &SyncContext<'_>,
    state: &mut SyncState,
    iid: &str,
    cause: &str,
    require_code_stale_kind: bool,
) -> Result<()> {
    for edge in ctx
        .all_relates
        .iter()
        .filter(|edge| edge.from_id == iid || edge.to_id == iid)
        // Kind-aware staleness: a meaning-only edge (every kind is
        // shares_vocab/same_domain/doc_reference) tracks concept overlap,
        // not this file's code — a code change must not re-open it. A
        // `stable` edge is a settled coupling the analyst exempted from
        // code-change churn (`loom edge stable`), so skip it too.
        .filter(|edge| {
            !require_code_stale_kind
                || (!edge.stable && crate::types::relates_stales_on_code_change(&edge.kinds))
        })
    {
        if state.related_edges_flagged.insert(edge.id.clone())
            && store.flag_relates_to_needs_reverification(edge, cause, ctx.now)?
        {
            state.relates_to_flagged += 1;
        }
    }
    Ok(())
}

fn flag_governs(
    store: &mut SqliteStore,
    ctx: &SyncContext<'_>,
    state: &mut SyncState,
    iid: &str,
    cause: &str,
) -> Result<()> {
    for edge in ctx.all_governs.iter().filter(|edge| edge.intent_id == iid) {
        if state.governs_edges_flagged_ids.insert(edge.id.clone())
            && store.flag_governs_needs_reverification(edge, cause, ctx.now)?
        {
            state.governs_flagged += 1;
        }
    }
    Ok(())
}

fn flag_targets(
    store: &mut SqliteStore,
    ctx: &SyncContext<'_>,
    state: &mut SyncState,
    iid: &str,
    cause: &str,
) -> Result<()> {
    for edge in ctx.all_targets.iter().filter(|edge| edge.intent_id == iid) {
        if state.targets_edges_flagged_ids.insert(edge.id.clone())
            && store.flag_targets_needs_reverification(edge, cause, ctx.now)?
        {
            state.targets_flagged += 1;
        }
    }
    Ok(())
}

fn flag_serves(
    store: &mut SqliteStore,
    ctx: &SyncContext<'_>,
    state: &mut SyncState,
    iid: &str,
    cause: &str,
) -> Result<()> {
    for edge in ctx.all_serves.iter().filter(|edge| edge.intent_id == iid) {
        if state.serves_edges_flagged_ids.insert(edge.id.clone())
            && store.flag_serves_needs_reverification(edge, cause, ctx.now)?
        {
            state.serves_flagged += 1;
        }
    }
    Ok(())
}

fn invalidate_validations(
    store: &mut SqliteStore,
    ctx: &SyncContext<'_>,
    state: &mut SyncState,
    intent_ids: &[String],
) -> Result<()> {
    for edge in ctx
        .all_validates
        .iter()
        .filter(|edge| intent_ids.iter().any(|iid| iid == &edge.intent_id))
    {
        if !state
            .invalidated_validation_ids
            .insert(edge.validation_id.clone())
        {
            continue;
        }
        if ctx.all_validations.iter().any(|validation| {
            validation.id == edge.validation_id
                && validation.last_result != "not_run"
                && validation.last_result != "blocked"
                && !validation.last_result.is_empty()
        }) && store.invalidate_validation(&edge.validation_id)?
        {
            state.validations_invalidated += 1;
        }
    }
    Ok(())
}

fn update_physical_facts_and_flag_locators(
    store: &mut SqliteStore,
    base: &Path,
    codefiles: &[CodeFile],
    ctx: &SyncContext<'_>,
    state: &mut SyncState,
) -> Result<()> {
    for cf in codefiles {
        if let Some(content) = state.text_contents.get(&cf.path) {
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
        } else if state.non_utf8_files.contains(&cf.path) {
            for im in ctx.all_implements {
                if im.codefile_path == cf.path && !im.locator.trim().is_empty() {
                    state.locators_stale.push(format!(
                        "{} @ '{}' (intent '{}') — file is not readable as text; locator unverifiable",
                        im.codefile_path, im.locator, im.intent_name
                    ));
                }
            }
        }
    }
    for im in ctx.all_implements {
        let Some(content) = state.text_contents.get(&im.codefile_path) else {
            continue;
        };
        if !crate::repo::locator_present(content, &im.locator) {
            state.locators_stale.push(format!(
                "{} @ '{}' (intent '{}')",
                im.codefile_path, im.locator, im.intent_name
            ));
            store.flag_implements_needs_reverification(&im.intent_id, &im.codefile_id)?;
        }
    }
    Ok(())
}

fn flag_unverifiable_files(
    store: &mut SqliteStore,
    codefiles: &[CodeFile],
    ctx: &SyncContext<'_>,
    state: &mut SyncState,
) -> Result<()> {
    // Unverifiable files: a registered file that is gone (missing), outside the
    // graph root (escaped), or unreadable as text (non-UTF8) cannot prove the
    // claims grounded in it, so those claims must not stay green. There is no
    // symbol narrowing possible (the content is unavailable), so EVERY intent
    // grounding such a file is affected: flag its IMPLEMENTS grounding and
    // ripple one hop (relates/governs/targets/serves), and invalidate linked
    // validations — mirroring the changed-file path above. Without this, an
    // intent reads fully realized/proven while its code is missing.
    let unverifiable: HashSet<String> = state
        .missing_files
        .iter()
        .chain(state.escaped_files.iter())
        .chain(state.non_utf8_files.iter())
        .cloned()
        .collect();
    for cf in codefiles {
        if !unverifiable.contains(cf.path.as_str()) {
            continue;
        }
        let cause = format!("{} unverifiable (missing/escaped/unreadable)", cf.path);
        let intent_ids = ctx
            .intents_by_codefile
            .get(cf.id.as_str())
            .cloned()
            .unwrap_or_default();
        for iid in intent_ids
            .iter()
            .filter(|intent_id| ctx.active.contains(*intent_id))
        {
            store.flag_implements_needs_reverification(iid, &cf.id)?;
            flag_relates(store, ctx, state, iid, &cause, true)?;
            flag_governs(store, ctx, state, iid, &cause)?;
            flag_targets(store, ctx, state, iid, &cause)?;
            flag_serves(store, ctx, state, iid, &cause)?;
        }
        invalidate_validations(store, ctx, state, &intent_ids)?;
    }
    Ok(())
}

fn ripple_delegations(
    store: &mut SqliteStore,
    base: &Path,
    ctx: &SyncContext<'_>,
    state: &mut SyncState,
) -> Result<()> {
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
                .filter(|i| ctx.active.contains(*i))
            {
                flag_relates(store, ctx, state, iid, &cause, false)?;
                invalidate_delegation_validations(store, ctx, state, iid)?;
            }
        }
        store.set_delegation_export_hash(&delegation.id, &new_hash)?;
    }
    Ok(())
}

fn invalidate_delegation_validations(
    store: &mut SqliteStore,
    ctx: &SyncContext<'_>,
    state: &mut SyncState,
    iid: &str,
) -> Result<()> {
    for edge in ctx
        .all_validates
        .iter()
        .filter(|edge| edge.intent_id == iid)
    {
        if state
            .invalidated_validation_ids
            .insert(edge.validation_id.clone())
            && store.invalidate_validation(&edge.validation_id)?
        {
            state.validations_invalidated += 1;
        }
    }
    Ok(())
}

fn flush_pending_hash_updates(
    store: &mut SqliteStore,
    pending_hash_updates: &[(String, String, Option<String>)],
) -> Result<()> {
    // Flush deferred content-hash updates LAST: every file's ripple above has
    // now landed, so advancing the hashes here can no longer leave a torn graph
    // (see pending_hash_updates above). A crash mid-flush is still safe — the
    // unflushed files simply re-process next sync (idempotently).
    for (cf_id, hash, mtime) in pending_hash_updates {
        match mtime {
            Some(mtime_str) => store.update_codefile_hash_and_mtime(cf_id, hash, mtime_str)?,
            None => store.update_codefile_hash(cf_id, hash)?,
        };
    }
    Ok(())
}

fn compact_transitions(store: &mut SqliteStore) -> Result<usize> {
    // Enforce the transition-note cap that the status nudge and `loom guide`
    // promise: drop routine transition churn beyond `cap` newest per target
    // (regression markers `-> failing`/`-> needs_change` are always preserved by
    // prunable_transition_notes). cap == 0 is the explicit uncapped opt-out.
    // Behavior now matches the words — long runs no longer leave five-digit
    // routine note counts dragging the read path.
    let transition_cap = store.transition_cap()?;
    if transition_cap == 0 {
        return Ok(0);
    }
    let prunable = store.prunable_transition_notes(transition_cap)?;
    for note in &prunable {
        store.delete_note_by_id(&note.id)?;
    }
    Ok(prunable.len())
}

fn build_sync_report(
    files_checked: usize,
    intents_priority_bumped: usize,
    transitions_compacted: usize,
    state: SyncState,
) -> SyncReport {
    SyncReport {
        files_checked,
        files_changed: state.files_changed,
        relates_to_edges_flagged: state.relates_to_flagged,
        intents_priority_bumped,
        targets_edges_flagged: state.targets_flagged,
        governs_edges_flagged: state.governs_flagged,
        serves_edges_flagged: state.serves_flagged,
        validations_invalidated: state.validations_invalidated,
        missing_files: state.missing_files,
        escaped_files: state.escaped_files,
        locators_stale: state.locators_stale,
        changes: state.changes,
        transitions_compacted,
    }
}

fn print_sync_report(
    store: &mut SqliteStore,
    printer: &Printer,
    post_snapshot: &crate::db::queries::QuerySnapshot,
    report: SyncReport,
) -> Result<()> {
    let next_step = next_sync_step(&report);

    if printer.json {
        print_sync_json(store, printer, report, &next_step)
    } else {
        print_sync_text(store, post_snapshot, &report, &next_step)
    }
}

fn next_sync_step(report: &SyncReport) -> String {
    if report.files_changed == 0
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
    }
}

fn print_sync_json(
    store: &mut SqliteStore,
    printer: &Printer,
    report: SyncReport,
    next_step: &str,
) -> Result<()> {
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
    printer.print_json(&crate::output::with_read_anchor(v, store, next_step)?);
    Ok(())
}

fn print_sync_text(
    store: &mut SqliteStore,
    post_snapshot: &crate::db::queries::QuerySnapshot,
    report: &SyncReport,
    next_step: &str,
) -> Result<()> {
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
    print_limited_list("Changed files", &report.changes);
    print_missing_files(&report.missing_files);
    print_escaped_files(&report.escaped_files);
    print_stale_locators(&report.locators_stale);
    println!();
    if report.files_changed == 0
        && report.missing_files.is_empty()
        && report.escaped_files.is_empty()
        && report.locators_stale.is_empty()
    {
        println!("  ✓ All files up to date — no edges need reverification.");
    } else if report.relates_to_edges_flagged + report.governs_edges_flagged > 0 {
        println!(
            "  Each flagged edge carries a transition note naming the changed file (`loom edge show <id>`)."
        );
    }
    let graph_state = store.graph_state(post_snapshot)?;
    println!("  → Next: {next_step}");
    println!("  {}", crate::output::fmt_pulse(&graph_state));
    Ok(())
}

fn print_limited_list(label: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    println!();
    println!("  {label} ({}):", values.len());
    for value in values.iter().take(REPORT_CAP) {
        println!("    {value}");
    }
}

fn print_missing_files(missing_files: &[String]) {
    if missing_files.is_empty() {
        return;
    }
    println!();
    println!(
        "  ⚠ Registered files MISSING on disk ({}):",
        missing_files.len()
    );
    for p in missing_files.iter().take(REPORT_CAP) {
        println!("    {}", p);
    }
    println!("    → `loom codefile remove <path>` to drop a phantom, or restore the file.");
}

fn print_escaped_files(escaped_files: &[String]) {
    if escaped_files.is_empty() {
        return;
    }
    println!();
    println!(
        "  ⚠ Registered paths ESCAPING the graph root ({}):",
        escaped_files.len()
    );
    for p in escaped_files.iter().take(REPORT_CAP) {
        println!("    {}", p);
    }
    println!(
        "    → `loom codefile remove <path>` — files outside the repository cannot be tracked."
    );
}

fn print_stale_locators(locators_stale: &[String]) {
    if locators_stale.is_empty() {
        return;
    }
    println!();
    println!(
        "  ⚠ STALE locators ({} — symbol renamed/moved? grounding flipped to needs_reverification):",
        locators_stale.len()
    );
    for l in locators_stale.iter().take(REPORT_CAP) {
        println!("    {}", l);
    }
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
