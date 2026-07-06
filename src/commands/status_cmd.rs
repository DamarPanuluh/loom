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
pub(crate) fn import(graph: Option<&Path>, file: &Path, json: bool) -> Result<()> {
    let root = if let Some(g) = graph {
        g.to_path_buf()
    } else {
        std::env::current_dir()?
    };
    let export = travel::read_export(file)?;
    let mut store = Store::init(&root, None, false)?;
    store.restore(&export.into_snapshot())?;
    let id = store.identity()?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "imported": true,
                "name": id.name,
                "graph_id": id.graph_id,
                "file": file,
            }))?
        );
    } else {
        println!("imported graph '{}' from {}", id.name, file.display());
    }
    Ok(())
}
pub(crate) fn sync_cmd(graph: Option<&Path>, json: bool) -> Result<()> {
    let root = resolve_root(graph)?;
    let store = Store::open(&root)?;
    let report = crate::sync::run(&store, &root)?;
    // Keep the committed portable artifact fresh as a byproduct of sync, so a
    // separate `loom export` is not a required step in the loop. Only an export
    // that already exists (the repo tracks it) and has drifted is rewritten:
    // never creates an untracked file, and preserves byte-determinism.
    let reexported = crate::travel::refresh_export_if_tracked(&store)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "files_scanned": report.files_scanned,
                "files_changed": report.files_changed,
                "edges_staled": report.edges_staled,
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
    println!(
        "sync: {} scanned, {} changed, {} edges staled, {} validations reset, {} findings",
        report.files_scanned,
        report.files_changed,
        report.edges_staled,
        report.validations_reset,
        report.findings
    );
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
    Ok(())
}
pub(crate) fn status(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open_read(graph)?;
    let id = store.identity()?;
    let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?.len();
    let files = store
        .list_nodes(Some(NodeType::CodeFile), usize::MAX)?
        .len();
    let edges = store.list_edges(None, usize::MAX)?.len();
    let ladder = crate::maturity::ladder(&store)?;
    let pulse = workitem::graph_state(&store)?;
    let queues = workitem::queue_counts(&store)?;
    let validation_summary = crate::maturity::validation_summary(&store)?;
    let (registered_codefiles, owned_codefiles, unowned_codefiles) =
        code_ownership_summary(&store)?;
    let layering = super::domain_cmd::layer_detector_state(&store)?;
    if json {
        let out = serde_json::json!({
            "graph": {
                "name": id.name,
                "graph_id": id.graph_id,
                "schema_version": id.schema_version,
                "observed": id.observed,
            },
            "counts": {
                "intents": intents,
                "codefiles": files,
                "edges": edges,
            },
            "compass": { "phase": ladder.phase, "next_command": ladder.next_command },
            "maturity": ladder,
            "graph_state": pulse,
            "queues": queues,
            "validation_summary": validation_summary,
            "code_ownership": {
                "registered": registered_codefiles,
                "owned": owned_codefiles,
                "unowned": unowned_codefiles.len(),
                "unowned_files": unowned_codefiles,
                "blocking": !unowned_codefiles.is_empty(),
            },
            "detectors": {
                "layering": layering,
            },
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
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
        "blocks realized rung"
    };
    println!(
        "  code ownership: {owned_codefiles}/{registered_codefiles} owned, {} unowned ({ownership_gate})",
        unowned_codefiles.len()
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
        };
        println!("    {mark} {:<12} {}", r.name, r.detail);
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
    println!(
        "  queues: fix={} validate={} build={} coverage={} quality={} analyze={} prove={} triage={} review={} elaborate={}{}",
        queues.fix,
        queues.validate,
        queues.build,
        queues.coverage,
        queues.quality,
        queues.analyze,
        queues.prove,
        queues.triage,
        queues.review,
        queues.elaborate,
        if pulse.open_questions > 0 {
            format!("  ({} question(s) for the human)", pulse.open_questions)
        } else {
            String::new()
        }
    );
    Ok(())
}
pub(crate) fn next_all(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open_read(graph)?;
    let ladder = crate::maturity::ladder(&store)?;
    let pulse = workitem::graph_state(&store)?;
    // Queue depths: `--all` serves the TOP item of each queue, not every item.
    // Surface the depth alongside so "one line per queue" never reads as "this
    // queue holds one item" (the counts also live in `loom status`).
    let counts = workitem::queue_counts(&store)?;
    let modes = [
        ("fix", workitem::Mode::Fix, counts.fix),
        ("validate", workitem::Mode::Validate, counts.validate),
        ("build", workitem::Mode::Build, counts.build),
        ("coverage", workitem::Mode::Coverage, counts.coverage),
        ("quality", workitem::Mode::Quality, counts.quality),
        ("prove", workitem::Mode::Prove, counts.prove),
        ("analyze", workitem::Mode::Analyze, counts.analyze),
        ("triage", workitem::Mode::Triage, counts.triage),
        ("review", workitem::Mode::Review, counts.review),
        ("elaborate", workitem::Mode::Elaborate, counts.elaborate),
    ];
    if json {
        let mut queues = serde_json::Map::new();
        for (name, m, _) in modes {
            let item = workitem::next(&store, Some(m))?;
            queues.insert(name.to_string(), serde_json::to_value(item)?);
        }
        let out = serde_json::json!({
            "compass": { "phase": ladder.phase, "next_command": ladder.next_command },
            "graph_state": pulse,
            // Top item per queue (the closeout view). Depths are in `queue_counts`.
            "queues": queues,
            "queue_counts": counts,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "closeout — compass phase={} → {} (top of each queue; depths in [n])",
            ladder.phase, ladder.next_command
        );
        for (name, m, depth) in modes {
            match workitem::next(&store, Some(m))? {
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
    let parsed = workitem::Mode::parse(mode).ok_or_else(|| anyhow!("unknown mode '{mode}'"))?;
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
        println!(
            "  {:>width$}. [{}] {} — {}",
            i + 1,
            it.effort,
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
        crate::store::Agent::Lane(r) if r == owner => Ok(()),
        crate::store::Agent::Lane(r) => bail!(
            "lane gate: agent '{}' may not write '{}'-owned facts",
            r.as_str(),
            owner.as_str()
        ),
    }
}
pub(crate) fn next_cmd(graph: Option<&Path>, mode: Option<&str>, json: bool) -> Result<()> {
    let store = open_read(graph)?;
    let parsed = match mode {
        Some(m) => Some(workitem::Mode::parse(m).ok_or_else(|| anyhow!("unknown mode '{m}'"))?),
        None => None,
    };
    let item = workitem::next(&store, parsed)?;
    let pulse = workitem::graph_state(&store)?;
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
fn print_work_item(item: &workitem::WorkItem) {
    let c = &item.prompt_contract;
    let short = &item.target.id[..8.min(item.target.id.len())];
    println!(
        "[{}] {} (effort {})",
        item.mode, item.target.name, item.effort
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
            let short = &entity.id[..8.min(entity.id.len())];
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
    println!("  write-back: {}", c.write_back);
    println!("  stop: {}", c.stop_condition);
    println!("  next_step: {}", item.next_step);
}
