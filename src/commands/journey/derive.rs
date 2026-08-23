use super::*;

/// Emit the strict technical-derivation packet. This function is read-only.
pub(crate) fn journey_derive(
    graph: Option<&Path>,
    journey_key: &str,
    candidate_json: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let store = open_read(graph)?;
    let (journey, spec, semantic_hash) = load_registered_journey(&store, journey_key)?;
    let mut existing = Vec::new();
    let mut covered = BTreeSet::new();
    for edge in store.edges_with(Some(EdgeKind::Derives), Some(&journey.id), None)? {
        let Some(intent) = store.get_node(&edge.to_id)? else {
            continue;
        };
        let step_ids = edge_json_facet(&store, &edge.id, "step_ids")
            .unwrap_or_else(|| Value::Array(Vec::new()));
        if let Some(ids) = step_ids.as_array() {
            covered.extend(ids.iter().filter_map(Value::as_str).map(str::to_owned));
        }
        existing.push(json!({
            "intent": node_json(&intent),
            "step_ids": step_ids,
            "rationale": store.get_facet(&edge.id, TargetKind::Edge, "rationale")?,
            "proposal_id": store.get_facet(&edge.id, TargetKind::Edge, "proposal_id")?,
            "manifest_hash": store.get_facet(&edge.id, TargetKind::Edge, "manifest_hash")?,
            "journey_hash": store.get_facet(&edge.id, TargetKind::Edge, "journey_hash")?,
        }));
    }
    let uncovered_step_ids: Vec<_> = spec
        .steps
        .iter()
        .filter(|step| !covered.contains(&step.id))
        .map(|step| step.id.clone())
        .collect();
    let mut packet = json!({
        "mode": "derive",
        "journey": node_json(&journey),
        "semantic_hash": semantic_hash,
        "spec": spec.canonical_value()?,
        "existing_derivations": existing,
        "uncovered_step_ids": uncovered_step_ids,
        "manifest_contract": {
            "schema": crate::journey::DERIVATION_SCHEMA,
            "journey_id": spec.id,
            "journey_hash": semantic_hash,
            "proposal_id": "stable-derivation-proposal-id",
            "proposal_rationale": "Why this exact technical projection is sufficient and minimal",
            "intents": [{
                "id": "stable-technical-intent-id",
                "operation": "create",
                "name": "Behavioral technical intent name",
                "criterion": "Falsifiable technical behavior criterion",
                "level": "feature",
                "visibility": "internal",
                "rationale": "Why this is the smallest independently falsifiable technical behavior for the mapped Journey step(s)",
                "step_ids": ["authored-step-id"]
            }],
            "relationships": [{
                "id": "stable-relationship-id",
                "kind": "requires",
                "from": "stable-technical-intent-id",
                "to": "another-included-create-or-reuse-entry-id",
                "rationale": "Why this dependency or hierarchy is required"
            }],
            "unresolved_question": null
        },
        "rules": [
            "Cover every authored step with at least one technical intent.",
            "Use operation=reuse with intent_id to reuse an existing Intent; every relationship endpoint names an included intent entry id.",
            "Every mapping requires a nonempty rationale.",
            "Declare only requires or hierarchy relationships, each with a stable id and nonempty rationale.",
            "A non-null unresolved_question must be answered before derive-accept.",
            "Do not add product behavior or transport details.",
            "Return only a loom.journey-derivation/v1 JSON manifest."
        ],
        "accept_command": format!(
            "loom journey derive-accept {} --manifest <manifest.json> --human-decision <exact-answer>",
            spec.id
        ),
        "human_gate": crate::workitem::derivation_human_gate(&journey),
        "next_action": "Present the human_gate options with an evidence-backed recommendation, wait for the human's exact answer, and only then run accept_command. Missing human authority is a pause, not a terminal handoff."
    });
    if let Some(candidate_json) = candidate_json {
        let manifest = parse_derivation_candidate(candidate_json)?;
        manifest.validate_for(&spec, &semantic_hash)?;
        packet
            .as_object_mut()
            .expect("derive packet is an object")
            .insert(
                "candidate_state".into(),
                derivation_candidate_state(&store, &journey, &spec, &manifest)?,
            );
    }
    emit_packet(&packet, json_output)
}

fn parse_derivation_candidate(raw: &str) -> Result<crate::journey::DerivationManifest> {
    let trimmed = raw.trim_start();
    let (text, source) = if trimmed.starts_with('{') {
        (raw.to_string(), "inline --candidate-json".to_string())
    } else {
        let path = Path::new(raw);
        (
            std::fs::read_to_string(path).with_context(|| {
                format!("reading candidate derivation manifest {}", path.display())
            })?,
            path.display().to_string(),
        )
    };
    serde_json::from_str(&text)
        .with_context(|| format!("parsing {source} as {}", crate::journey::DERIVATION_SCHEMA))
}

