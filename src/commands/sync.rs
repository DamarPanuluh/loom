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

    // Detect changes and re-extract physical facts FIRST, then ripple. The
    // independent-edge coupling gate must judge against CURRENT imports (a NEW
    // import in this sync must re-open an independent edge THIS sync, not never),
    // so `coupled_intent_pairs` is built from the codefiles AFTER re-extraction.
    let code_ripple_targets = scan_files_and_flag_changes(base, &codefiles, &ctx, &mut state)?;
    update_physical_facts_and_flag_locators(store, base, &codefiles, &ctx, &mut state)?;
    let codefiles_after = store.list_codefiles()?;
    let coupled_intent_pairs =
        compute_coupled_intent_pairs(&codefiles_after, &sync_data.all_implements);
    apply_code_ripples(
        store,
        &ctx,
        &mut state,
        &code_ripple_targets,
        &coupled_intent_pairs,
    )?;
    // After re-extraction, stamp mechanical relationship kinds from current
    // physical facts (the `loom populate kinds` derivation, folded into the loop).
    // Only on a code-changing sync — kinds derive from imports, which only move
    // when files do. This keeps the import-coupling staling exemption HONEST in
    // `loom explain` (an un-kinded edge between import-coupled files becomes
    // `[imports]`) instead of requiring a separate manual command.
    if !code_ripple_targets.is_empty() {
        backfill_mechanical_kinds(store, &mut state)?;
    }
    flag_unverifiable_files(store, &codefiles, &ctx, &mut state, &coupled_intent_pairs)?;
    ripple_delegations(store, base, &ctx, &mut state, &coupled_intent_pairs)?;
    flush_pending_hash_updates(store, &state.pending_hash_updates)?;
    // Reconcile settled hypothesis lineage: confirmed hypotheses' TARGETS no longer
    // stale (the ripple skips them), and any staled before that rule existed are
    // returned to passing here. Idempotent — a no-op once the lineage is settled.
    store.settle_confirmed_hypothesis_targets()?;

    let transitions_compacted = compact_transitions(store)?;

    // A TRUE no-op sync must NOT bump last_synced. That field travels in the
    // committed export and `export --check` is a byte-exact compare, so an
    // unconditional bump flips the freshness gate to STALE with nothing actually
    // changed — and never converges (export → no-op sync → STALE, forever). Only
    // stamp it when something moved; any real graph change also moves graph
    // CONTENT, which drives export staleness on its own, so skipping the
    // timestamp on a genuine no-op is safe and makes sync idempotent.
    let anything_changed = state.files_changed > 0
        || state.facts_rewritten > 0
        || state.kinds_backfilled > 0
        || state.seam_groundings_reopened > 0
        || state.targets_flagged > 0
        || state.relates_to_flagged > 0
        || state.governs_flagged > 0
        || state.serves_flagged > 0
        || state.validations_invalidated > 0
        || !state.pending_hash_updates.is_empty()
        || !state.locators_stale.is_empty()
        || transitions_compacted > 0;
    if anything_changed {
        store.set_last_synced(&now)?;
    }
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
    /// Physical-fact rewrites (imports/symbols/symbol_facts) persisted this sync.
    /// Folded into `anything_changed` so a fact rewrite — e.g. from an extractor
    /// change on a file whose content didn't move — can never masquerade as a
    /// no-op and silently drift the committed export out from under `export --check`.
    facts_rewritten: usize,
    /// RELATES_TO edges that got mechanical kinds (imports/shares_file/…) stamped
    /// from current physical facts this sync — `loom populate kinds`, folded into
    /// the loop so the import-coupling staling exemption is HONEST in `loom
    /// explain` without a separate manual step.
    kinds_backfilled: usize,
    /// Seam-intent IMPLEMENTS edges re-opened because a delegated child's export
    /// (its contract) changed — the cross-service federation ripple.
    seam_groundings_reopened: usize,
    changes: Vec<String>,
    missing_files: Vec<String>,
    escaped_files: Vec<String>,
    locators_stale: Vec<String>,
    text_contents: HashMap<String, String>,
    /// Paths whose on-disk content hash differs from the stored one — the ONLY
    /// files whose physical facts can have changed. Re-extraction (tree-sitter
    /// parse) is content-addressed: an unchanged file's facts are already
    /// current, so a no-op sync must not re-parse every file (it cost ~15s per
    /// large file, scaling with repo size instead of change size). Stale facts
    /// can only ever persist on an UNCHANGED file, where nothing changed to
    /// detect — any edit that could matter also moves the hash into this set.
    content_changed: HashSet<String>,
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

