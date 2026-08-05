//! Driver-loop command family — status, next, queue, mode, sync, export/import.
//!
//! Plane: CLI surface over the engine's orchestration: recompute (`sync`),
//! portability (`travel`), and routing (`workitem`/`maturity`). This module
//! resolves the target graph, dispatches, and renders — the compass and queue
//! decisions it prints are computed elsewhere and must not be second-guessed
//! or reordered here.

use super::*;

pub(crate) fn export(graph: Option<&Path>, check: bool, json: bool) -> Result<()> {
    let store = open(graph)?;
    if check {
        if travel::export_is_fresh(&store)? {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "fresh": true }))?
                );
            } else {
                println!("export is fresh");
            }
            Ok(())
        } else {
            bail!(
                "committed {} is stale — run `loom export`",
                crate::GRAPH_EXPORT
            );
        }
    } else {
        let path = travel::export_to_file(&store)?;
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "written": true,
                    "path": path,
                }))?
            );
        } else {
            println!("wrote {}", path.display());
        }
        Ok(())
    }
}
pub(crate) fn import(
    graph: Option<&Path>,
    file: &Path,
    repair_orphans: bool,
    json: bool,
) -> Result<()> {
    let root = if let Some(g) = graph {
        g.to_path_buf()
    } else {
        // Resolve to an existing graph root if the CWD is inside one, so
        // `loom import` from a subdirectory targets that graph instead of
        // silently initializing a second graph in the CWD. Fall back to the
        // CWD only when no graph exists (the create-fresh case).
        let cwd = std::env::current_dir()?;
        Store::find_root(&cwd).unwrap_or(cwd)
    };
    let export = travel::read_export(file)?;
    let imported_journal = export.journal.clone();
    let imported_baselines = export.baselines.clone();
    let mut snapshot = export.into_snapshot();
    let quarantined_commands = travel::quarantine_imported_execution(&mut snapshot)?;
    // Journal collisions are part of import validation, not a post-restore
    // sidecar failure. This pass is side-effect-free; restore_entries repeats
    // the same plan under lock after graph restore to close the race window.
    crate::journal::preflight_restore_entries(&root, &imported_journal)?;
    let mut store = Store::init(&root, None, false)?;
    let report = if repair_orphans {
        store.restore_repairing(&snapshot)?
    } else {
        store.restore(&snapshot)?;
        crate::store::RestoreReport::default()
    };
    // Sidecars are installed only after structural validation and transactional
    // graph restore succeed, so rejected/nonempty imports cannot contaminate
    // the existing journal or journey baselines.
    let journal_restored = crate::journal::restore_entries(&root, &imported_journal)?;
    let baselines_restored = crate::journey::restore_baselines(&root, &imported_baselines)?;
    // Restore's first verification pass ran before imported journal refs were
    // installed. Recheck authority and every other anchor against final state.
    store.reverify_all(&std::collections::BTreeSet::new())?;
    let id = store.identity()?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "imported": true,
                "name": id.name,
                "graph_id": id.graph_id,
                "file": file,
                "quarantined_commands": quarantined_commands,
                "journal_restored": journal_restored,
                "baselines_restored": baselines_restored,
                "repaired": repair_orphans,
                "preserved_soft_refs": report.preserved_soft_refs,
                "dropped_facts": report.dropped_facts
                    .iter()
                    .map(|(kind, id, claim)| serde_json::json!({
                        "subject_kind": kind, "subject_id": id, "claim": claim,
                    }))
                    .collect::<Vec<_>>(),
                "dropped_facets": report.dropped_facets
                    .iter()
                    .map(|(kind, id, key)| serde_json::json!({
                        "target_kind": kind, "target_id": id, "key": key,
                    }))
                    .collect::<Vec<_>>(),
                "dropped_tags": report.dropped_tags
                    .iter()
                    .map(|(kind, id, term)| serde_json::json!({
                        "target_kind": kind, "target_id": id, "term": term,
                    }))
                    .collect::<Vec<_>>(),
            }))?
        );
    } else {
        println!("imported graph '{}' from {}", id.name, file.display());
        if quarantined_commands > 0 {
            println!(
                "  quarantined {quarantined_commands} imported command(s) — review and re-enter each through validation/scan update before execution"
            );
        }
        if report.preserved_soft_refs > 0 {
            println!(
                "  preserved {} adjudication verdict(s) on not-yet-materialized findings (re-attach on next sync)",
                report.preserved_soft_refs
            );
        }
        for (kind, id, claim) in &report.dropped_facts {
            println!("  dropped orphan fact '{claim}' on {kind} {id}");
        }
        for (kind, id, key) in &report.dropped_facets {
            println!("  dropped orphan facet '{key}' on {kind} {id}");
        }
        for (kind, id, term) in &report.dropped_tags {
            println!("  dropped orphan tag '{term}' on {kind} {id}");
        }
        let dropped =
            report.dropped_facts.len() + report.dropped_facets.len() + report.dropped_tags.len();
        if dropped > 0 {
            println!("  ({dropped} dangling reference(s) dropped by --repair-orphans)");
        }
    }
    Ok(())
}
pub(crate) fn sync_cmd(graph: Option<&Path>, json: bool, quiet: bool, rebuild: bool) -> Result<()> {
    let root = resolve_root(graph)?;
    let store = Store::open(&root)?;
    if rebuild {
        // The INV-2 operation, which until now had no way to invoke it: sync
        // re-derives only files whose CONTENT changed, so an upgraded loom
        // leaves the old binary's derived facts in place. Asserted truth is
        // untouched — this discards only what loom computes for itself, and
        // the pass below recomputes it.
        store.wipe_derived()?;
    }
    // Discovery pass: expand remembered globs and register new files before
    // the deriver loop runs, so newly-appeared files are included in this sync.
    let rescan = super::codefile_cmd::rescan_globs(&store, &root)?;
    // Federation pass: reconcile linked upstream graphs (shadow nodes + staleness).
    let federation = crate::federation::run(&store, &root)?;
    let report = crate::sync::run(&store, &root)?;
    // Keep the committed portable artifact fresh as a byproduct of sync, so a
    // separate `loom export` is not a required step in the loop. Only an export
    // that already exists (the repo tracks it) and has drifted is rewritten:
    // never creates an untracked file, and preserves byte-determinism.
    let reexported = crate::travel::refresh_export_if_tracked(&store)?;
    crate::journal::append(
        store.root(),
        "sync",
        "graph",
        serde_json::json!({ "quiet": quiet, "files_changed": report.files_changed }),
    )?;
    if quiet {
        return Ok(());
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "new_files": rescan.new_files,
                "new_observed": rescan.new_observed,
                "federation": {
                    "upstreams_checked": federation.upstreams_checked,
                    "shadows_created": federation.shadows_created,
                    "shadows_updated": federation.shadows_updated,
                    "edges_staled": federation.edges_staled,
                },
                "files_scanned": report.files_scanned,
                "files_changed": report.files_changed,
                "edges_staled": report.edges_staled,
                "edges_spared": report.edges_spared,
                "evidence_reanchored": report.evidence_reanchored,
                "validations_reset": report.validations_reset,
                "findings": report.findings,
                "contracts_reset": report.contracts_reset,
                "surfaces_affected": report.surfaces_affected,
                "files_deleted": report.files_deleted,
                "missing": report.missing,
                "wiki_staled": report.wiki_staled,
                "reexported": reexported,
            }))?
        );
        return Ok(());
    }
    if !rescan.new_files.is_empty() {
        println!(
            "discovery: {} new file(s) registered under remembered globs{}",
            rescan.new_files.len(),
            if rescan.new_observed > 0 {
                format!(" ({} observed)", rescan.new_observed)
            } else {
                String::new()
            }
        );
        for f in rescan.new_files.iter().take(10) {
            println!("    + {f}");
        }
        if rescan.new_files.len() > 10 {
            println!("    … +{} more", rescan.new_files.len() - 10);
        }
    }
    println!(
        "sync: {} scanned, {} changed, {} edges staled, {} validations reset, {} findings",
        report.files_scanned,
        report.files_changed,
        report.edges_staled,
        report.validations_reset,
        report.findings
    );
    if report.edges_spared > 0 {
        println!(
            "  precision: {} grounding(s) kept fresh — the change did not touch their locator symbol",
            report.edges_spared
        );
    }
    if report.evidence_reanchored > 0 {
        println!(
            "  re-anchored: {} evidence span(s) moved with content intact — verdicts kept, moves journaled",
            report.evidence_reanchored
        );
    }
    if reexported {
        println!(
            "  refreshed {} (portable export kept fresh)",
            crate::GRAPH_EXPORT
        );
    }
    if report.contracts_reset > 0 {
        println!(
            "  integration: {} upstream surface(s) changed → {} of the reset contract(s) need re-verification   [loom next --mode validate]",
            report.surfaces_affected, report.contracts_reset
        );
    }
    if report.files_deleted > 0 {
        println!(
            "  {} registered file(s) gone since last sync — dependents re-flagged",
            report.files_deleted
        );
    }
    if !report.missing.is_empty() {
        println!(
            "  {} registered file(s) missing on disk:",
            report.missing.len()
        );
        for m in report.missing.iter().take(10) {
            println!("    {m}");
        }
    }
    if report.wiki_staled > 0 {
        println!(
            "  {} wiki page(s) went stale — a documented intent, its code, or its proof changed   [loom wiki next]",
            report.wiki_staled
        );
    }
    if federation.shadows_updated > 0 || federation.shadows_created > 0 {
        println!(
            "  federation: {} upstream(s) checked, {} shadow(s) created, {} updated, {} edge(s) staled",
            federation.upstreams_checked,
            federation.shadows_created,
            federation.shadows_updated,
            federation.edges_staled
        );
    }
    Ok(())
}
/// The machine-readable graph pulse. One implementation behind `loom status
/// --json` and the `loom_status` MCP tool — the text renderer below reads the
/// same values, so no surface can report a number another surface does not.
pub(crate) fn status_value(store: &Store) -> Result<serde_json::Value> {
    let id = store.identity()?;
    let (ladder, queues) = crate::maturity::ladder_and_depths(store)?;
    let (registered_codefiles, owned_codefiles, unowned_codefiles, observed_codefiles) =
        code_ownership_summary(store)?;
    Ok(serde_json::json!({
        "graph": {
            "name": id.name,
            "graph_id": id.graph_id,
            "schema_version": id.schema_version,
            "observed": id.observed,
        },
        "counts": {
            "intents": store.list_nodes(Some(NodeType::Intent), usize::MAX)?.len(),
            "codefiles": store.list_nodes(Some(NodeType::CodeFile), usize::MAX)?.len(),
            "edges": store.list_edges(None, usize::MAX)?.len(),
        },
        "compass": { "phase": ladder.phase, "rung": ladder.rung, "next_command": ladder.next_command },
        "maturity": ladder,
        "graph_state": workitem::graph_state(store)?,
        "queues": queues,
        "validation_summary": crate::maturity::validation_summary(store)?,
        "code_ownership": {
            "registered": registered_codefiles,
            "owned": owned_codefiles,
            "unowned": unowned_codefiles.len(),
            "unowned_files": unowned_codefiles,
            "observed": observed_codefiles,
            "blocking": !unowned_codefiles.is_empty(),
        },
        "detectors": {
            "layering": super::domain_cmd::layer_detector_state(store)?,
        },
    }))
}