fn derivation_manifest_hash(manifest: &crate::journey::DerivationManifest) -> Result<String> {
    let canonical = canonical_json(serde_json::to_value(manifest)?);
    Ok(crate::artifact::fingerprint(&serde_json::to_string(
        &canonical,
    )?))
}

fn candidate_intent_shape_matches(
    store: &Store,
    item: &crate::journey::DerivedIntent,
    node: &Node,
) -> Result<bool> {
    if item.operation == crate::journey::DerivedIntentOperation::Create
        && (node.name != item.name.as_deref().unwrap_or_default()
            || node.description != item.criterion.as_deref().unwrap_or_default())
    {
        return Ok(false);
    }
    Ok(store
        .get_facet(&node.id, TargetKind::Node, "level")?
        .as_deref()
        == Some(item.level.as_str())
        && store
            .get_facet(&node.id, TargetKind::Node, "visibility")?
            .as_deref()
            == Some(item.visibility.as_str()))
}

fn derivation_candidate_state(
    store: &Store,
    journey: &Node,
    spec: &crate::journey::JourneySpec,
    manifest: &crate::journey::DerivationManifest,
) -> Result<Value> {
    let manifest_hash = derivation_manifest_hash(manifest)?;

    let mut matching_proposals: Vec<_> = derivation_proposals(store, &journey.name)?
        .into_iter()
        .filter(|proposal| {
            proposal.status == "adopted"
                && proposal.body.get("proposal_id").and_then(Value::as_str)
                    == Some(manifest.proposal_id.as_str())
                && proposal.body.get("manifest_hash").and_then(Value::as_str)
                    == Some(manifest_hash.as_str())
                && proposal.body.get("journey_hash").and_then(Value::as_str)
                    == Some(manifest.journey_hash.as_str())
        })
        .collect();
    matching_proposals.sort_by(|a, b| a.id.cmp(&b.id));
    let proposal_ids: BTreeSet<_> = matching_proposals
        .iter()
        .map(|proposal| proposal.id.as_str())
        .collect();

    let mut matched = Vec::new();
    for item in &manifest.intents {
        let intent = match item.operation {
            crate::journey::DerivedIntentOperation::Create => {
                find_derived_intent(store, &journey.name, &item.id)?
            }
            crate::journey::DerivedIntentOperation::Reuse => item
                .intent_id
                .as_deref()
                .and_then(|key| store.resolve_node(key, Some(NodeType::Intent)).ok()),
        };
        if let Some(intent) = intent {
            if candidate_intent_shape_matches(store, item, &intent)? {
                matched.push((item, intent));
            }
        }
    }
    let candidate_ids: BTreeSet<_> = matched
        .iter()
        .map(|(_, intent)| intent.id.as_str())
        .collect();

    let mut derives_edges = Vec::new();
    for (item, intent) in &matched {
        let expected_steps = ordered_subset(spec, &item.step_ids);
        let expected_steps = serde_json::to_string(&expected_steps)?;
        for edge in
            store.edges_with(Some(EdgeKind::Derives), Some(&journey.id), Some(&intent.id))?
        {
            let proposal_id = store.get_facet(&edge.id, TargetKind::Edge, "proposal_id")?;
            if store
                .get_facet(&edge.id, TargetKind::Edge, "journey_hash")?
                .as_deref()
                != Some(manifest.journey_hash.as_str())
                || store
                    .get_facet(&edge.id, TargetKind::Edge, "manifest_hash")?
                    .as_deref()
                    != Some(manifest_hash.as_str())
                || store
                    .get_facet(&edge.id, TargetKind::Edge, "step_ids")?
                    .as_deref()
                    != Some(expected_steps.as_str())
                || store
                    .get_facet(&edge.id, TargetKind::Edge, "rationale")?
                    .as_deref()
                    != Some(item.rationale.as_str())
                || !proposal_id
                    .as_deref()
                    .is_some_and(|id| proposal_ids.contains(id))
            {
                continue;
            }
            derives_edges.push(json!({
                "entry_id": item.id,
                "edge": edge,
                "step_ids": edge_json_facet(store, &edge.id, "step_ids"),
                "step_hashes": edge_json_facet(store, &edge.id, "step_hashes"),
                "rationale": item.rationale,
                "proposal_id": proposal_id,
                "manifest_hash": manifest_hash,
                "journey_hash": manifest.journey_hash,
            }));
        }
    }

    let ratification_facts: Vec<_> = store
        .all_facts()?
        .into_iter()
        .filter(|fact| {
            candidate_ids.contains(fact.subject_id.as_str())
                && fact.claim == crate::model::Claim::Ratification
        })
        .collect();
    let build_queue_entries: Vec<_> =
        crate::workitem::queue_items(store, crate::lane::Lane::Build)?
            .into_iter()
            .filter(|entry| candidate_ids.contains(entry.target.id.as_str()))
            .collect();
    let readiness = crate::completeness::journey_readiness(store, journey)?;
    let readiness_derived_candidate_ids: Vec<_> = readiness
        .derived_intent_ids
        .into_iter()
        .filter(|id| candidate_ids.contains(id.as_str()))
        .collect();

    Ok(json!({
        "canonical_manifest_hash": manifest_hash,
        "matching_adopted_proposals": matching_proposals.iter().map(node_json).collect::<Vec<_>>(),
        "candidate_intent_matches": matched.iter().map(|(item, intent)| json!({
            "entry_id": item.id,
            "operation": item.operation,
            "intent": node_json(intent),
        })).collect::<Vec<_>>(),
        "derives_edges": derives_edges,
        "ratification_facts": ratification_facts,
        "build_queue_entries": build_queue_entries,
        "readiness_derived_candidate_ids": readiness_derived_candidate_ids,
    }))
}

