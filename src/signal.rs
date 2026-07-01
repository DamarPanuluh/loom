//! Signal plane — smells (structural), debt (statistical), doctor (integrity).
//!
//! Plane boundary (INV-3): smells are structural findings computed from graph
//! shape; debt is a statistical feed computed on demand and NEVER stored as an
//! edge or counted as required work. Doctor audits integrity after the fact.
//!
//! All three are pure reads over a `Snapshot`. Nothing here mutates the graph.

use crate::model::{
    Edge, EdgeKind, Facet, InspectionStatus, Node, NodeType, TargetKind, TruthClass,
};
use crate::registry;
use crate::store::{Snapshot, Store};
use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A structural smell: computed from graph shape, each with a remedy.
#[derive(Debug, Clone, Serialize)]
pub struct Smell {
    pub kind: String,
    pub message: String,
    pub remedy: String,
}

/// A statistical debt signal: ranked, advisory, never stored.
#[derive(Debug, Clone, Serialize)]
pub struct DebtCluster {
    pub kind: String,
    pub message: String,
    pub impact: u32,
    pub confirm: String,
}

/// A doctor finding: an integrity violation.
#[derive(Debug, Clone, Serialize)]
pub struct DoctorIssue {
    pub kind: String,
    pub message: String,
}

/// A derived code finding plus its durable adjudication state.
#[derive(Debug, Clone, Serialize)]
pub struct FindingView {
    pub node: Node,
    pub state: String,
    pub reason: String,
    pub stale: bool,
}

#[derive(Deserialize)]
struct Adjudication {
    verdict: String,
    reason: String,
    hash: String,
    #[serde(rename = "at")]
    _at: String,
}

const TANGLE_OWNERS: usize = 3;

// ---- smells ----------------------------------------------------------------

pub fn smells(store: &Store) -> Result<Vec<Smell>> {
    let snap = store.snapshot()?;
    let intents = active_intents(&snap);

    // shared indices (all borrow `snap`)
    let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for e in implements_edges(&snap) {
        owners
            .entry(e.to_id.as_str())
            .or_default()
            .push(e.from_id.as_str());
    }
    let imports = imports_by_file(&snap);
    let path_to_id: BTreeMap<&str, &str> = snap
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::CodeFile)
        .map(|n| (n.name.as_str(), n.id.as_str()))
        .collect();
    let tags_by_intent = tags_by_node(&snap);

    let mut out = Vec::new();
    out.extend(ownership_smells(&snap, &owners));
    out.extend(undeclared_coupling_smells(
        &snap,
        &owners,
        &imports,
        &path_to_id,
    ));
    out.extend(duplicated_responsibility_smells(
        &snap,
        &intents,
        &owners,
        &tags_by_intent,
    ));
    out.extend(layering_smells(
        store,
        &snap,
        &owners,
        &imports,
        &path_to_id,
    )?);
    out.extend(disclosure_smells(&intents, &tags_by_intent));
    out.extend(journey_proof_smells(&snap, &intents));
    Ok(out)
}

/// tangled files (>= N owners) and overlapping ownership (exactly 2 owners with
/// no relationship recorded between them).
fn ownership_smells<'a>(
    snap: &'a Snapshot,
    owners: &BTreeMap<&'a str, Vec<&'a str>>,
) -> Vec<Smell> {
    let mut out = Vec::new();
    for (cf, ids) in owners {
        if ids.len() >= TANGLE_OWNERS {
            out.push(Smell {
                kind: "tangled_file".into(),
                message: format!(
                    "{} is implemented by {} intents",
                    node_name(snap, cf),
                    ids.len()
                ),
                remedy: "split the file or the intents; one file, one cohesive responsibility"
                    .into(),
            });
        }
        if ids.len() == 2 && !edge_between(snap, ids[0], ids[1]) {
            out.push(Smell {
                kind: "overlapping_ownership".into(),
                message: format!(
                    "'{}' and '{}' both own {} with no relationship recorded",
                    node_name(snap, ids[0]),
                    node_name(snap, ids[1]),
                    node_name(snap, cf),
                ),
                remedy: "record a relates edge, or split ownership".into(),
            });
        }
    }
    out
}

