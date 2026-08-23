use super::*;

pub(crate) fn journey_add(graph: Option<&Path>, spec: PathBuf, json: bool) -> Result<()> {
    let store = open(graph)?;
    // Confinement is the read boundary: reject an out-of-root path before
    // opening or parsing it, even when its contents are malformed.
    let artifact = confined_artifact(&store, &spec)?;
    let parsed = crate::journey::parse(&spec)?;
    let semantic_hash = parsed.semantic_hash()?;
    let body = journey_body(&parsed, &artifact, &semantic_hash);
    let existing = journey_nodes(&store, &parsed.id)?;
    if existing.len() > 1 {
        bail!(
            "journey stable id '{}' is ambiguous ({} nodes)",
            parsed.id,
            existing.len()
        );
    }

    let (journey, added, changed, invalidated) = {
        let tx = store.begin()?;
        let result = match existing.into_iter().next() {
            Some(journey) => {
                let old_hash = journey.body.get("semantic_hash").and_then(Value::as_str);
                let changed = old_hash != Some(semantic_hash.as_str());
                let invalidated = if changed {
                    refresh_or_invalidate_projections(&store, &journey, &parsed, &semantic_hash)?
                } else {
                    0
                };
                if journey.body != body {
                    store.set_node_body(&journey.id, &body)?;
                }
                let current = store
                    .get_node(&journey.id)?
                    .ok_or_else(|| anyhow!("journey vanished during update"))?;
                (current, false, changed, invalidated)
            }
            None => {
                let description = parsed.description.as_deref().unwrap_or(&parsed.goal);
                let journey =
                    store.add_node(NodeType::Journey, &parsed.id, description, "authored", body)?;
                (journey, true, false, 0)
            }
        };
        tx.commit()?;
        result
    };

    pulse::emit_line(
        &store,
        json,
        json!({
            "added": added,
            "updated": !added,
            "changed": changed,
            "invalidated_projections": invalidated,
            "journey": node_json(&journey),
        }),
        &format!("loom journey derive {}", parsed.id),
        if added {
            format!("added Journey '{}'", parsed.id)
        } else {
            format!("updated Journey '{}'", parsed.id)
        },
    )
}

pub(crate) fn journey_remove(graph: Option<&Path>, id: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    let nodes = journey_nodes(&store, id)?;
    let [journey] = nodes.as_slice() else {
        if nodes.is_empty() {
            bail!("no Journey with stable id '{id}'");
        }
        bail!("Journey stable id '{id}' is ambiguous");
    };
    let compiled: Vec<Node> = store
        .edges_with(Some(EdgeKind::Proves), None, Some(&journey.id))?
        .into_iter()
        .filter_map(|edge| store.get_node(&edge.from_id).ok().flatten())
        .filter(|validation| validation.node_type == NodeType::Validation)
        .collect();
    let compiled_count = compiled.len();
    let tx = store.begin()?;
    for validation in compiled {
        store.delete_node(&validation.id)?;
    }
    store.delete_node(&journey.id)?;
    tx.commit()?;
    let cache = store
        .root()
        .join(crate::LOOM_DIR)
        .join("compiled")
        .join("journeys")
        .join(&journey.name);
    if cache.is_dir() {
        std::fs::remove_dir_all(&cache)
            .with_context(|| format!("removing compiled Journey cache {}", cache.display()))?;
    }
    pulse::emit_line(
        &store,
        json,
        json!({
            "removed": true,
            "journey": node_json(journey),
            "removed_compiled_validations": compiled_count,
        }),
        "loom journey list",
        format!("removed Journey '{id}'"),
    )
}

pub(crate) fn journey_list(
    graph: Option<&Path>,
    limit: usize,
    offset: usize,
    json: bool,
) -> Result<()> {
    let store = open_read(graph)?;
    let nodes = store.list_nodes_page(Some(NodeType::Journey), limit, offset)?;
    let total = store.count_nodes(Some(NodeType::Journey))?;
    if json {
        let rows: Vec<_> = nodes.iter().map(node_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&super::pagination_envelope(&rows, offset, limit, total))?
        );
    } else {
        for node in &nodes {
            println!(
                "{}  {}  hash={}",
                crate::model::short(&node.id),
                node.name,
                node.body
                    .get("semantic_hash")
                    .and_then(Value::as_str)
                    .unwrap_or("—")
            );
        }
        if let Some(footer) = super::page_footer(nodes.len(), offset, total) {
            println!("{footer}");
        }
    }
    Ok(())
}

