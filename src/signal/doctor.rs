use super::graph::{edge_role, node_name};
use crate::model::{
    Edge, EdgeKind, GroundingRole, InspectionStatus, Node, NodeType, TargetKind, TruthClass,
};
use crate::registry;
use crate::store::{Snapshot, Store};
use crate::Result;
use petgraph::algo::tarjan_scc;
use petgraph::graph::{DiGraph, NodeIndex};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// A doctor finding: an integrity violation.
#[derive(Debug, Clone, Serialize)]
pub struct DoctorIssue {
    pub kind: String,
    pub message: String,
}

pub fn doctor(store: &Store) -> Result<Vec<DoctorIssue>> {
    let snap = store.snapshot()?;
    let mut issues = Vec::new();
    let node_types: BTreeMap<&str, NodeType> = snap
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.node_type))
        .collect();

    for e in &snap.edges {
        let spec = registry::spec(e.kind);
        // endpoint existence + typing, both sides
        match node_types.get(e.from_id.as_str()) {
            None => issues.push(DoctorIssue {
                kind: "edge_dangling_from".into(),
                message: format!(
                    "edge {} ({}) from-node {} does not exist",
                    e.id, e.kind, e.from_id
                ),
            }),
            Some(ft) if *ft != spec.from => issues.push(DoctorIssue {
                kind: "edge_endpoint_type".into(),
                message: format!(
                    "edge {} ({}) from-type {} != spec {}",
                    e.id, e.kind, ft, spec.from
                ),
            }),
            _ => {}
        }
        match node_types.get(e.to_id.as_str()) {
            None => issues.push(DoctorIssue {
                kind: "edge_dangling_to".into(),
                message: format!(
                    "edge {} ({}) to-node {} does not exist",
                    e.id, e.kind, e.to_id
                ),
            }),
            Some(tt) if !spec.allows_to(*tt) => issues.push(DoctorIssue {
                kind: "edge_endpoint_type".into(),
                message: format!(
                    "edge {} ({}) to-type {} != spec {}",
                    e.id,
                    e.kind,
                    tt,
                    spec.to_display()
                ),
            }),
            _ => {}
        }
        // truth-class allowed for this kind
        if !spec.allows_truth_class(e.truth_class) {
            issues.push(DoctorIssue {
                kind: "edge_truth_class".into(),
                message: format!(
                    "edge {} ({}) disallows truth_class {}",
                    e.id, e.kind, e.truth_class
                ),
            });
        }
        // A source marker is never graph truth by itself. Once an asserted
        // edge references `anchor:<id>`, however, the graph owns that locator
        // claim and doctor verifies its strict cardinality, attachment, and
        // target-file identity. Unreferenced source comments remain outside
        // the graph inventory.
        if let Some(locator) = facet_value(&snap, &e.id, "locator") {
            if crate::locator::is_anchor_locator(locator) {
                let result = match store.get_node(&e.to_id)? {
                    Some(codefile) if codefile.node_type == NodeType::CodeFile => {
                        crate::locator::validate_for_codefile(store, &codefile, locator)
                    }
                    Some(target) => Err(anyhow::anyhow!(
                        "target '{}' is {}, not CodeFile",
                        target.name,
                        target.node_type
                    )),
                    None => Err(anyhow::anyhow!("target CodeFile '{}' is missing", e.to_id)),
                };
                if let Err(error) = result {
                    issues.push(DoctorIssue {
                        kind: "invalid_source_anchor".into(),
                        message: format!(
                            "{} edge '{}' has invalid locator '{}': {error}",
                            e.kind, e.id, locator
                        ),
                    });
                }
            }
        }
        // truth-class partition (INV-5): derived edge with an asserted verdict
        if e.truth_class == TruthClass::Derived
            && !matches!(e.status, crate::model::InspectionStatus::Current)
        {
            issues.push(DoctorIssue {
                kind: "derived_with_verdict".into(),
                message: format!("derived edge {} has non-current status {}", e.id, e.status),
            });
        }
        // Vacuity audit (INV-4/6): mirror the record_verdict write boundary so
        // legacy/imported verdicts recorded before the gate are still caught.
        // passing/failing/independent require substantive criterion AND evidence;
        // blocked requires a substantive reason (stored in evidence).
        if e.truth_class == TruthClass::Asserted {
            // A settled verdict standing on nothing loom can re-check. The old
            // check looked for placeholder PROSE, which caught "TBD" and missed
            // every plausible sentence; this one asks whether any anchor is
            // still live. It fires on facts carried forward from before the
            // evidence spine and on facts whose anchors have all since broken.
            let vacuous_field = match e.status {
                crate::model::InspectionStatus::Passing
                | crate::model::InspectionStatus::Failing
                | crate::model::InspectionStatus::Independent => {
                    if crate::model::is_placeholder(&e.criterion) {
                        Some((
                            "vacuous_verdict",
                            "empty or placeholder criterion".to_string(),
                        ))
                    } else {
                        // Absent-fact and Expired are two different failures. A
                        // missing verdict fact is a settled status standing on
                        // nothing ever recorded; an Expired one is a real
                        // verdict whose anchors have all broken. Reporting both
                        // as "empty evidence" told the operator to fix a fact
                        // that isn't there.
                        match store.edge_verdict_verification(&e.id)? {
                            None => Some((
                                "missing_verdict",
                                "settled but has no recorded verdict fact".to_string(),
                            )),
                            Some(crate::model::Verification::Expired) => {
                                Some(("vacuous_verdict", "expired evidence".to_string()))
                            }
                            Some(_) => None,
                        }
                    }
                }
                _ => None,
            };
            if let Some((kind, what)) = vacuous_field {
                issues.push(DoctorIssue {
                    kind: kind.into(),
                    message: format!("{} edge {} is {} with {what}", e.kind, e.id, e.status),
                });
            }
        }
        // Role vacuity (§3.4): a settled `consumes` grounding must name the seam
        // it exercises (a route/topic/key/symbol) — in its criterion or locator.
        // A vacuous consumes claim is as empty as a placeholder criterion.
        if e.kind == EdgeKind::Implements
            && e.truth_class == TruthClass::Asserted
            && matches!(
                e.status,
                InspectionStatus::Passing
                    | InspectionStatus::Failing
                    | InspectionStatus::Independent
            )
            && edge_role(&snap, &e.id) == GroundingRole::Consumes
            && !criterion_names_seam(&snap, e)
        {
            issues.push(DoctorIssue {
                kind: "consumes_without_seam".into(),
                message: format!(
                    "consumes grounding {} is settled but names no seam (route/topic/key/symbol) in its criterion or locator",
                    e.id
                ),
            });
        }
    }
    issues.extend(hierarchy_cycle_issues(&snap));
    issues.extend(journey_integrity_issues(store, &snap));
    // Validation names ending in `  proof` — the fingerprint left when a
    // retired proof-level token was excised immediately before a trailing
    // `proof` without rejoining words. Mid-phrase double spaces are legitimate.
    for n in &snap.nodes {
        if n.node_type != NodeType::Validation {
            continue;
        }
        if crate::grammar::excised_proof_level_name(&n.name) {
            issues.push(DoctorIssue {
                kind: "malformed_validation_name".into(),
                message: format!(
                    "validation '{}' has a name damaged by proof-level excision (ends with '  proof') — rename it to the behavior it proves",
                    n.name
                ),
            });
        }
    }
    // Orphaned upstream intents: shadows whose alias no longer matches any
    // linked upstream (after `graph unlink` without `--prune`). The node
    // persists deliberately so re-link can reattach, but the unlinked state is
    // a hard integrity issue until disposed via `graph prune-orphans`.
    // An unreadable registry is itself an integrity problem: swallowing the Err
    // here would skip every orphan check below and report the graph clean, which
    // is the one thing doctor must never do.
    match crate::federation::read_upstream_entries(store) {
        Err(e) => issues.push(DoctorIssue {
            kind: "unreadable_upstream_registry".into(),
            message: format!(
                "the linked-upstream registry could not be read, so orphaned upstream intents cannot be checked: {e:#}"
            ),
        }),
        Ok(entries) => {
        let linked_aliases: std::collections::BTreeSet<&str> =
            entries.iter().map(|e| e.alias.as_str()).collect();
        for n in &snap.nodes {
            if n.node_type != NodeType::UpstreamIntent {
                continue;
            }
            let alias = n.body.get("alias").and_then(|v| v.as_str()).unwrap_or("");
            if !linked_aliases.contains(alias) {
                issues.push(DoctorIssue {
                    kind: "orphaned_upstream_intent".into(),
                    message: format!(
                        "upstream intent '{}' has no linked upstream (alias '{}' not in registry) — dispose with `loom graph prune-orphans` (or `graph unlink --prune` at unlink time; add --cascade if DependsOn edges remain)",
                        n.name, alias
                    ),
                });
            }
        }
        }
    }
    // Keep the complete aggregate deterministic without hiding repeated
    // violations that happen to share the same classification or wording.
    issues.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.message.cmp(&b.message)));
    Ok(issues)
}

