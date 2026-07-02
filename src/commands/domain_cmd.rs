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
            let edge = store.ensure_edge(EdgeKind::Targets, &h.id, &t.id)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "hypothesis": node_json(&h),
                    "target": node_json(&t),
                    "edge": edge,
                }),
                "loom status",
                format!("hypothesis '{}' targets '{}'", h.name, t.name),
            )?;
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
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "hypothesis": {
                        "id": h.id,
                        "name": h.name,
                        "status": status,
                    },
                    "evidence": evidence,
                }),
                "loom status",
                format!("hypothesis '{}' {status}", h.name),
            )?;
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
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "hypothesis": {
                        "id": h.id,
                        "name": h.name,
                        "status": "adopted",
                    },
                    "spawned_intent": node_json(&intent),
                }),
                "loom status",
                format!("adopted '{}' → planned intent '{}'", h.name, intent.name),
            )?;
            Ok(())
        }
        HypothesisCmd::Reject { key, reason } => {
            let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
            store.set_node_status(&h.id, "rejected")?;
            store.add_note(&h.id, "decision", &format!("rejected: {reason}"))?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "hypothesis": {
                        "id": h.id,
                        "name": h.name,
                        "status": "rejected",
                    },
                    "reason": reason,
                }),
                "loom status",
                format!("rejected '{}'", h.name),
            )?;
            Ok(())
        }
        HypothesisCmd::Update {
            key,
            claim,
            proposal,
            predicted_outcome,
            reason,
        } => {
            if reason.trim().is_empty() {
                bail!("hypothesis update needs substantive --reason");
            }
            if claim.is_none() && proposal.is_none() && predicted_outcome.is_none() {
                bail!("nothing to update — pass --claim, --proposal, and/or --predicted-outcome");
            }
            let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
            if h.status != "proposed" {
                bail!(
                    "only proposed hypotheses can be updated (current: {}); proven/adopted/rejected hypotheses are history",
                    h.status
                );
            }
            let mut body = h.body.clone();
            if let Some(v) = &claim {
                body["claim"] = serde_json::json!(v);
                store.update_node(&h.id, None, Some(v), None)?;
            }
            if let Some(v) = &proposal {
                body["proposal"] = serde_json::json!(v);
            }
            if let Some(v) = &predicted_outcome {
                body["predicted_outcome"] = serde_json::json!(v);
            }
            store.set_node_body(&h.id, &body)?;
            store.add_note(&h.id, "decision", &format!("refined hypothesis: {reason}"))?;
            let display_claim = claim.as_deref().unwrap_or(&h.description);
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "hypothesis": {
                        "id": h.id,
                        "name": h.name,
                        "status": h.status,
                        "claim": display_claim,
                        "body": body,
                    },
                    "reason": reason,
                }),
                "loom status",
                format!("updated proposed hypothesis '{}'", h.name),
            )?;
            Ok(())
        }
        HypothesisCmd::Remove { key } => {
            let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
            store.delete_node(&h.id)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "removed": true,
                    "hypothesis": node_json(&h),
                }),
                "loom status",
                format!("removed mistaken hypothesis '{}'", h.name),
            )?;
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
            let exposes_edge = if let Some(cf) = codefile {
                let c = store.resolve_node(&cf, Some(NodeType::CodeFile))?;
                Some(store.add_edge(EdgeKind::Exposes, &s.id, &c.id, TruthClass::Asserted)?)
            } else {
                None
            };
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "surface": node_json(&s),
                    "exposes_edge": exposes_edge,
                }),
                "loom status",
                format!("declared surface '{}' [{}]", s.name, &s.id[..8]),
            )?;
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
            let exposes_edge = if let Some(cf) = codefile {
                let c = store.resolve_node(&cf, Some(NodeType::CodeFile))?;
                // re-bind: drop the old exposes edge(s) from this surface, add the new one.
                for e in store.edges_with(Some(EdgeKind::Exposes), Some(&s.id), None)? {
                    store.delete_edge(&e.id)?;
                }
                Some(store.add_edge(EdgeKind::Exposes, &s.id, &c.id, TruthClass::Asserted)?)
            } else {
                None
            };
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "surface": {
                        "id": s.id,
                        "name": s.name,
                        "status": s.status,
                        "body": body,
                    },
                    "exposes_edge": exposes_edge,
                }),
                "loom status",
                format!("updated surface '{}'", s.name),
            )?;
            Ok(())
        }
        SurfaceCmd::Delete { key } => {
            let n = store.resolve_node(&key, Some(NodeType::InterfaceSurface))?;
            store.delete_node(&n.id)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "deleted": true,
                    "surface": node_json(&n),
                }),
                "loom status",
                format!("deleted surface '{}'", n.name),
            )?;
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
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "term": term,
                    "why": why,
                }),
                "loom status",
                format!("registered vocab term '{term}'"),
            )?;
            Ok(())
        }
        VocabCmd::Remove { term } => {
            store.remove_vocab_term(&term)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "removed": true,
                    "term": term,
                }),
                "loom status",
                format!("removed vocab term '{term}' (and untagged any nodes carrying it)"),
            )?;
            Ok(())
        }
        VocabCmd::Rename { from, to, reason } => {
            if reason.trim().is_empty() {
                bail!("vocab rename needs substantive --reason");
            }
            let from = from.trim();
            let to = to.trim();
            if from.is_empty() || to.is_empty() {
                bail!("vocab terms must not be empty");
            }
            if from == to {
                bail!("vocab rename needs distinct <from> and <to> terms");
            }
            let terms = store.list_vocab()?;
            let from_why = terms
                .iter()
                .find(|(term, _)| term == from)
                .map(|(_, why)| why.clone())
                .ok_or_else(|| anyhow!("no vocab term '{from}'"))?;
            let to_existing = terms.iter().any(|(term, _)| term == to);
            if !to_existing {
                store.add_vocab_term(to, &from_why)?;
            }
            let tags = store.snapshot()?.tags;
            let mut retagged = 0usize;
            for tag in tags.iter().filter(|tag| tag.term == from) {
                store.set_tag(&tag.target_id, tag.target_kind, to)?;
                store.remove_tag(&tag.target_id, tag.target_kind, from)?;
                retagged += 1;
            }
            store.remove_vocab_term(from)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "from": from,
                    "to": to,
                    "merged": to_existing,
                    "retagged": retagged,
                    "reason": reason,
                }),
                "loom status",
                format!("renamed vocab term '{from}' → '{to}' ({retagged} tag(s) moved)"),
            )?;
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
