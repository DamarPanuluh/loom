use super::*;

pub(crate) fn finding(graph: Option<&Path>, cmd: FindingCmd, json: bool) -> Result<()> {
    match cmd {
        FindingCmd::Add {
            text,
            source,
            kind,
            evidence,
            impact,
            confidence,
            file,
            link,
        } => finding_add(
            graph,
            FindingAddInput {
                text,
                source,
                kind,
                evidence,
                impact,
                confidence,
                file,
                link,
            },
            json,
        ),
        FindingCmd::List { kind, state } => finding_list(graph, kind, state, json),
        FindingCmd::Verdict {
            id,
            verdict,
            reason,
            evidence,
        } => finding_verdict(graph, &id, &verdict, &reason, &evidence, json),
    }
}

struct FindingAddInput {
    text: String,
    source: String,
    kind: String,
    evidence: String,
    impact: String,
    confidence: f64,
    file: Option<String>,
    link: Option<String>,
}

fn finding_add(graph: Option<&Path>, input: FindingAddInput, json: bool) -> Result<()> {
    let FindingAddInput {
        text,
        source,
        kind,
        evidence,
        impact,
        confidence,
        file,
        link,
    } = input;
    match source.as_str() {
        "code_audit" | "adversarial_review" | "wiki" | "validation" | "llm" => {}
        "human" | "external" | "support" | "import" => {
            bail!("human/external input belongs in inbox; use loom door or loom inbox add")
        }
        "question" => bail!("product questions belong in loom question add"),
        other => bail!("unknown finding source '{other}' (use code_audit|adversarial_review|wiki|validation|llm)"),
    }
    for (field, value) in [
        ("text", text.as_str()),
        ("evidence", evidence.as_str()),
        ("impact", impact.as_str()),
    ] {
        if crate::model::is_placeholder(value) {
            bail!("finding add requires substantive {field} (not a placeholder like '…' or '<{field}>')");
        }
    }
    if !(0.0..=1.0).contains(&confidence) {
        bail!("finding add confidence must be between 0.0 and 1.0");
    }
    if file.is_none() && link.is_none() {
        bail!("finding add requires --file <registered codefile> or --link <ref>");
    }
    let store = open(graph)?;
    let codefile = match file.as_deref() {
        Some(path) => Some(store.resolve_node(path, Some(NodeType::CodeFile))?),
        None => None,
    };
    let mut body = serde_json::json!({
        "kind": kind,
        "source": source,
        "evidence": evidence,
        "impact": impact,
        "confidence": confidence,
    });
    if let Some(cf) = &codefile {
        body["file"] = serde_json::Value::String(cf.name.clone());
    }
    if let Some(l) = link {
        body["link"] = serde_json::Value::String(l);
    }
    let finding = store.add_node(NodeType::Finding, &text, &impact, &kind, body)?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({ "finding": node_json(&finding) }),
        "loom next --mode triage",
        format!(
            "finding [{}] captured for triage",
            crate::model::short(&finding.id)
        ),
    )
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
                let id = crate::model::short(&fv.node.id);
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
/// the CLI enforces — verdict vocabulary
/// (`needed|justified|rejected|deferred|blocked|duplicate|resolved`) plus a substantive
/// (non-placeholder) reason — returning the resolved finding.
/// Shared by `loom finding verdict` and the `loom apply` adjudications batch so
pub(crate) fn adjudicate_finding(
    store: &Store,
    id: &str,
    verdict: &str,
    reason: &str,
    evidence: &str,
) -> Result<crate::model::Node> {
    adjudicate_finding_batch(store, id, verdict, reason, evidence, None)
}

pub(crate) fn adjudicate_finding_batch(
    store: &Store,
    id: &str,
    verdict: &str,
    reason: &str,
    evidence: &str,
    batch_id: Option<&str>,
) -> Result<crate::model::Node> {
    validate_finding_verdict(verdict)?;
    if crate::model::is_placeholder(reason) {
        bail!("finding verdict requires a substantive reason (not a placeholder like '…' or '<reason>')");
    }
    let finding = store.resolve_finding(id)?;
    store.record_finding_verdict_batch(&finding.id, verdict, reason, evidence, batch_id)?;
    Ok(finding)
}
fn finding_verdict(
    graph: Option<&Path>,
    id: &str,
    verdict: &str,
    reason: &str,
    evidence: &str,
    json: bool,
) -> Result<()> {
    // Verdict-family door: absorb brief lock contention (see open_fact_write).
    let store = open_fact_write(graph)?;
    let finding = adjudicate_finding(&store, id, verdict, reason, evidence)?;
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
        "untriaged" | "stale" | "needed" | "justified" | "rejected" | "deferred" | "blocked"
        | "duplicate" | "resolved" => Ok(()),
        other => {
            bail!("unknown finding state '{other}' (use untriaged|stale|needed|justified|rejected|deferred|blocked|duplicate|resolved)")
        }
    }
}
fn validate_finding_verdict(verdict: &str) -> Result<()> {
    match verdict {
        "needed" | "justified" | "rejected" | "deferred" | "blocked" | "duplicate"
        | "resolved" => Ok(()),
        other => bail!(
            "unknown verdict '{other}' (use needed|justified|rejected|deferred|blocked|duplicate|resolved)"
        ),
    }
}
pub(crate) fn doctor_cmd(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open_read(graph)?;
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
        let message = format!("doctor found {} integrity issue(s)", issues.len());
        if json {
            Err(JsonStdoutComplete::fail(message))
        } else {
            bail!("{message}")
        }
    }
}
