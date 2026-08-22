use super::*;

pub(crate) fn emit_report(
    report: &crate::journey_runtime::RuntimeReport,
    json_output: bool,
) -> Result<()> {
    // A blocked run carries its cause in `detail` — a missing declared
    // environment variable, a stale temporal hash, a refused setup. Printing
    // only the counts turns "blocked (0 passed, 0 failed)" into a dead end that
    // sends the reader to --json to learn anything at all, and `diagnose` is
    // exactly where an operator lands when something already went wrong.
    let headline = format!(
        "Journey '{}:{}' {} ({} assertion(s) passed, {} failed)",
        report.journey_id,
        report.profile,
        report.status.as_str(),
        report.assertions_passed,
        report.assertions_failed
    );
    let text = match report.detail.as_deref().map(str::trim) {
        Some(detail) if !detail.is_empty() => format!("{headline}\n  {detail}"),
        _ => headline,
    };
    emit_runtime_value(serde_json::to_value(report)?, json_output, &text)
}

pub(crate) fn emit_runtime_value(value: Value, json_output: bool, text: &str) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{text}");
    }
    Ok(())
}
pub(crate) fn journey_nodes(store: &Store, stable_id: &str) -> Result<Vec<Node>> {
    Ok(store
        .list_nodes(Some(NodeType::Journey), usize::MAX)?
        .into_iter()
        .filter(|node| {
            node.name == stable_id
                || node.body.get("stable_id").and_then(Value::as_str) == Some(stable_id)
        })
        .collect())
}

pub(crate) fn resolve_journey(store: &Store, key: &str) -> Result<Node> {
    if let Ok(node) = store.resolve_node(key, Some(NodeType::Journey)) {
        return Ok(node);
    }
    let nodes = journey_nodes(store, key)?;
    match nodes.as_slice() {
        [node] => Ok(node.clone()),
        [] => bail!("no Journey matches '{key}'"),
        _ => bail!("Journey key '{key}' is ambiguous"),
    }
}

pub(crate) fn load_registered_journey(
    store: &Store,
    key: &str,
) -> Result<(Node, crate::journey::JourneySpec, String)> {
    let journey = resolve_journey(store, key)?;
    let artifact = journey
        .body
        .get("artifact")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Journey '{}' has no artifact", journey.name))?;
    let path = store.root().join(artifact);
    let spec = crate::journey::parse(&path)?;
    if spec.id != journey.name {
        bail!(
            "Journey artifact '{}' now declares stable id '{}', not '{}'",
            artifact,
            spec.id,
            journey.name
        );
    }
    let hash = spec.semantic_hash()?;
    let registered_hash = journey
        .body
        .get("semantic_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Journey '{}' has no semantic_hash", journey.name))?;
    if registered_hash != hash {
        bail!(
            "Journey artifact '{}' changed semantically; run `loom journey add {artifact}` before projecting it",
            journey.name
        );
    }
    Ok((journey, spec, hash))
}
pub(crate) fn node_json(node: &Node) -> Value {
    json!({
        "id": node.id,
        "type": node.node_type.as_str(),
        "name": node.name,
        "description": node.description,
        "status": node.status,
        "body": node.body,
    })
}

pub(crate) fn edge_json_facet(store: &Store, edge_id: &str, key: &str) -> Option<Value> {
    store
        .get_facet(edge_id, TargetKind::Edge, key)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

pub(crate) fn ordered_subset(spec: &crate::journey::JourneySpec, ids: &[String]) -> Vec<String> {
    let wanted: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    spec.steps
        .iter()
        .filter(|step| wanted.contains(step.id.as_str()))
        .map(|step| step.id.clone())
        .collect()
}
pub(crate) fn emit_packet(packet: &Value, _json_output: bool) -> Result<()> {
    // Packet commands are JSON operations in both modes: the non-global-json
    // form remains directly pipeable to an LLM or a manifest-writing tool.
    println!("{}", serde_json::to_string_pretty(packet)?);
    Ok(())
}
