//! Absorb — loom writes the graph from the work that already happened.
//!
//! Plane: read-only observation producing a Proposal for confirmation. Writes
//! nothing about the code; the adopt path is the ordinary gated one.
//!
//! Contract — **the agent confirms rather than authors.** An agent editing a
//! codebase already knows what it did; making it also describe what it did, in
//! loom's vocabulary, is the friction that gets loom skipped. So loom observes
//! the difference between the graph and the tree, proposes the mutations, and
//! asks for the one thing it genuinely cannot derive: the behavioral criterion
//! in human language.
//!
//! **The anti-fabrication property.** Every proposed item carries an
//! [`AbsorbEvidence`] stamped from disk at propose time. At adopt time it is
//! RECOMPUTED and compared; a mismatch rejects the item. `AbsorbEvidence` is
//! never accepted from input on any path, so a hand-edited proposal cannot
//! smuggle a mutation past the check — which matters because the proposal body
//! is ordinary JSON an agent can rewrite.

use crate::model::{NodeType, TargetKind};
use crate::store::Store;
use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// What loom observed, recomputed and compared at adopt time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbsorbEvidence {
    pub file: String,
    /// Fingerprint of the symbol this item is about, as it was on disk.
    pub symbol: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A new symbol in a file an intent already owns.
    ExtendLocator,
    /// A new symbol whose callers all land in one intent's files.
    GroundToIntent,
    /// A symbol a live locator names, which is no longer there.
    RepointLocator,
    /// A registered file nothing owns.
    ResolveCoverage,
    /// A new test symbol whose call closure reaches an intent's code.
    RegisterProof,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::ExtendLocator => "extend_locator",
            Kind::GroundToIntent => "ground_to_intent",
            Kind::RepointLocator => "repoint_locator",
            Kind::ResolveCoverage => "resolve_coverage",
            Kind::RegisterProof => "register_proof",
        }
    }
}

/// One proposed mutation, with what loom saw and what it still needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub kind: Kind,
    /// What loom would do, in one line.
    pub text: String,
    /// The intent this concerns, when loom could work it out.
    pub intent_id: Option<String>,
    pub evidence: AbsorbEvidence,
    /// What loom cannot derive. An item with unfilled needs is never adopted by
    /// `--confirm` — the criterion is the human's, always.
    pub needs: Vec<String>,
}

/// Observe the difference between the graph and the working tree.
///
/// Pure read. Notably it does NOT call the deriver: sync's derivation writes
/// while it observes, so calling it here would destroy the very change set
/// absorb needs to see.
pub fn observe(store: &Store, root: &Path) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    let graph = crate::callgraph::build(store)?;

    // Which files each intent owns, and what its locators name.
    let mut owned: BTreeMap<String, String> = BTreeMap::new(); // file name -> intent id
    let mut located: BTreeMap<String, (String, String)> = BTreeMap::new(); // symbol -> (intent, file)
    for intent in store.list_nodes(Some(NodeType::Intent), usize::MAX)? {
        if intent.status == "deprecated" {
            continue;
        }
        for e in store.edges_with(
            Some(crate::model::EdgeKind::Implements),
            Some(&intent.id),
            None,
        )? {
            if store.edge_superseded(&e.id)? {
                continue;
            }
            let Some(cf) = store.get_node(&e.to_id)? else {
                continue;
            };
            owned.insert(cf.name.clone(), intent.id.clone());
            if let Some(loc) = store.get_facet(&e.id, TargetKind::Edge, "locator")? {
                if let Some(sym) = locator_symbol(&loc) {
                    located.insert(sym, (intent.id.clone(), cf.name.clone()));
                }
            }
        }
    }

    for cf in store.list_nodes(Some(NodeType::CodeFile), usize::MAX)? {
        let Ok(content) = std::fs::read_to_string(root.join(&cf.name)) else {
            continue;
        };
        let extraction = crate::extract::extract(&cf.name, &content);
        let live: BTreeSet<String> = extraction.symbols.iter().map(|s| s.name.clone()).collect();

        // What loom knew about this file last time it looked.
        let known: BTreeMap<String, String> = store
            .get_facet(&cf.id, TargetKind::Node, crate::seed::SYMBOL_FINGERPRINTS_KEY)?
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();

        // A registered file nothing owns is coverage work, prefilled.
        if !owned.contains_key(&cf.name) {
            items.push(Item {
                kind: Kind::ResolveCoverage,
                text: format!("{} is registered but no behavior owns it", cf.name),
                intent_id: None,
                evidence: AbsorbEvidence {
                    file: cf.name.clone(),
                    symbol: String::new(),
                    fingerprint: crate::artifact::fingerprint(&content),
                },
                needs: vec!["which behavior this file realizes, or that it is a seam".into()],
            });
        }

        for sym in &extraction.symbols {
            if known.contains_key(&sym.name) {
                continue; // loom has seen this one
            }
            let evidence = AbsorbEvidence {
                file: cf.name.clone(),
                symbol: sym.name.clone(),
                fingerprint: crate::artifact::fingerprint(&content),
            };

            // Do this symbol's callers land in exactly one intent's files?
            let reached = graph.impact(&sym.name, 2);
            let owners: BTreeSet<&String> = reached
                .callers
                .iter()
                .filter_map(|c| owned.get(&c.file))
                .collect();

            // A new symbol in a TEST file whose call closure reaches a
            // behavior's code is a proof waiting to be registered. This is the
            // rule that needed `tests/` in the graph to be able to fire at all
            // — before that, no test file was a call-graph entry point.
            if crate::extract::Role::detect(&cf.name) == crate::extract::Role::Test {
                // From the LIVE extraction, not the stored call graph. The
                // graph is a derived projection refreshed by sync, so a test
                // written since the last sync — which is every test absorb
                // exists to notice — has no edges in it yet.
                let verified: BTreeSet<&String> = extraction
                    .calls
                    .iter()
                    .filter(|c| c.from == sym.name)
                    .filter_map(|c| {
                        let bare = c.callee.rsplit("::").next().unwrap_or(&c.callee);
                        located.get(bare).map(|(intent, _)| intent)
                    })
                    .collect();
                if verified.len() == 1 {
                    let intent = verified.into_iter().next().cloned();
                    items.push(Item {
                        kind: Kind::RegisterProof,
                        text: format!(
                            "'{}' in {} exercises one behavior's code — it can prove it",
                            sym.name, cf.name
                        ),
                        intent_id: intent,
                        evidence,
                        // loom can see WHAT it exercises; only a person can say
                        // the test actually checks the behavior rather than
                        // merely touching it.
                        needs: vec![
                            "confirm this test checks the behavior, not just that it runs".into(),
                        ],
                    });
                }
                continue;
            }

            match (owned.get(&cf.name), owners.len()) {
                // In a file an intent already owns: extend that locator.
                (Some(intent), _) => items.push(Item {
                    kind: Kind::ExtendLocator,
                    text: format!(
                        "'{}' is new in {}, which already realizes a behavior",
                        sym.name, cf.name
                    ),
                    intent_id: Some((*intent).clone()),
                    evidence,
                    needs: Vec::new(),
                }),
                // Not owned, but everything calling it belongs to one behavior.
                (None, 1) => {
                    let intent = owners.into_iter().next().cloned();
                    items.push(Item {
                        kind: Kind::GroundToIntent,
                        text: format!(
                            "'{}' in {} is called only from one behavior's code",
                            sym.name, cf.name
                        ),
                        intent_id: intent,
                        evidence,
                        needs: Vec::new(),
                    });
                }
                // Nothing to attach it to yet.
                (None, _) => {}
            }
        }

        // A locator naming a symbol that is gone.
        for (symbol, (intent, file)) in &located {
            if *file != cf.name || live.contains(symbol) {
                continue;
            }
            items.push(Item {
                kind: Kind::RepointLocator,
                text: format!("'{symbol}' is named by a live locator but is no longer in {file}"),
                intent_id: Some(intent.clone()),
                evidence: AbsorbEvidence {
                    file: cf.name.clone(),
                    symbol: symbol.clone(),
                    fingerprint: crate::artifact::fingerprint(&content),
                },
                needs: vec!["where the behavior moved to, or that it was removed".into()],
            });
        }
    }

    items.sort_by(|a, b| {
        a.evidence
            .file
            .cmp(&b.evidence.file)
            .then(a.evidence.symbol.cmp(&b.evidence.symbol))
            .then(a.kind.as_str().cmp(b.kind.as_str()))
    });
    Ok(items)
}

