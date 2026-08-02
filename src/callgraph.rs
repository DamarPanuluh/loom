//! Call graph — the question an agent cannot cheaply rebuild each session.
//!
//! Plane: derived projection. Reads the per-file `calls` facet extraction wrote
//! and resolves written names to defining files, in memory, on demand. Nothing
//! here is stored: resolution is a pure function of the derived plane, so it
//! rebuilds identically and never becomes a fact that can rot.
//!
//! Contract — **resolution is honest about its own confidence.** A written name
//! is not a target. `Store::open` resolving to the one file defining `open`
//! inside a type called `Store` is near-certain; a bare `run` in a repo with
//! nine of them is a guess. Those are reported as different things
//! ([`Resolution`]) and never blended into one number, because a blast-radius
//! figure that mixes them tells you nothing you can act on.

use crate::model::{NodeType, TargetKind};
use crate::store::Store;
use crate::Result;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// How much to trust one resolved edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// Exactly one symbol in the repo carries this name.
    Exact,
    /// Several do; the nearest by module/file proximity was chosen.
    Heuristic,
}

/// One resolved call edge between symbols.
#[derive(Debug, Clone, Serialize)]
pub struct CallEdge {
    pub from_file: String,
    pub from_symbol: String,
    pub to_file: String,
    pub to_symbol: String,
    pub resolution: Resolution,
}

/// The whole resolved graph, plus what it could not resolve.
#[derive(Debug, Default)]
pub struct CallGraph {
    pub edges: Vec<CallEdge>,
    /// Callee names matching no symbol in the repo — almost all of them are
    /// std/third-party. Counted, never guessed at.
    pub unresolved: usize,
    /// file → symbols it defines.
    defines: BTreeMap<String, BTreeSet<String>>,
    /// `to_symbol` → indices into `edges`. Built once so `impact`'s backward BFS
    /// looks up incoming edges in O(log n) rather than rescanning the whole edge
    /// vector per visited node (it is called per grounded symbol per validation
    /// on the sync hot path).
    incoming: BTreeMap<String, Vec<usize>>,
}

/// Build the graph from the derived plane.
pub fn build(store: &Store) -> Result<CallGraph> {
    let mut defines: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut owner: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut raw: Vec<(String, String, String)> = Vec::new();

    for cf in store.list_nodes(Some(NodeType::CodeFile), usize::MAX)? {
        for sym in symbols_of(store, &cf)? {
            defines
                .entry(cf.name.clone())
                .or_default()
                .insert(sym.clone());
            owner.entry(sym).or_default().push(cf.name.clone());
        }
        for (from, callee) in calls_of(store, &cf)? {
            raw.push((cf.name.clone(), from, callee));
        }
    }

    let mut graph = CallGraph {
        defines,
        ..Default::default()
    };
    for (file, from_symbol, callee) in raw {
        // A qualified `Type::method` tries the bare method name — the qualifier
        // is a disambiguation hint, not part of the definition's name.
        let bare = callee.rsplit("::").next().unwrap_or(&callee).to_string();
        let Some(candidates) = owner.get(&bare) else {
            graph.unresolved += 1;
            continue;
        };
        let (to_file, resolution) = match candidates.len() {
            1 => (candidates[0].clone(), Resolution::Exact),
            _ => {
                // Prefer a definition in the calling file, then the nearest
                // shared directory. Stated as a guess, and labelled as one.
                let same_file = candidates.iter().find(|c| **c == file).cloned();
                let nearest = same_file.or_else(|| {
                    candidates
                        .iter()
                        .max_by_key(|c| shared_prefix(c, &file))
                        .cloned()
                });
                match nearest {
                    Some(f) => (f, Resolution::Heuristic),
                    None => {
                        graph.unresolved += 1;
                        continue;
                    }
                }
            }
        };
        graph.edges.push(CallEdge {
            from_file: file,
            from_symbol,
            to_file,
            to_symbol: bare,
            resolution,
        });
    }
    graph.edges.sort_by(|a, b| {
        (&a.from_file, &a.from_symbol, &a.to_file, &a.to_symbol).cmp(&(
            &b.from_file,
            &b.from_symbol,
            &b.to_file,
            &b.to_symbol,
        ))
    });
    graph.edges.dedup_by(|a, b| {
        a.from_file == b.from_file
            && a.from_symbol == b.from_symbol
            && a.to_file == b.to_file
            && a.to_symbol == b.to_symbol
    });
    for (i, e) in graph.edges.iter().enumerate() {
        graph
            .incoming
            .entry(e.to_symbol.clone())
            .or_default()
            .push(i);
    }
    Ok(graph)
}

/// The symbols a file defines, from the derived fingerprint map.
fn symbols_of(store: &Store, cf: &crate::model::Node) -> Result<Vec<String>> {
    let Some(json) = store.get_facet(
        &cf.id,
        TargetKind::Node,
        crate::seed::SYMBOL_FINGERPRINTS_KEY,
    )?
    else {
        return Ok(Vec::new());
    };
    Ok(serde_json::from_str::<BTreeMap<String, String>>(&json)
        .map(|map| map.into_keys().collect())
        .unwrap_or_default())
}

/// The `caller > callee` pairs a file records, already split.
fn calls_of(store: &Store, cf: &crate::model::Node) -> Result<Vec<(String, String)>> {
    let Some(json) = store.get_facet(&cf.id, TargetKind::Node, crate::seed::CALLS_KEY)? else {
        return Ok(Vec::new());
    };
    Ok(serde_json::from_str::<Vec<String>>(&json)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            entry
                .split_once('>')
                .map(|(from, callee)| (from.to_string(), callee.to_string()))
        })
        .collect())
}

