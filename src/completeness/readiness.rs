use super::{
    current_derivation, exact_operation_bindings, exact_string_array, hash_bound,
    intent_journey_exempt, interface_has_code, interface_operations, projection_current,
    valid_cli_interface, JourneyDeriveGap, JourneyReadiness, JourneySurfaceGap,
};
use crate::model::{EdgeKind, InspectionStatus, Node, NodeType};
use crate::store::Store;
use crate::Result;

/// Compute all seven readiness stages for one authored Journey.
pub fn journey_readiness(store: &Store, journey: &Node) -> Result<JourneyReadiness> {
    let journal = crate::journal::read(store.root())?;
    journey_readiness_with_journal(store, journey, &journal)
}

/// The same readiness over a journal read ONCE by the caller. Ratification
/// standing consults the journal; the per-call `Store::ratification` re-parsed
/// the entire journal file for every derived intent, which multiplied to
/// seconds per readiness walk (the residual hotspot noted on finding 6825299d).
pub(crate) fn journey_readiness_with_journal(
    store: &Store,
    journey: &Node,
    journal: &[crate::journal::Entry],
) -> Result<JourneyReadiness> {
    let schema_ok = journey.body.get("schema").and_then(|v| v.as_str()) == Some("loom.journey/v1");
    let stable_id = journey
        .body
        .get("stable_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let semantic_hash = journey
        .body
        .get("semantic_hash")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let artifact = journey
        .body
        .get("artifact")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let step_ids = exact_string_array(journey.body.get("step_ids")).unwrap_or_default();
    let authored = journey.node_type == NodeType::Journey
        && schema_ok
        && !stable_id.is_empty()
        && stable_id == journey.name
        && !semantic_hash.is_empty()
        && !artifact.is_empty()
        && !step_ids.is_empty()
        && store.root().join(artifact).is_file();

    let derivations = store.edges_with(Some(EdgeKind::Derives), Some(&journey.id), None)?;
    let mut covered_steps = std::collections::BTreeSet::new();
    let mut derived_intent_ids = Vec::new();
    let mut derive_gaps = Vec::new();
    for edge in &derivations {
        match current_derivation(store, edge, &semantic_hash, &step_ids)? {
            Some(mapped) => {
                covered_steps.extend(mapped);
                if !derived_intent_ids.contains(&edge.to_id) {
                    derived_intent_ids.push(edge.to_id.clone());
                }
            }
            None => derive_gaps.push(format!(
                "derivation {} is stale, hash-mismatched, or has invalid step_ids",
                crate::model::short(&edge.id)
            )),
        }
    }
    for step in &step_ids {
        if !covered_steps.contains(step) {
            derive_gaps.push(format!("step '{step}' is unmapped"));
        }
    }
    derived_intent_ids.sort();
    let derived = authored && derive_gaps.is_empty() && !derived_intent_ids.is_empty();

    let mut derivations_ratified = derived;
    let mut implemented = derived;
    for id in &derived_intent_ids {
        let Some(intent) = store.get_node(id)? else {
            derivations_ratified = false;
            implemented = false;
            continue;
        };
        if store.ratification_with_journal(id, journal)? != "ratified" {
            derivations_ratified = false;
        }
        if intent.status != "implemented" || store.realizing_groundings(id)?.is_empty() {
            implemented = false;
        }
    }

    let surfaces = store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)?;
    let mut surface_declared = false;
    let mut surfaced = false;
    let mut surface_gaps = Vec::new();
    for edge in &surfaces {
        if !projection_current(edge) || !hash_bound(store, edge, &semantic_hash)? {
            surface_gaps.push(format!(
                "surface {} is stale or bound to an older Journey hash",
                crate::model::short(&edge.id)
            ));
            continue;
        }
        let Some(interface) = store.get_node(&edge.to_id)? else {
            surface_gaps.push("accepted CLI surface target is missing".into());
            continue;
        };
        if !valid_cli_interface(&interface) {
            surface_gaps
                .push("accepted surface is not a valid loom.interface-surface/v1 CLI".into());
            continue;
        }
        let Some(operations) = interface_operations(&interface) else {
            surface_gaps.push("accepted CLI surface has malformed operations".into());
            continue;
        };
        if exact_operation_bindings(store, &edge.id, &step_ids, &operations)?.is_none() {
            surface_gaps.push(format!(
                "surface {} lacks canonical complete operation bindings",
                crate::model::short(&edge.id)
            ));
            continue;
        }
        surface_declared = true;
        if interface_has_code(store, &interface)? {
            surfaced = true;
        } else {
            surface_gaps.push("accepted CLI surface has no exposed live CodeFile".into());
        }
    }
    if !surface_declared {
        surface_gaps.push("no current hash-bound CLI surface".into());
    }

    let current_surface_hash = crate::journey::surface_projection_hash(store, journey)?;
    let mut compiled = false;
    let mut proven = false;
    for edge in store.edges_with(Some(EdgeKind::Proves), None, Some(&journey.id))? {
        if !projection_current(&edge) {
            continue;
        }
        let Some(validation) = store.get_node(&edge.from_id)? else {
            continue;
        };
        let hash_current = surfaced
            && validation.body.get("journey_hash").and_then(|v| v.as_str())
                == Some(semantic_hash.as_str())
            && validation.body.get("surface_hash").and_then(|v| v.as_str())
                == current_surface_hash.as_deref()
            && current_surface_hash.is_some()
            && validation.body.get("profile").and_then(|v| v.as_str()) == Some("proof")
            && validation
                .body
                .get("compiler_version")
                .and_then(|v| v.as_str())
                == Some(crate::journey::JOURNEY_COMPILER_VERSION);
        if validation.node_type != NodeType::Validation || !hash_current {
            continue;
        }
        compiled = true;
        if edge.status == InspectionStatus::Passing
            && validation.status == "passed"
            && crate::proofstrength::of(store, &validation.id)?
                >= crate::proofstrength::Strength::END_TO_END
        {
            proven = true;
        }
    }
    let realized = authored
        && derived
        && derivations_ratified
        && implemented
        && surfaced
        && compiled
        && proven;

    Ok(JourneyReadiness {
        journey_id: journey.id.clone(),
        journey_name: journey.name.clone(),
        semantic_hash,
        step_ids,
        authored,
        derived,
        implemented,
        surfaced,
        compiled,
        proven,
        realized,
        derivations_ratified,
        derived_intent_ids,
        derive_gaps,
        surface_gaps,
    })
}