pub(crate) fn status(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open_read(graph)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status_value(&store)?)?);
        return Ok(());
    }
    let id = store.identity()?;
    let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?.len();
    let files = store
        .list_nodes(Some(NodeType::CodeFile), usize::MAX)?
        .len();
    let edges = store.list_edges(None, usize::MAX)?.len();
    let (ladder, queues) = crate::maturity::ladder_and_depths(&store)?;
    let pulse = workitem::graph_state(&store)?;
    let (registered_codefiles, owned_codefiles, unowned_codefiles, observed_codefiles) =
        code_ownership_summary(&store)?;
    let layering = super::domain_cmd::layer_detector_state(&store)?;
    println!(
        "graph: {} ({}){}",
        id.name,
        id.graph_id,
        if id.observed {
            "  [observed — monitoring; build/fix lanes off]"
        } else {
            ""
        }
    );
    println!("  intents: {intents}  codefiles: {files}  edges: {edges}");
    let ownership_gate = if unowned_codefiles.is_empty() {
        "coverage gate clear"
    } else {
        "blocks covered rung"
    };
    println!(
        "  code ownership: {owned_codefiles}/{registered_codefiles} owned, {} unowned ({ownership_gate}){}",
        unowned_codefiles.len(),
        if observed_codefiles > 0 {
            format!(", {observed_codefiles} observed")
        } else {
            String::new()
        }
    );
    if layering.get("armed").and_then(|v| v.as_bool()) == Some(false)
        && layering
            .get("layer_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            >= 2
    {
        println!(
            "  layering: unarmed — {}",
            layering
                .get("warning")
                .and_then(|v| v.as_str())
                .unwrap_or("no layer order declared")
        );
    }
    println!("  maturity:");
    for r in &ladder.rungs {
        if r.blocked {
            let by = r.blocked_by.as_deref().unwrap_or("");
            println!("    ⊘ {:<12} (blocked by {by})", r.name);
            continue;
        }
        let mark = match r.state {
            crate::maturity::RungState::Met => "✓",
            crate::maturity::RungState::Unmet => "·",
            crate::maturity::RungState::NotApplicable => "—",
            crate::maturity::RungState::Open => "∞",
        };
        println!("    {mark} {:<13} {}", r.name, r.detail);
    }
    let df = &ladder.derived_floor;
    println!(
        "  derived floor: {:.0}% ({} derived / {} asserted facts)",
        df.ratio * 100.0,
        df.derived,
        df.asserted
    );
    println!(
        "  compass: phase={} → {}",
        ladder.phase, ladder.next_command
    );
    // One line per lane that has work, in ladder order — the same order the
    // rungs above are climbed, so the queue line reads as the work behind them.
    let backlog: Vec<String> = crate::lane::Lane::LADDER
        .iter()
        .filter(|l| l.serves_items() && !l.requires_human_decision())
        .map(|l| format!("{}={}", l.as_str(), queues.get(*l)))
        .collect();
    println!(
        "  queues: {}{}{}{}",
        backlog.join(" "),
        if queues.get(crate::lane::Lane::Divergence) > 0 {
            format!(
                "  ({} awaiting the human)",
                queues.get(crate::lane::Lane::Divergence)
            )
        } else {
            String::new()
        },
        if pulse.open_questions > 0 {
            format!("  ({} question(s) for the human)", pulse.open_questions)
        } else {
            String::new()
        },
        {
            let staged = store.count_judgments("staged".parse()?).unwrap_or(0);
            if staged > 0 {
                format!("  ({staged} staged judgment proposal(s) — loom judgment digest)")
            } else {
                String::new()
            }
        }
    );
    Ok(())
}
pub(crate) fn next_all(graph: Option<&Path>, json: bool, full: bool) -> Result<()> {
    let store = open_read(graph)?;
    let (ladder, counts) = crate::maturity::ladder_and_depths(&store)?;
    let pulse = workitem::graph_state(&store)?;
    // Queue depths: `--all` serves the TOP item of each queue, not every item.
    // Surface the depth alongside so "one line per queue" never reads as "this
    // queue holds one item" (the counts also live in `loom status`).
    //
    // The human-decision lane is excluded from the served set: minting a packet
    // for it here would route an LLM driver into a write it is denied (INV-8).
    // This is the same filter the `loom status` queues line uses.
    let modes: Vec<(&'static str, crate::lane::Lane, usize)> = crate::lane::Lane::LADDER
        .iter()
        .filter(|l| l.serves_items() && !l.requires_human_decision())
        .map(|&l| (l.as_str(), l, counts.get(l)))
        .collect();
    // Compute each queue's top item once, then mint one packet per served item
    // and journal the batch as a single act of serving — a closeout is one act,
    // not one per lane. The ids are stamped back onto the items so JSON and the
    // journal agree on what left the process.
    let mut rows: Vec<(&'static str, usize, Option<workitem::WorkItem>)> = Vec::new();
    for &(name, m, depth) in &modes {
        rows.push((name, depth, workitem::next(&store, Some(m))?));
    }
    // Compact closeouts are a roster, not packet delivery. Minting here
    // inflates efficacy's denominator: every lane gets a "served" id, then
    // the driver picks one lane and `next --mode` mints a different packet.
    // Only `--full` (and singular `next`) actually hands out work packets.
    if full {
        let pairs: Vec<(&str, &str)> = rows
            .iter()
            .filter_map(|(_, _, item)| {
                item.as_ref()
                    .map(|w| (w.mode.as_str(), w.target.id.as_str()))
            })
            .collect();
        let served = crate::packet::mint_batch(&pairs);
        crate::packet::serve(store.root(), &served)?;
        let mut ids = served.into_iter().map(|s| s.id);
        for (_, _, item) in rows.iter_mut() {
            if let Some(w) = item.as_mut() {
                w.packet_id = ids.next();
            }
        }
    }
    if json {
        let mut queues = serde_json::Map::new();
        for (name, depth, item) in &rows {
            // Default closeout JSON is compact: prompt contracts + context on
            // every lane blew past 20k tokens on a dogfood graph. Opt into the
            // full packet with `--full` when a driver will actually work a lane.
            let value = match item {
                None => serde_json::json!({ "depth": depth }),
                Some(w) if full => {
                    let mut v = serde_json::to_value(w)?;
                    if let Some(obj) = v.as_object_mut() {
                        obj.insert("depth".into(), serde_json::json!(depth));
                    }
                    v
                }
                Some(w) => serde_json::json!({
                    "depth": depth,
                    "reason": w.reason,
                    "target": w.target,
                }),
            };
            queues.insert(name.to_string(), value);
        }
        let out = serde_json::json!({
            "compass": { "phase": ladder.phase, "next_command": ladder.next_command },
            "graph_state": pulse,
            // Top item per queue (the closeout view). Depths are also in `queue_depths`.
            "queues": queues,
            "queue_depths": counts,
            "packets": if full { "full" } else { "compact" },
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "closeout — compass phase={} → {} (top of each queue; depths in [n])",
            ladder.phase, ladder.next_command
        );
        for (name, depth, item) in &rows {
            match item {
                Some(w) => println!("  {name:<8} [{depth}] → {}", w.target.name),
                None => println!("  {name:<8} [{depth}] → (empty)"),
            }
        }
        println!(
            "  graph_state: planned={} stale={} uninspected={} findings={} open={} resolved={} untriaged={} stale_findings={} needed={} inbox={} low_confidence={} open_questions={}",
            pulse.planned,
            pulse.stale,
            pulse.uninspected,
            pulse.findings,
            pulse.open_findings,
            pulse.resolved_findings,
            pulse.untriaged,
            pulse.stale_findings,
            pulse.needed,
            pulse.inbox,
            pulse.low_confidence,
            pulse.open_questions
        );
    }
    Ok(())
}
/// `loom mode [owned|observed]`: show the graph mode, or set it. `observed`
/// maps code the driver does not own (build/fix/coverage/elaborate lanes off);
/// `owned` is the normal build-and-prove mode. Setting it is the post-init
/// counterpart to `init --observed`; `sync` never touches it. `set` is `None`
/// to just show, `Some(observed)` to set.
pub(crate) fn mode_cmd(graph: Option<&Path>, set: Option<bool>, json: bool) -> Result<()> {
    let observed = match set {
        Some(want) => open(graph)?.set_observed(want)?,
        None => open_read(graph)?.identity()?.observed,
    };
    let mode = if observed { "observed" } else { "owned" };
    let changed = set.is_some();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "mode": mode,
                "observed": observed,
                "changed": changed,
            }))?
        );
        return Ok(());
    }
    let lanes = if observed {
        "discovery/quality/validation only — build/fix/coverage/elaborate lanes off"
    } else {
        "all lanes active (build + prove)"
    };
    if changed {
        println!("mode set to '{mode}' — {lanes}");
    } else {
        println!("mode: {mode} — {lanes}");
    }
    Ok(())
}

