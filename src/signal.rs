//! Signal plane — smells (structural), debt (statistical), doctor (integrity).
//!
//! Plane boundary (INV-3): smells are structural findings computed from graph
//! shape; debt is a statistical feed computed on demand and NEVER stored as an
//! edge or counted as required work. Doctor audits integrity after the fact.
//!
//! All three are pure reads over a `Snapshot`. Nothing here mutates the graph.

use crate::model::{
    Edge, EdgeKind, Facet, GroundingRole, InspectionStatus, Node, NodeType, TargetKind, TruthClass,
};
use crate::registry;
use crate::store::{Snapshot, Store};
use crate::Result;
use anyhow::Context;
use petgraph::algo::tarjan_scc;
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A structural smell: computed from graph shape, each with a remedy.
#[derive(Debug, Clone, Serialize)]
pub struct Smell {
    pub kind: String,
    pub message: String,
    pub remedy: String,
    /// Stable subject key built from node/edge IDS — never display wording —
    /// so the materialized finding id (and its durable adjudication) survives
    /// renames and copy changes.
    #[serde(skip)]
    pub identity: String,
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

/// A code finding plus its durable adjudication state.
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
    /// Metric observed when the verdict was recorded (loc, complexity, …).
    /// Absent on pre-banding adjudications — those fall back to hash-only stale.
    #[serde(default)]
    metric: Option<u64>,
    #[serde(rename = "at")]
    _at: String,
}

/// Resolving adjudications (`justified`/`rejected`/`deferred`/`duplicate`) stay
/// settled across content-hash churn unless the finding's metric worsens by more
/// than this relative band (or absolute floor). `needed`/`blocked` still reopen
/// on any hash change — those are open work.
const RESOLVING_METRIC_BAND: f64 = 0.10;
const RESOLVING_METRIC_FLOOR: u64 = 50;

fn resolving_verdict(verdict: &str) -> bool {
    matches!(
        verdict,
        "justified" | "rejected" | "deferred" | "duplicate"
    )
}

/// Whether a resolving adjudication should reopen: metric grew past the band,
/// or (legacy) hash changed with no stamped metric.
fn resolving_is_stale(
    adj: &Adjudication,
    current_hash: Option<&String>,
    current_metric: Option<u64>,
) -> bool {
    let hash_changed = !adj.hash.is_empty() && current_hash != Some(&adj.hash);
    if !hash_changed {
        return false;
    }
    match (adj.metric, current_metric) {
        (Some(recorded), Some(now)) => {
            let floor =
                RESOLVING_METRIC_FLOOR.max((recorded as f64 * RESOLVING_METRIC_BAND).ceil() as u64);
            now > recorded.saturating_add(floor)
        }
        // Legacy adjudications without a metric: keep hash-only stale (safe).
        _ => true,
    }
}

/// The deterministic Finding det_key for a smell identity. `sync` materializes
/// smell findings under this key; `loom smells` joins live smells against
/// durable adjudications through it.
pub fn smell_det_key(identity: &str) -> String {
    format!("smell:{identity}")
}

/// Durable adjudication `(verdict, reason)` recorded for a node id, if any.
/// Reads the asserted `adjudication` facet directly, so it also resolves for
/// ids whose derived node has not been rebuilt yet.
pub fn adjudication_of(store: &Store, node_id: &str) -> Result<Option<(String, String)>> {
    let Some(raw) = store.get_facet(node_id, TargetKind::Node, "adjudication")? else {
        return Ok(None);
    };
    let Ok(adj) = serde_json::from_str::<Adjudication>(&raw) else {
        return Ok(None);
    };
    if !matches!(
        adj.verdict.as_str(),
        "needed" | "justified" | "rejected" | "deferred" | "blocked" | "duplicate"
    ) {
        return Ok(None);
    }
    Ok(Some((adj.verdict, adj.reason)))
}

