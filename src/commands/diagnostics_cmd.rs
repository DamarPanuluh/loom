use super::*;

pub(crate) fn smells_cmd(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let smells = crate::signal::smells(&store)?;
    // Join each live smell against its materialized finding (created by sync)
    // and any durable adjudication recorded through `loom finding verdict`.
    // The join is by deterministic id, so an adjudication resolves even while
    // the derived node awaits the next sync.
    let mut rows = Vec::new();
    for s in &smells {
        let id = Store::derived_node_id(
            NodeType::Finding,
            &crate::signal::smell_det_key(&s.identity),
        );
        let materialized = store.get_node(&id)?.is_some();
        let adjudication = crate::signal::adjudication_of(&store, &id)?;
        rows.push((s, id, materialized, adjudication));
    }
    if json {
        let out: Vec<_> = rows
            .iter()
            .map(|(s, id, materialized, adj)| {
                serde_json::json!({
                    "kind": s.kind,
                    "message": s.message,
                    "remedy": s.remedy,
                    "finding_id": if *materialized { serde_json::json!(id) } else { serde_json::Value::Null },
                    "state": adj.as_ref().map(|(v, _)| v.as_str()).unwrap_or("untriaged"),
                    "reason": adj.as_ref().map(|(_, r)| r.as_str()).unwrap_or(""),
                    "adjudicate": if *materialized {
                        format!(
                            "loom finding verdict {} <justified|needed|blocked> --reason '…'",
                            &id[..8.min(id.len())]
                        )
                    } else {
                        "loom sync   (materializes this smell as a finding first)".to_string()
                    },
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else if smells.is_empty() {
        println!("no open smells");
    } else {
        for (s, id, materialized, adj) in &rows {
            match adj {
                Some((verdict, reason)) => {
                    println!("[{}·{verdict}] {}", s.kind, s.message);
                    println!("    adjudicated: {reason}");
                }
                None => {
                    println!("[{}] {}", s.kind, s.message);
                    println!("    remedy: {}", s.remedy);
                    if *materialized {
                        println!(
                            "    adjudicate: loom finding verdict {} <justified|needed|blocked> --reason '…'",
                            &id[..8.min(id.len())]
                        );
                    } else {
                        println!(
                            "    adjudicate: run loom sync first (materializes this smell for triage)"
                        );
                    }
                }
            }
        }
        let open = rows.iter().filter(|(_, _, _, adj)| adj.is_none()).count();
        println!("{} smell(s); {} unadjudicated", rows.len(), open);
    }
    Ok(())
}
pub(crate) fn debt_cmd(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let debt = crate::signal::debt(&store)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&debt)?);
    } else if debt.is_empty() {
        println!("no debt signals");
    } else {
        for d in &debt {
            println!("[{}] {} (impact {})", d.kind, d.message, d.impact);
            println!("    confirm: {}", d.confirm);
        }
        println!("{} ranked signal(s) — advisory, never required", debt.len());
    }
    Ok(())
}
pub(crate) fn finding(graph: Option<&Path>, cmd: FindingCmd, json: bool) -> Result<()> {
    match cmd {
        FindingCmd::List { kind, state } => finding_list(graph, kind, state, json),
        FindingCmd::Verdict {
            id,
            verdict,
            reason,
        } => finding_verdict(graph, &id, &verdict, &reason, json),
    }
}
fn finding_list(
    graph: Option<&Path>,
    kind: Option<String>,
    state: Option<String>,
    json: bool,
) -> Result<()> {
    if let Some(s) = &state {
        validate_finding_filter_state(s)?;
    }
    let store = open(graph)?;
    let untriaged = crate::signal::untriaged_findings(&store)?.len();
    let stale_findings = crate::signal::stale_findings(&store)?.len();
    let mut findings = crate::signal::findings_view(&store)?;
    if let Some(k) = &kind {
        findings.retain(|fv| &fv.node.status == k);
    }
    if let Some(s) = &state {
        if s == "stale" {
            findings.retain(|fv| fv.stale);
        } else {
            findings.retain(|fv| &fv.state == s);
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else {
        if findings.is_empty() {
            println!("no findings");
        } else {
            for fv in &findings {
                let stale = if fv.stale { "·STALE" } else { "" };
                let id = &fv.node.id[..8.min(fv.node.id.len())];
                println!("[{}{}] {} {}", fv.state, stale, id, fv.node.name);
                if !fv.node.description.is_empty() {
                    println!("  ↳ {}", fv.node.description);
                }
                if fv.state != "untriaged" {
                    println!("  ↳ {}: {}", fv.state, fv.reason);
                }
            }
        }
        match &state {
            Some(s) => println!("{} finding(s) in state '{}'", findings.len(), s),
            None => println!(
                "{} finding(s); {} untriaged, {} stale — judge with `loom finding verdict <id> …`",
                findings.len(),
                untriaged,
                stale_findings
            ),
        }
    }
    Ok(())
}
fn finding_verdict(
    graph: Option<&Path>,
    id: &str,
    verdict: &str,
    reason: &str,
    json: bool,
) -> Result<()> {
    validate_finding_verdict(verdict)?;
    if crate::model::is_placeholder(reason) {
        bail!("finding verdict requires a substantive --reason (not a placeholder like '…' or '<reason>')");
    }
    let store = open(graph)?;
    let finding = store.resolve_finding(id)?;
    store.record_finding_verdict(&finding.id, verdict, reason)?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "finding": node_json(&finding),
            "verdict": verdict,
            "reason": reason,
        }),
        "loom status",
        format!("{verdict} '{}'", finding.name),
    )?;
    Ok(())
}
fn validate_finding_filter_state(state: &str) -> Result<()> {
    match state {
        "untriaged" | "stale" | "justified" | "needed" | "blocked" => Ok(()),
        other => {
            bail!("unknown finding state '{other}' (use untriaged|stale|justified|needed|blocked)")
        }
    }
}
fn validate_finding_verdict(verdict: &str) -> Result<()> {
    match verdict {
        "justified" | "needed" | "blocked" => Ok(()),
        other => bail!("unknown verdict '{other}' (use justified|needed|blocked)"),
    }
}
pub(crate) fn doctor_cmd(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let issues = crate::signal::doctor(&store)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&issues)?);
    } else {
        for i in &issues {
            println!("[{}] {}", i.kind, i.message);
        }
    }
    if issues.is_empty() {
        if !json {
            println!("doctor: clean");
        }
        Ok(())
    } else {
        bail!("doctor found {} integrity issue(s)", issues.len())
    }
}
pub(crate) fn coverage_cmd(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?;
    let active: Vec<_> = intents
        .iter()
        .filter(|n| n.status != "deprecated")
        .collect();
    let implemented: Vec<_> = active
        .iter()
        .filter(|n| n.status == "implemented")
        .collect();
    let parents: std::collections::HashSet<String> = store
        .list_edges(Some(EdgeKind::Hierarchy), usize::MAX)?
        .into_iter()
        .map(|e| e.from_id)
        .collect();
    let mut ungrounded = Vec::new();
    for n in &implemented {
        if parents.contains(&n.id) {
            continue; // roll-up parent — realized via children
        }
        if store.realizing_groundings(&n.id)?.is_empty() {
            ungrounded.push(n.name.clone());
        }
    }
    let (registered_codefiles, owned, unowned) = code_ownership_summary(&store)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "intents": {
                    "active": active.len(),
                    "implemented": implemented.len(),
                    "planned_or_needs_change": active.len() - implemented.len(),
                },
                "grounding": {
                    "grounded": implemented.len() - ungrounded.len(),
                    "ungrounded": ungrounded.len(),
                    "ungrounded_intents": ungrounded,
                },
                "codefiles": {
                    "registered": registered_codefiles,
                    "owned": owned,
                    "unowned": unowned.len(),
                    "unowned_files": unowned,
                }
            }))?
        );
        return Ok(());
    }
    println!("coverage (vertical spine):");
    println!(
        "  intents: {} active ({} implemented, {} planned/needs_change)",
        active.len(),
        implemented.len(),
        active.len() - implemented.len()
    );
    println!(
        "  grounding: {} implemented, {} ungrounded",
        implemented.len() - ungrounded.len(),
        ungrounded.len()
    );
    for u in ungrounded.iter().take(20) {
        println!("    ungrounded: {u}");
    }
    if ungrounded.len() > 20 {
        println!(
            "    … +{} more ungrounded (see --json)",
            ungrounded.len() - 20
        );
    }
    println!(
        "  codefiles: {registered_codefiles} registered, {owned} owned, {} unowned",
        unowned.len()
    );
    for u in unowned.iter().take(20) {
        println!("    unowned: {u}");
    }
    if unowned.len() > 20 {
        println!("    … +{} more unowned (see --json)", unowned.len() - 20);
    }
    Ok(())
}
pub(crate) fn ignore_cmd(graph: Option<&Path>, cmd: IgnoreCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        IgnoreCmd::Add { glob, reason } => {
            let mut list: Vec<serde_json::Value> = store
                .get_meta("ignores")?
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default();
            list.push(serde_json::json!({ "glob": glob, "reason": reason }));
            store.set_meta("ignores", &serde_json::to_string(&list)?)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "ignore": {
                        "glob": glob,
                        "reason": reason,
                    },
                }),
                "loom status",
                format!("ignoring '{glob}' ({reason})"),
            )?;
            Ok(())
        }
        IgnoreCmd::Remove { glob } => {
            let mut list: Vec<serde_json::Value> = store
                .get_meta("ignores")?
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default();
            let before = list.len();
            list.retain(|r| r.get("glob").and_then(|g| g.as_str()) != Some(glob.as_str()));
            if list.len() == before {
                bail!("no ignore rule for glob '{glob}'");
            }
            store.set_meta("ignores", &serde_json::to_string(&list)?)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "removed": true,
                    "glob": glob,
                }),
                "loom status",
                format!("removed ignore rule '{glob}'"),
            )?;
            Ok(())
        }
        IgnoreCmd::List => {
            let list: Vec<serde_json::Value> = store
                .get_meta("ignores")?
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default();
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else {
                if list.is_empty() {
                    println!("no ignore rules");
                }
                for r in &list {
                    println!(
                        "{}  — {}",
                        r.get("glob").and_then(|g| g.as_str()).unwrap_or(""),
                        r.get("reason").and_then(|g| g.as_str()).unwrap_or("")
                    );
                }
            }
            Ok(())
        }
    }
}
pub(crate) fn whoami_cmd(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let agent = store.agent();
    let identity = store.identity()?;
    if json {
        let (mode, lane) = match agent {
            crate::store::Agent::Solo => ("solo", None),
            crate::store::Agent::Lane(r) => ("lane", Some(r.as_str())),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "agent": {
                    "mode": mode,
                    "lane": lane,
                    "lane_gate": lane.is_some(),
                },
                "graph": {
                    "observed": identity.observed,
                    "mode": if identity.observed { "observed" } else { "owned" },
                }
            }))?
        );
        return Ok(());
    }
    match agent {
        crate::store::Agent::Solo => {
            println!("agent: solo (LOOM_AGENT unset/llm) — drives every lane; lane gate OFF");
        }
        crate::store::Agent::Lane(r) => {
            println!(
                "agent: {} — lane gate ON (may only write {}-owned facts)",
                r.as_str(),
                r.as_str()
            );
        }
    }
    if identity.observed {
        println!(
            "graph: observed — maps code you do not own; discovery/quality/validation only (build/fix disabled)"
        );
    } else {
        println!("graph: owned — you may build and fix here");
    }
    Ok(())
}

