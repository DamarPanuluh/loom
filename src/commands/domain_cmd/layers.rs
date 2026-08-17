use super::*;

pub(crate) fn layer_detector_state(store: &Store) -> Result<serde_json::Value> {
    let snap = store.snapshot()?;
    let active_intent_ids: std::collections::HashSet<&str> = snap
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::Intent && n.status != "deprecated")
        .map(|n| n.id.as_str())
        .collect();
    let layers: std::collections::BTreeSet<String> = snap
        .facets
        .iter()
        .filter(|f| {
            active_intent_ids.contains(f.target_id.as_str())
                && f.target_kind == TargetKind::Node
                && f.key == "layer"
        })
        .map(|f| f.value.clone())
        .collect();
    let order: Vec<String> = read_json_meta(store, "layer_order")?;
    let armed = !order.is_empty();
    let warning = if !armed && layers.len() >= 2 {
        Some("no layer order declared")
    } else if !armed {
        Some("fewer than two layers declared")
    } else {
        None
    };
    Ok(serde_json::json!({
        "armed": armed,
        "layer_count": layers.len(),
        "layers": layers.into_iter().collect::<Vec<_>>(),
        "order": order,
        "warning": warning,
    }))
}
