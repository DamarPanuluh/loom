//! Signal plane — smells (structural), debt (statistical), doctor (integrity).
//!
//! Plane boundary (INV-3): smells are structural findings computed from graph
//! shape; debt is a statistical feed computed on demand (optionally reading VCS
//! history) and NEVER stored as an edge or counted as required work. Doctor
//! audits integrity after the fact.
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

#[path = "signal/debt.rs"]
mod debt;
pub use debt::{debt, debt_cluster_id, DebtCluster};
pub(crate) use debt::{CO_CHANGE_MAX_COMMITS, GIT_TIMEOUT_SECS};

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

/// A finding's judgment, reassembled from the fact (verdict + reason) and the
/// derived stamp (what the world looked like when it was judged). Split so the
/// judgment travels through the write boundary while its bookkeeping stays
/// derived — a stamp cannot be used to forge a verdict.
struct Adjudication {
    verdict: String,
    reason: String,
    hash: String,
    /// Metric observed when the verdict was recorded (loc, complexity, …).
    /// Absent on pre-banding adjudications — those fall back to hash-only stale.
    metric: Option<u64>,
}

#[derive(Deserialize, Default)]
struct AdjudicationStamp {
    #[serde(default)]
    hash: String,
    #[serde(default)]
    metric: Option<u64>,
}

/// Read a finding's adjudication: the fact carries the judgment, the derived
/// stamp carries the staleness band.
fn adjudication(store: &Store, node_id: &str) -> Result<Option<Adjudication>> {
    let Some(view) = store.fact(
        &crate::store::Subject::Node(node_id.to_string()),
        crate::model::Claim::Adjudication,
    )?
    else {
        return Ok(None);
    };
    let stamp: AdjudicationStamp = store
        .get_facet(node_id, TargetKind::Node, "adjudication_stamp")?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    Ok(Some(Adjudication {
        verdict: view.fact.state,
        reason: view.fact.criterion,
        hash: stamp.hash,
        metric: stamp.metric,
    }))
}

/// Resolving adjudications (`justified`/`rejected`/`deferred`/`duplicate`/`resolved`) stay
/// settled across content-hash churn unless the finding's metric worsens by more
/// than this relative band (or absolute floor). `needed`/`blocked` still reopen
/// on any hash change — those are open work.
const RESOLVING_METRIC_BAND: f64 = 0.10;
const RESOLVING_METRIC_FLOOR: u64 = 50;

fn resolving_verdict(verdict: &str) -> bool {
    matches!(
        verdict,
        "justified" | "rejected" | "deferred" | "duplicate" | "resolved"
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
    let Some(adj) = adjudication(store, node_id)? else {
        return Ok(None);
    };
    if !matches!(
        adj.verdict.as_str(),
        "needed" | "justified" | "rejected" | "deferred" | "blocked" | "duplicate" | "resolved"
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
        Some((v, _)) if matches!(v.as_str(), "justified" | "rejected" | "deferred" | "duplicate" | "resolved")
    ))
}

// ---- smells ----------------------------------------------------------------