/// `loom scan` — external diagnostic adapters (any language's tools) whose
/// output becomes derived findings in the ordinary triage lifecycle.
pub(crate) fn scan_cmd(graph: Option<&Path>, cmd: crate::cli::ScanCmd, json: bool) -> Result<()> {
    use crate::cli::ScanCmd;
    let store = open(graph)?;
    match cmd {
        ScanCmd::Add { name, command, map } => {
            crate::scan::add_adapter(&store, &name, &command, map.as_deref())?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "added": name, "command": command, "map": map }),
                "loom scan run",
                format!("registered scan adapter '{name}'"),
            )
        }
        ScanCmd::Update { name, command, map } => {
            crate::scan::update_adapter(&store, &name, command.as_deref(), map.as_deref())?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "updated": name, "command": command, "map": map }),
                "loom scan run",
                format!("updated scan adapter '{name}'"),
            )
        }
        ScanCmd::List => {
            let adapters = crate::scan::list_adapters(&store)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&adapters)?);
            } else {
                if adapters.is_empty() {
                    println!("no scan adapters registered (loom scan add <name> <command>)");
                }
                for a in &adapters {
                    println!("{:<12} {}", a.name, a.command);
                }
            }
            Ok(())
        }
        ScanCmd::Remove { name } => {
            crate::scan::remove_adapter(&store, &name)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "removed": name }),
                "loom status",
                format!("removed scan adapter '{name}'"),
            )
        }
        ScanCmd::Run { name } => {
            let root = store.root().to_path_buf();
            let report = crate::scan::run(&store, &root, name.as_deref())?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "scan": report }),
                "loom next --mode triage",
                format!(
                    "scan: {} adapter(s), {} diagnostic(s) → {} new finding(s), {} resolved",
                    report.adapters_run,
                    report.diagnostics,
                    report.new_findings,
                    report.resolved_findings
                ),
            )
        }
    }
}