/// `loom next --mode <m> --all`: the full roster of one queue — every item it
/// would serve, in priority order (entry 1 is what `loom next --mode <m>`
/// serves), as lightweight rows. The depth view behind a queue `status` reports
/// as hundreds deep.
pub(crate) fn queue_list(graph: Option<&Path>, mode: &str, json: bool) -> Result<()> {
    let store = open_read(graph)?;
    let parsed = crate::lane::Lane::parse(mode).ok_or_else(|| anyhow!("unknown mode '{mode}'"))?;
    let items = workitem::queue_items(&store, parsed)?;
    if json {
        let out = serde_json::json!({
            "mode": mode,
            "count": items.len(),
            "items": items,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if items.is_empty() {
        println!("{mode}: (empty queue)");
        return Ok(());
    }
    println!(
        "{mode}: {} item(s) — full queue depth (work the top with `loom next --mode {mode}`)",
        items.len()
    );
    let width = items.len().to_string().len();
    for (i, it) in items.iter().enumerate() {
        let hint = it
            .routing_hint
            .as_deref()
            .map(|h| format!("/{h}"))
            .unwrap_or_default();
        let class = it
            .cause_class
            .as_deref()
            .map(|c| format!(" {c}"))
            .unwrap_or_default();
        println!(
            "  {:>width$}. [{}{}{}] {} — {}",
            i + 1,
            it.effort,
            hint,
            class,
            it.target.name,
            it.reason,
            width = width
        );
    }
    Ok(())
}

pub(crate) fn require_lane(store: &Store, owner: crate::registry::OwnerRole) -> Result<()> {
    match store.agent() {
        crate::store::Agent::Solo => Ok(()),
        crate::store::Agent::Lane(r) if r.satisfies(owner) => Ok(()),
        crate::store::Agent::Lane(r) => bail!(
            "lane gate: agent '{}' may not write '{}'-owned facts",
            r.as_str(),
            owner.as_str()
        ),
    }
}
/// Serve one work packet, stamping and journaling its id. The one
/// implementation behind `loom next` and the `loom_next` MCP tool.
pub(crate) fn next_output(store: &Store, mode: Option<&str>) -> Result<workitem::NextOutput> {
    let parsed = match mode {
        Some(m) => Some(crate::lane::Lane::parse(m).ok_or_else(|| anyhow!("unknown mode '{m}'"))?),
        None => None,
    };
    let mut item = workitem::next(store, parsed)?;
    if let Some(w) = item.as_mut() {
        w.packet_id = Some(crate::packet::serve_one(
            store.root(),
            &w.mode,
            &w.target.id,
        )?);
    }
    Ok(workitem::NextOutput {
        work_item: item,
        graph_state: workitem::graph_state(store)?,
    })
}

pub(crate) fn next_cmd(graph: Option<&Path>, mode: Option<&str>, json: bool) -> Result<()> {
    let store = open_read(graph)?;
    let out = next_output(&store, mode)?;
    let item = out.work_item;
    let pulse = out.graph_state;
    if json {
        let out = workitem::NextOutput {
            work_item: item,
            graph_state: pulse,
        };
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        match &item {
            Some(w) => print_work_item(w),
            None => println!(
                "no work in this queue — see `loom status` for the next move, or `loom guide` to pick a lane"
            ),
        }
        println!(
            "  graph_state: planned={} stale={} uninspected={} findings={} open={} resolved={} untriaged={} stale_findings={} needed={} inbox={} low_confidence={} open_questions={}",
            pulse.planned,
            pulse.stale,
            pulse.uninspected,
            pulse.findings,
            pulse.open_findings,
            pulse.resolved_findings,
            pulse.untriaged,
            pulse.stale_findings,
            pulse.needed,
            pulse.inbox,
            pulse.low_confidence,
            pulse.open_questions
        );
    }
    Ok(())
}
/// Flatten a contract's `examples` into readable lines, whatever its shape.
fn render_examples(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Object(map) => map
            .iter()
            .filter(|(_, v)| !v.is_null())
            .map(|(k, v)| match v.as_str() {
                Some(text) => format!("{k}: {text}"),
                None => format!("{k}: {v}"),
            })
            .collect(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                let situation = item.get("situation")?.as_str()?;
                let action = item.get("do")?.as_str()?;
                Some(format!("{situation} → {action}"))
            })
            .collect(),
        serde_json::Value::Null => Vec::new(),
        other => vec![other.to_string()],
    }
}

fn print_work_item(item: &workitem::WorkItem) {
    let c = &item.prompt_contract;
    let short = crate::model::short(&item.target.id);
    let hint = item
        .routing_hint
        .as_deref()
        .map(|h| format!(", {h}"))
        .unwrap_or_default();
    println!(
        "[{}] {} (effort {}{})",
        item.mode, item.target.name, item.effort, hint
    );
    println!("  id: {short}");
    println!("  why: {}", item.reason);
    if !item.stale_causes.is_empty() {
        println!("  stale cause: {}", item.stale_causes.join("; "));
    }
    if let Some(card) = &item.scorecard {
        println!("  completeness:");
        for a in card["axes"].as_array().into_iter().flatten() {
            let state = a["state"].as_str().unwrap_or("");
            let mark = match state {
                "met" => "✓",
                "open" => "·",
                "waived" => "~",
                _ => "-",
            };
            println!(
                "    {mark} {:<14} {}",
                a["axis"].as_str().unwrap_or(""),
                a["detail"].as_str().unwrap_or("")
            );
        }
    }
    if !item.context.linked_entities.is_empty() {
        println!("  linked:");
        for entity in &item.context.linked_entities {
            let short = crate::model::short(&entity.id);
            let status = entity
                .status
                .as_ref()
                .map(|s| format!(" [{s}]"))
                .unwrap_or_default();
            let edge = match (&entity.edge_kind, &entity.edge_status) {
                (Some(kind), Some(edge_status)) => format!(" {kind}/{edge_status}"),
                _ => String::new(),
            };
            let locator = entity
                .locator
                .as_ref()
                .map(|l| format!(" @ {l}"))
                .unwrap_or_default();
            println!(
                "    - {}: {} '{}' [{}]{}{}{}",
                entity.role, entity.kind, entity.name, short, status, edge, locator
            );
        }
    }
    if !item.context.read_set.is_empty() {
        println!("  read these files:");
        for r in &item.context.read_set {
            let locator = r
                .locator
                .as_ref()
                .map(|l| format!(" @ {l}"))
                .unwrap_or_default();
            println!("    - {}{} — {}", r.path, locator, r.why);
        }
    }
    if !item.context.suggested_reads.is_empty() {
        println!("  inspect first:");
        for read in &item.context.suggested_reads {
            println!("    - {} — {}", read.reason, read.command);
        }
    }
    if let Some(patterns) = &item.pattern_guidance {
        println!(
            "  patterns: matched={} included={} omitted={}",
            patterns.matched, patterns.included, patterns.omitted
        );
        for pattern in &patterns.items {
            println!(
                "    - {} — {} @ {}",
                pattern.name, pattern.path, pattern.locator
            );
            println!("      rationale: {}", pattern.rationale);
            println!("      use: {}", pattern.when_to_use);
            println!("      avoid: {}", pattern.when_not_to_use);
            println!("      source:\n{}", pattern.source_excerpt);
        }
        println!("    lookup: {}", patterns.lookup_command);
    }
    let g = &item.truth_gap;
    println!("  truth axis: {} — {}", g.axis.as_str(), g.missing_form);
    println!("    correct when: {}", g.correct_when);
    println!("    make true:  {}", g.authoritative_write);
    println!("    never here: {}", g.forbidden_write);
    println!("    then:       {}", g.after_write);
    println!("  role: {}", c.role);
    println!("  mindset: {}", c.mindset);
    println!("  allowed:");
    for a in &c.allowed_actions {
        println!("    - {a}");
    }
    println!("  forbidden:");
    for f in &c.forbidden_actions {
        println!("    - {f}");
    }
    println!("  evidence: {}", c.required_evidence);
    // The checkable form of the same requirement. Printed under the prose
    // because a worker reads the sentence first and is REFUSED by the clause.
    for clause in &c.evidence_clauses {
        println!("    ▸ {}", clause.describe());
    }
    if let Some(t) = &c.evidence_template {
        println!("    template: {t}");
    }
    // These three were JSON-only, which meant a policy-configured human gate
    // was invisible to every text-mode worker — the packet said "go ahead" to
    // exactly the readers who most needed to be told to stop.
    // Render whatever shape the lane put here: quality rules carry
    // passing/failing, the coverage contract carries a list of situations.
    // Looking for fixed "good"/"bad" keys meant quality examples never printed
    // at all — the header appeared and the content did not.
    if let Some(examples) = &c.examples {
        let lines = render_examples(examples);
        if !lines.is_empty() {
            println!("  examples:");
            for line in lines {
                println!("    {line}");
            }
        }
    }
    if let Some(note) = &c.pre_screen {
        println!("  {note}");
    }
    if !c.pre_screened_hits.is_empty() {
        println!("  candidates:");
        for hit in &c.pre_screened_hits {
            println!(
                "    - {}:{} [{}] {}",
                hit.path, hit.line, hit.pattern, hit.excerpt
            );
        }
    }
    println!("  write-back: {}", c.write_back);
    println!("  stop: {}", c.stop_condition);
    if let Some(gate) = &c.human_gate {
        println!("  ⚠ HUMAN GATE: {}", gate.question);
        for option in &gate.options {
            println!(
                "    - {}: {} — {}",
                option.id, option.label, option.description
            );
        }
        println!("    recommendation: {}", gate.recommendation);
        println!("    after answer: {}", gate.after_answer);
    }
    println!("  next_step: {}", item.next_step);
}

#[cfg(test)]
mod render_tests {
    use super::render_examples;
    use serde_json::json;

    /// Examples render whatever shape a lane provides.
    ///
    /// The quality contract stores `passing`/`failing`; the coverage contract
    /// stores a list of situations. Looking for fixed "good"/"bad" keys printed
    /// a section header with nothing under it — worse than hiding the examples,
    /// because it looked like the rule had none to give.
    #[test]
    fn every_example_shape_a_lane_uses_renders() {
        let quality = json!({
            "passing": "src/a.rs:1 — secrets read from env",
            "failing": "src/a.rs:9 — literal key",
        });
        let lines = render_examples(&quality);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines.iter().any(|l| l.starts_with("passing: ")),
            "{lines:?}"
        );

        let coverage = json!([{ "situation": "it lives here", "do": "ground it" }]);
        let lines = render_examples(&coverage);
        assert_eq!(lines, vec!["it lives here → ground it"]);

        // Nothing to say produces no section at all.
        assert!(render_examples(&json!(null)).is_empty());
        assert!(render_examples(&json!({ "passing": null })).is_empty());
    }
}
