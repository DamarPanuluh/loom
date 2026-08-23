use super::*;

pub(crate) fn layer_detector_state(store: &Store) -> Result<serde_json::Value> {
    layer_detector_state_with(store, &store.snapshot()?)
}

/// [`layer_detector_state`] over a snapshot the caller already holds.
pub(crate) fn layer_detector_state_with(
    store: &Store,
    snap: &crate::store::Snapshot,
) -> Result<serde_json::Value> {
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
    let order: Vec<String> = read_json_meta(store, crate::store::LAYER_ORDER_META_KEY)?;
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

/// `loom layer` — declare, list, or clear the architecture layer order.
///
/// Lives here rather than in `commands.rs` because every other command arm
/// routes to a handler module; this one held its whole body in the dispatcher
/// while already reaching into `layer_detector_state` next door for half of it.
pub(crate) fn layer(graph: Option<&Path>, cmd: LayerCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        LayerCmd::Order { layers } => {
            if layers.is_empty() {
                bail!("provide the layer order, top first");
            }
            store.set_meta(crate::store::LAYER_ORDER_META_KEY, &serde_json::to_string(&layers)?)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ crate::store::LAYER_ORDER_META_KEY: layers }),
                "loom sync",
                format!("layer order: {}", layers.join(" > ")),
            )
        }
        LayerCmd::List => {
            let state = domain_cmd::layer_detector_state(&store)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&state)?);
            } else if let Some(order) = state.get("order").and_then(|v| v.as_array()) {
                if order.is_empty() {
                    println!("no layer order declared");
                } else {
                    let labels: Vec<&str> = order.iter().filter_map(|v| v.as_str()).collect();
                    println!("{}", labels.join(" > "));
                }
            }
            Ok(())
        }
        LayerCmd::Clear => {
            store.set_meta(crate::store::LAYER_ORDER_META_KEY, "[]")?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ crate::store::LAYER_ORDER_META_KEY: [] }),
                "loom status",
                "layer order cleared".to_string(),
            )
        }
    }
}