/// `loom completeness` — the Definition-of-Complete scorecard: which axes
/// around each behavioral idea are met, open, waived, or not applicable.
pub(crate) fn completeness_cmd(graph: Option<&Path>, key: Option<&str>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let cards = match key {
        Some(k) => {
            let intent = store.resolve_node(k, Some(crate::model::NodeType::Intent))?;
            vec![crate::completeness::scorecard(&store, &intent)?]
        }
        None => crate::completeness::all_scorecards(&store)?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&cards)?);
        return Ok(());
    }
    if cards.is_empty() {
        println!("no feature-level intents to score");
    }
    for card in &cards {
        println!(
            "{} [{}]  open={}",
            card.intent_name,
            &card.intent_id[..8.min(card.intent_id.len())],
            card.open
        );
        for a in &card.axes {
            let mark = match a.state.as_str() {
                "met" => "✓",
                "open" => "·",
                "waived" => "~",
                _ => "-",
            };
            let waiver = a
                .waived_reason
                .as_ref()
                .map(|r| format!(" (waived: {r})"))
                .unwrap_or_default();
            println!("  {mark} {:<14} {}{}", a.axis, a.detail, waiver);
        }
    }
    if key.is_none() && cards.iter().any(|c| c.open > 0) {
        println!("drain open axes: loom next --mode elaborate");
    }
    Ok(())
}
