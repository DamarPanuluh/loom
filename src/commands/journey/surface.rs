use super::*;

/// Emit the reusable CLI-surface contract packet. This function is read-only.
pub(crate) fn journey_surface(
    graph: Option<&Path>,
    journey_key: &str,
    json_output: bool,
) -> Result<()> {
    let store = open_read(graph)?;
    let (journey, spec, semantic_hash) = load_registered_journey(&store, journey_key)?;
    let mut derivations = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Derives), Some(&journey.id), None)? {
        if let Some(intent) = store.get_node(&edge.to_id)? {
            derivations.push(json!({
                "intent": node_json(&intent),
                "step_ids": edge_json_facet(&store, &edge.id, "step_ids")
            }));
        }
    }
    let mut surfaces = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)? {
        if let Some(surface) = store.get_node(&edge.to_id)? {
            surfaces.push(json!({
                "surface": node_json(&surface),
                "setup": edge_json_facet(&store, &edge.id, "setup"),
                "operation_bindings": edge_json_facet(&store, &edge.id, "operation_bindings")
            }));
        }
    }
    let packet = json!({
        "mode": "surface",
        "journey": node_json(&journey),
        "semantic_hash": semantic_hash,
        "spec": spec.canonical_value()?,
        "accepted_derivations": derivations,
        "existing_surfaces": surfaces,
        "manifest_contract": crate::journey::surface_contract_template(&spec)?,
        "human_decision_binding_contract": {
            "step_id": "authored-human-step-id",
            "human_decision": {
                "operation_id": "prior-bound-presentation-operation-id",
                "pointer": "/work_item"
            }
        },
        "rules": [
            "Bind every authored step exactly once.",
            "Each binding is a strict union: either operation_id, or human_decision naming a prior bound operation and JSON pointer; never both.",
            "A human_decision binding requires graph=local_snapshot, carries no CLI operation or answer, and may leave setup.operations empty.",
            "Operations are reusable structured argv, never shell strings.",
            "The surface exposes exactly one registered CodeFile at a live symbol locator or globally unique attached anchor:<id>.",
            "Optional operation.exercises declare downstream code entries reached through that public operation; they are not additional surface owners and require observed_by to name an assertion in the same operation.",
            "Replace only repository-specific CodeFile keys and locators in the template before acceptance. Operations and bindings are generated from the authored Journey steps.",
            "Source anchors are navigation-only and never prove behavior or create graph relationships.",
            "Every operation emits JSON; do not include HTTP endpoints.",
            "Carry temporary setup from the Journey profile as declarative data only.",
            "When setup is required, bind ordered mutable operations to graph=local_snapshot; the runtime confines every operation to that clone.",
            "Optional git mode=isolated_snapshot may name only nonempty unique registered CodeFile paths; its one-commit fixture and dirty state exist only in the local snapshot.",
            "Optional setup.before_steps maps authored step ids to atomic registered-file transitions in the local snapshot; each action declares expected_hash and exactly one of content or template.",
            "A before_steps template may interpolate only non-secret inputs, run.id, or non-redacted outputs captured by earlier authored steps.",
            "An argv token may be exactly one non-secret scalar ${{ inputs.<id> }} or ${{ steps.<prior-step>.outputs.<id> }} template; mixed token interpolation is forbidden.",
            "Return only a loom.journey.surface/v1 JSON manifest."
        ],
        "accept_command": format!(
            "loom journey surface-accept {} --manifest <manifest.json>",
            spec.id
        )
    });
    emit_packet(&packet, json_output)
}

/// Atomically create/reuse an InterfaceSurface and bind the Journey to it.
pub(crate) fn journey_surface_accept(
    graph: Option<&Path>,
    journey_key: &str,
    manifest_path: &Path,
    json_output: bool,
) -> Result<()> {
    let manifest = crate::journey::SurfaceManifest::parse_json(manifest_path)?;
    let store = open(graph)?;
    let (journey, spec, semantic_hash) = load_registered_journey(&store, journey_key)?;
    manifest.validate_for(&spec, &semantic_hash)?;
    manifest.validate_setup_for_store(&store)?;
    let lint = manifest.lint(&store, &spec, &manifest_path.to_string_lossy())?;
    if let Some(finding) = lint
        .findings
        .iter()
        .find(|finding| finding.severity == crate::journey::JourneyLintSeverity::Blocking)
    {
        let location = finding
            .operation
            .as_deref()
            .map(|operation| format!(" operation '{operation}'"))
            .unwrap_or_default();
        bail!(
            "surface lint blocked acceptance: {}{}: {}",
            finding.rule,
            location,
            finding.message
        );
    }

    let (surface, exposes, created, removed_edges) = {
        let tx = store.begin()?;
        let (surface, exposes, created) =
            super::domain_cmd::create_or_reuse_interface_surface(&store, &manifest.surface)?;
        // loom-stability-exempt: local surface trust state is not a proof verdict.
        store.set_node_status(&surface.id, "declared")?;
        let surface = store
            .get_node(&surface.id)?
            .ok_or_else(|| anyhow!("InterfaceSurface vanished during local authorization"))?;
        let edge = store.ensure_edge(EdgeKind::Surfaces, &journey.id, &surface.id)?;
        store.set_facet(
            &edge.id,
            TargetKind::Edge,
            "journey_hash",
            &semantic_hash,
            TruthClass::Asserted,
        )?;
        match manifest.canonical_setup()? {
            Some(setup) => store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "setup",
                &serde_json::to_string(&setup)?,
                TruthClass::Asserted,
            )?,
            None => store.clear_facet(&edge.id, TargetKind::Edge, "setup")?,
        }
        let step_hashes = spec.step_hashes()?;
        let binding_hashes: BTreeMap<&str, String> = manifest
            .bindings
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
        store.set_facet(
            &edge.id,
            TargetKind::Edge,
            "operation_bindings",
            &serde_json::to_string(&manifest.canonical_bindings(&spec))?,
            TruthClass::Asserted,
        )?;
        let mut removed_edges = 0usize;
        for other in store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)? {
            if other.id != edge.id {
                store.delete_edge(&other.id)?;
                removed_edges += 1;
            }
        }
        tx.commit()?;
        (surface, exposes, created, removed_edges)
    };

    pulse::emit_line(
        &store,
        json_output,
        json!({
            "accepted": true,
            "journey_id": journey.name,
            "journey_hash": semantic_hash,
            "surface": node_json(&surface),
            "surface_created": created,
            "exposes_edge": exposes,
            "setup": manifest.canonical_setup()?,
            "operation_bindings": manifest.canonical_bindings(&spec),
            "removed_projection_edges": removed_edges,
        }),
        "loom status",
        format!(
            "accepted surface '{}' for Journey '{}'",
            surface.name, journey.name
        ),
    )
}