/// undeclared coupling: codefile A imports B, but their owning intents share no
/// edge — a dependency the graph hasn't accounted for.
fn undeclared_coupling_smells<'a>(
    snap: &'a Snapshot,
    owners: &BTreeMap<&'a str, Vec<&'a str>>,
    imports: &BTreeMap<String, Vec<String>>,
    path_to_id: &BTreeMap<&'a str, &'a str>,
) -> Vec<Smell> {
    let mut out = Vec::new();
    for (file_id, imps) in imports {
        let Some(a_owners) = owners.get(file_id.as_str()) else {
            continue;
        };
        for imp in imps {
            // resolve an imported path to a known codefile (suffix match)
            let Some(b_id) = path_to_id
                .iter()
                .find(|(p, _)| imp.contains(*p) || p.contains(imp.as_str()))
                .map(|(_, id)| *id)
            else {
                continue;
            };
            if b_id == file_id.as_str() {
                continue;
            }
            let Some(b_owners) = owners.get(b_id) else {
                continue;
            };
            let connected = a_owners
                .iter()
                .any(|a| b_owners.iter().any(|b| a == b || edge_between(snap, a, b)));
            if !connected {
                out.push(Smell {
                    kind: "undeclared_coupling".into(),
                    message: format!(
                        "{} imports {} but their intents have no recorded relationship",
                        node_name(snap, file_id),
                        node_name(snap, b_id)
                    ),
                    remedy:
                        "record a relates edge between the owning intents, or justify with a note"
                            .into(),
                });
                break; // one per file is enough signal
            }
        }
    }
    out
}

/// duplicated responsibility: two intents share a registered tag but live in
/// disjoint files with no edge — possibly the same job done twice.
fn duplicated_responsibility_smells<'a>(
    snap: &'a Snapshot,
    intents: &[&'a Node],
    owners: &BTreeMap<&'a str, Vec<&'a str>>,
    tags_by_intent: &BTreeMap<&'a str, BTreeSet<String>>,
) -> Vec<Smell> {
    let mut out = Vec::new();
    let files_by_intent = files_by_intent(snap, owners);
    let intent_ids: Vec<&str> = intents.iter().map(|n| n.id.as_str()).collect();
    for i in 0..intent_ids.len() {
        for j in (i + 1)..intent_ids.len() {
            let (a, b) = (intent_ids[i], intent_ids[j]);
            let (Some(ta), Some(tb)) = (tags_by_intent.get(a), tags_by_intent.get(b)) else {
                continue;
            };
            let shared: Vec<&String> = ta.intersection(tb).collect();
            if shared.is_empty() {
                continue;
            }
            let fa = files_by_intent.get(a).cloned().unwrap_or_default();
            let fb = files_by_intent.get(b).cloned().unwrap_or_default();
            if fa.is_disjoint(&fb) && !edge_between(snap, a, b) {
                out.push(Smell {
                    kind: "duplicated_responsibility".into(),
                    message: format!(
                        "'{}' and '{}' share tag '{}' in disjoint code with no edge",
                        node_name(snap, a),
                        node_name(snap, b),
                        shared[0]
                    ),
                    remedy: "merge the responsibility, or record why they legitimately differ"
                        .into(),
                });
            }
        }
    }
    out
}