/// Whether a live smell carries a durable resolving adjudication — an outcome
/// that no longer counts as open. `needed`/`blocked` remain open work, as does
/// an untriaged smell.
pub fn smell_has_resolving_adjudication(store: &Store, identity: &str) -> Result<bool> {
    let id = Store::derived_node_id(NodeType::Finding, &smell_det_key(identity));
    Ok(matches!(
        adjudication_of(store, &id)?,
        Some((v, _)) if matches!(v.as_str(), "justified" | "rejected" | "deferred" | "duplicate")
    ))
}

// ---- smells ----------------------------------------------------------------

pub fn smells(store: &Store) -> Result<Vec<Smell>> {
    let snap = store.snapshot()?;
    let intents = active_intents(&snap);

    // shared indices (all borrow `snap`)
    let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for e in implements_edges(&snap) {
        // Only `realizes` groundings confer ownership; a `consumes`/`configures`/
        // `verifies` edge (or a superseded one) does not put the file in an
        // intent's cluster. Feeding non-realizing edges here would leak consumer
        // surfaces into ownership/coupling/layering/duplication smells.
        if edge_is_superseded(&snap, &e.id) || edge_role(&snap, &e.id) != GroundingRole::Realizes {
            continue;
        }
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
    let id_to_path: BTreeMap<&str, &str> = snap
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::CodeFile)
        .map(|n| (n.id.as_str(), n.name.as_str()))
        .collect();
    let tags_by_intent = tags_by_node(&snap);

    let thresholds = crate::thresholds::load(store)?;
    let mut out = Vec::new();
    out.extend(ownership_smells(&snap, &owners, thresholds.max_file_owners));
    out.extend(consumer_owned_file_smells(&snap, &owners));
    out.extend(undeclared_coupling_smells(
        &snap,
        &owners,
        &imports,
        &path_to_id,
        &id_to_path,
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
        &id_to_path,
    )?);
    out.extend(disclosure_smells(&intents, &tags_by_intent));
    out.extend(journey_proof_smells(&snap, &intents));
    out.extend(vague_intent_smells(&intents));
    out.extend(pack_drift_smells(&snap));
    Ok(out)
}

/// Seeded quality rules whose stored bodies drifted from the shipped pack
/// definition (typically after a loom upgrade enriched the pack with patterns
/// or examples). Asserted nodes are never rewritten by machine (INV-5): the
/// remedy is the explicit, idempotent re-seed — or keeping a deliberate
/// customization, in which case the smell is the standing record of it.
fn pack_drift_smells(snap: &Snapshot) -> Vec<Smell> {
    let mut drifted: BTreeMap<String, usize> = BTreeMap::new();
    for n in &snap.nodes {
        if n.node_type != NodeType::QualityRule {
            continue;
        }
        let Some(pack_name) = n.body.get("pack").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(rule) = crate::packs::pack(pack_name)
            .iter()
            .find(|r| r.name == n.name)
        else {
            continue;
        };
        if crate::packs::rule_body(pack_name, rule) != n.body {
            *drifted.entry(pack_name.to_string()).or_default() += 1;
        }
    }
    drifted
        .into_iter()
        .map(|(pack, count)| Smell {
            kind: "pack_drift".into(),
            message: format!(
                "{count} seeded rule(s) in pack '{pack}' drifted from the shipped definition \
                 (missing newer guidance like patterns/examples, or locally customized)"
            ),
            remedy: format!(
                "loom rule seed {pack}  (idempotent refresh) — or keep the customization and \
                 accept this smell as its record"
            ),
            identity: format!("pack_drift:{pack}"),
        })
        .collect()
}

