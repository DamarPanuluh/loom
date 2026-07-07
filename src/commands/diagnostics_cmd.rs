//! Diagnostics command family — smells, debt, doctor, findings, coverage,
//! completeness, scan, calibrate, thresholds, policy, ignore, whoami.
//!
//! Plane: CLI surface over the signal plane. Renders advisory reads computed
//! live from the graph (INV-3: smells/debt are feeds, never stored as required
//! work or edges). Graph writes here are limited to durable finding
//! adjudications (`record_finding_verdict`) and configuration (ignore globs,
//! thresholds, policy) — never structural or derived truth.

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
/// Validate, resolve, and record a finding adjudication through the single gate
/// the CLI enforces — verdict vocabulary (`justified|needed|blocked`) plus a
/// substantive (non-placeholder) reason — returning the resolved finding.
/// Shared by `loom finding verdict` and the `loom apply` adjudications batch so
/// the batch can never accept what the per-verb command would reject.
pub(crate) fn adjudicate_finding(
    store: &Store,
    id: &str,
    verdict: &str,
    reason: &str,
) -> Result<crate::model::Node> {
    validate_finding_verdict(verdict)?;
    if crate::model::is_placeholder(reason) {
        bail!("finding verdict requires a substantive reason (not a placeholder like '…' or '<reason>')");
    }
    let finding = store.resolve_finding(id)?;
    store.record_finding_verdict(&finding.id, verdict, reason)?;
    Ok(finding)
}
fn finding_verdict(
    graph: Option<&Path>,
    id: &str,
    verdict: &str,
    reason: &str,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let finding = adjudicate_finding(&store, id, verdict, reason)?;
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
    let (registered_codefiles, owned, unowned, observed) = code_ownership_summary(&store)?;
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
                    "observed": observed,
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
        "  codefiles: {registered_codefiles} registered, {owned} owned, {} unowned{}",
        unowned.len(),
        if observed > 0 {
            format!(", {observed} observed")
        } else {
            String::new()
        }
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
    // `scan run` must not hold the store while adapter commands execute (up to
    // 120s each): it manages its own short-lived opens (a shared read for config,
    // a brief exclusive write to reconcile) so other agents keep working. Every
    // other subcommand is a quick config read/write under one store.
    if let ScanCmd::Run { name } = &cmd {
        let root = resolve_root(graph)?;
        let report = crate::scan::run_unlocked(&root, name.as_deref())?;
        let store = open_read(graph)?;
        return pulse::emit_line(
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
        );
    }
    let store = open(graph)?;
    match cmd {
        ScanCmd::Add {
            name,
            command,
            map,
            format,
        } => {
            crate::scan::add_adapter(&store, &name, &command, map.as_deref(), format.into())?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "added": name, "command": command, "map": map }),
                "loom scan run",
                format!("registered scan adapter '{name}'"),
            )
        }
        ScanCmd::Update {
            name,
            command,
            map,
            format,
        } => {
            crate::scan::update_adapter(
                &store,
                &name,
                command.as_deref(),
                map.as_deref(),
                format.map(Into::into),
            )?;
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
        ScanCmd::Run { .. } => unreachable!("scan run is handled before the store open"),
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

/// `loom calibrate` — derive structural finding thresholds from the repo's own
/// distribution (the worst-tail quantile per metric, floored). Preview by
/// default; `--write` persists the proposal as portable config.
pub(crate) fn calibrate_cmd(graph: Option<&Path>, write: bool, json: bool) -> Result<()> {
    let root = resolve_root(graph)?;
    let store = if write {
        open(graph)?
    } else {
        open_read(graph)?
    };
    let cal = crate::thresholds::calibrate(&store, &root)?;
    if write {
        crate::thresholds::save(&store, &cal.proposed)?;
        return pulse::emit_line(
            &store,
            json,
            serde_json::json!({ "calibration": cal, "written": true }),
            "loom sync",
            format!(
                "thresholds calibrated from {} file(s) / {} symbol(s): {}",
                cal.files_sampled,
                cal.symbols_sampled,
                threshold_line(&cal.proposed)
            ),
        );
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({ "calibration": cal, "written": false })
            )?
        );
    } else {
        println!(
            "calibration over {} file(s), {} callable symbol(s):",
            cal.files_sampled, cal.symbols_sampled
        );
        println!("  current:  {}", threshold_line(&cal.current));
        println!("  proposed: {}", threshold_line(&cal.proposed));
        println!("next: loom calibrate --write   (persist; travels in the export)");
    }
    Ok(())
}

fn threshold_line(t: &crate::thresholds::Thresholds) -> String {
    format!(
        "file loc {} | symbol complexity {} | symbol loc {} | nesting {} | args {} | file owners {}",
        t.max_file_loc,
        t.max_symbol_complexity,
        t.max_symbol_loc,
        t.max_nesting,
        t.max_args,
        t.max_file_owners
    )
}