/// layering violation: a file imports another that points UP the declared layer
/// order (an inversion). Needs a declared layer order + intent layer facets.
fn layering_smells<'a>(
    store: &Store,
    snap: &'a Snapshot,
    owners: &BTreeMap<&'a str, Vec<&'a str>>,
    imports: &BTreeMap<String, Vec<String>>,
    path_to_id: &BTreeMap<&'a str, &'a str>,
) -> Result<Vec<Smell>> {
    let mut out = Vec::new();
    let Some(order) = layer_order(store)? else {
        return Ok(out);
    };
    let layer_of = facet_map(snap, "layer");
    let rank: BTreeMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(i, l)| (l.as_str(), i))
        .collect();
    for (file_id, imps) in imports {
        let Some(a_owners) = owners.get(file_id.as_str()) else {
            continue;
        };
        let a_layer = a_owners.iter().filter_map(|o| layer_of.get(*o)).next();
        for imp in imps {
            let Some(b_id) = path_to_id
                .iter()
                .find(|(p, _)| imp.contains(*p))
                .map(|(_, id)| *id)
            else {
                continue;
            };
            let Some(b_owners) = owners.get(b_id) else {
                continue;
            };
            let b_layer = b_owners.iter().filter_map(|o| layer_of.get(*o)).next();
            let (Some(a), Some(b)) = (a_layer, b_layer) else {
                continue;
            };
            let (Some(ra), Some(rb)) = (rank.get(a.as_str()), rank.get(b.as_str())) else {
                continue;
            };
            // importing UP the order (lower rank index = higher layer)
            if ra > rb {
                out.push(Smell {
                    kind: "layering_violation".into(),
                    message: format!(
                        "{} (layer {a}) imports {} (layer {b}) — points up the declared order",
                        node_name(snap, file_id),
                        node_name(snap, b_id)
                    ),
                    remedy: "your call: invert the dependency, or record a verdict justifying it as an accepted exception — judge it, don't defer it"
                        .into(),
                });
            }
        }
    }
    Ok(out)
}

/// disclosure: the duplicate detector is blind while every coded intent is
/// untagged — surface that the signal is unarmed.
fn disclosure_smells(
    intents: &[&Node],
    tags_by_intent: &BTreeMap<&str, BTreeSet<String>>,
) -> Vec<Smell> {
    let mut out = Vec::new();
    let coded = intents.iter().filter(|n| n.status != "planned").count();
    let untagged = intents
        .iter()
        .filter(|n| n.status != "planned" && !tags_by_intent.contains_key(n.id.as_str()))
        .count();
    if coded >= 2 && untagged == coded {
        out.push(Smell {
            kind: "duplicate_detection_unarmed".into(),
            message: format!(
                "{untagged}/{coded} coded intents are untagged; duplicated_responsibility is blind"
            ),
            remedy: "register vocab terms and tag intents (loom vocab add / loom intent tag add)"
                .into(),
        });
    }
    out
}

fn journey_proof_smells(snap: &Snapshot, intents: &[&Node]) -> Vec<Smell> {
    let visibility = facet_map(snap, "visibility");
    let nodes_by_id: BTreeMap<&str, &Node> =
        snap.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut validations_by_intent: BTreeMap<&str, Vec<(&Node, &Edge)>> = BTreeMap::new();
    for edge in &snap.edges {
        if edge.kind != EdgeKind::Validates {
            continue;
        }
        let Some(validation) = nodes_by_id.get(edge.from_id.as_str()) else {
            continue;
        };
        validations_by_intent
            .entry(edge.to_id.as_str())
            .or_default()
            .push((*validation, edge));
    }

    let mut out = Vec::new();
    for intent in intents {
        if intent.status != "implemented"
            || visibility.get(intent.id.as_str()).map(String::as_str) != Some("user_visible")
        {
            continue;
        }
        let validations = validations_by_intent
            .get(intent.id.as_str())
            .cloned()
            .unwrap_or_default();
        let has_journey = validations.iter().any(|(validation, edge)| {
            edge.status == InspectionStatus::Passing
                && validation.status == "passed"
                && validation.body.get("proof_kind").and_then(|v| v.as_str()) == Some("journey")
                && matches!(
                    validation.body.get("proof_level").and_then(|v| v.as_str()),
                    Some("L5" | "L6")
                )
        });
        if has_journey {
            continue;
        }
        let kind = if validations.is_empty() {
            "missing_journey_proof"
        } else {
            "proof_too_shallow_for_intent"
        };
        let message = if validations.is_empty() {
            format!(
                "user-visible intent '{}' has no linked validation; boundary-facing behavior needs an L5 journey proof",
                intent.name
            )
        } else {
            format!(
                "user-visible intent '{}' has validations but no passing L5/L6 journey proof",
                intent.name
            )
        };
        out.push(Smell {
            kind: kind.into(),
            message,
            remedy:
                "add or update a repo-native JourneyProof validation (proof_kind=journey, proof_level=L5+) that exercises the real boundary and asserts outcome"
                    .into(),
        });
    }
    out
}