#[derive(Clone)]
struct PlannedRelationship {
    id: String,
    kind: EdgeKind,
    from: String,
    to: String,
    rationale: String,
}

type AcceptedRelationship = (PlannedRelationship, crate::model::Edge);

struct ReconciledRelationships {
    accepted: Vec<AcceptedRelationship>,
    created: usize,
    removed: usize,
}

struct DerivationCurrentContext<'a> {
    journey: &'a Node,
    spec: &'a crate::journey::JourneySpec,
    manifest: &'a crate::journey::DerivationManifest,
    manifest_hash: &'a str,
    proposal: &'a Node,
    intents: &'a BTreeMap<String, Node>,
    relationships: &'a [PlannedRelationship],
}

fn plan_relationships(
    store: &Store,
    manifest: &crate::journey::DerivationManifest,
    preexisting: &BTreeMap<String, Node>,
) -> Result<Vec<PlannedRelationship>> {
    let mut planned = Vec::with_capacity(manifest.relationships.len());
    let mut seen = BTreeSet::new();
    for relationship in &manifest.relationships {
        let kind = match relationship.kind {
            crate::journey::DerivedRelationshipKind::Requires => EdgeKind::Requires,
            crate::journey::DerivedRelationshipKind::Hierarchy => EdgeKind::Hierarchy,
        };
        let from_token = intent_plan_token(&relationship.from, preexisting);
        let to_token = intent_plan_token(&relationship.to, preexisting);
        if from_token == to_token {
            bail!(
                "derivation relationship '{}' resolves both endpoints to the same Intent",
                relationship.kind.as_str()
            );
        }
        let key = relationship_key(kind, &from_token, &to_token);
        if !seen.insert(key) {
            bail!(
                "derivation contains a duplicate resolved '{}' relationship",
                relationship.kind.as_str()
            );
        }
        planned.push(PlannedRelationship {
            id: relationship.id.clone(),
            kind,
            from: relationship.from.clone(),
            to: relationship.to.clone(),
            rationale: relationship.rationale.clone(),
        });
    }
    validate_prospective_relationship_cycles(store, &planned, preexisting)?;
    Ok(planned)
}

fn intent_plan_token(local: &str, preexisting: &BTreeMap<String, Node>) -> String {
    preexisting
        .get(local)
        .map(|node| node.id.clone())
        .unwrap_or_else(|| format!("new:{local}"))
}

fn relationship_key(kind: EdgeKind, from: &str, to: &str) -> String {
    format!("{}\0{from}\0{to}", kind.as_str())
}

fn validate_prospective_relationship_cycles(
    store: &Store,
    planned: &[PlannedRelationship],
    preexisting: &BTreeMap<String, Node>,
) -> Result<()> {
    for kind in [EdgeKind::Requires, EdgeKind::Hierarchy] {
        let mut edges: Vec<(String, String)> = store
            .edges_with(Some(kind), None, None)?
            .into_iter()
            .map(|edge| (edge.from_id, edge.to_id))
            .collect();
        let additions: Vec<(String, String)> = planned
            .iter()
            .filter(|relationship| relationship.kind == kind)
            .map(|relationship| {
                (
                    intent_plan_token(&relationship.from, preexisting),
                    intent_plan_token(&relationship.to, preexisting),
                )
            })
            .collect();
        edges.extend(additions.iter().cloned());
        for (from, to) in additions {
            if command_relationship_path_exists(&to, &from, &edges, &mut BTreeSet::new()) {
                bail!(
                    "accepting derivation would create a '{}' relationship cycle through '{}' and '{}'",
                    kind.as_str(),
                    from,
                    to
                );
            }
        }
    }
    Ok(())
}

fn command_relationship_path_exists(
    current: &str,
    target: &str,
    edges: &[(String, String)],
    seen: &mut BTreeSet<String>,
) -> bool {
    if current == target {
        return true;
    }
    if !seen.insert(current.to_string()) {
        return false;
    }
    edges
        .iter()
        .filter(|(from, _)| from == current)
        .any(|(_, to)| command_relationship_path_exists(to, target, edges, seen))
}

