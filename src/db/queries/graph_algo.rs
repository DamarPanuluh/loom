//! Pure graph algorithms over index-addressed adjacency lists — the shared
//! engine for the "multi-hop audit layer" features that look BEYOND a single
//! edge or a one-hop neighborhood:
//!
//! - [`betweenness_centrality`] — bridge centrality for `loom next` ranking, so
//!   a low-degree chokepoint can outrank a high-degree clique node.
//! - [`connected_components`] — intent islands unreachable from a system root
//!   (`intent_island` smell).
//! - [`hop_distances`] — the graded sync ripple: distance from the changed
//!   region drives a decaying priority bump.
//!
//! Everything here is computed in Rust over an already-loaded snapshot (the
//! design contract on every one of these features: "computed in Rust over the
//! snapshot rather than via SQL recursion"). The functions are deliberately
//! string-id-free: callers index their active node set once and translate back,
//! which keeps the algorithms small, allocation-light, and unit-testable
//! without a graph store.

use std::collections::VecDeque;

/// Brandes' algorithm for betweenness centrality on an UNWEIGHTED, UNDIRECTED
/// graph. `adjacency[v]` lists v's neighbors (symmetric; duplicates are
/// harmless but wasteful — pass a simple graph). Returns a vec indexed by node:
/// the sum, over all node pairs, of the fraction of shortest paths through that
/// node. Undirected, so each unordered pair is counted once (the raw Brandes
/// accumulation double-counts and is halved here).
///
/// O(V·E) time, O(V+E) space — fine for loom-scale graphs (hundreds–low
/// thousands of intents); no recursion.
pub fn betweenness_centrality(n: usize, adjacency: &[Vec<usize>]) -> Vec<f64> {
    let mut centrality = vec![0.0f64; n];
    for s in 0..n {
        // Single-source shortest-path counting (BFS, unit weights).
        let mut order: Vec<usize> = Vec::new(); // nodes in non-decreasing distance
        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut sigma = vec![0.0f64; n]; // # shortest paths s→v
        let mut dist = vec![-1i64; n];
        sigma[s] = 1.0;
        dist[s] = 0;
        let mut queue = VecDeque::new();
        queue.push_back(s);
        while let Some(v) = queue.pop_front() {
            order.push(v);
            for &w in &adjacency[v] {
                if dist[w] < 0 {
                    dist[w] = dist[v] + 1;
                    queue.push_back(w);
                }
                if dist[w] == dist[v] + 1 {
                    sigma[w] += sigma[v];
                    preds[w].push(v);
                }
            }
        }
        // Back-propagate dependencies (reverse BFS order).
        let mut delta = vec![0.0f64; n];
        while let Some(w) = order.pop() {
            for &v in &preds[w] {
                delta[v] += (sigma[v] / sigma[w]) * (1.0 + delta[w]);
            }
            if w != s {
                centrality[w] += delta[w];
            }
        }
    }
    // Undirected: every shortest path is discovered from both endpoints.
    for c in centrality.iter_mut() {
        *c /= 2.0;
    }
    centrality
}

/// Connected components of an UNDIRECTED graph (`adjacency` symmetric). Returns
/// each component as a vec of node indices. Used by the island detector: a
/// component holding no system-level root is unreachable from one.
pub fn connected_components(n: usize, adjacency: &[Vec<usize>]) -> Vec<Vec<usize>> {
    const UNASSIGNED: usize = usize::MAX;
    let mut comp_of = vec![UNASSIGNED; n];
    let mut components: Vec<Vec<usize>> = Vec::new();
    for start in 0..n {
        if comp_of[start] != UNASSIGNED {
            continue;
        }
        let cid = components.len();
        let mut members = Vec::new();
        let mut queue = VecDeque::new();
        comp_of[start] = cid;
        queue.push_back(start);
        while let Some(v) = queue.pop_front() {
            members.push(v);
            for &w in &adjacency[v] {
                if comp_of[w] == UNASSIGNED {
                    comp_of[w] = cid;
                    queue.push_back(w);
                }
            }
        }
        components.push(members);
    }
    components
}