// ---- findings (derived flags + durable adjudication) ------------------------

pub fn findings_view(store: &Store) -> Result<Vec<FindingView>> {
    let mut out = Vec::new();
    for node in store.list_nodes(Some(NodeType::Finding), usize::MAX)? {
        let Some(raw) = store.get_facet(&node.id, TargetKind::Node, "adjudication")? else {
            out.push(FindingView {
                node,
                state: "untriaged".into(),
                reason: String::new(),
                stale: false,
            });
            continue;
        };

        let Ok(adj) = serde_json::from_str::<Adjudication>(&raw) else {
            out.push(FindingView {
                node,
                state: "untriaged".into(),
                reason: String::new(),
                stale: false,
            });
            continue;
        };
        if !matches!(adj.verdict.as_str(), "justified" | "needed" | "blocked") {
            out.push(FindingView {
                node,
                state: "untriaged".into(),
                reason: String::new(),
                stale: false,
            });
            continue;
        }
        let current_hash = store.finding_codefile_hash(&node.id)?;
        let stale = !adj.hash.is_empty() && current_hash.as_ref() != Some(&adj.hash);
        out.push(FindingView {
            node,
            state: adj.verdict,
            reason: adj.reason,
            stale,
        });
    }
    out.sort_by(|a, b| a.state.cmp(&b.state).then(a.node.name.cmp(&b.node.name)));
    Ok(out)
}

pub fn untriaged_findings(store: &Store) -> Result<Vec<FindingView>> {
    Ok(findings_view(store)?
        .into_iter()
        .filter(|fv| fv.state == "untriaged")
        .collect())
}

pub fn stale_findings(store: &Store) -> Result<Vec<FindingView>> {
    Ok(findings_view(store)?
        .into_iter()
        .filter(|fv| fv.stale)
        .collect())
}

pub fn triage_findings(store: &Store) -> Result<Vec<FindingView>> {
    Ok(findings_view(store)?
        .into_iter()
        .filter(|fv| fv.state == "untriaged" || fv.stale)
        .collect())
}

// ---- debt (statistical, never stored) --------------------------------------

pub fn debt(store: &Store) -> Result<Vec<DebtCluster>> {
    let snap = store.snapshot()?;
    let mut out = Vec::new();

    // size outliers: files whose loc exceeds the Tukey upper fence of the repo.
    // (a statistical signal computed on demand — never stored, never required.)
    let locs: Vec<(String, f64)> = snap
        .facets
        .iter()
        .filter(|f| f.key == "loc")
        .filter_map(|f| {
            f.value
                .parse::<f64>()
                .ok()
                .map(|v| (f.target_id.clone(), v))
        })
        .collect();
    if locs.len() >= 4 {
        let mut vals: Vec<f64> = locs.iter().map(|(_, v)| *v).collect();
        vals.sort_by(|a, b| a.total_cmp(b));
        let q1 = quantile(&vals, 0.25);
        let q3 = quantile(&vals, 0.75);
        let fence = q3 + 1.5 * (q3 - q1);
        for (id, v) in &locs {
            if *v > fence && *v > 200.0 {
                out.push(DebtCluster {
                    kind: "size_outlier".into(),
                    message: format!(
                        "{} is {} loc (repo upper fence {:.0})",
                        node_name(&snap, id),
                        *v as u64,
                        fence
                    ),
                    impact: *v as u32,
                    confirm:
                        "your call: split it if it's tangled, or justify the size as genuine cohesion — judge and act, don't defer to a human"
                            .into(),
                });
            }
        }
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.impact));
    Ok(out)
}