/// `loom threshold` — hand-set the structural finding gates (the manual
/// counterpart to `calibrate`). Values persist to portable `config.thresholds`;
/// `reset` drops the config so the gate reverts to the shipped default.
pub(crate) fn threshold_cmd(
    graph: Option<&Path>,
    cmd: crate::cli::ThresholdCmd,
    json: bool,
) -> Result<()> {
    use crate::cli::ThresholdCmd;
    match cmd {
        ThresholdCmd::List => {
            let store = open_read(graph)?;
            let t = crate::thresholds::load(&store)?;
            if json {
                let obj: serde_json::Map<String, serde_json::Value> = t
                    .pairs()
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&obj)?);
            } else {
                for (gate, value) in t.pairs() {
                    println!("{gate:<24} {value}");
                }
            }
            Ok(())
        }
        ThresholdCmd::Set { gate, value } => {
            if value == 0 {
                bail!("threshold value must be >= 1 (0 would flag every symbol/file)");
            }
            let store = open(graph)?;
            let mut t = crate::thresholds::load(&store)?;
            t.set_gate(&gate, value)?;
            crate::thresholds::save(&store, &t)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "gate": gate, "value": value }),
                "loom sync",
                format!("threshold {gate} = {value}"),
            )
        }
        ThresholdCmd::Reset { gate } => {
            let store = open(graph)?;
            match gate {
                Some(gate) => {
                    let mut t = crate::thresholds::load(&store)?;
                    t.reset_gate(&gate)?;
                    // If every gate is back to default, drop the config entirely
                    // so it reverts to "absent = shipped default" rather than a
                    // pinned snapshot.
                    if t == crate::thresholds::Thresholds::default() {
                        crate::thresholds::clear(&store)?;
                    } else {
                        crate::thresholds::save(&store, &t)?;
                    }
                    pulse::emit_line(
                        &store,
                        json,
                        serde_json::json!({ "reset": gate }),
                        "loom sync",
                        format!("threshold {gate} reset to default"),
                    )
                }
                None => {
                    crate::thresholds::clear(&store)?;
                    pulse::emit_line(
                        &store,
                        json,
                        serde_json::json!({ "reset": "all" }),
                        "loom sync",
                        "all thresholds reset to shipped defaults".to_string(),
                    )
                }
            }
        }
    }
}

/// `loom policy` — read or set the evidence policy (review-confidence floor +
/// human-gate placement). Values persist to portable `config.evidence_policy`;
/// `reset` drops the config so the policy reverts to the shipped defaults.
pub(crate) fn policy_cmd(
    graph: Option<&Path>,
    cmd: crate::cli::PolicyCmd,
    json: bool,
) -> Result<()> {
    use crate::cli::PolicyCmd;
    use crate::policy::{self, EvidencePolicy};
    match cmd {
        PolicyCmd::Show => {
            let store = open_read(graph)?;
            let p = policy::load(&store)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&p)?);
            } else {
                println!("review_confidence_floor  {}", p.review_confidence_floor);
                let gated = if p.human_gated_roles.is_empty() {
                    "(none)".to_string()
                } else {
                    p.human_gated_roles.join(", ")
                };
                println!("human_gated_roles        {gated}");
            }
            Ok(())
        }
        PolicyCmd::SetFloor { value } => {
            let store = open(graph)?;
            let mut p = policy::load(&store)?;
            p.review_confidence_floor = value;
            policy::save(&store, &p)?; // save validates the range
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "review_confidence_floor": value }),
                "loom status",
                format!("review confidence floor = {value}"),
            )
        }
        PolicyCmd::GateAdd { role } => {
            let store = open(graph)?;
            let mut p = policy::load(&store)?;
            if !p.human_gated_roles.iter().any(|r| r == &role) {
                p.human_gated_roles.push(role);
                p.human_gated_roles.sort();
            }
            policy::save(&store, &p)?; // save validates the role name
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "human_gated_roles": p.human_gated_roles }),
                "loom status",
                format!("human-gated lanes: {}", p.human_gated_roles.join(", ")),
            )
        }
        PolicyCmd::GateRemove { role } => {
            let store = open(graph)?;
            let mut p = policy::load(&store)?;
            p.human_gated_roles.retain(|r| r != &role);
            if p == EvidencePolicy::default() {
                policy::clear(&store)?;
            } else {
                policy::save(&store, &p)?;
            }
            let gated = if p.human_gated_roles.is_empty() {
                "(none)".to_string()
            } else {
                p.human_gated_roles.join(", ")
            };
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "human_gated_roles": p.human_gated_roles }),
                "loom status",
                format!("human-gated lanes: {gated}"),
            )
        }
        PolicyCmd::Reset => {
            let store = open(graph)?;
            policy::clear(&store)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "reset": "all" }),
                "loom status",
                "evidence policy reset to shipped defaults".to_string(),
            )
        }
    }
}
