use super::*;

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
        let report = crate::seed::ScanDeriver.run_on_demand(&root, name.as_deref())?;
        let store = open_read(graph)?;
        return pulse::emit_line(
            &store,
            json,
            serde_json::json!({ "scan": report }),
            "loom next --mode triage",
            format!(
                "scan: {} adapter(s), {} diagnostic(s) → {} new finding(s), {} resolved{}",
                report.adapters_run,
                report.diagnostics,
                report.new_findings,
                report.resolved_findings,
                if report.unattached > 0 {
                    format!(
                        " ({} diagnostic(s) matched no tracked file — check the adapter's paths)",
                        report.unattached
                    )
                } else {
                    String::new()
                }
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
            crate::model::short(&card.intent_id),
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
        "file loc {} | symbol complexity {} | symbol loc {} | nesting {} | args {}",
        t.max_file_loc, t.max_symbol_complexity, t.max_symbol_loc, t.max_nesting, t.max_args,
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
                println!(
                    "adversarial_review_frontier {}",
                    p.adversarial_review_frontier
                );
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
        PolicyCmd::SetAdversarialFrontier { value } => {
            let store = open(graph)?;
            let mut p = policy::load(&store)?;
            p.adversarial_review_frontier = value;
            policy::save(&store, &p)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "adversarial_review_frontier": value }),
                "loom status",
                format!("adversarial review frontier = {value}"),
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