/// Intent pairs whose grounded code is structurally import-coupled: a file
/// grounding intent A statically imports a file grounding intent B (either
/// direction). Built from the SAME load-time snapshot the ripple judges edges
/// against, mirroring `detect_undeclared_coupling` exactly (the physical plane's
/// one coupling signal). The code-change ripple uses this to decide whether an
/// `independent` RELATES_TO verdict — the claim "these two intents do NOT
/// interact" — is actually undermined: a behavior-preserving edit to one side
/// does not create an interaction, so only the appearance of an import coupling
/// re-opens it. Keys are lexicographically-sorted (min, max) id pairs so a
/// lookup is direction-agnostic.
fn compute_coupled_intent_pairs(
    codefiles: &[CodeFile],
    all_implements: &[Implements],
) -> HashSet<(String, String)> {
    let mut intents_on_file: HashMap<&str, HashSet<&str>> = HashMap::new();
    for im in all_implements {
        intents_on_file
            .entry(im.codefile_path.as_str())
            .or_default()
            .insert(im.intent_id.as_str());
    }
    let mut coupled: HashSet<(String, String)> = HashSet::new();
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
                    if a == b {
                        continue;
                    }
                    coupled.insert(super::sorted_pair(a, b));
                }
            }
        }
    }
    coupled
}

/// One changed file's deferred code ripple: the intents it affects and the
/// human-readable cause. Collected during change detection and applied AFTER
/// physical-fact re-extraction, so the relates ripple judges independence
/// against CURRENT import coupling.
struct CodeRippleTarget {
    effective_ids: Vec<String>,
    cause: String,
}

fn scan_files_and_flag_changes(
    base: &Path,
    codefiles: &[CodeFile],
    ctx: &SyncContext<'_>,
    state: &mut SyncState,
) -> Result<Vec<CodeRippleTarget>> {
    let mut targets = Vec::new();
    for cf in codefiles {
        let Some(scanned) = scan_codefile(base, cf, state)? else {
            continue;
        };
        let changed = codefile_changed(cf, &scanned);
        let hash_updated = scanned.new_hash != cf.content_hash;
        if hash_updated {
            // Hash differs from the stored one (a real edit OR a legacy file with
            // no recorded hash): its physical facts may have changed, so the
            // facts pass must re-extract it. Equal hash → byte-identical → skip.
            state.content_changed.insert(cf.path.clone());
        }
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

        // Defer the ripple: it must judge `independent` edges against the
        // imports re-extracted AFTER this pass (see `apply_code_ripples`).
        targets.push(CodeRippleTarget {
            effective_ids,
            cause,
        });
    }
    Ok(targets)
}