/// Every active authored Journey, stable by authored id.
pub fn all_journey_readiness(store: &Store) -> Result<Vec<JourneyReadiness>> {
    // One journal read for the whole walk — see `journey_readiness_with_journal`.
    let journal = crate::journal::read(store.root())?;
    let mut out = Vec::new();
    for journey in store.list_nodes(Some(NodeType::Journey), usize::MAX)? {
        if journey.status == "deprecated" {
            continue;
        }
        out.push(journey_readiness_with_journal(store, &journey, &journal)?);
    }
    out.sort_by(|a, b| a.journey_name.cmp(&b.journey_name));
    Ok(out)
}

/// The single enumerated predicate behind Derive depth, roster, and serving.
pub fn journey_derive_gaps(store: &Store) -> Result<Vec<JourneyDeriveGap>> {
    let readiness = all_journey_readiness(store)?;
    journey_derive_gaps_with(store, &readiness)
}

/// The same predicate over an already-computed readiness snapshot, so one
/// gather never pays the whole-graph readiness walk twice (finding 6825299d:
/// per-call recomputes stacked up to CPU-minutes on a 1586-edge graph).
pub fn journey_derive_gaps_with(
    store: &Store,
    readiness: &[JourneyReadiness],
) -> Result<Vec<JourneyDeriveGap>> {
    let mut out = Vec::new();
    for journey in readiness {
        for detail in &journey.derive_gaps {
            let (kind, subject_id, subject_name) = match detail
                .strip_prefix("step '")
                .and_then(|rest| rest.strip_suffix("' is unmapped"))
            {
                Some(step) => ("unmapped_step", step, step),
                None => (
                    "stale_derivation",
                    journey.journey_id.as_str(),
                    journey.journey_name.as_str(),
                ),
            };
            out.push(JourneyDeriveGap {
                journey_id: journey.journey_id.clone(),
                kind: kind.into(),
                subject_id: subject_id.into(),
                subject_name: subject_name.into(),
                detail: detail.clone(),
            });
        }
        // A derived Journey whose technical Intents are not ratified is
        // waiting on the human gate behind `derive-accept`, not on more
        // derivation prep — so it stays Derive-lane work with a distinct
        // kind. Without this row the compile refusal ("derivation acceptance
        // is pending") named no served queue at all.
        if journey.derived && !journey.derivations_ratified {
            out.push(JourneyDeriveGap {
                journey_id: journey.journey_id.clone(),
                kind: "derivation_acceptance_pending".into(),
                subject_id: journey.journey_id.clone(),
                subject_name: journey.journey_name.clone(),
                detail: format!(
                    "Journey '{}' has accepted mappings awaiting human ratification via derive-accept",
                    journey.journey_name
                ),
            });
        }
    }

    // An unrooted Intent is assigned to a Journey only when a relationship
    // neighbor is already derived there. Pinning every orphan to the first
    // authored Journey made Derive serve a false host.
    let currently_rooted: std::collections::BTreeSet<&str> = readiness
        .iter()
        .flat_map(|journey| journey.derived_intent_ids.iter().map(String::as_str))
        .collect();
    for intent in store.list_nodes(Some(NodeType::Intent), usize::MAX)? {
        if intent.status == "deprecated"
            || currently_rooted.contains(intent.id.as_str())
            || intent_journey_exempt(store, &intent.id)?
        {
            continue;
        }
        let Some(host_id) = host_journey_for_unrooted(store, &intent.id, readiness)? else {
            continue;
        };
        out.push(JourneyDeriveGap {
            journey_id: host_id,
            kind: "unrooted_intent".into(),
            subject_id: intent.id.clone(),
            subject_name: intent.name.clone(),
            detail: format!(
                "intent '{}' has no current Journey derivation and no valid journey_exemption",
                intent.name
            ),
        });
    }
    out.sort_by(|a, b| {
        let rank = |kind: &str| match kind {
            "unmapped_step" => 0,
            "stale_derivation" => 1,
            _ => 2,
        };
        rank(&a.kind)
            .cmp(&rank(&b.kind))
            .then(a.journey_id.cmp(&b.journey_id))
            .then(a.subject_name.cmp(&b.subject_name))
    });
    Ok(out)
}

