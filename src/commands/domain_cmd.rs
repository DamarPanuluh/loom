//! Domain command family — hypotheses, interface surfaces, vocabulary, layers.
//!
//! Plane: CLI surface over asserted domain knowledge (judgment-plane inputs).
//! Owns the human-declared vocabulary the detectors read: hypothesis lifecycle,
//! surface registration, vocab terms, and the layer order that arms the
//! layering detector. Declarations only — this module never records verdicts
//! on behalf of a declaration and never writes derived truth.

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
    let order: Vec<String> = super::read_json_meta(store, "layer_order")?;
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
            outcome,
            evidence,
        } => {
            let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
            let status = match outcome.as_str() {
                "supported" => "supported",
                "refuted" => "refuted",
                other => bail!("unknown outcome '{other}' (use supported|refuted)"),
            };
            if crate::model::is_placeholder(&evidence) {
                bail!("{status} verdict requires substantive evidence (not a placeholder like '…' or '<reason>')");
            }
            store.set_node_status(&h.id, status)?;
            store.add_note(&h.id, "decision", &format!("{status}: {evidence}"))?;
            // Teach the follow-through where it is most needed: a supported
            // claim is not work until adopted (nothing re-queues it), so point
            // straight at adoption; a refuted claim stands as an honest record.
            let next_step = if status == "supported" {
                format!(
                    "loom hypothesis adopt {} — promotes the proven idea to build work (optionally add --spawned '<behavioral intent name>' to rename the spawned intent)",
                    crate::workitem::q(&h.name)
                )
            } else {
                "loom status  (the refuted claim stands as an honest record — no adoption)"
                    .to_string()
            };
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
                &next_step,
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
            let name = spawned.unwrap_or_else(|| format!("{} (adopted)", h.name));
            if name.trim().is_empty() {
                bail!("adopted intent name must be non-empty");
            }
            if looks_like_symbol(&name) && h.description.trim().is_empty() {
                bail!(
                    "intent name '{name}' looks like a code symbol. Hypothesis adoption \
                     requires a non-empty hypothesis description for symbol-like intent names."
                );
            }
            // The experiment record must survive the handoff: the spawned
            // intent's build packet reaches notes, not the hypothesis body,
            // so copy proposal/prediction/evidence onto the intent itself.
            let proposal = h
                .body
                .get("proposal")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let predicted = h
                .body
                .get("predicted_outcome")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let evidence = store
                .notes_for(&h.id)?
                .into_iter()
                .find_map(|n| {
                    (n.status == "decision")
                        .then_some(n.description)
                        .and_then(|d| d.strip_prefix("supported: ").map(str::to_string))
                })
                .unwrap_or_else(|| "(proof evidence unavailable)".into());
            store.set_node_status(&h.id, "adopted")?;
            let intent = store.add_node(
                NodeType::Intent,
                &name,
                &h.description,
                "planned",
                serde_json::json!({ "level": "feature" }),
            )?;
            store.set_facet(
                &intent.id,
                TargetKind::Node,
                "visibility",
                "internal",
                TruthClass::Asserted,
            )?;
            store.add_note(
                &intent.id,
                "decision",
                &format!(
                    "adopted from hypothesis '{}' [{}] — proposal: {proposal}; predicted: {predicted}; evidence: {evidence}",
                    h.name,
                    &h.id[..8]
                ),
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
        HypothesisCmd::List { limit, offset } => {
            let hypotheses = store.list_nodes_page(Some(NodeType::Hypothesis), limit, offset)?;
            let total = store.count_nodes(Some(NodeType::Hypothesis))?;
            if json {
                let rows: Vec<_> = hypotheses.iter().map(node_json).collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&super::pagination_envelope(
                        &rows, offset, limit, total
                    ))?
                );
            } else {
                let shown = hypotheses.len();
                for n in hypotheses {
                    println!("{:<10} {} [{}]", n.status, n.name, &n.id[..8]);
                }
                if let Some(footer) = super::page_footer(shown, offset, total) {
                    println!("{footer}");
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
        SurfaceCmd::Remove { key } => {
            let n = store.resolve_node(&key, Some(NodeType::InterfaceSurface))?;
            store.delete_node(&n.id)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "removed": true,
                    "surface": node_json(&n),
                }),
                "loom status",
                format!("removed surface '{}'", n.name),
            )?;
            Ok(())
        }
        SurfaceCmd::List { limit, offset } => {
            let surfaces =
                store.list_nodes_page(Some(NodeType::InterfaceSurface), limit, offset)?;
            let total = store.count_nodes(Some(NodeType::InterfaceSurface))?;
            if json {
                let rows: Vec<_> = surfaces.iter().map(node_json).collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&super::pagination_envelope(
                        &rows, offset, limit, total
                    ))?
                );
            } else {
                let shown = surfaces.len();
                for n in surfaces {
                    println!("{} [{}]", n.name, &n.id[..8]);
                }
                if let Some(footer) = super::page_footer(shown, offset, total) {
                    println!("{footer}");
                }
            }
            Ok(())
        }
        SurfaceCmd::Gaps => {
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
                        "message": format!("surface '{}' is never called by a validation", s.name),
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
                println!("surface plane unmodeled: 0 surfaces declared; no surface-gap analysis possible");
            } else {
                for gap in &gaps {
                    println!("{}", gap["message"].as_str().unwrap_or(""));
                }
                println!(
                    "{} surface gap(s) across {} surface(s)",
                    gaps.len(),
                    surfaces.len()
                );
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