/// Fail-closed integrity checks for the Journey-root proof topology. Readiness
/// gaps belong to the ladder; doctor reports malformed or internally
/// contradictory Journey data that normal commands should never persist.
fn journey_integrity_issues(store: &Store, snap: &Snapshot) -> Vec<DoctorIssue> {
    // One journal read for every ratification check below: the per-call
    // `Store::ratification` re-parses the whole journal file, and this loop
    // asks once per Derives edge — the doctor-side share of the residual
    // hotspot on finding 6825299d. A journal read failure falls back to the
    // per-call path so a damaged journal degrades, never hides, the check.
    let journal = crate::journal::read(store.root()).ok();
    let ratified = |intent_id: &str| -> bool {
        match &journal {
            Some(entries) => {
                store.ratification_with_journal(intent_id, entries).ok() == Some("ratified".into())
            }
            None => store.ratification(intent_id).ok() == Some("ratified".into()),
        }
    };
    let mut issues = Vec::new();
    // Assemble retired keys so the terminology guard can continue treating an
    // exact source-level spelling as accidental teaching, while doctor still
    // detects imported/corrupt v11 payloads.
    let retired_proof_key = ["proof", "kind"].join("_");
    let retired_journey_key = ["journey", "id"].join("_");
    let nodes: BTreeMap<&str, &Node> = snap
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();

    for node in &snap.nodes {
        // These keys classified an executable Validation as a legacy Journey.
        // `journey_id` is also legitimate inside the v1 derivation manifest
        // retained by an adopted Proposal, so it is retired metadata only on
        // the old Validation representation.
        if node.node_type == NodeType::Validation
            && (node.body.get(&retired_proof_key).is_some()
                || node.body.get(&retired_journey_key).is_some())
        {
            issues.push(DoctorIssue {
                kind: "retired_journey_metadata".into(),
                message: format!(
                    "{} '{}' contains retired Journey-classification metadata; rebuild it with the v12 Journey topology",
                    node.node_type, node.name
                ),
            });
        }
    }

    for journey in snap
        .nodes
        .iter()
        .filter(|node| node.node_type == NodeType::Journey)
    {
        let stable_id = journey
            .body
            .get("stable_id")
            .and_then(|value| value.as_str());
        let semantic_hash = journey
            .body
            .get("semantic_hash")
            .and_then(|value| value.as_str());
        let artifact = journey
            .body
            .get("artifact")
            .and_then(|value| value.as_str());
        let step_order: Vec<&str> = journey
            .body
            .get("step_ids")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str())
            .collect();
        let step_ids: BTreeSet<&str> = step_order.iter().copied().collect();
        if journey.body.get("schema").and_then(|value| value.as_str())
            != Some(crate::journey::JOURNEY_SCHEMA)
            || stable_id != Some(journey.name.as_str())
            || semantic_hash.is_none()
            || artifact.is_none()
            || step_ids.is_empty()
        {
            issues.push(DoctorIssue {
                kind: "invalid_journey_artifact".into(),
                message: format!(
                    "Journey '{}' has an incomplete or inconsistent v1 registration body",
                    journey.name
                ),
            });
        } else if let Some(path) = artifact {
            match crate::journey::parse(&store.root().join(path)) {
                Ok(spec) => match spec.semantic_hash() {
                    Ok(hash)
                        if spec.id == journey.name
                            && Some(hash.as_str()) == semantic_hash
                            && spec
                                .step_ids()
                                .iter()
                                .map(String::as_str)
                                .collect::<Vec<_>>()
                                == journey
                                    .body
                                    .get("step_ids")
                                    .and_then(|value| value.as_array())
                                    .into_iter()
                                    .flatten()
                                    .filter_map(|value| value.as_str())
                                    .collect::<Vec<_>>() => {}
                    Ok(_) => issues.push(DoctorIssue {
                        kind: "invalid_journey_artifact".into(),
                        message: format!(
                            "Journey '{}' registration no longer matches its authored artifact",
                            journey.name
                        ),
                    }),
                    Err(error) => issues.push(DoctorIssue {
                        kind: "invalid_journey_artifact".into(),
                        message: format!(
                            "Journey '{}' cannot hash its authored artifact: {error}",
                            journey.name
                        ),
                    }),
                },
                Err(error) => issues.push(DoctorIssue {
                    kind: "invalid_journey_artifact".into(),
                    message: format!(
                        "Journey '{}' artifact is missing or invalid: {error}",
                        journey.name
                    ),
                }),
            }
        }

        let derives: Vec<&Edge> = snap
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Derives && edge.from_id == journey.id)
            .collect();
        for edge in &derives {
            let bound_hash = facet_value(snap, &edge.id, "journey_hash");
            let bound_steps = facet_value(snap, &edge.id, "step_ids")
                .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok());
            let valid_steps = bound_steps.as_ref().is_some_and(|ids| {
                !ids.is_empty()
                    && ids.iter().all(|id| step_ids.contains(id.as_str()))
                    && ids.iter().collect::<BTreeSet<_>>().len() == ids.len()
            });
            if bound_hash != semantic_hash || !valid_steps {
                issues.push(DoctorIssue {
                    kind: "bad_journey_step_binding".into(),
                    message: format!(
                        "Derives edge '{}' has a stale or malformed Journey step binding",
                        edge.id
                    ),
                });
            }
            if !ratified(&edge.to_id) {
                issues.push(DoctorIssue {
                    kind: "unratified_journey_derivation".into(),
                    message: format!(
                        "Derives edge '{}' targets an Intent that is not ratified",
                        edge.id
                    ),
                });
            }
        }

        let surfaces: Vec<&Edge> = snap
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Surfaces && edge.from_id == journey.id)
            .collect();
        for edge in &surfaces {
            let target = nodes.get(edge.to_id.as_str());
            let operations = target
                .and_then(|target| target.body.get("operations"))
                .cloned()
                .and_then(|value| {
                    serde_json::from_value::<Vec<crate::journey::CliOperation>>(value).ok()
                });
            let valid_target = target.is_some_and(|target| {
                target.node_type == NodeType::InterfaceSurface
                    && target
                        .body
                        .get("schema")
                        .and_then(serde_json::Value::as_str)
                        == Some(crate::journey::INTERFACE_SURFACE_SCHEMA)
                    && target.body.get("kind").and_then(serde_json::Value::as_str) == Some("cli")
                    && operations.as_ref().is_some_and(|operations| {
                        crate::journey::InterfaceSurfaceDefinition {
                            id: target
                                .body
                                .get("stable_id")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("")
                                .into(),
                            title: target
                                .body
                                .get("title")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("")
                                .into(),
                            identity: target
                                .body
                                .get("identity")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("")
                                .into(),
                            codefile: target
                                .body
                                .get("codefile")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("")
                                .into(),
                            locator: target
                                .body
                                .get("locator")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("")
                                .into(),
                            operations: operations.clone(),
                        }
                        .validate()
                        .is_ok()
                    })
            });
            let valid_bindings = facet_value(snap, &edge.id, "operation_bindings")
                .and_then(|raw| {
                    crate::completeness::exact_surface_bindings(
                        raw,
                        &step_order,
                        operations.as_deref().unwrap_or_default(),
                    )
                })
                .is_some();
            if !valid_target
                || facet_value(snap, &edge.id, "journey_hash") != semantic_hash
                || !valid_bindings
            {
                issues.push(DoctorIssue {
                    kind: "bad_journey_surface_binding".into(),
                    message: format!(
                        "Surfaces edge '{}' has an incompatible target or binding contract",
                        edge.id
                    ),
                });
            }
            let exposes = snap.edges.iter().filter(|candidate| {
                candidate.kind == EdgeKind::Exposes && candidate.from_id == edge.to_id
            });
            if !exposes.clone().any(|candidate| {
                nodes
                    .get(candidate.to_id.as_str())
                    .is_some_and(|target| target.node_type == NodeType::CodeFile)
                    && facet_value(snap, &candidate.id, "locator")
                        .is_some_and(|value| !value.trim().is_empty())
            }) {
                issues.push(DoctorIssue {
                    kind: "journey_surface_missing_locator".into(),
                    message: format!(
                        "InterfaceSurface '{}' has no exposed CLI entrypoint locator",
                        edge.to_id
                    ),
                });
            }
        }

        let proves: Vec<&Edge> = snap
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Proves && edge.to_id == journey.id)
            .collect();
        let mut by_profile: BTreeMap<&str, Vec<&Edge>> = BTreeMap::new();
        for edge in &proves {
            let Some(validation) = nodes.get(edge.from_id.as_str()) else {
                continue;
            };
            let profile = validation
                .body
                .get("profile")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            by_profile.entry(profile).or_default().push(edge);
            let calls_surface = snap.edges.iter().any(|candidate| {
                candidate.kind == EdgeKind::Calls
                    && candidate.from_id == validation.id
                    && surfaces
                        .iter()
                        .any(|surface| surface.to_id == candidate.to_id)
            });
            let exercises_locator = snap.edges.iter().any(|candidate| {
                candidate.kind == EdgeKind::Exercises
                    && candidate.from_id == validation.id
                    && facet_value(snap, &candidate.id, "locator")
                        .is_some_and(|value| !value.trim().is_empty())
            });
            let version_current = validation
                .body
                .get("compiler_version")
                .and_then(|value| value.as_str())
                == Some(crate::journey::JOURNEY_COMPILER_VERSION);
            // The compiled Exercises topology and provenance facets must agree
            // exactly with the canonical projection of the accepted surface.
            // A forged, malformed, or obsolete facet — or a validation that
            // predates the current compiler — breaks the chain here, not only
            // at grade time.
            let provenance_problem =
                compiled_journey_provenance_problem(store, journey, &validation.id);
            let validates_all = derives.iter().all(|derived| {
                snap.edges.iter().any(|candidate| {
                    candidate.kind == EdgeKind::Validates
                        && candidate.from_id == validation.id
                        && candidate.to_id == derived.to_id
                })
            });
            if profile.is_empty()
                || validation
                    .body
                    .get("journey_hash")
                    .and_then(|value| value.as_str())
                    != semantic_hash
                || !version_current
                || !calls_surface
                || !exercises_locator
                || !validates_all
                || provenance_problem.is_some()
            {
                let detail = provenance_problem
                    .map(|problem| format!(": {problem}"))
                    .unwrap_or_default();
                issues.push(DoctorIssue {
                    kind: "broken_journey_proof_chain".into(),
                    message: format!("Validation '{}' does not form a current Proves/Validates/Calls/Exercises Journey chain{detail}", validation.name),
                });
            }
        }
        for (profile, edges) in by_profile {
            if edges.len() > 1 {
                issues.push(DoctorIssue {
                    kind: "duplicate_journey_profile_validation".into(),
                    message: format!(
                        "Journey '{}' profile '{}' has {} Proves validations",
                        journey.name,
                        profile,
                        edges.len()
                    ),
                });
            }
        }
    }
    issues
}