/// Multi-source BFS hop distance on an UNDIRECTED graph. Every node in
/// `sources` is distance 0; unreached nodes are `usize::MAX`. The graded sync
/// ripple reads this: distance from the changed-region frontier maps to a
/// decaying priority bump.
pub fn hop_distances(n: usize, adjacency: &[Vec<usize>], sources: &[usize]) -> Vec<usize> {
    let mut dist = vec![usize::MAX; n];
    let mut queue = VecDeque::new();
    for &s in sources {
        if s < n && dist[s] == usize::MAX {
            dist[s] = 0;
            queue.push_back(s);
        }
    }
    while let Some(v) = queue.pop_front() {
        let d = dist[v];
        for &w in &adjacency[v] {
            if dist[w] == usize::MAX {
                dist[w] = d + 1;
                queue.push_back(w);
            }
        }
    }
    dist
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a symmetric adjacency from undirected edges.
    fn undirected(n: usize, edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
        let mut adj = vec![Vec::new(); n];
        for &(a, b) in edges {
            adj[a].push(b);
            adj[b].push(a);
        }
        adj
    }

    #[test]
    fn betweenness_of_a_path_peaks_in_the_middle() {
        // 0 - 1 - 2 - 3 - 4 : the center (2) lies on the most shortest paths.
        let adj = undirected(5, &[(0, 1), (1, 2), (2, 3), (3, 4)]);
        let bc = betweenness_centrality(5, &adj);
        assert_eq!(bc[0], 0.0, "an endpoint routes nothing");
        assert_eq!(bc[4], 0.0);
        // Closed form for a path: node i (0-indexed) has bc = i*(n-1-i).
        assert_eq!(bc[1], 3.0, "1*(5-1-1)=3");
        assert_eq!(bc[2], 4.0, "2*(5-1-2)=4 — the peak");
        assert_eq!(bc[3], 3.0);
        assert!(bc[2] > bc[1] && bc[1] > bc[0]);
    }

    #[test]
    fn betweenness_is_zero_in_a_complete_graph() {
        // Every pair is directly adjacent — nothing routes through anyone.
        let adj = undirected(4, &[(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)]);
        let bc = betweenness_centrality(4, &adj);
        assert!(
            bc.iter().all(|&c| c == 0.0),
            "complete graph: bc=0 — {bc:?}"
        );
    }

    #[test]
    fn bridge_node_has_the_highest_betweenness() {
        // Two triangles {0,1,2} and {3,4,5} joined only through bridge node 6.
        let adj = undirected(
            7,
            &[
                (0, 1),
                (0, 2),
                (1, 2),
                (3, 4),
                (3, 5),
                (4, 5),
                (2, 6),
                (6, 3),
            ],
        );
        let bc = betweenness_centrality(7, &adj);
        let max = bc.iter().cloned().fold(0.0, f64::max);
        assert_eq!(bc[6], max, "the bridge is the most central — {bc:?}");
        assert!(bc[6] > bc[0], "bridge outscores a clique member");
    }

    #[test]
    fn connected_components_splits_a_disconnected_graph() {
        // {0,1,2} connected, {3,4} connected, 5 alone.
        let adj = undirected(6, &[(0, 1), (1, 2), (3, 4)]);
        let comps = connected_components(6, &adj);
        let mut sizes: Vec<usize> = comps.iter().map(|c| c.len()).collect();
        sizes.sort();
        assert_eq!(sizes, vec![1, 2, 3]);
    }

    #[test]
    fn hop_distances_decay_from_the_source() {
        // 0 - 1 - 2 - 3 - 4, source = {0}.
        let adj = undirected(5, &[(0, 1), (1, 2), (2, 3), (3, 4)]);
        let d = hop_distances(5, &adj, &[0]);
        assert_eq!(d, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn hop_distances_take_the_nearest_source_and_leave_islands_unreached() {
        // 0 - 1 - 2 - 3 ; 4 isolated. Sources {0,3}: node 1 is 1 from 0,
        // node 2 is 1 from 3.
        let adj = undirected(5, &[(0, 1), (1, 2), (2, 3)]);
        let d = hop_distances(5, &adj, &[0, 3]);
        assert_eq!(d[0], 0);
        assert_eq!(d[3], 0);
        assert_eq!(d[1], 1);
        assert_eq!(d[2], 1);
        assert_eq!(d[4], usize::MAX, "an isolated node is unreached");
    }
}
