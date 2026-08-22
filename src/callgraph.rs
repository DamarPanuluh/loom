//! Call graph — the question an agent cannot cheaply rebuild each session.
//!
//! Plane: derived projection. Reads the per-file `calls` facet extraction wrote
//! and resolves written names to defining files, in memory, on demand. Nothing
//! here is stored: resolution is a pure function of the derived plane, so it
//! rebuilds identically and never becomes a fact that can rot.
//!
//! Contract — **resolution is honest about its own confidence.** A written name
//! is not a target. `sync::run` can identify `src/sync.rs` among several `run`
//! definitions; a bare generic `run`, `Store::open`, or two distinct `sync.rs`
//! modules are still guesses. A globally unique, specific bare symbol remains
//! exact. Those cases are reported as different things ([`Resolution`]) and
//! never blended into one number, because a blast-radius figure that mixes them
//! tells you nothing you can act on.

use crate::model::{NodeType, TargetKind};
use crate::store::Store;
use crate::Result;
use anyhow::Context;
use serde::Serialize;

/// Call hops an impact walk goes back when the caller does not say.
///
/// Declared here, beside the walk, and consumed by every surface that offers
/// the choice — `loom impact --depth` and the `loom_impact` MCP tool — so the
/// two cannot answer the same question from different defaults.
pub const DEFAULT_IMPACT_DEPTH: usize = 3;

/// Most call hops an impact walk will go back.
///
/// Past this the answer stops being a blast radius and becomes "most of the
/// crate", which no reader can act on. Enforced once inside `impact_report`,
/// which both surfaces call, so the bound cannot hold on one surface and not
/// the other. Surfaced by `loom limits`.
pub const MAX_IMPACT_DEPTH: usize = 10;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

/// How much to trust one resolved edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// A written module qualifier matches exactly one defining module/file, or
    /// a specific bare symbol has exactly one owner in the graph.
    Exact,
    /// The call is bare/type-qualified, or several plausible definitions
    /// remain; the nearest by module/file proximity was chosen.
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
    /// std/third-party. Counted, never guessed at. Graph-wide; impact
    /// neighborhoods read [`Self::unresolved_from`] instead.
    pub unresolved: usize,
    /// Unresolved callees grouped by the calling `(file, symbol)`, so an
    /// impact walk can report only the neighborhood's unknown calls rather
    /// than the whole repository's std/third-party remainder.
    unresolved_from: BTreeMap<(String, String), usize>,
    /// file → symbols it defines.
    defines: BTreeMap<String, BTreeSet<String>>,
    /// file → harness-executed test functions it defines (`#[test]` / inside
    /// `#[cfg(test)]`). Only these may serve as derived proof entry points.
    test_defines: BTreeMap<String, BTreeSet<String>>,
    /// `(to_file, to_symbol)` → indices into `edges`. File-qualified so a bare
    /// name shared by two definitions cannot pull callers of one into the
    /// impact of the other. Built once so `impact`'s backward BFS looks up
    /// incoming edges in O(log n) rather than rescanning the whole edge vector
    /// per visited node (it is called per grounded symbol per validation on the
    /// sync hot path).
    incoming: BTreeMap<(String, String), Vec<usize>>,
}

/// Build the graph from the derived plane.
pub fn build(store: &Store) -> Result<CallGraph> {
    let mut defines: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut test_defines: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
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
        for sym in test_symbols_of(store, &cf)? {
            test_defines.entry(cf.name.clone()).or_default().insert(sym);
        }
        for (from, callee) in calls_of(store, &cf)? {
            raw.push((cf.name.clone(), from, callee));
        }
    }

    let mut graph = CallGraph {
        defines,
        test_defines,
        ..Default::default()
    };
    for (file, from_symbol, callee) in raw {
        let bare = callee.rsplit("::").next().unwrap_or(&callee).to_string();
        let Some(candidates) = owner.get(&bare) else {
            graph.unresolved += 1;
            *graph
                .unresolved_from
                .entry((file.clone(), from_symbol.clone()))
                .or_default() += 1;
            continue;
        };
        let Some((to_file, resolution)) = resolve_target(&file, &callee, candidates) else {
            graph.unresolved += 1;
            *graph
                .unresolved_from
                .entry((file.clone(), from_symbol.clone()))
                .or_default() += 1;
            continue;
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
            .entry((e.to_file.clone(), e.to_symbol.clone()))
            .or_default()
            .push(i);
    }
    Ok(graph)
}