fn validate_reused_intent_shape(
    store: &Store,
    item: &crate::journey::DerivedIntent,
    node: &Node,
) -> Result<()> {
    let level = store.get_facet(&node.id, TargetKind::Node, "level")?;
    let visibility = store.get_facet(&node.id, TargetKind::Node, "visibility")?;
    if level.as_deref() != Some(item.level.as_str())
        || visibility.as_deref() != Some(item.visibility.as_str())
    {
        bail!(
            "derived intent '{}' declares level='{}' visibility='{}', but reused Intent '{}' does not match",
            item.id,
            item.level,
            item.visibility,
            node.name
        );
    }
    Ok(())
}

fn derivation_proposals(store: &Store, journey_id: &str) -> Result<Vec<Node>> {
    Ok(store
        .list_nodes(Some(NodeType::Proposal), usize::MAX)?
        .into_iter()
        .filter(|proposal| {
            proposal.body.get("source").and_then(Value::as_str) == Some("journey_derivation")
                && proposal.body.get("journey_id").and_then(Value::as_str) == Some(journey_id)
        })
        .collect())
}

fn proposal_body(
    manifest: &crate::journey::DerivationManifest,
    manifest_hash: &str,
    journey: &Node,
    decision: &crate::ratification::HumanDecision,
    accepted: &[(crate::journey::DerivedIntent, Node)],
    relationships: &[AcceptedRelationship],
) -> Result<Value> {
    let canonical_manifest = canonical_json(serde_json::to_value(manifest)?);
    let raw = serde_json::to_string(&canonical_manifest)?;
    Ok(json!({
        "source": "journey_derivation",
        "source_path": Value::Null,
        "raw": raw,
        "proposal_id": manifest.proposal_id,
        "proposal_rationale": manifest.proposal_rationale,
        "journey_id": journey.name,
        "journey_node_id": journey.id,
        "journey_hash": manifest.journey_hash,
        "manifest_hash": manifest_hash,
        "human_decision": decision,
        "items": accepted.iter().enumerate().map(|(index, (item, node))| json!({
            "number": index + 1,
            "kind": "journey_intent",
            "status": "adopted",
            "text": item.rationale,
            "intent_entry_id": item.id,
            "operation": item.operation,
            "step_ids": item.step_ids,
            "spawned": node.id,
        })).collect::<Vec<_>>(),
        "relationships": relationships.iter().map(|(relationship, edge)| json!({
            "id": relationship.id,
            "kind": relationship.kind.as_str(),
            "from": relationship.from,
            "to": relationship.to,
            "rationale": relationship.rationale,
            "edge_id": edge.id,
        })).collect::<Vec<_>>(),
    }))
}

fn relationship_bindings(store: &Store, edge_id: &str) -> Result<BTreeMap<String, Value>> {
    store
        .get_facet(edge_id, TargetKind::Edge, "journey_derivation_bindings")?
        .map(|raw| serde_json::from_str(&raw).context("decoding relationship ownership"))
        .transpose()
        .map(|value| value.unwrap_or_default())
}

fn reconcile_derivation_relationships(
    store: &Store,
    journey_proposals: &[Node],
    proposal: &Node,
    manifest_hash: &str,
    relationships: &[PlannedRelationship],
    intents: &BTreeMap<String, Node>,
) -> Result<ReconciledRelationships> {
    let replacing: BTreeSet<&str> = journey_proposals
        .iter()
        .map(|proposal| proposal.id.as_str())
        .chain(std::iter::once(proposal.id.as_str()))
        .collect();
    let mut declared = BTreeMap::new();
    for relationship in relationships {
        let from = intents.get(&relationship.from).ok_or_else(|| {
            anyhow!(
                "relationship from intent '{}' was not resolved",
                relationship.from
            )
        })?;
        let to = intents.get(&relationship.to).ok_or_else(|| {
            anyhow!(
                "relationship to intent '{}' was not resolved",
                relationship.to
            )
        })?;
        declared.insert(
            relationship_key(relationship.kind, &from.id, &to.id),
            relationship.clone(),
        );
    }

    let mut accepted = Vec::new();
    let mut seen = BTreeSet::new();
    let mut removed = 0usize;
    for kind in [EdgeKind::Requires, EdgeKind::Hierarchy] {
        for edge in store.edges_with(Some(kind), None, None)? {
            let key = relationship_key(kind, &edge.from_id, &edge.to_id);
            let wanted = declared.get(&key);
            let mut bindings = relationship_bindings(store, &edge.id)?;
            bindings.retain(|owner, _| !replacing.contains(owner.as_str()));
            if let Some(relationship) = wanted {
                bindings.insert(
                    proposal.id.clone(),
                    json!({
                        "relationship_id": relationship.id,
                        "rationale": relationship.rationale,
                        "manifest_hash": manifest_hash,
                    }),
                );
                accepted.push((relationship.clone(), edge.clone()));
                seen.insert(key.clone());
            }
            if bindings.is_empty() {
                let created_by_derivation = store
                    .get_facet(&edge.id, TargetKind::Edge, "journey_derivation_created")?
                    .as_deref()
                    == Some("true");
                if wanted.is_none() && created_by_derivation {
                    store.delete_edge(&edge.id)?;
                    removed += 1;
                } else {
                    store.clear_facet(&edge.id, TargetKind::Edge, "journey_derivation_bindings")?;
                }
            } else {
                store.set_facet(
                    &edge.id,
                    TargetKind::Edge,
                    "journey_derivation_bindings",
                    &serde_json::to_string(&bindings)?,
                    TruthClass::Asserted,
                )?;
            }
        }
    }

    let mut created = 0usize;
    for (key, relationship) in declared {
        if seen.contains(&key) {
            continue;
        }
        let from = &intents[&relationship.from];
        let to = &intents[&relationship.to];
        let edge = store.add_edge(relationship.kind, &from.id, &to.id, TruthClass::Asserted)?;
        store.set_facet(
            &edge.id,
            TargetKind::Edge,
            "journey_derivation_created",
            "true",
            TruthClass::Asserted,
        )?;
        store.set_facet(
            &edge.id,
            TargetKind::Edge,
            "journey_derivation_bindings",
            &serde_json::to_string(&BTreeMap::from([(
                proposal.id.clone(),
                json!({
                    "relationship_id": relationship.id,
                    "rationale": relationship.rationale,
                    "manifest_hash": manifest_hash,
                }),
            )]))?,
            TruthClass::Asserted,
        )?;
        accepted.push((relationship, edge));
        created += 1;
    }
    accepted.sort_by(|left, right| left.0.id.cmp(&right.0.id));
    Ok(ReconciledRelationships {
        accepted,
        created,
        removed,
    })
}

