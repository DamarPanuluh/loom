use super::*;

pub(crate) fn coverage_cmd(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open_read(graph)?;
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
    let scope = crate::coverage::coverage_scope_summary(&store)?;
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
                    "registered": scope.total_registered,
                    "in_scope": scope.in_scope,
                    "owned": scope.owned,
                    "unowned": scope.unowned(),
                    "unowned_files": &scope.unowned_files,
                    "observed": scope.observed,
                    "excluded": scope.excluded(),
                    "excluded_files": &scope.excluded_files,
                    "exclusions_by_reason": &scope.exclusions_by_reason,
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
        "  codefiles: {} registered — {} owned, {} unowned, {} excluded ({:.1}%), {} observed",
        scope.total_registered,
        scope.owned,
        scope.unowned(),
        scope.excluded(),
        if scope.total_registered == 0 {
            0.0
        } else {
            scope.excluded() as f64 * 100.0 / scope.total_registered as f64
        },
        scope.observed,
    );
    for (reason, count) in &scope.exclusions_by_reason {
        println!("    excluded: {count} — {reason}");
    }
    for u in scope.unowned_files.iter().take(20) {
        println!("    unowned: {u}");
    }
    if scope.unowned() > 20 {
        println!("    … +{} more unowned (see --json)", scope.unowned() - 20);
    }
    Ok(())
}
pub(crate) fn ignore_cmd(graph: Option<&Path>, cmd: IgnoreCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        IgnoreCmd::Add { glob, reason } => {
            let mut list: Vec<serde_json::Value> = read_json_meta(&store, "ignores")?;
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
            let mut list: Vec<serde_json::Value> = read_json_meta(&store, "ignores")?;
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
            let list: Vec<serde_json::Value> = read_json_meta(&store, "ignores")?;
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
    let execution = store.execution_identity();
    let executor = execution.executor();
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
                    "actor": execution.actor(),
                    "profile": execution.profile(),
                    "mode": mode,
                    "lane": lane,
                    "lane_gate": lane.is_some(),
                },
                "authority": {
                    "actor": execution.actor(),
                    "lane": lane,
                },
                "executor": executor.map(|executor| serde_json::json!({
                    "profile": executor.profile(),
                    "source": executor.source().as_str(),
                    "verified": executor.verified(),
                })),
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
            println!("agent: solo (LOOM_AGENT unset/solo) — drives every lane; lane gate OFF");
        }
        crate::store::Agent::Lane(r) => {
            println!(
                "agent: {} — lane gate ON (may only write {}-owned facts)",
                r.as_str(),
                r.as_str()
            );
        }
    }
    if let Some(executor) = executor {
        println!(
            "executor profile: {} (source: {}; verified: {}; attribution only)",
            executor.profile(),
            executor.source().as_str(),
            executor.verified()
        );
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
