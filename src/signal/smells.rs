use super::graph::{edge_is_superseded, edge_role, node_name};
use super::imports::{dir_cluster, is_module_tree_edge, resolve_import};
use crate::model::{
    Edge, EdgeKind, Facet, GroundingRole, InspectionStatus, Node, NodeType, TargetKind,
};
use crate::store::{Snapshot, Store};
use crate::Result;
use anyhow::Context;
use serde::Serialize;
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

// ---- smells ----------------------------------------------------------------

pub fn smells(store: &Store) -> Result<Vec<Smell>> {
    smells_with(store, &store.snapshot()?)
}

/// [`smells`] over a snapshot the caller already holds.
///
/// The whole-graph snapshot is the expensive part, and `loom status` used to
/// build five of them on one command. Callers that already have one pass it
/// here; `smells` stays for the standalone callers.
pub fn smells_with(store: &Store, snap: &Snapshot) -> Result<Vec<Smell>> {
    let intents = active_intents(snap);

    // shared indices (all borrow `snap`)
    // Only ACTIVE intents own anything. `active_intents` above already excludes
    // deprecated behaviors, but the ownership index did not — so a retired
    // intent kept co-owning its files and kept generating structural smells
    // about them. Retiring a behavior is loom's own sanctioned move when code
    // is deliberately removed; it has to actually remove the behavior from
    // every derived view, not just the one that lists intents.
    let active_ids: BTreeSet<&str> = intents.iter().map(|n| n.id.as_str()).collect();
    let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for e in implements_edges(snap) {
        if !active_ids.contains(e.from_id.as_str()) {
            continue;
        }
        // Only `realizes` groundings confer ownership; a `consumes`/`configures`/
        // `verifies` edge (or a superseded one) does not put the file in an
        // intent's cluster. Feeding non-realizing edges here would leak consumer
        // surfaces into ownership/coupling/layering/duplication smells.
        if edge_is_superseded(snap, &e.id) || edge_role(snap, &e.id) != GroundingRole::Realizes {
            continue;
        }
        owners
            .entry(e.to_id.as_str())
            .or_default()
            .push(e.from_id.as_str());
    }
    let imports = imports_by_file(snap);
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
    let tags_by_intent = tags_by_node(snap);
    let rel_adjacency = relationship_adjacency(snap);

    let mut out = Vec::new();
    out.extend(ownership_smells(snap, &owners, &rel_adjacency));
    out.extend(shared_proof_command_smells(snap));
    out.extend(unstable_proof_smells(snap));
    out.extend(consumer_owned_file_smells(snap, &owners));
    out.extend(undeclared_coupling_smells(
        snap,
        &owners,
        &imports,
        &path_to_id,
        &id_to_path,
        &rel_adjacency,
    ));
    out.extend(duplicated_responsibility_smells(
        snap,
        &intents,
        &owners,
        &tags_by_intent,
        &rel_adjacency,
    ));
    out.extend(layering_smells(
        store,
        snap,
        &owners,
        &imports,
        &path_to_id,
        &id_to_path,
    )?);
    out.extend(disclosure_smells(&intents, &tags_by_intent));
    out.extend(journey_proof_smells(snap, &intents));
    out.extend(vague_intent_smells(&intents));
    out.extend(pack_drift_smells(snap));
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

fn active_intents(snap: &Snapshot) -> Vec<&Node> {
    snap.nodes
        .iter()
        .filter(|n| n.node_type == NodeType::Intent && n.status != "deprecated")
        .collect()
}

fn implements_edges(snap: &Snapshot) -> impl Iterator<Item = &Edge> {
    snap.edges.iter().filter(|e| e.kind == EdgeKind::Implements)
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
    match store.get_meta(crate::store::LAYER_ORDER_META_KEY)? {
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