/// Apply each changed file's deferred ripple now that physical facts (imports)
/// have been re-extracted and `coupled_intent_pairs` reflects current coupling.
fn apply_code_ripples(
    store: &mut SqliteStore,
    ctx: &SyncContext<'_>,
    state: &mut SyncState,
    targets: &[CodeRippleTarget],
    coupled: &HashSet<(String, String)>,
) -> Result<()> {
    for target in targets {
        flag_code_ripple_for_intents(
            store,
            ctx,
            state,
            &target.effective_ids,
            &target.cause,
            coupled,
        )?;
        invalidate_validations(store, ctx, state, &target.effective_ids)?;
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
        Err(e) => {
            // A non-UTF-8 source (a latin-1 byte in a string/comment, a stray
            // 0xff) must NOT leave the file ungraded forever with the misleading
            // "run `loom sync` to refresh" (sync can't change the file's
            // encoding). Decode LOSSILY: identifiers are ASCII, so tree-sitter
            // still extracts every symbol — only the offending bytes become
            // U+FFFD. content_hash is computed on the RAW bytes above, so change
            // detection is unaffected. Track it for a soft advisory, but it is
            // NOT unverifiable: its symbols and locators resolve normally.
            let content = String::from_utf8_lossy(&e.into_bytes()).into_owned();
            state.text_contents.insert(cf.path.clone(), content);
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
    coupled: &HashSet<(String, String)>,
) -> Result<()> {
    for iid in intent_ids
        .iter()
        .filter(|intent_id| ctx.active.contains(*intent_id))
    {
        flag_relates(store, ctx, state, iid, cause, true, coupled)?;
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
    coupled: &HashSet<(String, String)>,
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
        //
        // Independence is durable against behavior-preserving change: an
        // `independent` verdict is the claim "these two intents do NOT
        // interact", and editing one side's code does not create an
        // interaction. So the code-change ripple re-opens an independent edge
        // ONLY when a structural import coupling now exists between the pair
        // (the same physical-plane signal `detect_undeclared_coupling` uses) —
        // never on every unrelated edit. This is what stops a few central
        // files from re-staling the whole N×N grid every sync. The
        // federation/meaning ripple (require_code_stale_kind == false) is not a
        // code change, so it keeps the status-only flip for every status.
        .filter(|edge| {
            if !require_code_stale_kind {
                return true;
            }
            if edge.stable {
                return false;
            }
            if edge.inspection_status == "independent" {
                return coupled.contains(&super::sorted_pair(&edge.from_id, &edge.to_id));
            }
            // A meaning-only edge (every kind is shares_vocab/same_domain/
            // doc_reference) tracks concept overlap, not code — never re-open it.
            // Checked FIRST so the import-only branch below can't mistake a
            // no-staling-kind edge for an import coupling.
            if !crate::types::relates_stales_on_code_change(&edge.kinds) {
                return false;
            }
            // A PASSING edge with no behavior-sensitive coupling (only `imports`,
            // or un-kinded) is mechanically re-derivable: a behavior-preserving
            // edit to a grounded hub file (the storage trait, the output printer)
            // does NOT change the import, so re-staling it into a manual
            // re-verification is laundering-prone busywork — the exact tension that
            // re-opened hundreds of edges on a cosmetic change. Re-derive it from
            // the LIVE coupling instead: keep it passing while the pair is still
            // import-coupled; stale it ONLY when the import is now GONE, mirroring
            // how an `independent` edge re-opens only when a coupling appears. This
            // uses the live `coupled` set, NOT the stored kind, so it fires for the
            // un-kinded edges the analyzer ground flow produces (before `loom
            // populate kinds` ever runs). Judgment couplings (calls/inheritance/
            // shares_state) and `shares_file` still stale.
            if edge.inspection_status == "passing"
                && crate::types::relates_is_import_only_coupling(&edge.kinds)
            {
                return !coupled.contains(&super::sorted_pair(&edge.from_id, &edge.to_id));
            }
            // Reaching here means it stales (a non-passing edge, or a judgment
            // coupling on a passing edge).
            true
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
            // Content-addressed: a file whose hash matches the stored one is
            // byte-identical, so re-parsing it reproduces the exact same facts.
            // Skip the tree-sitter parse for it — the #9 win (a no-op sync no
            // longer re-parses every file, ~15s each on large files).
            if !state.content_changed.contains(&cf.path) {
                continue;
            }
            let facts = crate::repo::extract_physical_facts(base, &cf.path, content);
            if facts.imports != cf.imports {
                store.update_codefile_imports(&cf.id, &facts.imports)?;
                state.facts_rewritten += 1;
            }
            if facts.symbols != cf.symbols {
                store.update_codefile_symbols(&cf.id, &facts.symbols)?;
                state.facts_rewritten += 1;
            }
            if facts.symbol_facts != cf.symbol_facts {
                store.update_codefile_symbol_facts(&cf.id, &facts.symbol_facts)?;
                state.facts_rewritten += 1;
            }
            if facts.extractor_grade != cf.extractor_grade {
                store.update_codefile_extractor_grade(&cf.id, &facts.extractor_grade)?;
                state.facts_rewritten += 1;
            }
        }
    }
    // Symbol-aware staleness: a locator is fresh if it names a still-extracted
    // symbol (agreeing with `loom codefile show`), even when the raw/lossy bytes
    // are not a contiguous substring; a bare identifier matching no symbol is
    // stale even if it survives in a comment. Cache the per-file symbol names so
    // multiple groundings on one file re-extract it once.
    let mut symbols_by_path: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for im in ctx.all_implements {
        let Some(content) = state.text_contents.get(&im.codefile_path) else {
            continue;
        };
        let symbol_names = symbols_by_path
            .entry(im.codefile_path.clone())
            .or_insert_with(|| {
                crate::repo::extract_physical_facts(base, &im.codefile_path, content)
                    .symbol_facts
                    .into_iter()
                    .map(|f| f.name)
                    .collect()
            });
        if !crate::repo::locator_fresh(content, &im.locator, symbol_names) {
            state.locators_stale.push(format!(
                "{} @ '{}' (intent '{}')",
                im.codefile_path, im.locator, im.intent_name
            ));
            store.flag_implements_needs_reverification(&im.intent_id, &im.codefile_id)?;
        }
    }
    Ok(())
}

/// Stamp mechanical relationship kinds (imports/shares_file/shares_vocab/
/// same_domain) onto RELATES_TO edges from CURRENT physical facts — the same
/// derivation `loom populate kinds` does, folded into the sync loop so the
/// import-coupling staling exemption is HONEST in `loom explain` (an un-kinded
/// edge between import-coupled files becomes `[imports]`) without a separate
/// manual step. Judgment kinds (analyzer-asserted: calls/inheritance/…) are
/// preserved; only the mechanical tier is (re)derived.
fn backfill_mechanical_kinds(store: &mut SqliteStore, state: &mut SyncState) -> Result<()> {
    let snapshot = store.query_snapshot()?;
    let discovery = crate::db::queries::DiscoverySnapshot::from_query(&snapshot)?;
    let by_id: HashMap<&str, &crate::types::Intent> = snapshot
        .intents
        .iter()
        .map(|i| (i.id.as_str(), i))
        .collect();
    for e in &snapshot.relates {
        let (Some(a), Some(b)) = (by_id.get(e.from_id.as_str()), by_id.get(e.to_id.as_str()))
        else {
            continue;
        };
        let mechanical: Vec<String> =
            crate::db::queries::mechanical_kinds_for_pair(&discovery, a, b)
                .into_iter()
                .map(|k| k.as_str().to_string())
                .collect();
        let mut new_kinds: Vec<String> = e
            .kinds
            .iter()
            .filter(|k| {
                k.parse::<crate::types::RelationKind>()
                    .map(|rk| !rk.is_mechanical())
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        for m in mechanical {
            if !new_kinds.contains(&m) {
                new_kinds.push(m);
            }
        }
        new_kinds.sort();
        let mut current = e.kinds.clone();
        current.sort();
        if new_kinds != current {
            store.update_relates_to_kinds(&e.from_id, &e.to_id, &new_kinds)?;
            state.kinds_backfilled += 1;
        }
    }
    Ok(())
}

fn flag_unverifiable_files(
    store: &mut SqliteStore,
    codefiles: &[CodeFile],
    ctx: &SyncContext<'_>,
    state: &mut SyncState,
    coupled: &HashSet<(String, String)>,
) -> Result<()> {
    // Unverifiable files: a registered file that is gone (missing), outside the
    // graph root (escaped), or unreadable as text (non-UTF8) cannot prove the
    // claims grounded in it, so those claims must not stay green. There is no
    // symbol narrowing possible (the content is unavailable), so EVERY intent
    // grounding such a file is affected: flag its IMPLEMENTS grounding and
    // ripple one hop (relates/governs/targets/serves), and invalidate linked
    // validations — mirroring the changed-file path above. Without this, an
    // intent reads fully realized/proven while its code is missing.
    // Non-UTF-8 files are decoded lossily and extract normally, so they are NOT
    // unverifiable — only genuinely missing/escaped files ripple-invalidate here.
    let unverifiable: HashSet<String> = state
        .missing_files
        .iter()
        .chain(state.escaped_files.iter())
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
            flag_relates(store, ctx, state, iid, &cause, true, coupled)?;
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
    coupled: &HashSet<(String, String)>,
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
                flag_relates(store, ctx, state, iid, &cause, false, coupled)?;
                invalidate_delegation_validations(store, ctx, state, iid)?;
                // Re-open the seam intent's GROUNDINGS too: its binding code
                // claims to use the child contract correctly, and that contract
                // just moved. A seam grounded ONLY via IMPLEMENTS (no RELATES_TO,
                // no delegation validation) would otherwise never re-open — the
                // cross-service ripple silently never firing.
                for im in ctx.all_implements.iter().filter(|im| im.intent_id == **iid) {
                    if store.flag_implements_needs_reverification(&im.intent_id, &im.codefile_id)? {
                        state.seam_groundings_reopened += 1;
                    }
                }
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
        non_utf8_lossy: {
            let mut v: Vec<String> = state.non_utf8_files.into_iter().collect();
            v.sort();
            v
        },
        seam_groundings_reopened: state.seam_groundings_reopened,
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
    let stale_or_seam = !report.locators_stale.is_empty() || report.seam_groundings_reopened > 0;
    let fix_lane = report.relates_to_edges_flagged + report.governs_edges_flagged > 0;
    if report.files_changed == 0
        && report.missing_files.is_empty()
        && report.escaped_files.is_empty()
        && !stale_or_seam
        && !fix_lane
    {
        "`loom status` (or `loom next --all` for closeout)".to_string()
    } else if stale_or_seam {
        // A stale/re-opened IMPLEMENTS grounding is RE-GROUNDED, not re-verified —
        // the fix lane does NOT serve it. This is the PRIMARY directive (and the
        // JSON next_step an orchestrator parses), so it must name the actual
        // recovery REGARDLESS of files_changed — previously a rename (a code change)
        // fell through to the empty `--mode fix` route. Mention the fix lane only as
        // a secondary step when there are also flagged RELATES_TO/GOVERNS edges.
        let mut s = String::from(
            "re-ground each re-opened grounding: `loom edge implement <intent> <file> --locator \"<current symbol>\"` (`loom codefile show <file>` lists current symbols)",
        );
        if fix_lane {
            s.push_str("; then `loom next --mode fix` for the flagged RELATES_TO/GOVERNS edges");
        }
        s.push('.');
        s
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
    if !report.non_utf8_lossy.is_empty() {
        println!();
        println!(
            "  ⓘ {} file(s) not valid UTF-8 — decoded lossily (symbols still extracted; fix the encoding when convenient):",
            report.non_utf8_lossy.len()
        );
        for p in report.non_utf8_lossy.iter().take(REPORT_CAP) {
            println!("    {p}");
        }
    }
    if report.seam_groundings_reopened > 0 {
        println!();
        println!(
            "  ⚠ {} seam grounding(s) re-opened — a delegated child's contract (committed export) changed; re-verify the binding code against it.",
            report.seam_groundings_reopened
        );
    }
    println!();
    if report.files_changed == 0
        && report.missing_files.is_empty()
        && report.escaped_files.is_empty()
        && report.locators_stale.is_empty()
        && report.seam_groundings_reopened == 0
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
    // A stale IMPLEMENTS locator is RE-GROUNDED, not re-verified — the fixer lane
    // (`loom next --mode fix`) does NOT serve it, so don't send the driver to an
    // empty lane. Name the actual recovery.
    println!(
        "  → re-ground each: `loom edge implement <intent> <file> --locator \"<current symbol>\"` \
         (list the file's current symbols with `loom codefile show <file>`; the intent id is on its `loom explain`/`loom intent show`)."
    );
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
    // Every symbol name tracked in THIS file (old ∪ new facts). A locator that
    // matches NONE of them names a symbol tree-sitter does not extract — a method
    // or other NESTED symbol (e.g. `def set_state` inside `class JobStore`, where
    // only `JobStore` is a fact). Such a grounding can't be symbol-attributed, so
    // a precise diff would silently skip it on every change and leave a gutted
    // method body's proof green (stale-green false-pass). Treat it as affected
    // whenever the file content changed — the same file-level attribution
    // `loom impact` already predicts, so the two reads now agree.
    let tracked_names: Vec<&str> = name_of
        .values()
        .copied()
        .filter(|n| !n.is_empty())
        .collect();
    // An intent is affected iff one of its IMPLEMENTS edges on THIS file is
    // file-level (empty locator), names a changed symbol, or names NO tracked
    // symbol (nested). IDENTIFIER-WORD match, not raw substring: a changed `add`
    // must NOT invalidate a grounding on `add_tax` (a bare `contains` re-opened
    // every grounding whose locator merely contained the name as a sub-token —
    // wasted re-verification churn).
    let mut affected = HashSet::new();
    for im in all_implements.iter().filter(|im| im.codefile_id == cf.id) {
        let loc = im.locator.trim();
        let names_changed = changed_names
            .iter()
            .any(|n| crate::db::queries::symbol_match::contains_identifier_word(loc, n));
        let names_tracked = tracked_names
            .iter()
            .any(|n| crate::db::queries::symbol_match::contains_identifier_word(loc, n));
        if loc.is_empty() || names_changed || !names_tracked {
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
            extractor_grade: String::new(),
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

    // The method-level narrowing: two methods share one `impl`, so a body
    // change to one bumps the WHOLE impl's hash — whose changed name is the
    // type, not the method. Without per-method facts the method-level grounding
    // matched neither the type name nor "" and silently stayed green. Per-method
    // facts give `add` its own hash, so its grounding flips and `sub`'s does not.
    #[cfg(feature = "treesitter")]
    #[test]
    fn affected_flips_only_the_changed_impl_method() {
        let base = std::env::temp_dir();
        let old = "struct Calc;\n\
                   impl Calc {\n    fn add(&self) -> i32 {\n        1 + 1\n    }\n\
                   \n    fn sub(&self) -> i32 {\n        2 - 1\n    }\n}\n";
        let new = "struct Calc;\n\
                   impl Calc {\n    fn add(&self) -> i32 {\n        1 + 999\n    }\n\
                   \n    fn sub(&self) -> i32 {\n        2 - 1\n    }\n}\n";
        let old_facts = crate::repo::extract_physical_facts(&base, "src/foo.rs", old).symbol_facts;
        assert!(
            old_facts.iter().any(|f| f.name == "add"),
            "the method is its own fact: {old_facts:?}"
        );
        let codefile = cf(old_facts);
        let impls = vec![imp("iadd", "fn add"), imp("isub", "fn sub")];
        let affected = affected_intents(&base, &codefile, Some(&new.to_string()), &impls)
            .expect("symbol-level diff, not the whole-file fallback");
        assert!(
            affected.contains("iadd"),
            "the grounding on the CHANGED method flips (no longer a silent false-green): {affected:?}"
        );
        assert!(
            !affected.contains("isub"),
            "the grounding on the UNCHANGED sibling method must NOT flip: {affected:?}"
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