fn derivation_acceptance_is_current(
    store: &Store,
    context: &DerivationCurrentContext<'_>,
) -> Result<bool> {
    let DerivationCurrentContext {
        journey,
        spec,
        manifest,
        manifest_hash,
        proposal,
        intents,
        relationships,
    } = context;
    if proposal.status != "adopted"
        || proposal.body.get("manifest_hash").and_then(Value::as_str) != Some(manifest_hash)
        || proposal.body.get("journey_hash").and_then(Value::as_str)
            != Some(manifest.journey_hash.as_str())
        || intents.len() != manifest.intents.len()
    {
        return Ok(false);
    }
    let derives = store.edges_with(Some(EdgeKind::Derives), Some(&journey.id), None)?;
    if derives.len() != manifest.intents.len() {
        return Ok(false);
    }
    let step_hashes = spec.step_hashes()?;
    for item in &manifest.intents {
        let Some(intent) = intents.get(&item.id) else {
            return Ok(false);
        };
        if !super::intent::is_ratified(store, &intent.id)? {
            return Ok(false);
        }
        let Some(edge) = derives.iter().find(|edge| edge.to_id == intent.id) else {
            return Ok(false);
        };
        let ordered_steps = ordered_subset(spec, &item.step_ids);
        let subset: BTreeMap<&str, &str> = ordered_steps
            .iter()
            .filter_map(|id| step_hashes.get(id).map(|hash| (id.as_str(), hash.as_str())))
            .collect();
        if store
            .get_facet(&edge.id, TargetKind::Edge, "journey_hash")?
            .as_deref()
            != Some(manifest.journey_hash.as_str())
            || edge_json_facet(store, &edge.id, "step_ids")
                != Some(serde_json::to_value(&ordered_steps)?)
            || edge_json_facet(store, &edge.id, "step_hashes")
                != Some(serde_json::to_value(&subset)?)
            || store
                .get_facet(&edge.id, TargetKind::Edge, "rationale")?
                .as_deref()
                != Some(item.rationale.as_str())
            || store
                .get_facet(&edge.id, TargetKind::Edge, "proposal_id")?
                .as_deref()
                != Some(proposal.id.as_str())
            || store
                .get_facet(&edge.id, TargetKind::Edge, "manifest_hash")?
                .as_deref()
                != Some(manifest_hash)
        {
            return Ok(false);
        }
    }

    let mut declared = BTreeSet::new();
    for relationship in *relationships {
        let from = &intents[&relationship.from];
        let to = &intents[&relationship.to];
        let key = relationship_key(relationship.kind, &from.id, &to.id);
        declared.insert(key);
        let Some(edge) = store
            .edges_with(Some(relationship.kind), Some(&from.id), Some(&to.id))?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };
        let bindings = relationship_bindings(store, &edge.id)?;
        let expected = json!({
            "relationship_id": relationship.id,
            "rationale": relationship.rationale,
            "manifest_hash": manifest_hash,
        });
        if bindings.get(&proposal.id) != Some(&expected) {
            return Ok(false);
        }
    }
    for kind in [EdgeKind::Requires, EdgeKind::Hierarchy] {
        for edge in store.edges_with(Some(kind), None, None)? {
            if relationship_bindings(store, &edge.id)?.contains_key(&proposal.id)
                && !declared.contains(&relationship_key(kind, &edge.from_id, &edge.to_id))
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Atomically accept a human-approved technical derivation manifest.
pub(crate) fn journey_derive_accept(
    graph: Option<&Path>,
    journey_key: &str,
    manifest_path: &Path,
    human_decision: String,
    json_output: bool,
) -> Result<()> {
    let manifest = crate::journey::DerivationManifest::parse_json(manifest_path)?;
    let store = open(graph)?;
    let (journey, spec, semantic_hash) = load_registered_journey(&store, journey_key)?;
    manifest.validate_for(&spec, &semantic_hash)?;
    if let Some(question) = &manifest.unresolved_question {
        bail!(
            "derivation question '{}' is unresolved: {}; answer it and submit a manifest with unresolved_question:null",
            question.id,
            question.text
        );
    }
    let decision = super::ratification_decision(&journey.name, Some(human_decision))?;
    let manifest_hash = derivation_manifest_hash(&manifest)?;
    let evidence = format!(
        "accepted Journey '{}' derivation at semantic hash {} from canonical manifest {}",
        journey.name, semantic_hash, manifest_hash
    );

    // Resolve all references and provenance replays before the first mutation.
    let mut preexisting = BTreeMap::new();
    let mut resolved_intent_ids = BTreeSet::new();
    for item in &manifest.intents {
        let node = match item.operation {
            crate::journey::DerivedIntentOperation::Reuse => store.resolve_node(
                item.intent_id.as_deref().unwrap_or(""),
                Some(NodeType::Intent),
            )?,
            crate::journey::DerivedIntentOperation::Create => {
                match find_derived_intent(&store, &journey.name, &item.id)? {
                    Some(node) => {
                        if node.name != item.name.as_deref().unwrap_or("")
                            || node.description != item.criterion.as_deref().unwrap_or("")
                        {
                            bail!(
                                "derived intent '{}' already exists with conflicting meaning",
                                item.id
                            );
                        }
                        node
                    }
                    None => continue,
                }
            }
        };
        validate_reused_intent_shape(&store, item, &node)?;
        if !resolved_intent_ids.insert(node.id.clone()) {
            bail!(
                "derived intent '{}' resolves to Intent '{}' already selected by another mapping",
                item.id,
                node.name
            );
        }
        preexisting.insert(item.id.clone(), node);
    }
    let relationships = plan_relationships(&store, &manifest, &preexisting)?;
    let proposals = derivation_proposals(&store, &journey.name)?;
    if let Some(conflict) = proposals.iter().find(|proposal| {
        proposal.body.get("proposal_id").and_then(Value::as_str)
            == Some(manifest.proposal_id.as_str())
            && proposal.body.get("manifest_hash").and_then(Value::as_str)
                != Some(manifest_hash.as_str())
    }) {
        bail!(
            "derivation proposal id '{}' is already adopted by Proposal '{}' with a different manifest",
            manifest.proposal_id,
            conflict.id
        );
    }
    let mut matching: Vec<Node> = proposals
        .iter()
        .filter(|proposal| {
            proposal.body.get("proposal_id").and_then(Value::as_str)
                == Some(manifest.proposal_id.as_str())
                && proposal.body.get("manifest_hash").and_then(Value::as_str)
                    == Some(manifest_hash.as_str())
        })
        .cloned()
        .collect();
    if matching.len() > 1 {
        bail!(
            "Journey '{}' has {} adopted Proposal records for manifest '{}'",
            journey.name,
            matching.len(),
            manifest_hash
        );
    }
    let existing_proposal = matching.pop();

    if let Some(proposal) = &existing_proposal {
        let current = DerivationCurrentContext {
            journey: &journey,
            spec: &spec,
            manifest: &manifest,
            manifest_hash: &manifest_hash,
            proposal,
            intents: &preexisting,
            relationships: &relationships,
        };
        if derivation_acceptance_is_current(&store, &current)? {
            let accepted_nodes: Vec<&Node> = manifest
                .intents
                .iter()
                .filter_map(|item| preexisting.get(&item.id))
                .collect();
            let accepted_ids: Vec<String> =
                accepted_nodes.iter().map(|node| node.id.clone()).collect();
            // Cold import keeps the adopted mapping but drops local journal
            // envelopes. Re-seal the exact derive-accept witness without
            // treating that as a new product decision.
            if !local_derive_accept_envelope_exists(&store, &accepted_ids)? {
                ratification_batch(
                    &store,
                    &accepted_nodes,
                    &evidence,
                    &decision,
                    &semantic_hash,
                )?;
            }
            let accepted: Vec<_> = accepted_nodes.iter().copied().map(node_json).collect();
            return pulse::emit_line(
                &store,
                json_output,
                json!({
                    "accepted": true,
                    "idempotent": true,
                    "journey_id": journey.name,
                    "journey_hash": semantic_hash,
                    "manifest_hash": manifest_hash,
                    "proposal": node_json(proposal),
                    "intents": accepted,
                    "created": 0,
                    "relationships_created": 0,
                    "removed_projection_edges": 0,
                    "removed_relationship_edges": 0,
                }),
                &format!("loom journey surface {}", journey.name),
                format!("derivation for '{}' is already accepted", journey.name),
            );
        }
    }

    let (accepted, proposal, created, removed_edges, relationships_created, relationships_removed) = {
        let tx = store.begin()?;
        let mut accepted: Vec<(crate::journey::DerivedIntent, Node)> = Vec::new();
        let mut accepted_by_local = BTreeMap::new();
        let mut accepted_ids = BTreeSet::new();
        let mut created = 0usize;
        for item in &manifest.intents {
            let node = if let Some(node) = preexisting.get(&item.id) {
                node.clone()
            } else {
                let args = super::intent::IntentAddArgs {
                    name: item.name.clone().unwrap_or_default(),
                    description: item.criterion.clone().unwrap_or_default(),
                    level: item.level.clone(),
                    lifecycle: "planned".into(),
                    visibility: Some(item.visibility.clone()),
                    layer: None,
                    aspect: None,
                    allow_symbol_name: false,
                };
                let node = super::intent::create_intent(&store, &args)
                    .with_context(|| format!("creating derived intent '{}'", item.id))?;
                let mut body = node.body.clone();
                body["source_journey"] = json!(journey.name);
                body["derivation_id"] = json!(item.id);
                body["journey_hash"] = json!(semantic_hash);
                store.set_node_body(&node.id, &body)?;
                created += 1;
                store
                    .get_node(&node.id)?
                    .ok_or_else(|| anyhow!("derived intent vanished after creation"))?
            };
            if !accepted_ids.insert(node.id.clone()) {
                bail!(
                    "derived intent '{}' resolves to Intent '{}' already selected by another mapping",
                    item.id,
                    node.name
                );
            }
            accepted_by_local.insert(item.id.clone(), node.clone());
            accepted.push((item.clone(), node));
        }

        let proposal = match &existing_proposal {
            Some(proposal) => {
                if proposal.status != "adopted" {
                    // loom-stability-exempt: Proposal lifecycle records approval provenance,
                    // not an executable proof outcome.
                    store.set_node_status(&proposal.id, "adopted")?;
                }
                store
                    .get_node(&proposal.id)?
                    .ok_or_else(|| anyhow!("derivation Proposal vanished"))?
            }
            None => store.add_node(
                NodeType::Proposal,
                &format!(
                    "Journey {} derivation {}",
                    journey.name, manifest.proposal_id
                ),
                &manifest.proposal_rationale,
                "adopted",
                proposal_body(&manifest, &manifest_hash, &journey, &decision, &[], &[])?,
            )?,
        };

        let mut to_ratify: Vec<&Node> = Vec::new();
        for (_, node) in &accepted {
            if !super::intent::is_ratified(&store, &node.id)? {
                to_ratify.push(node);
            }
        }
        let batch_id =
            ratification_batch(&store, &to_ratify, &evidence, &decision, &semantic_hash)?;
        for node in &to_ratify {
            match &batch_id {
                Some(batch_id) => store
                    .ratify_intent_from_human_batch(&node.id, &evidence, &decision, batch_id)?,
                None => store.ratify_intent_from_human(&node.id, &evidence, &decision)?,
            }
        }

        let mut wanted_targets = BTreeSet::new();
        for (item, node) in &accepted {
            wanted_targets.insert(node.id.clone());
            let edge = store.ensure_edge(EdgeKind::Derives, &journey.id, &node.id)?;
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "journey_hash",
                &semantic_hash,
                TruthClass::Asserted,
            )?;
            let ordered_steps = ordered_subset(&spec, &item.step_ids);
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "step_ids",
                &serde_json::to_string(&ordered_steps)?,
                TruthClass::Asserted,
            )?;
            let hashes = spec.step_hashes()?;
            let subset: BTreeMap<&str, &str> = ordered_steps
                .iter()
                .filter_map(|id| hashes.get(id).map(|hash| (id.as_str(), hash.as_str())))
                .collect();
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "step_hashes",
                &serde_json::to_string(&subset)?,
                TruthClass::Asserted,
            )?;
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "rationale",
                &item.rationale,
                TruthClass::Asserted,
            )?;
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "proposal_id",
                &proposal.id,
                TruthClass::Asserted,
            )?;
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "manifest_hash",
                &manifest_hash,
                TruthClass::Asserted,
            )?;
        }
        let mut removed_edges = 0usize;
        for edge in store.edges_with(Some(EdgeKind::Derives), Some(&journey.id), None)? {
            if !wanted_targets.contains(&edge.to_id) {
                store.delete_edge(&edge.id)?;
                removed_edges += 1;
            }
        }

        let relationship_result = reconcile_derivation_relationships(
            &store,
            &proposals,
            &proposal,
            &manifest_hash,
            &relationships,
            &accepted_by_local,
        )?;
        let final_body = proposal_body(
            &manifest,
            &manifest_hash,
            &journey,
            &decision,
            &accepted,
            &relationship_result.accepted,
        )?;
        if proposal.body != final_body {
            store.set_node_body(&proposal.id, &final_body)?;
        }
        for prior in &proposals {
            if prior.id != proposal.id && prior.status == "adopted" {
                // loom-stability-exempt: superseding an approval Proposal does not
                // settle or reset any executable proof.
                store.set_node_status(&prior.id, "superseded")?;
            }
        }

        // The adopted Proposal is the durable approval record. Journal only
        // when acceptance actually changes graph state; the exact-state fast
        // path above deliberately emits no duplicate record.
        store.append_journal(
            "journey_derivation_accept",
            &journey.id,
            json!({
                "journey_id": journey.name,
                "journey_hash": semantic_hash,
                "manifest_hash": manifest_hash,
                "proposal_id": proposal.id,
                "subjects": accepted.iter().map(|(_, node)| node.id.clone()).collect::<Vec<_>>(),
                "human_decision": decision,
                "evidence": evidence,
            }),
        )?;
        tx.commit()?;
        let proposal = store
            .get_node(&proposal.id)?
            .ok_or_else(|| anyhow!("derivation Proposal vanished after acceptance"))?;
        (
            accepted,
            proposal,
            created,
            removed_edges,
            relationship_result.created,
            relationship_result.removed,
        )
    };

    pulse::emit_line(
        &store,
        json_output,
        json!({
            "accepted": true,
            "journey_id": journey.name,
            "journey_hash": semantic_hash,
            "manifest_hash": manifest_hash,
            "proposal": node_json(&proposal),
            "intents": accepted.iter().map(|(_, node)| node_json(node)).collect::<Vec<_>>(),
            "created": created,
            "relationships_created": relationships_created,
            "removed_projection_edges": removed_edges,
            "removed_relationship_edges": relationships_removed,
        }),
        &format!("loom journey surface {}", journey.name),
        format!(
            "accepted derivation for '{}' ({} intent(s))",
            journey.name,
            accepted.len()
        ),
    )
}
fn find_derived_intent(
    store: &Store,
    journey_id: &str,
    derivation_id: &str,
) -> Result<Option<Node>> {
    let mut matches: Vec<_> = store
        .list_nodes(Some(NodeType::Intent), usize::MAX)?
        .into_iter()
        .filter(|node| {
            node.body.get("source_journey").and_then(Value::as_str) == Some(journey_id)
                && node.body.get("derivation_id").and_then(Value::as_str) == Some(derivation_id)
        })
        .collect();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        count => bail!(
            "derived intent id '{}' is ambiguous for Journey '{}' ({count} nodes)",
            derivation_id,
            journey_id
        ),
    }
}