// ---- doctor (integrity audit) ----------------------------------------------

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
            Some(tt) if *tt != spec.to => issues.push(DoctorIssue {
                kind: "edge_endpoint_type".into(),
                message: format!(
                    "edge {} ({}) to-type {} != spec {}",
                    e.id, e.kind, tt, spec.to
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
        // truth-class partition (INV-5): derived edge with an asserted verdict
        if e.truth_class == TruthClass::Derived
            && !matches!(e.status, crate::model::InspectionStatus::Current)
        {
            issues.push(DoctorIssue {
                kind: "derived_with_verdict".into(),
                message: format!("derived edge {} has non-current status {}", e.id, e.status),
            });
        }
        // evidence vacuity (INV-4/6): settled verdict with empty evidence
        if e.truth_class == TruthClass::Asserted
            && matches!(
                e.status,
                crate::model::InspectionStatus::Passing
                    | crate::model::InspectionStatus::Failing
                    | crate::model::InspectionStatus::Independent
            )
            && e.evidence.trim().is_empty()
        {
            issues.push(DoctorIssue {
                kind: "vacuous_verdict".into(),
                message: format!(
                    "{} edge {} is {} with empty evidence",
                    e.kind, e.id, e.status
                ),
            });
        }
    }
    Ok(issues)
}

// ---- helpers ---------------------------------------------------------------

fn active_intents(snap: &Snapshot) -> Vec<&Node> {
    snap.nodes
        .iter()
        .filter(|n| n.node_type == NodeType::Intent && n.status != "deprecated")
        .collect()
}

fn implements_edges(snap: &Snapshot) -> impl Iterator<Item = &Edge> {
    snap.edges.iter().filter(|e| e.kind == EdgeKind::Implements)
}

fn node_name(snap: &Snapshot, id: &str) -> String {
    snap.nodes
        .iter()
        .find(|n| n.id == id)
        .map(|n| n.name.clone())
        .unwrap_or_else(|| id.to_string())
}

fn edge_between(snap: &Snapshot, a: &str, b: &str) -> bool {
    snap.edges.iter().any(|e| {
        matches!(
            e.kind,
            EdgeKind::Relates
                | EdgeKind::Requires
                | EdgeKind::Hierarchy
                | EdgeKind::ScenarioOf
                | EdgeKind::VariantOf
                | EdgeKind::Triggers
                | EdgeKind::Sequence
        ) && ((e.from_id == a && e.to_id == b) || (e.from_id == b && e.to_id == a))
    })
}

fn imports_by_file(snap: &Snapshot) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for f in &snap.facets {
        if f.key == "imports" {
            let list: Vec<String> = serde_json::from_str(&f.value).unwrap_or_default();
            if !list.is_empty() {
                out.insert(f.target_id.clone(), list);
            }
        }
    }
    out
}

fn tags_by_node(snap: &Snapshot) -> BTreeMap<&str, BTreeSet<String>> {
    let mut out: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for t in &snap.tags {
        if t.target_kind == TargetKind::Node {
            out.entry(t.target_id.as_str())
                .or_default()
                .insert(t.term.clone());
        }
    }
    out
}

fn files_by_intent<'a>(
    _snap: &'a Snapshot,
    owners: &BTreeMap<&'a str, Vec<&'a str>>,
) -> BTreeMap<&'a str, BTreeSet<&'a str>> {
    let mut out: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (cf, ids) in owners {
        for id in ids {
            out.entry(*id).or_default().insert(*cf);
        }
    }
    out
}

fn facet_map<'a>(snap: &'a Snapshot, key: &str) -> BTreeMap<&'a str, String> {
    snap.facets
        .iter()
        .filter(|f: &&Facet| f.key == key && f.target_kind == TargetKind::Node)
        .map(|f| (f.target_id.as_str(), f.value.clone()))
        .collect()
}

fn layer_order(store: &Store) -> Result<Option<Vec<String>>> {
    match store.get_meta("layer_order")? {
        Some(v) => {
            let layers: Vec<String> = serde_json::from_str(&v).unwrap_or_default();
            Ok(if layers.is_empty() {
                None
            } else {
                Some(layers)
            })
        }
        None => Ok(None),
    }
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let pos = q * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        sorted[lo] + (pos - lo as f64) * (sorted[hi] - sorted[lo])
    }
}