pub(crate) fn journey_show(
    graph: Option<&Path>,
    journey_key: &str,
    json_output: bool,
) -> Result<()> {
    let store = open_read(graph)?;
    let (journey, spec, _) = load_registered_journey(&store, journey_key)?;
    let readiness = crate::completeness::journey_readiness(&store, &journey)?;
    let mut derivations = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Derives), Some(&journey.id), None)? {
        derivations.push(json!({
            "edge": edge,
            "intent": store.get_node(&edge.to_id)?.map(|node| node_json(&node)),
            "journey_hash": store.get_facet(&edge.id, TargetKind::Edge, "journey_hash")?,
            "step_ids": edge_json_facet(&store, &edge.id, "step_ids"),
        }));
    }
    let mut surfaces = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)? {
        surfaces.push(json!({
            "edge": edge,
            "surface": store.get_node(&edge.to_id)?.map(|node| node_json(&node)),
            "journey_hash": store.get_facet(&edge.id, TargetKind::Edge, "journey_hash")?,
            "setup": edge_json_facet(&store, &edge.id, "setup"),
            "operation_bindings": edge_json_facet(&store, &edge.id, "operation_bindings"),
        }));
    }
    let mut proofs = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Proves), None, Some(&journey.id))? {
        proofs.push(json!({
            "edge": edge,
            "validation": store.get_node(&edge.from_id)?.map(|node| node_json(&node)),
        }));
    }
    // Identity first, appendices after. `serde_json`'s Map sorts keys, which
    // buried the journey's name/goal behind an ever-growing `derivations`
    // section — past the bounded run-excerpt head, so an operator recovering
    // the product purpose through a recorded response lost exactly those
    // fields. A struct serializes in declaration order.
    #[derive(serde::Serialize)]
    struct JourneyShow {
        journey: Value,
        spec: Value,
        readiness: crate::completeness::JourneyReadiness,
        derivations: Vec<Value>,
        surfaces: Vec<Value>,
        proofs: Vec<Value>,
    }
    let value = JourneyShow {
        journey: node_json(&journey),
        spec: spec.canonical_value()?,
        readiness,
        derivations,
        surfaces,
        proofs,
    };
    if json_output {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!(
            "{}  authored={} derived={} implemented={} surfaced={} compiled={} proven={}",
            journey.name,
            value.readiness.authored,
            value.readiness.derived,
            value.readiness.implemented,
            value.readiness.surfaced,
            value.readiness.compiled,
            value.readiness.proven
        );
    }
    Ok(())
}

pub(crate) fn journey_map(graph: Option<&Path>, json_output: bool) -> Result<()> {
    let store = open_read(graph)?;
    let readiness = crate::completeness::all_journey_readiness(&store)?;
    let rooted: BTreeSet<String> = readiness
        .iter()
        .flat_map(|journey| journey.derived_intent_ids.iter().cloned())
        .collect();
    let mut unrooted = Vec::new();
    for intent in store.list_nodes(Some(NodeType::Intent), usize::MAX)? {
        if intent.status == "deprecated"
            || rooted.contains(&intent.id)
            || crate::completeness::intent_journey_exempt(&store, &intent.id)?
        {
            continue;
        }
        unrooted.push(node_json(&intent));
    }
    unrooted.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .cmp(&right.get("name").and_then(Value::as_str))
    });
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "journeys": readiness,
                "unrooted_intents": unrooted,
            }))?
        );
    } else {
        for journey in &readiness {
            println!(
                "{}  authored={} derived={} implemented={} surfaced={} compiled={} proven={}",
                journey.journey_name,
                journey.authored,
                journey.derived,
                journey.implemented,
                journey.surfaced,
                journey.compiled,
                journey.proven
            );
        }
        for intent in &unrooted {
            println!(
                "unrooted  {}  {}",
                intent.get("id").and_then(Value::as_str).unwrap_or(""),
                intent.get("name").and_then(Value::as_str).unwrap_or("")
            );
        }
    }
    Ok(())
}
fn journey_body(spec: &crate::journey::JourneySpec, artifact: &str, semantic_hash: &str) -> Value {
    let output_ids = spec.steps.iter().flat_map(|step| {
        step.produces
            .keys()
            .map(move |output| format!("steps.{}.outputs.{output}", step.id))
    });
    json!({
        "schema": crate::journey::JOURNEY_SCHEMA,
        "stable_id": spec.id,
        "name": spec.name,
        "actor": spec.actor,
        "goal": spec.goal,
        "description": spec.description,
        "artifact": artifact,
        "semantic_hash": semantic_hash,
        "step_order_hash": spec.step_order_hash(),
        "step_semantics_hash": spec.step_semantics_hash().expect("validated Journey serializes"),
        "step_hashes": spec.step_hashes().expect("validated Journey serializes"),
        "root_semantics_hash": spec.root_semantics_hash().expect("validated Journey serializes"),
        "input_ids": sorted_ids(spec.inputs.keys().map(String::as_str)),
        "preconditions": spec.preconditions,
        "step_ids": spec.step_ids(),
        "output_ids": output_ids.collect::<Vec<_>>(),
        "profile_ids": sorted_ids(spec.profiles.keys().map(String::as_str)),
    })
}