const RELATIONSHIP_KINDS: &[EdgeKind] = &[
    EdgeKind::Relates,
    EdgeKind::Requires,
    EdgeKind::Hierarchy,
    EdgeKind::ScenarioOf,
    EdgeKind::VariantOf,
    EdgeKind::Triggers,
    EdgeKind::Sequence,
];

fn host_journey_for_unrooted(
    store: &Store,
    intent_id: &str,
    readiness: &[JourneyReadiness],
) -> Result<Option<String>> {
    let mut neighbors = std::collections::BTreeSet::new();
    for kind in RELATIONSHIP_KINDS {
        for edge in store.edges_with(Some(*kind), Some(intent_id), None)? {
            neighbors.insert(edge.to_id);
        }
        for edge in store.edges_with(Some(*kind), None, Some(intent_id))? {
            neighbors.insert(edge.from_id);
        }
    }
    neighbors.remove(intent_id);
    if neighbors.is_empty() {
        return Ok(None);
    }
    let mut hosts: Vec<&JourneyReadiness> = readiness
        .iter()
        .filter(|journey| {
            journey.authored
                && journey
                    .derived_intent_ids
                    .iter()
                    .any(|id| neighbors.contains(id))
        })
        .collect();
    hosts.sort_by(|a, b| a.journey_id.cmp(&b.journey_id));
    Ok(hosts.first().map(|journey| journey.journey_id.clone()))
}

/// The single enumerated predicate behind Surface depth, roster, and serving.
pub fn journey_surface_gaps(store: &Store) -> Result<Vec<JourneySurfaceGap>> {
    Ok(journey_surface_gaps_with(&all_journey_readiness(store)?))
}

/// The same predicate over an already-computed readiness snapshot (see
/// `journey_derive_gaps_with`).
pub fn journey_surface_gaps_with(readiness: &[JourneyReadiness]) -> Vec<JourneySurfaceGap> {
    let mut out = Vec::new();
    for journey in readiness {
        if journey.authored
            && journey.derived
            && journey.derivations_ratified
            && journey.implemented
            && !journey.surfaced
        {
            out.push(JourneySurfaceGap {
                journey_id: journey.journey_id.clone(),
                detail: journey.surface_gaps.join("; "),
            });
        }
    }
    out
}