pub fn smells(store: &Store) -> Result<Vec<Smell>> {
    let snap = store.snapshot()?;
    let intents = active_intents(&snap);

    // shared indices (all borrow `snap`)
    // Only ACTIVE intents own anything. `active_intents` above already excludes
    // deprecated behaviors, but the ownership index did not — so a retired
    // intent kept co-owning its files and kept generating structural smells
    // about them. Retiring a behavior is loom's own sanctioned move when code
    // is deliberately removed; it has to actually remove the behavior from
    // every derived view, not just the one that lists intents.
    let active_ids: BTreeSet<&str> = intents.iter().map(|n| n.id.as_str()).collect();
    let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for e in implements_edges(&snap) {
        if !active_ids.contains(e.from_id.as_str()) {
            continue;
        }
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
    let rel_adjacency = relationship_adjacency(&snap);

    let mut out = Vec::new();
    out.extend(ownership_smells(&snap, &owners, &rel_adjacency));
    out.extend(shared_proof_command_smells(&snap));
    out.extend(unstable_proof_smells(&snap));
    out.extend(consumer_owned_file_smells(&snap, &owners));
    out.extend(undeclared_coupling_smells(
        &snap,
        &owners,
        &imports,
        &path_to_id,
        &id_to_path,
        &rel_adjacency,
    ));
    out.extend(duplicated_responsibility_smells(
        &snap,
        &intents,
        &owners,
        &tags_by_intent,
        &rel_adjacency,
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

/// Undeclared shared ownership: ≥2 realizing owners of one file that do **not**
/// form one connected neighborhood via relationship edges (relates / hierarchy /
/// scenario-of / …).
///
/// Connectedness is the gate — not an owner-count threshold. A parent plus its
/// scenarios sharing a module (a star) is declared cohesion and stays silent.
/// Two unrelated intents on one file fire. Ten related intents stay silent
/// (maybe large, but not undeclared). Former `overlapping_ownership` is the
/// two-owner case of the same rule; identity is always `tangled_file:{cf}`.
/// Proofs that changed their mind about unchanged code.
///
/// `record_stability` flags a validation whose outcome flipped while its anchor
/// set stood still. That is a proof whose colour depends on something other
/// than the code — scheduling, ordering, a clock, the network — and a proof
/// like that establishes nothing on the run you happened to observe.
///
/// It matters most exactly where it is least visible: the INV-8 proofs, which
/// defend the human-authority ratification seam, were flaky for want of a
/// lock and passed four runs out of five.
///
/// SCOPE, stated so the claim is not oversold: this reports. Being a smell it
/// blocks the `sound` rung like any other, but it does NOT feed proof strength
/// and does NOT gate `proven` — an unstable proof still counts as a proof for
/// the ladder. Wiring it into strength would re-open every affected claim at
/// once, which is the debt wall the calibration ratchet exists to prevent. If
/// that trade is ever revisited, revisit it deliberately rather than by
/// widening this.
fn unstable_proof_smells(snap: &Snapshot) -> Vec<Smell> {
    let flagged = facet_map(snap, "proof_unstable");
    let mut out = Vec::new();
    for n in &snap.nodes {
        if n.node_type != NodeType::Validation {
            continue;
        }
        let Some(detail) = flagged.get(n.id.as_str()) else {
            continue;
        };
        out.push(Smell {
            kind: "unstable_proof".into(),
            message: format!(
                "proof '{}' reported {detail} — its outcome does not depend only on the code it covers",
                n.name
            ),
            remedy: "make the proof deterministic (serialize shared state, pin clocks/ordering, remove network reliance), then adjudicate this smell (deferred/resolved/justified) — agreement alone no longer clears it"
                .into(),
            identity: format!("unstable_proof:{}", n.id),
        });
    }
    out
}

/// Behaviors whose proof is the SAME command.
///
/// If one command proves seven behaviors, it is at most proving one of them.
/// The others inherit its green from whatever it really exercises, and a claim
/// stays proven for exactly as long as some unrelated suite keeps passing.
///
/// This is not hypothetical. An intent claiming "a locator that cannot resolve
/// falls back to file-scope reopening" carried TWO passing validations, both
/// running `cargo test --test ring6 -q`, while thirteen groundings with
/// unresolvable locators sat green underneath it — the behavior did not exist
/// at all. Nothing caught it: `proof_too_shallow_for_intent` gates only
/// user-visible intents, and the strength machinery already grades these S2
/// (no call witness) without any rung consuming that below user_visible.
///
/// Reported, not gated. A ring genuinely covering several behaviors is a
/// legitimate shape, so this wants a verdict with a reason — the same way
/// measured structural debt is judged rather than auto-blocked.
fn shared_proof_command_smells(snap: &Snapshot) -> Vec<Smell> {
    // validation id -> EVERY intent it validates. One Validation may carry
    // several Validates edges; keeping only the last one would attribute its
    // command to a single behavior and hide the very collision this reports.
    let mut proves: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for e in &snap.edges {
        if e.kind == EdgeKind::Validates {
            proves
                .entry(e.from_id.as_str())
                .or_default()
                .insert(e.to_id.as_str());
        }
    }
    // command -> the distinct behaviors leaning on it
    let mut by_command: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for n in &snap.nodes {
        if n.node_type != NodeType::Validation {
            continue;
        }
        let Some(command) = n.body.get("command").and_then(|v| v.as_str()) else {
            continue;
        };
        if command.trim().is_empty() {
            continue;
        }
        if let Some(intents) = proves.get(n.id.as_str()) {
            by_command.entry(command).or_default().extend(intents);
        }
    }

    let mut out = Vec::new();
    for (command, intents) in by_command {
        if intents.len() < 2 {
            continue;
        }
        // Identity keys on the COMMAND, not on the intents: the set of
        // behaviors leaning on one suite changes as work lands, and an
        // adjudication should survive that rather than re-open on every new
        // sibling.
        out.push(Smell {
            kind: "shared_proof_command".into(),
            message: format!(
                "{} behaviors are proved by the same command `{}` — it can be exercising at most one of them",
                intents.len(),
                command
            ),
            remedy: "narrow each proof to the behavior it proves (a single test, or a journey step asserting that behavior's output), or record why one suite legitimately proves them all"
                .into(),
            identity: format!("shared_proof_command:{command}"),
        });
    }
    out
}

fn ownership_smells<'a>(
    snap: &'a Snapshot,
    owners: &BTreeMap<&'a str, Vec<&'a str>>,
    adj: &RelAdjacency,
) -> Vec<Smell> {
    let mut out = Vec::new();
    for (cf, ids) in owners {
        if ids.len() < 2 || owners_connected(adj, ids) {
            continue;
        }
        let message = if ids.len() == 2 {
            format!(
                "'{}' and '{}' both realize {} with no relationship connecting them",
                node_name(snap, ids[0]),
                node_name(snap, ids[1]),
                node_name(snap, cf),
            )
        } else {
            format!(
                "{} is realized by {} intents with no recorded relationship connecting them",
                node_name(snap, cf),
                ids.len()
            )
        };
        out.push(Smell {
            kind: "tangled_file".into(),
            message,
            remedy: "split the file or the intents, or record relates/hierarchy/scenario-of so the co-owners form one connected neighborhood"
                .into(),
            identity: format!("tangled_file:{cf}"),
        });
    }
    out
}

/// True when every co-owner is reachable from every other via relationship
/// edges (undirected BFS on the owner subgraph). A star (parent ↔ scenarios)
/// counts; a pairwise clique is not required.
fn owners_connected(adj: &RelAdjacency, ids: &[&str]) -> bool {
    if ids.len() < 2 {
        return true;
    }
    let set: BTreeSet<&str> = ids.iter().copied().collect();
    let start = ids[0];
    let mut seen = BTreeSet::new();
    let mut stack = vec![start];
    while let Some(cur) = stack.pop() {
        if !seen.insert(cur) {
            continue;
        }
        for other in &set {
            if *other != cur && edge_between(adj, cur, other) {
                stack.push(*other);
            }
        }
    }
    seen.len() == set.len()
}

/// undeclared coupling: codefile A imports B, but their owning intents share no
/// edge — a dependency the graph hasn't accounted for.
fn undeclared_coupling_smells<'a>(
    snap: &'a Snapshot,
    owners: &BTreeMap<&'a str, Vec<&'a str>>,
    imports: &BTreeMap<String, Vec<String>>,
    path_to_id: &BTreeMap<&'a str, &'a str>,
    id_to_path: &BTreeMap<&'a str, &'a str>,
    adj: &RelAdjacency,
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
                .any(|a| b_owners.iter().any(|b| a == b || edge_between(adj, a, b)));
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
    adj: &RelAdjacency,
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
            if fa.is_disjoint(&fb) && !edge_between(adj, a, b) {
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

/// The derived grade, read out of the snapshot's facets. Smells run over a
/// snapshot rather than the store, and the grade is a derived facet, so it
/// travels with everything else the detector reads.
fn strength_from(snap: &Snapshot, validation_id: &str) -> crate::proofstrength::Strength {
    snap.facets
        .iter()
        .find(|f| {
            f.target_id == validation_id
                && f.target_kind == crate::model::TargetKind::Node
                && f.key == "proof_strength"
        })
        .and_then(|f| serde_json::from_str::<crate::proofstrength::StrengthWitness>(&f.value).ok())
        .and_then(|w| crate::proofstrength::Strength::parse(&w.grade))
        .unwrap_or(crate::proofstrength::Strength::S0)
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
        // Gate on the PROPERTY, not the label. `proof_kind == "journey"` was a
        // stand-in for "this proof exercises the real path" from before strength
        // was derived — and S3 now says exactly that, checked from the call
        // graph rather than taken from a tag someone typed. Requiring both
        // meant a suite whose call closure demonstrably reaches the grounded
        // symbol still read as "does not reach the code it proves", which is
        // the opposite of what the graph knew. The boundary crossing a journey
        // is really for is S5, and it is scored there.
        let has_journey = validations.iter().any(|(validation, edge)| {
            edge.status == InspectionStatus::Passing
                && validation.status == "passed"
                && strength_from(snap, &validation.id) >= crate::proofstrength::Strength::END_TO_END
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
                "user-visible intent '{}' has no linked validation; boundary-facing behavior needs an S3-or-stronger journey proof",
                intent.name
            )
        } else {
            format!(
                "user-visible intent '{}' has validations but no passing S3-or-stronger journey proof",
                intent.name
            )
        };
        out.push(Smell {
            kind: kind.into(),
            message,
            remedy:
                "compile and run the current Journey proof profile until it reaches S3 — \
                 Loom observes positive structured output assertions, the compiled Validation Calls \
                 the accepted surface, and its Exercises entry reaches the grounded symbol"
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
        let Some(adj) = adjudication(store, &node.id)? else {
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
            "needed" | "justified" | "rejected" | "deferred" | "blocked" | "duplicate" | "resolved"
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

/// Findings adjudicated `needed` whose judgment still stands (not stale).
/// Routing splits these by named-repair owner: code edits go to fix,
/// proof-reruns to validate, undeclared coupling to analyze. Stale `needed`
/// findings are excluded — the file changed, so they are back in triage for
/// re-adjudication, and one finding must never sit in two queues.
pub fn needed_findings(store: &Store) -> Result<Vec<FindingView>> {
    Ok(findings_view(store)?
        .into_iter()
        .filter(|fv| fv.state == "needed" && !fv.stale)
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
            let edge_ref = edge.map(|e| crate::model::short(&e.id)).unwrap_or("<edge>");
            out.push(Smell {
                kind: "consumer_owned_file".into(),
                message: format!(
                    "{} is realized only by '{}', whose other files live in a different area — this looks like a consumer surface owned by the behavior it calls",
                    this_path,
                    node_name(snap, owner)
                ),
                remedy: format!(
                    "inspect before changing role: a sibling slice of the same criterion may realize in another directory; if this file only calls that behavior, `loom edge set-role {edge_ref} consumes --reason '…'` and ground whatever criterion actually lives here as realizes (existing intent if it names this slice, or a distinct surface intent minted outside coverage)"
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

/// Undirected adjacency over the relationship edge kinds, built once so the
/// smell detectors do not rescan every edge per pair (`owners_connected`'s BFS
/// alone is otherwise O(cluster² · edges)).
type RelAdjacency<'a> = BTreeMap<&'a str, BTreeSet<&'a str>>;

fn relationship_adjacency(snap: &Snapshot) -> RelAdjacency<'_> {
    let mut adj: RelAdjacency = BTreeMap::new();
    for e in &snap.edges {
        if matches!(
            e.kind,
            EdgeKind::Relates
                | EdgeKind::Requires
                | EdgeKind::Hierarchy
                | EdgeKind::ScenarioOf
                | EdgeKind::VariantOf
                | EdgeKind::Triggers
                | EdgeKind::Sequence
        ) {
            adj.entry(e.from_id.as_str())
                .or_default()
                .insert(e.to_id.as_str());
            adj.entry(e.to_id.as_str())
                .or_default()
                .insert(e.from_id.as_str());
        }
    }
    adj
}

fn edge_between(adj: &RelAdjacency, a: &str, b: &str) -> bool {
    adj.get(a).is_some_and(|s| s.contains(b))
}

fn imports_by_file(snap: &Snapshot) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for f in &snap.facets {
        if f.key == "imports" {
            // A malformed derived facet must not silently disable the
            // coupling/layering checks that read it: name the corruption so
            // the operator can rebuild (`loom sync`) instead of trusting a
            // quietly emptied import map.
            let list: Vec<String> = match serde_json::from_str(&f.value) {
                Ok(list) => list,
                Err(error) => {
                    eprintln!(
                        "warning: derived 'imports' facet on {} is malformed ({error}) — \
                         coupling checks skip this file until the next sync rebuilds it",
                        f.target_id
                    );
                    continue;
                }
            };
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