fn refresh_or_invalidate_projections(
    store: &Store,
    journey: &Node,
    spec: &crate::journey::JourneySpec,
    semantic_hash: &str,
) -> Result<usize> {
    let old_root_hash = journey
        .body
        .get("root_semantics_hash")
        .and_then(Value::as_str);
    let root_hash = spec.root_semantics_hash()?;
    let old_step_hashes: BTreeMap<String, String> = journey
        .body
        .get("step_hashes")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    let step_hashes = spec.step_hashes()?;
    let changed_steps: BTreeSet<String> = old_step_hashes
        .keys()
        .chain(step_hashes.keys())
        .filter(|id| old_step_hashes.get(*id) != step_hashes.get(*id))
        .cloned()
        .collect();
    let global_changed = old_root_hash != Some(root_hash.as_str()) || old_step_hashes.is_empty();
    let all_steps: BTreeSet<&str> = spec.steps.iter().map(|step| step.id.as_str()).collect();
    let order: BTreeMap<&str, usize> = spec
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.id.as_str(), index))
        .collect();
    let mut invalidated = 0;

    for kind in [EdgeKind::Derives, EdgeKind::Surfaces] {
        for edge in store.edges_with(Some(kind), Some(&journey.id), None)? {
            let (mut bound_steps, bindings) = match kind {
                EdgeKind::Derives => {
                    let steps: Vec<String> = edge_json_facet(store, &edge.id, "step_ids")
                        .and_then(|value| serde_json::from_value(value).ok())
                        .unwrap_or_default();
                    (steps, None)
                }
                EdgeKind::Surfaces => {
                    let bindings: Vec<crate::journey::SurfaceBinding> =
                        edge_json_facet(store, &edge.id, "operation_bindings")
                            .and_then(|value| serde_json::from_value(value).ok())
                            .unwrap_or_default();
                    let steps = bindings
                        .iter()
                        .map(|binding| binding.step_id().to_string())
                        .collect();
                    (steps, Some(bindings))
                }
                _ => unreachable!(),
            };
            let bound: BTreeSet<&str> = bound_steps.iter().map(String::as_str).collect();
            let malformed_derivation = kind == EdgeKind::Derives && bound.is_empty();
            let incomplete_surface = kind == EdgeKind::Surfaces && bound != all_steps;
            let touches_changed = changed_steps.iter().any(|id| bound.contains(id.as_str()));
            if global_changed || malformed_derivation || incomplete_surface || touches_changed {
                store.delete_edge(&edge.id)?;
                invalidated += 1;
                continue;
            }

            bound_steps.sort_by_key(|id| order.get(id.as_str()).copied().unwrap_or(usize::MAX));
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "journey_hash",
                semantic_hash,
                TruthClass::Asserted,
            )?;
            let subset_hashes: BTreeMap<&str, &str> = bound_steps
                .iter()
                .filter_map(|id| step_hashes.get(id).map(|hash| (id.as_str(), hash.as_str())))
                .collect();
            match (kind, bindings) {
                (EdgeKind::Derives, _) => {
                    store.set_facet(
                        &edge.id,
                        TargetKind::Edge,
                        "step_ids",
                        &serde_json::to_string(&bound_steps)?,
                        TruthClass::Asserted,
                    )?;
                    store.set_facet(
                        &edge.id,
                        TargetKind::Edge,
                        "step_hashes",
                        &serde_json::to_string(&subset_hashes)?,
                        TruthClass::Asserted,
                    )?;
                }
                (EdgeKind::Surfaces, Some(mut bindings)) => {
                    bindings.sort_by_key(|binding| {
                        order.get(binding.step_id()).copied().unwrap_or(usize::MAX)
                    });
                    store.set_facet(
                        &edge.id,
                        TargetKind::Edge,
                        "operation_bindings",
                        &serde_json::to_string(&bindings)?,
                        TruthClass::Asserted,
                    )?;
                    let binding_hashes: BTreeMap<&str, String> = bindings
                        .iter()
                        .filter_map(|binding| {
                            step_hashes.get(binding.step_id()).map(|step_hash| {
                                (
                                    binding.step_id(),
                                    crate::artifact::fingerprint(&format!(
                                        "{step_hash}\0{}",
                                        binding.identity()
                                    )),
                                )
                            })
                        })
                        .collect();
                    store.set_facet(
                        &edge.id,
                        TargetKind::Edge,
                        "binding_hashes",
                        &serde_json::to_string(&binding_hashes)?,
                        TruthClass::Asserted,
                    )?;
                }
                _ => unreachable!(),
            }
        }
    }
    Ok(invalidated)
}
fn confined_artifact(store: &Store, path: &Path) -> Result<String> {
    let root = store
        .root()
        .canonicalize()
        .with_context(|| format!("resolving graph root {}", store.root().display()))?;
    let artifact = path
        .canonicalize()
        .with_context(|| format!("resolving Journey artifact {}", path.display()))?;
    if !artifact.starts_with(&root) {
        bail!(
            "Journey artifact '{}' is outside graph root {}",
            path.display(),
            store.root().display()
        );
    }
    Ok(artifact.strip_prefix(root)?.to_string_lossy().into_owned())
}
fn sorted_ids<'a>(ids: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut ids: Vec<String> = ids.map(str::to_owned).collect();
    ids.sort();
    ids
}