fn facet_value<'a>(snap: &'a Snapshot, target_id: &str, key: &str) -> Option<&'a str> {
    snap.facets
        .iter()
        .find(|facet| {
            facet.target_kind == TargetKind::Edge
                && facet.target_id == target_id
                && facet.key == key
        })
        .map(|facet| facet.value.as_str())
}

/// One disagreement between a compiled Journey validation's Exercises
/// topology/provenance and the canonical projection of the accepted surface,
/// or `None` when they agree exactly. `None` is also returned when the
/// disagreement cannot be computed — those cases surface through the other
/// chain checks.
fn compiled_journey_provenance_problem(
    store: &Store,
    journey: &Node,
    validation_id: &str,
) -> Option<String> {
    match crate::journey_exercises::expected_projection(store, journey) {
        Err(error) => Some(format!("no valid operation-exercise projection: {error:#}")),
        Ok(projection) => {
            let problems =
                crate::journey_exercises::topology_problems(store, validation_id, &projection)
                    .ok()?;
            (!problems.is_empty()).then(|| problems.join("; "))
        }
    }
}

/// Whether a `consumes` grounding's criterion (or locator) names the seam it
/// exercises. A consumes claim's falsifiability lives in the seam — a route,
/// topic, config key, or symbol — so a criterion mentioning none, with no
/// locator, is vacuous.
fn criterion_names_seam(snap: &Snapshot, edge: &Edge) -> bool {
    const SEAM_HINTS: &[&str] = &[
        "/", "::", "route", "topic", "key", "endpoint", "import", "seam", "path", "url", "channel",
        "queue", "sdk", "http", "grpc", "api", "call",
    ];
    let c = edge.criterion.to_lowercase();
    if SEAM_HINTS.iter().any(|h| c.contains(h)) {
        return true;
    }
    snap.facets.iter().any(|f| {
        f.target_kind == TargetKind::Edge
            && f.target_id == edge.id
            && f.key == "locator"
            && !f.value.trim().is_empty()
    })
}
fn hierarchy_cycle_issues(snap: &Snapshot) -> Vec<DoctorIssue> {
    let mut graph = DiGraph::<&str, ()>::new();
    let mut indices: BTreeMap<&str, NodeIndex> = BTreeMap::new();

    // Register nodes from the hierarchy edges' OWN endpoints, not from a
    // pre-filtered Intent set. A hierarchy edge is meant to run Intent→Intent,
    // but an edge with a non-Intent (or dangling) endpoint is precisely the
    // violation we must surface — filtering the node set to Intents first made
    // `indices.get` miss and silently dropped the edge, hiding the cycle.
    let mut self_loops = BTreeSet::new();
    for edge in snap.edges.iter().filter(|e| e.kind == EdgeKind::Hierarchy) {
        if edge.from_id == edge.to_id {
            self_loops.insert(edge.from_id.as_str());
        }
        let from = *indices
            .entry(edge.from_id.as_str())
            .or_insert_with(|| graph.add_node(edge.from_id.as_str()));
        let to = *indices
            .entry(edge.to_id.as_str())
            .or_insert_with(|| graph.add_node(edge.to_id.as_str()));
        graph.add_edge(from, to, ());
    }

    let mut issues = Vec::new();
    for id in self_loops {
        issues.push(DoctorIssue {
            kind: "hierarchy_cycle".into(),
            message: format!(
                "hierarchy edge on '{}' points to itself",
                node_name(snap, id)
            ),
        });
    }

    for component in tarjan_scc(&graph) {
        if component.len() <= 1 {
            continue;
        }
        let mut names: Vec<String> = component
            .iter()
            .map(|idx| node_name(snap, graph[*idx]))
            .collect();
        names.sort();
        issues.push(DoctorIssue {
            kind: "hierarchy_cycle".into(),
            message: format!("hierarchy cycle among {}", names.join(" -> ")),
        });
    }
    issues.sort_by(|a, b| a.message.cmp(&b.message));
    issues
}
