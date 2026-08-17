use super::*;

pub(crate) fn surface(graph: Option<&Path>, cmd: SurfaceCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        SurfaceCmd::Add {
            name,
            kind,
            identity,
            codefile,
        } => surface_add(&store, json, name, kind, identity, codefile),
        SurfaceCmd::Show { key } => surface_show(&store, json, key),
        SurfaceCmd::Update {
            key,
            kind,
            identity,
            codefile,
        } => surface_update(&store, json, key, kind, identity, codefile),
        SurfaceCmd::Remove { key, reason } => surface_remove(&store, json, key, reason),
        SurfaceCmd::List { limit, offset } => surface_list(&store, json, limit, offset),
        SurfaceCmd::Gaps => surface_gaps(&store, json),
    }
}

fn surface_add(
    store: &Store,
    json: bool,
    name: String,
    kind: String,
    identity: String,
    codefile: Option<String>,
) -> Result<()> {
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
        store,
        json,
        serde_json::json!({
            "surface": node_json(&s),
            "exposes_edge": exposes_edge,
        }),
        "loom status",
        format!(
            "declared surface '{}' [{}]",
            s.name,
            crate::model::short(&s.id)
        ),
    )
}

fn surface_show(store: &Store, json: bool, key: String) -> Result<()> {
    let n = store.resolve_node(&key, Some(NodeType::InterfaceSurface))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&node_json(&n))?);
    } else {
        println!("{} [{}]", n.name, n.id);
        println!("  {}", n.body);
    }
    Ok(())
}

fn surface_update(
    store: &Store,
    json: bool,
    key: String,
    kind: Option<String>,
    identity: Option<String>,
    codefile: Option<String>,
) -> Result<()> {
    let s = store.resolve_node(&key, Some(NodeType::InterfaceSurface))?;
    // Resolve EVERY endpoint before the first mutation: a bad
    // `--codefile` used to land the body edit and then fail, leaving a
    // rejected write half-committed.
    let resolved_codefile = codefile
        .as_ref()
        .map(|cf| store.resolve_node(cf, Some(NodeType::CodeFile)))
        .transpose()?;
    let mut body = s.body.clone();
    if let Some(k) = &kind {
        body["kind"] = serde_json::json!(k);
    }
    if let Some(id) = &identity {
        body["identity"] = serde_json::json!(id);
    }
    store.set_node_body(&s.id, &body)?;
    let exposes_edge = if let Some(c) = resolved_codefile {
        // re-bind: drop the old exposes edge(s) from this surface, add the new one.
        for e in store.edges_with(Some(EdgeKind::Exposes), Some(&s.id), None)? {
            store.delete_edge(&e.id)?;
        }
        Some(store.add_edge(EdgeKind::Exposes, &s.id, &c.id, TruthClass::Asserted)?)
    } else {
        None
    };
    pulse::emit_line(
        store,
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
    )
}

fn surface_remove(store: &Store, json: bool, key: String, reason: String) -> Result<()> {
    if crate::model::is_placeholder(&reason) {
        bail!("surface remove needs substantive --reason");
    }
    let n = store.resolve_node(&key, Some(NodeType::InterfaceSurface))?;
    let tx = store.begin()?;
    store.delete_node(&n.id)?;
    store.append_journal(
        "node_removed",
        &n.id,
        serde_json::json!({
            "kind": "interface_surface",
            "name": n.name,
            "reason": reason,
        }),
    )?;
    tx.commit()?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "removed": true,
            "surface": node_json(&n),
            "reason": reason,
        }),
        "loom status",
        format!("removed surface '{}'", n.name),
    )
}

fn surface_list(store: &Store, json: bool, limit: usize, offset: usize) -> Result<()> {
    let surfaces = store.list_nodes_page(Some(NodeType::InterfaceSurface), limit, offset)?;
    let total = store.count_nodes(Some(NodeType::InterfaceSurface))?;
    if json {
        let rows: Vec<_> = surfaces.iter().map(node_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&pagination_envelope(&rows, offset, limit, total))?
        );
    } else {
        let shown = surfaces.len();
        for n in surfaces {
            println!("{} [{}]", n.name, crate::model::short(&n.id));
        }
        if let Some(footer) = page_footer(shown, offset, total) {
            println!("{footer}");
        }
    }
    Ok(())
}

fn surface_gaps(store: &Store, json: bool) -> Result<()> {
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

/// Create or reuse the canonical reusable CLI surface described by a Journey
/// surface manifest. The caller owns the surrounding transaction.
///
/// All fallible references are resolved before the first mutation. Reusing a
/// stable surface id revises the canonical body in place so operations can
/// grow without `surface remove`. Switching the exposed CodeFile is still
/// refused: that would silently retarget every Journey already projected
/// onto the surface.
pub(crate) fn create_or_reuse_interface_surface(
    store: &Store,
    definition: &crate::journey::InterfaceSurfaceDefinition,
) -> Result<(crate::model::Node, Option<crate::model::Edge>, bool)> {
    definition.validate()?;
    let wanted_body = definition.node_body()?;
    let codefile = store.resolve_node(&definition.codefile, Some(NodeType::CodeFile))?;
    crate::locator::validate_for_codefile(store, &codefile, &definition.locator).with_context(
        || {
            format!(
                "interface surface '{}' locator '{}' does not resolve in CodeFile '{}'",
                definition.id, definition.locator, codefile.name
            )
        },
    )?;

    let mut matches: Vec<_> = store
        .list_nodes(Some(NodeType::InterfaceSurface), usize::MAX)?
        .into_iter()
        .filter(|surface| {
            surface.name == definition.id
                || surface
                    .body
                    .get("stable_id")
                    .and_then(serde_json::Value::as_str)
                    == Some(definition.id.as_str())
        })
        .collect();
    if matches.len() > 1 {
        bail!(
            "interface surface stable id '{}' is ambiguous ({} nodes)",
            definition.id,
            matches.len()
        );
    }

    let (surface, created) = match matches.pop() {
        Some(surface) => {
            if surface.body != wanted_body {
                store.set_node_body(&surface.id, &wanted_body)?;
            }
            if surface.description != definition.title {
                store.update_node(&surface.id, None, Some(&definition.title), None)?;
            }
            let surface = store.get_node(&surface.id)?.ok_or_else(|| {
                anyhow!(
                    "interface surface '{}' disappeared during reuse",
                    definition.id
                )
            })?;
            (surface, false)
        }
        None => (
            store.add_node(
                NodeType::InterfaceSurface,
                &definition.id,
                &definition.title,
                "declared",
                wanted_body,
            )?,
            true,
        ),
    };

    let existing_exposes = store.edges_with(Some(EdgeKind::Exposes), Some(&surface.id), None)?;
    if existing_exposes
        .iter()
        .any(|edge| edge.to_id != codefile.id)
    {
        bail!(
            "interface surface '{}' already exposes a different CodeFile",
            definition.id
        );
    }
    let exposes = store.ensure_edge(EdgeKind::Exposes, &surface.id, &codefile.id)?;
    store.set_facet(
        &exposes.id,
        TargetKind::Edge,
        "locator",
        &definition.locator,
        TruthClass::Asserted,
    )?;
    Ok((surface, Some(exposes), created))
}
