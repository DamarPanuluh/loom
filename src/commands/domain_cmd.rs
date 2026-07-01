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
    let order: Vec<String> = store
        .get_meta("layer_order")?
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default();
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
pub(crate) fn hypothesis(graph: Option<&Path>, cmd: HypothesisCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        HypothesisCmd::Add {
            name,
            claim,
            proposal,
            predicted_outcome,
            target,
        } => {
            let t = store.resolve_node(&target, Some(NodeType::Intent))?;
            let h = store.add_node(
                NodeType::Hypothesis,
                &name,
                &claim,
                "proposed",
                serde_json::json!({ "proposal": proposal, "predicted_outcome": predicted_outcome }),
            )?;
            store.ensure_edge(EdgeKind::Targets, &h.id, &t.id)?;
            println!("hypothesis '{}' targets '{}'", h.name, t.name);
            Ok(())
        }
        HypothesisCmd::Prove {
            key,
            verdict,
            evidence,
        } => {
            let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
            let status = match verdict.as_str() {
                "supported" => "supported",
                "refuted" => "refuted",
                other => bail!("unknown verdict '{other}' (use supported|refuted)"),
            };
            store.set_node_status(&h.id, status)?;
            store.add_note(&h.id, "decision", &format!("{status}: {evidence}"))?;
            println!("hypothesis '{}' {status}", h.name);
            Ok(())
        }
        HypothesisCmd::Adopt { key, spawned } => {
            let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
            if h.status != "supported" {
                bail!(
                    "only a supported hypothesis can be adopted (current: {})",
                    h.status
                );
            }
            store.set_node_status(&h.id, "adopted")?;
            let name = spawned.unwrap_or_else(|| format!("{} (adopted)", h.name));
            let intent = store.add_node(
                NodeType::Intent,
                &name,
                &h.description,
                "planned",
                serde_json::json!({}),
            )?;
            store.add_note(
                &h.id,
                "decision",
                &format!("adopted → spawned intent {}", intent.id),
            )?;
            println!("adopted '{}' → planned intent '{}'", h.name, intent.name);
            Ok(())
        }
        HypothesisCmd::Reject { key, reason } => {
            let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
            store.set_node_status(&h.id, "rejected")?;
            store.add_note(&h.id, "decision", &format!("rejected: {reason}"))?;
            println!("rejected '{}'", h.name);
            Ok(())
        }
        HypothesisCmd::Show { key } => {
            let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
            let targets: Vec<_> = store
                .edges_with(Some(EdgeKind::Targets), Some(&h.id), None)?
                .into_iter()
                .map(|e| {
                    let name = store
                        .get_node(&e.to_id)?
                        .map(|n| n.name)
                        .unwrap_or_else(|| e.to_id.clone());
                    Ok(serde_json::json!({
                        "id": e.to_id,
                        "name": name,
                        "edge_id": e.id,
                    }))
                })
                .collect::<Result<Vec<_>>>()?;
            if json {
                let mut row = node_json(&h);
                row["targets"] = serde_json::json!(targets);
                println!("{}", serde_json::to_string_pretty(&row)?);
            } else {
                println!("{} [{}]", h.name, h.id);
                println!("  status: {}", h.status);
                if !h.description.is_empty() {
                    println!("  claim: {}", h.description);
                }
                println!("  {}", h.body);
                for t in targets {
                    if let Some(name) = t.get("name").and_then(|v| v.as_str()) {
                        println!("  targets: {name}");
                    }
                }
            }
            Ok(())
        }
        HypothesisCmd::List { limit } => {
            let hypotheses = store.list_nodes(Some(NodeType::Hypothesis), limit)?;
            if json {
                let rows: Vec<_> = hypotheses.iter().map(node_json).collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for n in hypotheses {
                    println!("{:<10} {} [{}]", n.status, n.name, &n.id[..8]);
                }
            }
            Ok(())
        }
    }
}
pub(crate) fn surface(graph: Option<&Path>, cmd: SurfaceCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        SurfaceCmd::Add {
            name,
            kind,
            identity,
            codefile,
        } => {
            let s = store.add_node(
                NodeType::InterfaceSurface,
                &name,
                "",
                "",
                serde_json::json!({ "kind": kind, "identity": identity }),
            )?;
            if let Some(cf) = codefile {
                let c = store.resolve_node(&cf, Some(NodeType::CodeFile))?;
                store.add_edge(EdgeKind::Exposes, &s.id, &c.id, TruthClass::Asserted)?;
            }
            println!("declared surface '{}' [{}]", s.name, &s.id[..8]);
            Ok(())
        }
        SurfaceCmd::Show { key } => {
            let n = store.resolve_node(&key, Some(NodeType::InterfaceSurface))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&node_json(&n))?);
            } else {
                println!("{} [{}]", n.name, n.id);
                println!("  {}", n.body);
            }
            Ok(())
        }
        SurfaceCmd::Update {
            key,
            kind,
            identity,
            codefile,
        } => {
            let s = store.resolve_node(&key, Some(NodeType::InterfaceSurface))?;
            let mut body = s.body.clone();
            if let Some(k) = &kind {
                body["kind"] = serde_json::json!(k);
            }
            if let Some(id) = &identity {
                body["identity"] = serde_json::json!(id);
            }
            store.set_node_body(&s.id, &body)?;
            if let Some(cf) = codefile {
                let c = store.resolve_node(&cf, Some(NodeType::CodeFile))?;
                // re-bind: drop the old exposes edge(s) from this surface, add the new one.
                for e in store.edges_with(Some(EdgeKind::Exposes), Some(&s.id), None)? {
                    store.delete_edge(&e.id)?;
                }
                store.add_edge(EdgeKind::Exposes, &s.id, &c.id, TruthClass::Asserted)?;
            }
            println!("updated surface '{}'", s.name);
            Ok(())
        }
        SurfaceCmd::Delete { key } => {
            let n = store.resolve_node(&key, Some(NodeType::InterfaceSurface))?;
            store.delete_node(&n.id)?;
            println!("deleted surface '{}'", n.name);
            Ok(())
        }
        SurfaceCmd::List { limit } => {
            let surfaces = store.list_nodes(Some(NodeType::InterfaceSurface), limit)?;
            if json {
                let rows: Vec<_> = surfaces.iter().map(node_json).collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for n in surfaces {
                    println!("{} [{}]", n.name, &n.id[..8]);
                }
            }
            Ok(())
        }
    }
}
pub(crate) fn vocab(graph: Option<&Path>, cmd: VocabCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        VocabCmd::Add { term, why } => {
            store.add_vocab_term(&term, &why)?;
            println!("registered vocab term '{term}'");
            Ok(())
        }
        VocabCmd::Remove { term } => {
            store.remove_vocab_term(&term)?;
            println!("removed vocab term '{term}' (and untagged any nodes carrying it)");
            Ok(())
        }
        VocabCmd::List => {
            let terms = store.list_vocab()?;
            if json {
                let rows: Vec<_> = terms
                    .iter()
                    .map(|(term, why)| serde_json::json!({ "term": term, "why": why }))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for (term, why) in terms {
                    println!("{term}  — {why}");
                }
            }
            Ok(())
        }
    }
}
pub(crate) fn interface(graph: Option<&Path>, cmd: InterfaceCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        InterfaceCmd::Gaps => {
            let surfaces = store.list_nodes(Some(NodeType::InterfaceSurface), usize::MAX)?;
            let mut gaps = Vec::new();
            for s in &surfaces {
                let exposes = store.edges_with(Some(EdgeKind::Exposes), Some(&s.id), None)?;
                let calls = store.edges_with(Some(EdgeKind::Calls), None, Some(&s.id))?;
                if exposes.is_empty() {
                    gaps.push(serde_json::json!({
                        "surface_id": s.id,
                        "surface": s.name,
                        "kind": "unexposed_surface",
                        "message": format!("surface '{}' exposes no codefile", s.name),
                    }));
                }
                if calls.is_empty() {
                    gaps.push(serde_json::json!({
                        "surface_id": s.id,
                        "surface": s.name,
                        "kind": "uncalled_surface",
                        "message": format!("surface '{}' is never called by a validation/saga", s.name),
                    }));
                }
            }
            let armed = !surfaces.is_empty();
            let warning = if armed {
                None
            } else {
                Some("no surfaces declared")
            };
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "armed": armed,
                        "surface_count": surfaces.len(),
                        "gaps": gaps,
                        "warning": warning,
                    }))?
                );
            } else if !armed {
                println!("interface plane unmodeled: 0 surfaces declared; no interface-gap analysis possible");
            } else {
                for gap in &gaps {
                    println!("{}", gap["message"].as_str().unwrap_or(""));
                }
                println!(
                    "{} interface gap(s) across {} surface(s)",
                    gaps.len(),
                    surfaces.len()
                );
            }
            Ok(())
        }
    }
}
