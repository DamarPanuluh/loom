//! `loom judgment` — the LLM-proposal judgment inbox.
//!
//! Plane: CLI surface over `store::judgments` and the existing direct-write
//! chokepoints. Staging a proposal is an ungated recommendation; confirming
//! one executes the same write the direct command would. `ratify`/`reject`
//! demand the human's answer through their existing INV-8 paths; `redefine`
//! preserves the direct command's builder-owner lane gate and applies through
//! `redefine_intent` with its normal ripple. The inbox changes where candidates
//! wait, not the authority required for each judgment kind.

use super::status_cmd;
use super::*;
use crate::cli::JudgmentCmd;
use crate::registry::OwnerRole;

pub(crate) fn dispatch(graph: Option<&Path>, cmd: JudgmentCmd, json: bool) -> Result<()> {
    match cmd {
        JudgmentCmd::Propose {
            kind,
            intent,
            evidence,
            description,
        } => propose(graph, &kind, &intent, &evidence, description, json),
        JudgmentCmd::Digest { all } => digest(graph, all, json),
        JudgmentCmd::Confirm {
            key,
            human_decision,
        } => confirm(graph, &key, human_decision, json),
        JudgmentCmd::Withdraw { key, reason } => withdraw(graph, &key, &reason, json),
    }
}

fn propose(
    graph: Option<&Path>,
    kind: &str,
    intent: &str,
    evidence: &str,
    description: Option<String>,
    json: bool,
) -> Result<()> {
    let kind: crate::store::JudgmentKind = kind.parse()?;
    let store = open(graph)?;
    if crate::model::is_placeholder(evidence) {
        anyhow::bail!("--evidence must say WHY the judgment holds, substantively — the reviewer needs this text");
    }
    let target = store.resolve_node(intent, Some(NodeType::Intent))?;
    let detail = match kind.as_str() {
        "redefine" => {
            let d = description.unwrap_or_default();
            if crate::model::is_placeholder(&d) {
                anyhow::bail!(
                    "a redefine proposal needs --description: the replacement statement to apply"
                );
            }
            if d.trim() == target.description.trim() {
                anyhow::bail!("--description restates the current statement — nothing to redefine");
            }
            d
        }
        "ratify" | "reject" => String::new(),
        other => unreachable!("validated judgment kind '{other}'"),
    };
    if kind.as_str() == "ratify" && super::intent::is_ratified(&store, &target.id)? {
        anyhow::bail!(
            "'{}' is already ratified — nothing to propose (a redefinition that staled it \
             would route through the ratify queue, not the inbox)",
            target.name
        );
    }
    if target.status == "deprecated" && kind.as_str() != "redefine" {
        anyhow::bail!(
            "'{}' is deprecated — a {kind} proposal judges a live intent",
            target.name
        );
    }
    let agent = std::env::var("LOOM_AGENT").unwrap_or_else(|_| "solo".into());
    let p = store.stage_judgment(kind, &target.id, evidence, &detail, &agent)?;
    crate::journal::append(
        store.root(),
        "judgment_proposed",
        &p.id,
        serde_json::json!({
            "kind": kind,
            "intent": { "id": target.id, "name": target.name },
            "evidence": evidence,
            "detail": detail,
            "staged_by": agent,
        }),
    )?;
    let id8 = crate::model::short(&p.id);
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "proposal": p,
            "intent": { "id": target.id, "name": target.name },
        }),
        "loom judgment digest",
        format!(
            "staged {kind} proposal [{id8}] for '{}' — review it with `loom judgment digest`",
            target.name
        ),
    )?;
    Ok(())
}

fn digest(graph: Option<&Path>, all: bool, json: bool) -> Result<()> {
    let store = open_read(graph)?;
    let proposals = store.list_judgments(if all { None } else { Some("staged".parse()?) })?;
    let rows: Vec<serde_json::Value> = proposals
        .iter()
        .map(|p| {
            // A proposal whose intent vanished is reviewable history, not a
            // crash: name it dangling so a reviewer can withdraw it.
            let intent = store
                .get_node(&p.intent_id)
                .ok()
                .flatten()
                .map(|n| serde_json::json!({ "id": n.id, "name": n.name, "status": n.status }))
                .unwrap_or_else(|| serde_json::json!({ "id": p.intent_id, "dangling": true }));
            serde_json::json!({
                "id": p.id,
                "id8": crate::model::short(&p.id),
                "kind": p.kind,
                "intent": intent,
                "evidence": p.evidence,
                "description": if p.detail.is_empty() { None } else { Some(&p.detail) },
                "staged_by": p.staged_by,
                "staged_at": p.staged_at,
                "state": p.state,
                "decided_at": if p.decided_at.is_empty() { None } else { Some(&p.decided_at) },
                "confirm": format!("loom judgment confirm {}", crate::model::short(&p.id)),
            })
        })
        .collect();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "staged": store.count_judgments("staged".parse()?)?,
                "proposals": rows,
            }))?
        );
        return Ok(());
    }
    if rows.is_empty() {
        println!(
            "{}",
            if all {
                "no judgment proposals"
            } else {
                "no staged judgment proposals — the inbox is empty"
            }
        );
        return Ok(());
    }
    println!("{} proposal(s):", rows.len());
    for r in &rows {
        let state = r["state"].as_str().unwrap_or("?");
        let marker = if state == "staged" {
            String::new()
        } else {
            format!("  ({state})")
        };
        let intent = r["intent"]["name"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("dangling:{}", r["intent"]["id"].as_str().unwrap_or("?")));
        println!(
            "  [{}] {} '{}'{}",
            r["id8"].as_str().unwrap_or("?"),
            r["kind"].as_str().unwrap_or("?"),
            intent,
            marker
        );
        println!("      evidence: {}", r["evidence"].as_str().unwrap_or(""));
        if let Some(d) = r["description"].as_str() {
            println!("      redefine to: {d}");
        }
        println!(
            "      staged by {} at {}",
            r["staged_by"].as_str().unwrap_or("?"),
            r["staged_at"].as_str().unwrap_or("?")
        );
        if state == "staged" {
            if r["kind"].as_str() == Some("redefine") {
                println!(
                    "      confirm: loom judgment confirm {}",
                    r["id8"].as_str().unwrap_or("?")
                );
            } else {
                println!(
                    "      confirm: loom judgment confirm {} --human-decision \"<the human's answer>\"",
                    r["id8"].as_str().unwrap_or("?")
                );
            }
        }
    }
    Ok(())
}