fn local_derive_accept_envelope_exists(store: &Store, intent_ids: &[String]) -> Result<bool> {
    if intent_ids.is_empty() {
        return Ok(true);
    }
    let expected_command = format!("journey-derive-accept:{}", intent_ids.len());
    let expected_subjects: BTreeSet<&str> = intent_ids.iter().map(String::as_str).collect();
    for (_, envelope) in crate::batch_auth::load_envelopes(store.root())? {
        let subjects: BTreeSet<&str> = envelope.subjects.iter().map(String::as_str).collect();
        if envelope.command_id == expected_command
            && envelope.claim == crate::batch_auth::BatchClaim::Ratification
            && envelope.operation == "ratify"
            && subjects == expected_subjects
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ratification_batch(
    store: &Store,
    targets: &[&Node],
    evidence: &str,
    decision: &crate::ratification::HumanDecision,
    journey_hash: &str,
) -> Result<Option<String>> {
    if targets.is_empty() {
        return Ok(None);
    }
    let subjects: Vec<String> = targets.iter().map(|node| node.id.clone()).collect();
    let digest = crate::batch_auth::subject_digest(&subjects);
    let pre = store.append_journal(
        "batch_journey_derivation",
        &digest,
        json!({
            "operation": "ratify",
            "subjects": subjects,
            "human_decision": decision,
            "evidence": evidence,
            "journey_hash": journey_hash,
        }),
    )?;
    let now = crate::journal::now_iso();
    let executor = store.execution_identity().actor();
    let envelope = crate::batch_auth::BatchAuthorization::seal(
        crate::batch_auth::BatchClaim::Ratification,
        "ratify",
        subjects,
        "human",
        &executor,
        evidence,
        vec![format!("journal:{}", pre.id)],
    )?
    .with_command_id(format!("journey-derive-accept:{}", targets.len()))
    .with_time_bounds(&now, &now)
    .with_human_decision(decision.clone());
    Ok(Some(
        crate::batch_auth::append_envelope(store, &envelope)?.id,
    ))
}
/// Canonical JSON key ordering. The rule lives in `crate::canonical` — it used
/// to exist five times under four names, each feeding a hash another module
/// compared against.
use crate::canonical::canonicalize as canonical_json;
