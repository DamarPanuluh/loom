use super::*;

pub(crate) fn smells_cmd(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let smells = crate::signal::smells(&store)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&smells)?);
    } else if smells.is_empty() {
        println!("no open smells");
    } else {
        for s in &smells {
            println!("[{}] {}", s.kind, s.message);
            println!("    remedy: {}", s.remedy);
        }
        println!("{} open finding(s)", smells.len());
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
        } => finding_verdict(graph, &id, &verdict, &reason),
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
fn finding_verdict(graph: Option<&Path>, id: &str, verdict: &str, reason: &str) -> Result<()> {
    validate_finding_verdict(verdict)?;
    if reason.trim().is_empty() {
        bail!("finding verdict requires --reason");
    }
    let store = open(graph)?;
    let finding = store.resolve_finding(id)?;
    store.record_finding_verdict(&finding.id, verdict, reason)?;
    println!("{verdict} '{}'", finding.name);
    print_next_move(&store)?;
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
        if store
            .edges_with(Some(EdgeKind::Implements), Some(&n.id), None)?
            .is_empty()
        {
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
    for u in ungrounded.iter().take(10) {
        println!("    ungrounded: {u}");
    }
    println!(
        "  codefiles: {registered_codefiles} registered, {owned} owned, {} unowned",
        unowned.len()
    );
    for u in unowned.iter().take(10) {
        println!("    unowned: {u}");
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
            println!("ignoring '{glob}' ({reason})");
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
            println!("removed ignore rule '{glob}'");
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