fn confirm(
    graph: Option<&Path>,
    key: &str,
    human_decision: Option<String>,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let p = store.resolve_judgment(key)?;
    if p.state != "staged".parse()? {
        anyhow::bail!(
            "judgment proposal [{}] is {} — only a staged proposal can be confirmed",
            crate::model::short(&p.id),
            p.state
        );
    }
    let intent = store
        .resolve_node(&p.intent_id, Some(NodeType::Intent))
        .map_err(|_| {
            anyhow::anyhow!(
                "the intent this proposal judges is gone — withdraw it: \
                 loom judgment withdraw {} --reason \"target removed\"",
                crate::model::short(&p.id)
            )
        })?;
    let id8 = crate::model::short(&p.id);
    let mediated = human_decision.is_some();

    // Execute the direct write's authority check FIRST. If either the human
    // gate (ratify/reject) or builder-owner gate (redefine) refuses, the
    // proposal stays staged and nothing else moves.
    let outcome = match p.kind.as_str() {
        "ratify" => {
            let decision = super::ratification_decision(&intent.name, human_decision)?;
            store.ratify_intent_from_human(&intent.id, &p.evidence, &decision)?;
            serde_json::json!({ "ratified": { "id": intent.id, "name": intent.name } })
        }
        "reject" => {
            let decision = match human_decision {
                Some(response) => super::mediated_decision(response)?,
                None if super::human_present() => {
                    crate::ratification::HumanDecision::direct("tty")?
                }
                None => anyhow::bail!(
                    "INV-8: only a human may judge whether a behavior is wanted — ask the human, \
                     then pass their exact answer with --human-decision"
                ),
            };
            let minted =
                super::intent::reject_intent_core(&store, &intent, &p.evidence, &decision)?;
            serde_json::json!({
                "rejected": { "id": intent.id, "name": intent.name },
                "removal_work": minted,
            })
        }
        "redefine" => {
            status_cmd::require_lane(&store, OwnerRole::Builder)?;
            let reopened = store.redefine_intent(&intent.id, &p.detail)?;
            store.add_note(
                &intent.id,
                "decision",
                &format!("redefined (judgment proposal {id8}): {}", p.evidence),
            )?;
            serde_json::json!({
                "redefined": { "id": intent.id, "name": intent.name },
                "description": p.detail,
                "reopened_edges": reopened,
            })
        }
        other => unreachable!("validated judgment kind '{other}'"),
    };
    store.decide_judgment(&p.id, "confirmed".parse()?)?;
    crate::journal::append(
        store.root(),
        "judgment_confirmed",
        &p.id,
        serde_json::json!({
            "kind": p.kind,
            "intent": { "id": intent.id, "name": intent.name },
            "mediated": mediated,
        }),
    )?;
    let message = if p.kind.as_str() == "redefine" {
        format!(
            "confirmed redefine proposal [{id8}] — '{}' redefined through the builder-owner gate",
            intent.name
        )
    } else {
        format!(
            "confirmed {} proposal [{id8}] — '{}' judged through the human gate",
            p.kind, intent.name
        )
    };
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "proposal": { "id": p.id, "kind": p.kind, "state": "confirmed" },
            "outcome": outcome,
        }),
        "loom judgment digest",
        message,
    )?;
    Ok(())
}

fn withdraw(graph: Option<&Path>, key: &str, reason: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    if crate::model::is_placeholder(reason) {
        anyhow::bail!("--reason must say why the proposal is withdrawn, substantively");
    }
    let p = store.resolve_judgment(key)?;
    if p.state != "staged".parse()? {
        anyhow::bail!(
            "judgment proposal [{}] is {} — only a staged proposal can be withdrawn",
            crate::model::short(&p.id),
            p.state
        );
    }
    store.decide_judgment(&p.id, "withdrawn".parse()?)?;
    crate::journal::append(
        store.root(),
        "judgment_withdrawn",
        &p.id,
        serde_json::json!({ "kind": p.kind, "intent_id": p.intent_id, "reason": reason }),
    )?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "proposal": { "id": p.id, "kind": p.kind, "state": "withdrawn" },
            "reason": reason,
        }),
        "loom judgment digest",
        format!(
            "withdrew {} proposal [{}]: {reason}",
            p.kind,
            crate::model::short(&p.id)
        ),
    )?;
    Ok(())
}