/// tangled files (more than `max_owners` realizing owners) and overlapping
/// ownership (exactly 2 owners with no relationship recorded between them).
fn ownership_smells<'a>(
    snap: &'a Snapshot,
    owners: &BTreeMap<&'a str, Vec<&'a str>>,
    max_owners: usize,
) -> Vec<Smell> {
    let mut out = Vec::new();
    for (cf, ids) in owners {
        if ids.len() > max_owners {
            out.push(Smell {
                kind: "tangled_file".into(),
                message: format!(
                    "{} is implemented by {} intents",
                    node_name(snap, cf),
                    ids.len()
                ),
                remedy: "split the file or the intents; one file, one cohesive responsibility"
                    .into(),
                identity: format!("tangled_file:{cf}"),
            });
        }
        if ids.len() == 2 && !edge_between(snap, ids[0], ids[1]) {
            let (a, b) = if ids[0] <= ids[1] {
                (ids[0], ids[1])
            } else {
                (ids[1], ids[0])
            };
            out.push(Smell {
                kind: "overlapping_ownership".into(),
                message: format!(
                    "'{}' and '{}' both own {} with no relationship recorded",
                    node_name(snap, ids[0]),
                    node_name(snap, ids[1]),
                    node_name(snap, cf),
                ),
                remedy: "record a relates edge, or split ownership".into(),
                identity: format!("overlapping_ownership:{cf}:{a}:{b}"),
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
    id_to_path: &BTreeMap<&'a str, &'a str>,
) -> Vec<Smell> {
    let mut out = Vec::new();
    for (file_id, imps) in imports {
        let Some(a_owners) = owners.get(file_id.as_str()) else {
            continue;
        };
        let Some(a_path) = id_to_path.get(file_id.as_str()).copied() else {
            continue;
        };
        for imp in imps {
            // resolve an imported module path or file path to a registered codefile
            let Some(b_id) = resolve_import(imp, a_path, path_to_id) else {
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
                    identity: format!("undeclared_coupling:{file_id}:{b_id}"),
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
                    identity: format!("duplicated_responsibility:{a}:{b}:{}", shared[0]),
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
    id_to_path: &BTreeMap<&'a str, &'a str>,
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
        // All owner layers — a multi-owner file must not be judged by only its
        // first owner's layer (L-3).
        let a_layers: Vec<&String> = a_owners.iter().filter_map(|o| layer_of.get(*o)).collect();
        if a_layers.is_empty() {
            continue;
        }
        let Some(a_path) = id_to_path.get(file_id.as_str()).copied() else {
            continue;
        };
        for imp in imps {
            let Some(b_id) = resolve_import(imp, a_path, path_to_id) else {
                continue;
            };
            if b_id == file_id.as_str() {
                continue;
            }
            let Some(b_owners) = owners.get(b_id) else {
                continue;
            };
            let Some(b_path) = id_to_path.get(b_id).copied() else {
                continue;
            };
            // A `mod`-tree parent/child edge is structural (a `mod x;` you
            // cannot invert), not an architectural layer crossing.
            if is_module_tree_edge(a_path, b_path) {
                continue;
            }
            let b_layers: Vec<&String> = b_owners.iter().filter_map(|o| layer_of.get(*o)).collect();
            // A multi-owner file spans several layers. Flag only when EVERY
            // source-owner layer sits strictly BELOW EVERY target layer in the
            // declared order — i.e. every owner pairing points up (min source
            // rank > max target rank), so no legal (non-inverting) pairing
            // exists. If one does, the file has a defensible home on both sides
            // and this is not an inversion (L-3). Report the widest gap: the
            // highest source layer against the lowest target layer.
            let a_top = a_layers
                .iter()
                .filter_map(|l| rank.get(l.as_str()).map(|r| (*r, l.as_str())))
                .min();
            let b_bottom = b_layers
                .iter()
                .filter_map(|l| rank.get(l.as_str()).map(|r| (*r, l.as_str())))
                .max();
            if let (Some((ra, a)), Some((rb, b))) = (a_top, b_bottom) {
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
                        identity: format!("layering_violation:{file_id}:{b_id}"),
                    });
                }
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
            identity: "duplicate_detection_unarmed".into(),
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
            identity: format!("{kind}:{}", intent.id),
        });
    }
    out
}

/// Hedge terms that assert quality without naming an observable outcome. A
/// description leaning on these is unfalsifiable — every verdict recorded
/// against it is judgment theater, so the graph's ceiling drops to prose.
const HEDGE_TERMS: &[&str] = &[
    "handle",
    "handles",
    "handled",
    "handling",
    "properly",
    "correctly",
    "robustly",
    "gracefully",
    "appropriately",
    "sanely",
];

/// Signals that a description names something observable: an ACTION verb,
/// stem-matched against word starts ("returns"/"returning", "rejects"/
/// "rejected"). Deliberately excludes bare nouns ("error", "failure",
/// "timeout") and bare conditionals ("when", "if") — "handles errors when
/// they occur" names no outcome; "returns an error" does.
const OUTCOME_STEMS: &[&str] = &[
    "return",
    "emit",
    "exit",
    "write",
    "wrote",
    "read",
    "reject",
    "refuse",
    "accept",
    "retr",
    "produce",
    "print",
    "send",
    "sent",
    "persist",
    "record",
    "render",
    "redirect",
    "respond",
    "display",
    "creat",
    "delet",
    "insert",
    "remov",
    "updat",
    "append",
    "increment",
    "rais",
    "throw",
    "sav",
    "load",
    "pars",
    "notif",
    "report",
];

/// Intents whose description hedges ("handles errors correctly") without one
/// observable outcome. Falsifiability lint, not a style gate: it fires only on
/// hedge + nothing checkable, and is adjudicable like every smell — a
/// deliberate summary-level intent gets a `justified` finding verdict.
fn vague_intent_smells(intents: &[&Node]) -> Vec<Smell> {
    let mut out = Vec::new();
    for intent in intents {
        let desc = intent.description.to_lowercase();
        let words: Vec<&str> = desc
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        let Some(hedge) = words.iter().find(|w| HEDGE_TERMS.contains(*w)) else {
            continue;
        };
        let concrete = desc.chars().any(|c| c.is_ascii_digit())
            || ['`', '"', '\''].iter().any(|q| desc.contains(*q))
            || desc.contains("::")
            || desc.contains('/')
            || desc.contains("()")
            || words
                .iter()
                .any(|w| OUTCOME_STEMS.iter().any(|s| w.starts_with(s)))
            // "… by <doing something>": an outcome named as a gerund.
            || words
                .windows(2)
                .any(|p| p[0] == "by" && p[1].ends_with("ing"));
        if concrete {
            continue;
        }
        out.push(Smell {
            kind: "vague_intent".into(),
            message: format!(
                "intent '{}' hedges ('{hedge}') without an observable outcome — nothing in its description can be falsified",
                intent.name
            ),
            remedy: "state what an observer could check (inputs → outputs, emitted errors, side effects) via loom intent update --description --reword; or adjudicate the finding justified"
                .into(),
            identity: format!("vague_intent:{}", intent.id),
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
        if !matches!(
            adj.verdict.as_str(),
            "needed" | "justified" | "rejected" | "deferred" | "blocked" | "duplicate"
        ) {
            out.push(FindingView {
                node,
                state: "untriaged".into(),
                reason: String::new(),
                stale: false,
            });
            continue;
        }
        let current_hash = store.finding_codefile_hash(&node.id)?;
        let current_metric = store.finding_metric(&node.id)?;
        let stale = if resolving_verdict(&adj.verdict) {
            resolving_is_stale(&adj, current_hash.as_ref(), current_metric)
        } else {
            // Open work (needed/blocked): any codefile edit reopens triage.
            !adj.hash.is_empty() && current_hash.as_ref() != Some(&adj.hash)
        };
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
        // Vacuity audit (INV-4/6): mirror the record_verdict write boundary so
        // legacy/imported verdicts recorded before the gate are still caught.
        // passing/failing/independent require substantive criterion AND evidence;
        // blocked requires a substantive reason (stored in evidence).
        if e.truth_class == TruthClass::Asserted {
            let vacuous_field = match e.status {
                crate::model::InspectionStatus::Passing
                | crate::model::InspectionStatus::Failing
                | crate::model::InspectionStatus::Independent => {
                    if crate::model::is_placeholder(&e.criterion) {
                        Some("criterion")
                    } else if crate::model::is_placeholder(&e.evidence) {
                        Some("evidence")
                    } else {
                        None
                    }
                }
                crate::model::InspectionStatus::Blocked => {
                    crate::model::is_placeholder(&e.evidence).then_some("reason")
                }
                _ => None,
            };
            if let Some(field) = vacuous_field {
                issues.push(DoctorIssue {
                    kind: "vacuous_verdict".into(),
                    message: format!(
                        "{} edge {} is {} with empty or placeholder {field}",
                        e.kind, e.id, e.status
                    ),
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
    // Orphaned upstream intents: shadows whose alias no longer matches any
    // linked upstream (after `graph unlink`). The node persists deliberately
    // (never auto-deleted), but the unlinked state is worth flagging.
    if let Ok(entries) = crate::federation::read_upstream_entries(store) {
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
                        "upstream intent '{}' has no linked upstream (alias '{}' not in registry)",
                        n.name, alias
                    ),
                });
            }
        }
    }
    Ok(issues)
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
    for node in snap
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::Intent)
    {
        let idx = graph.add_node(node.id.as_str());
        indices.insert(node.id.as_str(), idx);
    }

    let mut self_loops = BTreeSet::new();
    for edge in snap.edges.iter().filter(|e| e.kind == EdgeKind::Hierarchy) {
        if edge.from_id == edge.to_id {
            self_loops.insert(edge.from_id.as_str());
        }
        let (Some(&from), Some(&to)) = (
            indices.get(edge.from_id.as_str()),
            indices.get(edge.to_id.as_str()),
        ) else {
            continue;
        };
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

/// Grounding role of an `implements` edge, read from the snapshot facets — a
/// pure mirror of `Store::grounding_role`. A missing `role` facet reads as
/// `Realizes` (pre-role default).
fn edge_role(snap: &Snapshot, edge_id: &str) -> GroundingRole {
    snap.facets
        .iter()
        .find(|f| f.target_kind == TargetKind::Edge && f.target_id == edge_id && f.key == "role")
        .and_then(|f| f.value.parse().ok())
        .unwrap_or(GroundingRole::Realizes)
}

/// Whether an edge was superseded by a `rehome` (bears a `superseded_by` facet).
fn edge_is_superseded(snap: &Snapshot, edge_id: &str) -> bool {
    snap.facets.iter().any(|f| {
        f.target_kind == TargetKind::Edge && f.target_id == edge_id && f.key == "superseded_by"
    })
}

/// The top-level directory ("cluster") of a file path — the first path segment.
/// Two files in genuinely different areas of the tree (e.g. `routes/` vs `src/`)
/// have different clusters; sub-directories under a shared root do not.
fn dir_cluster(path: &str) -> &str {
    path.split('/').next().unwrap_or(path)
}

/// Resolve an extracted import to a registered codefile id. Imports from a Rust
/// file (`.rs`) are module syntax — resolved through Rust's module→file mapping
/// by `resolve_rust_module`, so they never leak across crates or to the standard
/// library, and a bare `use serde;` never falls back to a loose path match
/// (H-7, and the cross-crate/stdlib mis-resolution that fabricated phantom
/// layering violations). Other languages' path-style imports (`./foo`, `pkg/bar`)
/// match by longest path substring.
fn resolve_import<'a>(
    imp: &str,
    importer: &str,
    path_to_id: &BTreeMap<&'a str, &'a str>,
) -> Option<&'a str> {
    if importer.ends_with(".rs") || imp.contains("::") {
        return resolve_rust_module(imp, importer, path_to_id);
    }
    // Longest path match wins (more specific), for path-style imports.
    let mut best: Option<(&str, usize)> = None;
    for (p, id) in path_to_id {
        if (imp.contains(*p) || p.contains(imp))
            && best.map(|(_, len)| p.len() > len).unwrap_or(true)
        {
            best = Some((*id, p.len()));
        }
    }
    best.map(|(id, _)| id)
}

/// Resolve a Rust `use` path to a registered codefile, honoring the module tree.
///
///  * `crate`/`self`/`super` stay inside the importing file's own crate and are
///    anchored to its module path, so they never resolve to a same-named file in
///    another crate.
///  * `std`/`core`/`alloc` (and any extern head that is not a unique crate
///    directory here) resolve to nothing — a stdlib path is not a repo file, and
///    for a smell that asks a human to invert a dependency an unresolved import
///    is safer than a confidently wrong one.
///  * candidate paths are constructed exactly (`{base}{frag}.rs`,
///    `{base}{frag}/mod.rs`) and looked up, never matched by an unbounded suffix
///    — so `delivery` cannot hit `commerce_delivery.rs`.
fn resolve_rust_module<'a>(
    imp: &str,
    importer: &str,
    path_to_id: &BTreeMap<&'a str, &'a str>,
) -> Option<&'a str> {
    let segs: Vec<&str> = imp
        .split("::")
        .map(str::trim)
        .take_while(|s| !s.starts_with('{') && *s != "*" && !s.is_empty())
        .collect();
    let head = *segs.first()?;
    if matches!(head, "std" | "core" | "alloc") {
        return None;
    }
    let (base, rest): (String, &[&str]) = match head {
        "crate" => (crate_src_root(importer), &segs[1..]),
        "self" => (self_dir(importer), &segs[1..]),
        "super" => {
            let k = segs.iter().take_while(|s| **s == "super").count();
            let mut b = self_dir(importer);
            for _ in 0..k {
                b = parent_dir(&b);
            }
            (b, &segs[k..])
        }
        _ => (extern_crate_root(head, path_to_id)?, &segs[1..]),
    };
    // Leading snake_case segments are modules; a CamelCase (type) or trailing
    // item ends the module path. Longest prefix first, shortening so a
    // `crate::a::b::func` still falls back to `a/b.rs`.
    let mods: Vec<&str> = rest
        .iter()
        .copied()
        .take_while(|s| {
            s.chars()
                .next()
                .is_some_and(|c| c.is_lowercase() || c == '_')
        })
        .collect();
    for take in (1..=mods.len()).rev() {
        let frag = mods[..take].join("/");
        for cand in [format!("{base}{frag}.rs"), format!("{base}{frag}/mod.rs")] {
            if let Some(id) = path_to_id.get(cand.as_str()) {
                return Some(*id);
            }
        }
    }
    None
}

/// The crate source root of a codefile path — the prefix through the `src/`
/// segment (`pulse-machine/src/webhook.rs` → `pulse-machine/src/`), or the
/// top-level directory when there is no `src/`. `crate::` paths anchor here.
fn crate_src_root(path: &str) -> String {
    let segs: Vec<&str> = path.split('/').collect();
    if let Some(i) = segs.iter().position(|s| *s == "src") {
        format!("{}/", segs[..=i].join("/"))
    } else if segs.len() > 1 {
        format!("{}/", segs[0])
    } else {
        String::new()
    }
}

/// The directory holding a file's own submodules (`self::`): `a/b/foo.rs` →
/// `a/b/foo/`, but a `mod.rs`/`lib.rs`/`main.rs` → its containing directory.
fn self_dir(path: &str) -> String {
    let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    if matches!(file, "mod.rs" | "lib.rs" | "main.rs") {
        prefix
    } else {
        format!("{prefix}{}/", file.strip_suffix(".rs").unwrap_or(file))
    }
}

/// The parent of a trailing-slash directory (`a/b/c/` → `a/b/`, `a/` → ``).
fn parent_dir(dir: &str) -> String {
    match dir.strip_suffix('/').unwrap_or(dir).rsplit_once('/') {
        Some((p, _)) => format!("{p}/"),
        None => String::new(),
    }
}

/// The crate name carried by a source root (`pulse-machine/src/` →
/// `pulse-machine`, `crates/foo/src/` → `foo`) — the basename of the directory
/// that holds `src/`. Used to match an extern crate head against a directory.
fn crate_name_of_root(root: &str) -> &str {
    let trimmed = root.strip_suffix('/').unwrap_or(root);
    let dir = trimmed.strip_suffix("/src").unwrap_or(trimmed);
    dir.rsplit('/').next().unwrap_or(dir)
}

/// The single crate source root whose crate name matches an extern crate head
/// (dash/underscore-insensitive). None when zero or more than one crate matches
/// — an ambiguous or absent crate is never resolved. Keyed on `crate_src_root`
/// so a nested workspace (`crates/foo/src/…`) resolves by crate, not by the
/// shared `crates/` top directory.
fn extern_crate_root(head: &str, path_to_id: &BTreeMap<&str, &str>) -> Option<String> {
    let want = head.replace('_', "-");
    let roots: BTreeSet<String> = path_to_id
        .keys()
        .map(|p| crate_src_root(p))
        .filter(|root| crate_name_of_root(root).replace('_', "-") == want)
        .collect();
    if roots.len() == 1 {
        roots.into_iter().next()
    } else {
        None
    }
}

/// A same-crate `mod`-tree edge — importer and importee share a crate source
/// root and one is an ancestor/descendant module of the other (`approval.rs` ↔
/// `approval/completion.rs`). A `mod x;` declaration is structural, not an
/// invertible architectural dependency, so it never counts as a layer crossing.
fn is_module_tree_edge(a: &str, b: &str) -> bool {
    crate_src_root(a) == crate_src_root(b)
        && (b.starts_with(&self_dir(a)) || a.starts_with(&self_dir(b)))
}

/// consumer-owned file: a file whose sole realizing owner is an intent whose
/// other realizing files all live in a different top-level cluster. This is the
/// systematic mis-attachment the role split exists to catch — a consumer
/// surface (a page that calls a backend route, say) grounded as `realizes` to
/// the behavior it merely exercises, which then silently satisfies coverage for
/// a realizing intent that does not yet exist.
fn consumer_owned_file_smells<'a>(
    snap: &'a Snapshot,
    owners: &BTreeMap<&'a str, Vec<&'a str>>,
) -> Vec<Smell> {
    let files_by_intent = files_by_intent(snap, owners);
    let mut out = Vec::new();
    for (file_id, intent_ids) in owners {
        if intent_ids.len() != 1 {
            continue; // multiple realizing owners — a genuine shared/vertical file
        }
        let owner = intent_ids[0];
        let Some(owner_files) = files_by_intent.get(owner) else {
            continue;
        };
        if owner_files.len() < 2 {
            continue; // owner realizes only this file — nothing to contrast
        }
        let this_path = node_name(snap, file_id);
        let this_cluster = dir_cluster(&this_path).to_string();
        let others_elsewhere = owner_files.iter().filter(|f| **f != *file_id).all(|f| {
            let p = node_name(snap, f);
            dir_cluster(&p) != this_cluster
        });
        if others_elsewhere {
            // The single realizing edge for this file — name it so the remedy is
            // copy-paste runnable (§3.3).
            let edge = implements_edges(snap).find(|e| {
                e.to_id == *file_id
                    && e.from_id == owner
                    && !edge_is_superseded(snap, &e.id)
                    && edge_role(snap, &e.id) == GroundingRole::Realizes
            });
            let edge_ref = edge.map(|e| &e.id[..8]).unwrap_or("<edge>");
            out.push(Smell {
                kind: "consumer_owned_file".into(),
                message: format!(
                    "{} is realized only by '{}', whose other files live in a different area — this looks like a consumer surface owned by the behavior it calls",
                    this_path,
                    node_name(snap, owner)
                ),
                remedy: format!(
                    "if this file only exercises that behavior across a seam: `loom edge set-role {edge_ref} consumes --reason '…'`, then create a realizing intent for this surface and ground it --role realizes"
                ),
                identity: format!("consumer_owned_file:{file_id}:{owner}"),
            });
        }
    }
    out
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
            let layers: Vec<String> = serde_json::from_str(&v)
                .with_context(|| format!("meta.layer_order is malformed JSON: {v}"))?;
            Ok(if layers.is_empty() {
                None
            } else {
                Some(layers)
            })
        }
        None => Ok(None),
    }
}

pub(crate) fn quantile(sorted: &[f64], q: f64) -> f64 {
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