/// How many leading path segments two files share.
fn shared_prefix(a: &str, b: &str) -> usize {
    a.split('/')
        .zip(b.split('/'))
        .take_while(|(x, y)| x == y)
        .count()
}

/// What a change here could reach, by walking callers backwards.
#[derive(Debug, Serialize)]
pub struct Impact {
    pub target: String,
    /// Symbols that transitively call the target, nearest first.
    pub callers: Vec<Caller>,
    pub exact: usize,
    pub heuristic: usize,
    pub unresolved_calls: usize,
}

#[derive(Debug, Serialize)]
pub struct Caller {
    pub file: String,
    pub symbol: String,
    pub hops: usize,
    pub resolution: Resolution,
}

impl CallGraph {
    /// Which files define a symbol of this name.
    pub fn definers(&self, symbol: &str) -> Vec<&str> {
        self.defines
            .iter()
            .filter(|(_, syms)| syms.contains(symbol))
            .map(|(f, _)| f.as_str())
            .collect()
    }

    /// Everything that transitively reaches `symbol`, up to `depth` hops.
    ///
    /// Breadth-first from the target BACKWARDS, so `hops` is the true shortest
    /// call distance — which is what makes the answer rankable rather than a
    /// pile of names.
    pub fn impact(&self, symbol: &str, depth: usize) -> Impact {
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        let mut callers: Vec<Caller> = Vec::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        queue.push_back((symbol.to_string(), 0));
        let mut visited_symbols: BTreeSet<String> = BTreeSet::new();
        visited_symbols.insert(symbol.to_string());

        while let Some((current, hops)) = queue.pop_front() {
            if hops >= depth {
                continue;
            }
            let Some(indices) = self.incoming.get(&current) else {
                continue;
            };
            for e in indices.iter().map(|&i| &self.edges[i]) {
                let key = (e.from_file.clone(), e.from_symbol.clone());
                if e.from_symbol.is_empty() || !seen.insert(key) {
                    continue;
                }
                callers.push(Caller {
                    file: e.from_file.clone(),
                    symbol: e.from_symbol.clone(),
                    hops: hops + 1,
                    resolution: e.resolution,
                });
                if visited_symbols.insert(e.from_symbol.clone()) {
                    queue.push_back((e.from_symbol.clone(), hops + 1));
                }
            }
        }
        callers.sort_by(|a, b| {
            a.hops
                .cmp(&b.hops)
                .then(a.file.cmp(&b.file))
                .then(a.symbol.cmp(&b.symbol))
        });
        Impact {
            target: symbol.to_string(),
            exact: callers
                .iter()
                .filter(|c| c.resolution == Resolution::Exact)
                .count(),
            heuristic: callers
                .iter()
                .filter(|c| c.resolution == Resolution::Heuristic)
                .count(),
            unresolved_calls: self.unresolved,
            callers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(edges: &[(&str, &str, &str, &str)]) -> CallGraph {
        let mut graph = CallGraph::default();
        for &(from_file, from_symbol, to_file, to_symbol) in edges {
            graph
                .defines
                .entry(from_file.into())
                .or_default()
                .insert(from_symbol.into());
            graph
                .defines
                .entry(to_file.into())
                .or_default()
                .insert(to_symbol.into());
            graph.edges.push(CallEdge {
                from_file: from_file.into(),
                from_symbol: from_symbol.into(),
                to_file: to_file.into(),
                to_symbol: to_symbol.into(),
                resolution: Resolution::Exact,
            });
        }
        for (i, e) in graph.edges.iter().enumerate() {
            graph
                .incoming
                .entry(e.to_symbol.clone())
                .or_default()
                .push(i);
        }
        graph
    }

    /// Finding d3107a6d: a proof whose test reaches the grounded symbol at 6
    /// hops was graded S2 because `call_witness` walked only 4. Depth must be
    /// deep enough that a demonstrable exact caller is visible.
    #[test]
    fn impact_sees_exact_callers_beyond_four_hops() {
        // target ← a ← b ← c ← d ← e ← test  (6 hops)
        let g = chain(&[
            ("src/a.rs", "a", "src/target.rs", "target"),
            ("src/b.rs", "b", "src/a.rs", "a"),
            ("src/c.rs", "c", "src/b.rs", "b"),
            ("src/d.rs", "d", "src/c.rs", "c"),
            ("src/e.rs", "e", "src/d.rs", "d"),
            ("tests/proof.rs", "the_test", "src/e.rs", "e"),
        ]);
        let shallow = g.impact("target", 4);
        assert!(
            !shallow
                .callers
                .iter()
                .any(|c| c.file == "tests/proof.rs" && c.symbol == "the_test"),
            "depth 4 must miss the 6-hop exact caller (the old call_witness bug)"
        );
        let deep = g.impact("target", crate::proofstrength::CALL_WITNESS_DEPTH);
        let hit = deep
            .callers
            .iter()
            .find(|c| c.file == "tests/proof.rs" && c.symbol == "the_test")
            .expect("CALL_WITNESS_DEPTH must see the 6-hop exact caller");
        assert_eq!(hit.hops, 6);
        assert_eq!(hit.resolution, Resolution::Exact);
    }
}