/// Re-derive one item's evidence from disk. The comparison at adopt time.
pub fn restamp(root: &Path, item: &Item) -> Option<AbsorbEvidence> {
    let content = std::fs::read_to_string(root.join(&item.evidence.file)).ok()?;
    Some(AbsorbEvidence {
        file: item.evidence.file.clone(),
        symbol: item.evidence.symbol.clone(),
        fingerprint: crate::artifact::fingerprint(&content),
    })
}

/// Does this item still describe the world it was stamped from?
///
/// Note what this catches: not only a hand-edited stamp, but a proposal adopted
/// after the file moved on. Both are the same mistake — acting on an
/// observation that is no longer true.
pub fn still_holds(root: &Path, item: &Item) -> bool {
    restamp(root, item).map(|f| f == item.evidence).unwrap_or(false)
}

/// The symbol a locator names, if it names one.
fn locator_symbol(locator: &str) -> Option<String> {
    let tok = locator.split_whitespace().next_back()?;
    // Path qualification first, THEN the line suffix. The other order splits
    // `Store::open` at the first colon and yields the type name.
    let tok = tok.rsplit("::").next().unwrap_or(tok);
    let tok = tok.split(':').next().unwrap_or(tok);
    (!tok.is_empty()).then(|| tok.to_string())
}

/// Persist a batch as a Proposal, reusing the ordinary adopt/defer/reject flow.
pub fn record(store: &Store, items: &[Item]) -> Result<crate::model::Node> {
    let entries: Vec<serde_json::Value> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            serde_json::json!({
                "number": i + 1,
                "text": item.text,
                "kind": item.kind.as_str(),
                "status": "open",
                "intent_id": item.intent_id,
                "absorb_evidence": item.evidence,
                "needs": item.needs,
            })
        })
        .collect();
    let body = serde_json::json!({
        "raw": format!("{} observation(s) from the working tree", items.len()),
        "source": "absorb",
        "source_path": serde_json::Value::Null,
        "items": entries,
    });
    store.add_node(
        NodeType::Proposal,
        "absorbed from the working tree",
        &format!("{} proposed mutation(s)", items.len()),
        "captured",
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locator_yields_its_symbol() {
        assert_eq!(locator_symbol("fn capture_payment").as_deref(), Some("capture_payment"));
        assert_eq!(locator_symbol("Store::open").as_deref(), Some("open"));
        assert_eq!(locator_symbol("capture:88").as_deref(), Some("capture"));
    }

    #[test]
    fn evidence_is_compared_by_value_not_trusted() {
        let a = AbsorbEvidence {
            file: "src/a.rs".into(),
            symbol: "f".into(),
            fingerprint: "abc".into(),
        };
        let mut b = a.clone();
        assert_eq!(a, b);
        b.fingerprint = "tampered".into();
        assert_ne!(a, b, "a rewritten stamp must not compare equal");
    }
}
