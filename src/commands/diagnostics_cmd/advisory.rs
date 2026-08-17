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
                            "loom finding verdict {} <needed|justified|rejected|deferred|blocked|duplicate|resolved> --reason '…'",
                            crate::model::short(id)
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
                            "    adjudicate: loom finding verdict {} <needed|justified|rejected|deferred|blocked|duplicate|resolved> --reason '…'",
                            crate::model::short(id)
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
/// `loom debt` — live statistical debt feed, or `loom debt promote` to assert
/// one cluster as a Finding without converting the advisory signal (INV-3).
pub(crate) fn debt(graph: Option<&Path>, cmd: Option<DebtCmd>, json: bool) -> Result<()> {
    match cmd {
        None => debt_cmd(graph, json),
        Some(DebtCmd::Promote {
            cluster_id,
            evidence,
            confidence,
        }) => debt_promote(graph, &cluster_id, evidence, confidence, json),
    }
}

pub(crate) fn debt_cmd(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open_read(graph)?;
    let debt = crate::signal::debt(&store)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&debt)?);
    } else if debt.is_empty() {
        println!("no debt signals");
    } else {
        for d in &debt {
            println!("[{}] {} (impact {})", d.kind, d.message, d.impact);
            println!("    id: {}", d.cluster_id);
            println!("    confirm: {}", d.confirm);
        }
        println!("{} ranked signal(s) — advisory, never required", debt.len());
    }
    Ok(())
}

/// Promote a live debt cluster into one asserted Finding. Validates evidence and
/// confidence before opening a writer; recomputes the feed so the id is live.
fn debt_promote(
    graph: Option<&Path>,
    key: &str,
    evidence: String,
    confidence: f64,
    json: bool,
) -> Result<()> {
    let evidence = evidence.trim().to_string();
    if evidence.is_empty() || crate::model::is_placeholder(&evidence) {
        bail!(
            "debt promote requires substantive --evidence (not a placeholder like '…' or '<evidence>')"
        );
    }
    if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
        bail!("debt promote confidence must be a finite value between 0.0 and 1.0");
    }

    let store = open(graph)?;
    let feed = crate::signal::debt(&store)?;
    let cluster = resolve_debt_cluster(&feed, key)?;

    let subject_names = resolve_debt_subjects(&store, cluster)?;
    let result = store.add_promoted_debt_finding(crate::store::DebtPromotionInput {
        cluster_id: &cluster.cluster_id,
        kind: &cluster.kind,
        message: &cluster.message,
        impact: cluster.impact,
        confirm: &cluster.confirm,
        subject_ids: &cluster.subject_ids,
        subject_names: &subject_names,
        evidence: &evidence,
        confidence,
    })?;

    let finding = result.finding;
    let short = crate::model::short(&finding.id);
    let next_step = format!(
        "loom finding verdict {short} <needed|justified|rejected|deferred|blocked|duplicate|resolved> --reason '…'"
    );
    let line = if result.created {
        format!("promoted to asserted finding {short}")
    } else {
        format!("already promoted as asserted finding {short}")
    };
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "cluster_id": cluster.cluster_id,
            "destination": "finding",
            "created": result.created,
            "finding": node_json(&finding),
        }),
        &next_step,
        line,
    )
}

/// Exact cluster id, else unique prefix; zero/ambiguous match fail closed.
fn resolve_debt_cluster<'a>(
    feed: &'a [crate::signal::DebtCluster],
    key: &str,
) -> Result<&'a crate::signal::DebtCluster> {
    if let Some(exact) = feed.iter().find(|c| c.cluster_id == key) {
        return Ok(exact);
    }
    let mut matches: Vec<&crate::signal::DebtCluster> = feed
        .iter()
        .filter(|c| c.cluster_id.starts_with(key))
        .collect();
    match matches.len() {
        0 => bail!("no debt cluster matches '{key}' — run loom debt for the live feed"),
        1 => Ok(matches.remove(0)),
        _ => {
            let mut ids: Vec<&str> = matches.iter().map(|c| c.cluster_id.as_str()).collect();
            ids.sort_unstable();
            bail!("ambiguous debt cluster prefix '{key}': {}", ids.join(", "))
        }
    }
}

/// Resolve every subject id to an existing CodeFile and return canonical names
/// (sorted for co_change; single name for size_outlier).
fn resolve_debt_subjects(
    store: &Store,
    cluster: &crate::signal::DebtCluster,
) -> Result<Vec<String>> {
    match cluster.kind.as_str() {
        "size_outlier" => {
            if cluster.subject_ids.len() != 1 {
                bail!(
                    "size_outlier debt cluster '{}' must have exactly one subject (got {})",
                    cluster.cluster_id,
                    cluster.subject_ids.len()
                );
            }
            let id = &cluster.subject_ids[0];
            let node = store
                .get_node(id)?
                .ok_or_else(|| anyhow!("debt cluster subject '{id}' is not a registered node"))?;
            if node.node_type != NodeType::CodeFile {
                bail!(
                    "debt cluster subject '{id}' is '{}' not CodeFile",
                    node.node_type.as_str()
                );
            }
            Ok(vec![node.name])
        }
        "co_change" => {
            if cluster.subject_ids.len() < 2 {
                bail!(
                    "co_change debt cluster '{}' must have at least two subjects (got {})",
                    cluster.cluster_id,
                    cluster.subject_ids.len()
                );
            }
            let mut names = Vec::with_capacity(cluster.subject_ids.len());
            for id in &cluster.subject_ids {
                let node = store.get_node(id)?.ok_or_else(|| {
                    anyhow!("debt cluster subject '{id}' is not a registered node")
                })?;
                if node.node_type != NodeType::CodeFile {
                    bail!(
                        "debt cluster subject '{id}' is '{}' not CodeFile",
                        node.node_type.as_str()
                    );
                }
                names.push(node.name);
            }
            names.sort();
            Ok(names)
        }
        other => bail!("unknown debt cluster kind '{other}'"),
    }
}
