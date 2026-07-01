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
    Ok(())
}
pub(crate) fn status(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let id = store.identity()?;
    let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?.len();
    let files = store
        .list_nodes(Some(NodeType::CodeFile), usize::MAX)?
        .len();
    let edges = store.list_edges(None, usize::MAX)?.len();
    let ladder = crate::maturity::ladder(&store)?;
    let pulse = workitem::graph_state(&store)?;
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
            "maturity": ladder,
            "graph_state": pulse,
            "validation_summary": validation_summary,
            "code_ownership": {
                "registered": registered_codefiles,
                "owned": owned_codefiles,
                "unowned": unowned_codefiles.len(),
                "unowned_files": unowned_codefiles,
                "blocking": false,
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
    println!(
        "  code ownership: {owned_codefiles}/{registered_codefiles} owned, {} unowned (advisory)",
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
        let mark = match r.state {
            crate::maturity::RungState::Met => "✓",
            crate::maturity::RungState::Unmet => "·",
            crate::maturity::RungState::NotApplicable => "—",
        };
        println!("    {mark} {:<12} {}", r.name, r.detail);
    }
    println!(
        "  compass: phase={} → {}",
        ladder.phase, ladder.next_command
    );
    println!(
        "  queues: build={} fix={} analyze={} inbox={}  (advisory: findings={} untriaged={} stale_findings={} needed={})",
        pulse.planned,
        pulse.stale,
        pulse.uninspected,
        pulse.inbox,
        pulse.findings,
        pulse.untriaged,
        pulse.stale_findings,
        pulse.needed
    );
    Ok(())
}
pub(crate) fn next_all(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let ladder = crate::maturity::ladder(&store)?;
    let pulse = workitem::graph_state(&store)?;
    let modes = [
        ("fix", workitem::Mode::Fix),
        ("validate", workitem::Mode::Validate),
        ("build", workitem::Mode::Build),
        ("quality", workitem::Mode::Quality),
        ("prove", workitem::Mode::Prove),
        ("analyze", workitem::Mode::Analyze),
        ("triage", workitem::Mode::Triage),
    ];
    if json {
        let mut queues = serde_json::Map::new();
        for (name, m) in modes {
            let item = workitem::next(&store, Some(m))?;
            queues.insert(name.to_string(), serde_json::to_value(item)?);
        }
        let out = serde_json::json!({
            "compass": { "phase": ladder.phase, "next_command": ladder.next_command },
            "graph_state": pulse,
            "queues": queues,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "closeout — compass phase={} → {}",
            ladder.phase, ladder.next_command
        );
        for (name, m) in modes {
            match workitem::next(&store, Some(m))? {
                Some(w) => println!("  {name:<8} → {}", w.target.name),
                None => println!("  {name:<8} → (empty)"),
            }
        }
        println!(
            "  graph_state: planned={} stale={} uninspected={} findings={} untriaged={} stale_findings={} needed={} inbox={}",
            pulse.planned,
            pulse.stale,
            pulse.uninspected,
            pulse.findings,
            pulse.untriaged,
            pulse.stale_findings,
            pulse.needed,
            pulse.inbox
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
    let store = open(graph)?;
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
            "  graph_state: planned={} stale={} uninspected={} findings={} untriaged={} stale_findings={} needed={} inbox={}",
            pulse.planned,
            pulse.stale,
            pulse.uninspected,
            pulse.findings,
            pulse.untriaged,
            pulse.stale_findings,
            pulse.needed,
            pulse.inbox
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