/// Resolve one written callee without overstating what its syntax establishes.
///
/// Extraction preserves Rust paths (`sync::run`, `crate::sync::run`). The
/// segment immediately before the bare symbol can identify a module file, but
/// only an exact, unique module/file-stem match earns Exact. The generic bare
/// `run`, `Self`/`self`, type-looking qualifiers, and ambiguous module matches
/// retain a nearest-file diagnostic edge marked Heuristic, so exact-only
/// consumers fail closed. Other globally unique bare symbols retain their
/// established exactness.
fn resolve_target(
    calling_file: &str,
    callee: &str,
    candidates: &[String],
) -> Option<(String, Resolution)> {
    let qualified: Vec<String> = module_qualifier(callee)
        .map(|qualifier| {
            candidates
                .iter()
                .filter(|candidate| file_is_module(candidate, qualifier))
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    if qualified.len() == 1 {
        return Some((qualified[0].clone(), Resolution::Exact));
    }

    // A specific bare name with one graph-wide owner is unambiguous. Preserve
    // that established behavior for call paths such as `perform_behavior()`.
    // `run` is deliberately excluded: it is a ubiquitous entrypoint name, and
    // without the extracted module qualifier it cannot certify a module edge.
    if !callee.contains("::") && callee != "run" && candidates.len() == 1 {
        return Some((candidates[0].clone(), Resolution::Exact));
    }

    // An ambiguous qualifier is still useful for diagnostic proximity, but it
    // cannot certify one definition. If it matched nothing (or the call is
    // bare/type/self-qualified), fall back to every bare-symbol owner.
    let pool = if qualified.is_empty() {
        candidates
    } else {
        qualified.as_slice()
    };
    nearest_candidate(calling_file, pool).map(|file| (file, Resolution::Heuristic))
}

/// The module segment immediately before the called symbol, when that segment
/// is safe to interpret as a Rust module name rather than `self`/`Self`, a
/// root traversal marker, or a conventional UpperCamel type.
fn module_qualifier(callee: &str) -> Option<&str> {
    let (prefix, _) = callee.rsplit_once("::")?;
    let qualifier = prefix.rsplit("::").next()?;
    if matches!(qualifier, "self" | "Self" | "super" | "crate") {
        return None;
    }
    let mut chars = qualifier.chars();
    let first = chars.next()?;
    if !first.is_ascii_lowercase()
        || !chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
    {
        return None;
    }
    Some(qualifier)
}

/// Rust module `foo` may live at `foo.rs` or `foo/mod.rs`. Match the exact
/// segment only; suffix/proximity matching would turn a qualifier back into a
/// guess while labelling it Exact.
fn file_is_module(file: &str, qualifier: &str) -> bool {
    let path = Path::new(file);
    match path.file_stem().and_then(|stem| stem.to_str()) {
        Some(stem) if stem == qualifier => true,
        Some("mod") => {
            path.parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                == Some(qualifier)
        }
        _ => false,
    }
}

fn nearest_candidate(calling_file: &str, candidates: &[String]) -> Option<String> {
    // Prefer a definition in the calling file, then the nearest shared
    // directory. This remains explicitly heuristic even if the pool contains
    // only one owner when the caller reached this fallback: the syntax did not
    // establish an exact module edge.
    candidates
        .iter()
        .find(|candidate| candidate.as_str() == calling_file)
        .cloned()
        .or_else(|| {
            candidates
                .iter()
                .max_by_key(|candidate| shared_prefix(candidate, calling_file))
                .cloned()
        })
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
    let symbols = serde_json::from_str::<BTreeMap<String, String>>(&json).with_context(|| {
        format!(
            "code file '{}' has malformed '{}' facet JSON",
            cf.name,
            crate::seed::SYMBOL_FINGERPRINTS_KEY
        )
    })?;
    Ok(symbols.into_keys().collect())
}

/// The harness-executed test functions a code file defines, from the derived
/// `test_symbols` facet.
fn test_symbols_of(store: &Store, cf: &crate::model::Node) -> Result<Vec<String>> {
    let Some(json) = store.get_facet(&cf.id, TargetKind::Node, crate::seed::TEST_SYMBOLS_KEY)?
    else {
        return Ok(Vec::new());
    };
    serde_json::from_str::<Vec<String>>(&json).with_context(|| {
        format!(
            "code file '{}' has malformed '{}' facet JSON",
            cf.name,
            crate::seed::TEST_SYMBOLS_KEY
        )
    })
}

/// The `caller > callee` pairs a file records, already split.
fn calls_of(store: &Store, cf: &crate::model::Node) -> Result<Vec<(String, String)>> {
    let Some(json) = store.get_facet(&cf.id, TargetKind::Node, crate::seed::CALLS_KEY)? else {
        return Ok(Vec::new());
    };
    let calls = serde_json::from_str::<Vec<String>>(&json).with_context(|| {
        format!(
            "code file '{}' has malformed '{}' facet JSON",
            cf.name,
            crate::seed::CALLS_KEY
        )
    })?;
    Ok(calls
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

    /// Symbols defined in one registered file, sorted by the graph's stable set order.
    pub fn symbols_in_file(&self, file: &str) -> Vec<&str> {
        self.defines
            .get(file)
            .map(|symbols| symbols.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// The resolved call edges (used by S3 derivation to confirm an entry
    /// symbol is actually invoked somewhere, including from file scope).
    pub fn edges(&self) -> &[CallEdge] {
        &self.edges
    }

    /// Registered files that expose at least one indexed symbol.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.defines.keys().map(String::as_str)
    }

    /// Whether one file defines this exact symbol.
    pub fn file_defines(&self, file: &str, symbol: &str) -> bool {
        self.defines
            .get(file)
            .is_some_and(|symbols| symbols.contains(symbol))
    }

    /// The harness-executed test functions a file defines. Only these (plus
    /// symbols they reach) may serve as derived proof entry points.
    pub fn file_test_symbols(&self, file: &str) -> BTreeSet<String> {
        self.test_defines.get(file).cloned().unwrap_or_default()
    }

    /// Everything that transitively reaches `symbol`, up to `depth` hops.
    ///
    /// Breadth-first from every defining file of `symbol` BACKWARDS, so `hops`
    /// is the true shortest call distance and a shared bare name cannot
    /// smuggle callers from one definition into another. Heuristic edges are
    /// included for diagnostic blast-radius; grading uses [`exact_impact`].
    pub fn impact(&self, symbol: &str, depth: usize) -> Impact {
        self.impact_with(symbol, depth, false)
    }

    /// Callers that reach `symbol` through an all-Exact resolution path.
    ///
    /// Grading (S3 call witness, harness reachability) must never credit a
    /// nearest-file guess: only uniquely-resolved edges count as proof that
    /// one symbol actually calls another.
    pub fn exact_impact(&self, symbol: &str, depth: usize) -> Impact {
        self.impact_with(symbol, depth, true)
    }

    /// Exact callers of one specific `(file, symbol)` definition site.
    ///
    /// Prefer this over [`exact_impact`] for grading: a realizing grounding
    /// names a file, and same-named symbols in other files must not share its
    /// witness.
    pub fn exact_impact_at(&self, file: &str, symbol: &str, depth: usize) -> Impact {
        self.impact_from(vec![(file.to_string(), symbol.to_string())], depth, true)
    }

    fn impact_with(&self, symbol: &str, depth: usize, exact_only: bool) -> Impact {
        self.impact_from(self.starts_for(symbol, exact_only), depth, exact_only)
    }

    fn impact_from(&self, starts: Vec<(String, String)>, depth: usize, exact_only: bool) -> Impact {
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        let mut callers: Vec<Caller> = Vec::new();
        // Queue file-qualified symbols so two `open` definitions never share a
        // caller set just because their bare names collide.
        let mut queue: VecDeque<(String, String, usize)> = VecDeque::new();
        let mut visited: BTreeSet<(String, String)> = BTreeSet::new();
        let target = starts
            .first()
            .map(|(_, symbol)| symbol.clone())
            .unwrap_or_default();
        for start in starts {
            if visited.insert(start.clone()) {
                queue.push_back((start.0, start.1, 0));
            }
        }

        while let Some((current_file, current_symbol, hops)) = queue.pop_front() {
            if hops >= depth {
                continue;
            }
            let Some(indices) = self
                .incoming
                .get(&(current_file.clone(), current_symbol.clone()))
            else {
                continue;
            };
            for e in indices.iter().map(|&i| &self.edges[i]) {
                if exact_only && e.resolution != Resolution::Exact {
                    continue;
                }
                let key = (e.from_file.clone(), e.from_symbol.clone());
                if e.from_symbol.is_empty() || !seen.insert(key.clone()) {
                    continue;
                }
                callers.push(Caller {
                    file: e.from_file.clone(),
                    symbol: e.from_symbol.clone(),
                    hops: hops + 1,
                    resolution: e.resolution,
                });
                if visited.insert(key.clone()) {
                    queue.push_back((e.from_file.clone(), e.from_symbol.clone(), hops + 1));
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
            target,
            exact: callers
                .iter()
                .filter(|c| c.resolution == Resolution::Exact)
                .count(),
            heuristic: callers
                .iter()
                .filter(|c| c.resolution == Resolution::Heuristic)
                .count(),
            unresolved_calls: visited
                .iter()
                .map(|key| self.unresolved_from.get(key).copied().unwrap_or(0))
                .sum(),
            callers,
        }
    }

    /// Combined blast radius of several symbols (a file target, or every
    /// symbol an impact report already resolved). Unresolved calls are the
    /// union of those neighborhoods, not the graph-wide remainder.
    pub fn impact_of(&self, symbols: &[String], depth: usize) -> Impact {
        let mut starts = Vec::new();
        for symbol in symbols {
            starts.extend(self.starts_for(symbol, false));
        }
        self.impact_from(starts, depth, false)
    }

    fn starts_for(&self, symbol: &str, exact_only: bool) -> Vec<(String, String)> {
        let mut starts = Vec::new();
        let defining_files = self.definers(symbol);
        if defining_files.is_empty() {
            if !exact_only {
                for (to_file, to_symbol) in self.incoming.keys() {
                    if to_symbol == symbol {
                        starts.push((to_file.clone(), to_symbol.clone()));
                    }
                }
            }
        } else {
            for file in defining_files {
                starts.push((file.to_string(), symbol.to_string()));
            }
        }
        starts
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
                .entry((e.to_file.clone(), e.to_symbol.clone()))
                .or_default()
                .push(i);
        }
        graph
    }

    fn candidates(files: &[&str]) -> Vec<String> {
        files.iter().map(|file| (*file).to_string()).collect()
    }

    #[test]
    fn qualified_module_calls_resolve_exactly_by_file_stem() {
        let run = candidates(&["src/sync.rs", "src/absorb.rs", "tests/support.rs"]);
        assert_eq!(
            resolve_target("src/commands/status_cmd.rs", "sync::run", &run),
            Some(("src/sync.rs".into(), Resolution::Exact))
        );
        assert_eq!(
            resolve_target("src/commands/status_cmd.rs", "crate::sync::run", &run),
            Some(("src/sync.rs".into(), Resolution::Exact)),
            "root path segments must not erase the immediate module qualifier"
        );

        let observe = candidates(&["src/absorb.rs", "src/commands/proof_cmd.rs"]);
        assert_eq!(
            resolve_target(
                "src/commands/diagnostics_cmd.rs",
                "absorb::observe",
                &observe
            ),
            Some(("src/absorb.rs".into(), Resolution::Exact))
        );
    }

    #[test]
    fn directory_module_qualifier_resolves_foo_mod_rs_exactly() {
        let run = candidates(&["src/foo/mod.rs", "src/other.rs"]);
        assert_eq!(
            resolve_target("src/main.rs", "foo::run", &run),
            Some(("src/foo/mod.rs".into(), Resolution::Exact))
        );
    }

    #[test]
    fn ambiguous_module_qualifiers_remain_heuristic() {
        let run = candidates(&["crates/a/src/sync.rs", "crates/b/src/sync.rs"]);
        let (file, resolution) = resolve_target("crates/a/src/main.rs", "sync::run", &run).unwrap();
        assert_eq!(file, "crates/a/src/sync.rs");
        assert_eq!(resolution, Resolution::Heuristic);
    }

    #[test]
    fn bare_type_self_and_unmatched_qualifiers_remain_heuristic() {
        let run = candidates(&["src/sync.rs"]);
        for callee in ["run", "Self::run", "self::run", "Store::run", "other::run"] {
            assert_eq!(
                resolve_target("src/main.rs", callee, &run),
                Some(("src/sync.rs".into(), Resolution::Heuristic)),
                "{callee} must not earn exact resolution"
            );
        }

        let open = candidates(&["src/store.rs"]);
        assert_eq!(
            resolve_target("src/main.rs", "Store::open", &open),
            Some(("src/store.rs".into(), Resolution::Heuristic))
        );

        let behavior = candidates(&["src/behavior.rs"]);
        assert_eq!(
            resolve_target("tests/behavior.rs", "perform_behavior", &behavior),
            Some(("src/behavior.rs".into(), Resolution::Exact)),
            "a globally unique, specific bare symbol remains unambiguous"
        );
    }

    #[test]
    fn multiple_bare_definitions_use_proximity_but_remain_heuristic() {
        let run = candidates(&["src/sync.rs", "tests/support.rs"]);
        assert_eq!(
            resolve_target("tests/proof.rs", "run", &run),
            Some(("tests/support.rs".into(), Resolution::Heuristic))
        );
    }

    /// A heuristic edge to a same-named symbol in another file must never
    /// make that other definition look reached for grading.
    #[test]
    fn bare_symbol_collision_does_not_cross_file_pollute_exact_impact() {
        let mut g = CallGraph::default();
        for (file, sym) in [
            ("src/a.rs", "open"),
            ("src/b.rs", "open"),
            ("tests/a_test.rs", "tests_a"),
        ] {
            g.defines.entry(file.into()).or_default().insert(sym.into());
        }
        // Only a heuristic guess points at b::open. Under a bare-name index
        // this would still surface as a caller of "open"; with file-qualified
        // exact-only walk it must not.
        g.edges.push(CallEdge {
            from_file: "tests/a_test.rs".into(),
            from_symbol: "tests_a".into(),
            to_file: "src/b.rs".into(),
            to_symbol: "open".into(),
            resolution: Resolution::Heuristic,
        });
        for (i, e) in g.edges.iter().enumerate() {
            g.incoming
                .entry((e.to_file.clone(), e.to_symbol.clone()))
                .or_default()
                .push(i);
        }
        let exact = g.exact_impact("open", 4);
        assert!(
            exact.callers.is_empty(),
            "a heuristic same-name edge must not earn exact callers: {:?}",
            exact.callers
        );
        let all = g.impact("open", 4);
        assert_eq!(all.heuristic, 1);
        assert!(all
            .callers
            .iter()
            .any(|c| c.file == "tests/a_test.rs" && c.symbol == "tests_a"));
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

    #[test]
    fn impact_unresolved_counts_the_neighborhood_not_the_whole_graph() {
        let mut graph = CallGraph {
            unresolved: 99,
            ..CallGraph::default()
        };
        graph
            .unresolved_from
            .insert(("src/a.rs".into(), "foo".into()), 2);
        graph
            .unresolved_from
            .insert(("src/b.rs".into(), "bar".into()), 50);
        graph
            .defines
            .entry("src/a.rs".into())
            .or_default()
            .insert("foo".into());
        graph
            .defines
            .entry("src/b.rs".into())
            .or_default()
            .insert("bar".into());
        assert_eq!(graph.impact("foo", 8).unresolved_calls, 2);
        assert_eq!(graph.unresolved, 99);
    }
}
