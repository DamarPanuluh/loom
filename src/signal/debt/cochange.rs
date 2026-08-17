//! Co-change clustering over mapped git history.
//!
//! Pure after parse: rename lineage → per-commit node-id sets → integer-threshold
//! pair stats → complete-link clusters → DebtCluster rows.

use super::git::{GitCommit, GitHistory, GitStatus};
use super::{debt_cluster_id, DebtCluster};
use crate::model::NodeType;
use crate::store::Snapshot;
use std::collections::{BTreeMap, BTreeSet};

const CO_CHANGE_MIN_ANALYZABLE: usize = 10;
const CO_CHANGE_MAX_RAW_PATHS: usize = 50;
const CO_CHANGE_MAX_TRACKED: usize = 20;
const CO_CHANGE_MIN_JOINT: u32 = 3;
/// Upper bound on how many members enter the O(n²) qualifying-pair scan. A repo
/// window can touch thousands of files; without a cap the pair scan is unbounded
/// quadratic. Members are pre-filtered to those that could pair at all, then the
/// highest-support survivors are kept up to this cap.
const CO_CHANGE_MAX_PAIR_MEMBERS: usize = 400;
const CO_CHANGE_CONFIRM: &str =
    "your call: do these files form one cohesive module that should stay together, or is the coupling accidental and should be cut? judge the architecture, don't defer";

struct Incidence<'a> {
    files: Vec<&'a str>,
    bits: Vec<Vec<u64>>,
    support: Vec<u32>,
}

/// Pure co-change detector. Builds rename lineage newest→oldest, maps paths to
/// unique CodeFile nodes, then scores pairs with integer thresholds and
/// complete-link clusters.
pub(super) fn co_change_clusters(snap: &Snapshot, history: &GitHistory) -> Vec<DebtCluster> {
    co_change_from_raw(snap, &history.commits)
}

/// Map raw commits (with renames) onto per-commit node-id sets, then cluster.
pub(super) fn co_change_from_raw(snap: &Snapshot, commits: &[GitCommit]) -> Vec<DebtCluster> {
    let path_to_id = unique_codefile_path_map(snap);
    let lineage = build_rename_lineage(&path_to_id, commits);
    let sets = map_commits_to_member_sets(commits, &path_to_id, &lineage);
    co_change_clusters_from_sets(snap, &sets)
}

/// path → node id, only when exactly one CodeFile has that name.
fn unique_codefile_path_map(snap: &Snapshot) -> BTreeMap<&str, &str> {
    let mut path_counts: BTreeMap<&str, u32> = BTreeMap::new();
    for n in &snap.nodes {
        if n.node_type == NodeType::CodeFile {
            *path_counts.entry(n.name.as_str()).or_insert(0) += 1;
        }
    }
    snap.nodes
        .iter()
        .filter(|n| n.node_type == NodeType::CodeFile)
        .filter(|n| path_counts.get(n.name.as_str()).copied() == Some(1))
        .map(|n| (n.name.as_str(), n.id.as_str()))
        .collect()
}

/// Rename lineage: map any historical path onto the current path that a
/// CodeFile node owns. Walk newest→oldest so chained renames collapse.
/// R maps old → new's current; D never contributes a member touch.
fn build_rename_lineage(
    path_to_id: &BTreeMap<&str, &str>,
    commits: &[GitCommit],
) -> BTreeMap<String, String> {
    let mut lineage: BTreeMap<String, String> = BTreeMap::new();
    // Seed with current paths.
    for p in path_to_id.keys() {
        lineage.insert((*p).to_string(), (*p).to_string());
    }

    // commits are newest-first (git log --topo-order default from HEAD).
    for commit in commits {
        for ch in &commit.changes {
            if ch.status == GitStatus::Rename {
                if let Some(old) = &ch.other {
                    // new path may already map to a current node path
                    let current = lineage
                        .get(&ch.path)
                        .cloned()
                        .unwrap_or_else(|| ch.path.clone());
                    lineage.insert(old.clone(), current);
                }
            }
        }
    }
    lineage
}

fn resolve_path(
    path: &str,
    lineage: &BTreeMap<String, String>,
    path_to_id: &BTreeMap<&str, &str>,
) -> Option<String> {
    let current = lineage.get(path).map(|s| s.as_str()).unwrap_or(path);
    path_to_id.get(current).map(|id| (*id).to_string())
}

fn raw_path_count(commit: &GitCommit) -> usize {
    commit
        .changes
        .iter()
        .map(|ch| match ch.status {
            GitStatus::Rename | GitStatus::Copy => 2,
            _ => 1,
        })
        .sum()
}

fn map_commits_to_member_sets(
    commits: &[GitCommit],
    path_to_id: &BTreeMap<&str, &str>,
    lineage: &BTreeMap<String, String>,
) -> Vec<BTreeSet<String>> {
    let mut sets: Vec<BTreeSet<String>> = Vec::new();
    for commit in commits {
        // Bulk-noise: raw path endpoints (R/C count both sides; D counts) > 50 → skip.
        if raw_path_count(commit) > CO_CHANGE_MAX_RAW_PATHS {
            continue;
        }
        let members = commit_member_set(commit, path_to_id, lineage);
        if members.len() > CO_CHANGE_MAX_TRACKED {
            continue; // bulk-ish tracked set — skip
        }
        if members.is_empty() {
            continue;
        }
        // per-commit dedupe already via BTreeSet
        sets.push(members);
    }
    sets
}

fn commit_member_set(
    commit: &GitCommit,
    path_to_id: &BTreeMap<&str, &str>,
    lineage: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    let mut members: BTreeSet<String> = BTreeSet::new();
    for ch in &commit.changes {
        match ch.status {
            GitStatus::Delete => {
                // counts toward bulk noise only (already counted in raw_paths)
            }
            GitStatus::Rename => {
                // touch the current node of the renamed path (new side)
                if let Some(id) = resolve_path(&ch.path, lineage, path_to_id) {
                    members.insert(id);
                }
            }
            GitStatus::Copy => {
                // both endpoints independent
                if let Some(id) = resolve_path(&ch.path, lineage, path_to_id) {
                    members.insert(id);
                }
                if let Some(old) = &ch.other {
                    if let Some(id) = resolve_path(old, lineage, path_to_id) {
                        members.insert(id);
                    }
                }
            }
            GitStatus::Modify => {
                if let Some(id) = resolve_path(&ch.path, lineage, path_to_id) {
                    members.insert(id);
                }
            }
        }
        if members.len() > CO_CHANGE_MAX_TRACKED {
            break;
        }
    }
    members
}

fn co_change_clusters_from_sets(snap: &Snapshot, commits: &[BTreeSet<String>]) -> Vec<DebtCluster> {
    if commits.len() < CO_CHANGE_MIN_ANALYZABLE {
        return Vec::new();
    }
    let n_commits = commits.len();
    let Some(Incidence {
        files: members,
        bits,
        support,
    }) = build_incidence(commits)
    else {
        return Vec::new();
    };
    let words = n_commits.div_ceil(64);
    let pairs = qualifying_pairs(&members, &bits, &support, words, n_commits);
    let clusters = complete_link_clusters(&members, &pairs, &bits, words, n_commits);
    render_debt_clusters(
        &RenderCtx {
            snap,
            members: &members,
            bits: &bits,
            words,
            n_commits,
        },
        &clusters,
    )
}

/// Collect member universe, incidence bitsets, and per-member support.
/// Returns None when fewer than two members appear.
fn build_incidence(commits: &[BTreeSet<String>]) -> Option<Incidence<'_>> {
    let n_commits = commits.len();
    // Collect member universe (sorted for deterministic bit indices).
    let mut universe: BTreeSet<&str> = BTreeSet::new();
    for c in commits {
        for id in c {
            universe.insert(id.as_str());
        }
    }
    let members: Vec<&str> = universe.into_iter().collect();
    if members.len() < 2 {
        return None;
    }
    let idx: BTreeMap<&str, usize> = members.iter().enumerate().map(|(i, m)| (*m, i)).collect();
    let mcount = members.len();

    // Incidence: bitset words per member (u64 words covering n_commits).
    let words = n_commits.div_ceil(64);
    let mut bits: Vec<Vec<u64>> = vec![vec![0u64; words]; mcount];
    let mut support: Vec<u32> = vec![0u32; mcount];
    for (ci, commit) in commits.iter().enumerate() {
        let w = ci / 64;
        let b = ci % 64;
        for id in commit {
            if let Some(&mi) = idx.get(id.as_str()) {
                bits[mi][w] |= 1u64 << b;
                support[mi] = support[mi].saturating_add(1);
            }
        }
    }
    Some(Incidence {
        files: members,
        bits,
        support,
    })
}

#[derive(Clone)]
struct Pair {
    a: usize,
    b: usize, // a < b
    joint: u32,
    jaccard_bp: u32, // basis points
    lift_bp: u32,
}

/// Shared immutable deps for complete-link attach/merge decisions.
struct LinkCtx<'a> {
    pair_ok: &'a BTreeSet<(usize, usize)>,
    bits: &'a [Vec<u64>],
    words: usize,
    n_commits: usize,
}

/// Borrowed inputs for turning cluster index sets into DebtCluster rows.
struct RenderCtx<'a> {
    snap: &'a Snapshot,
    members: &'a [&'a str],
    bits: &'a [Vec<u64>],
    words: usize,
    n_commits: usize,
}

fn pair_joint(bits: &[Vec<u64>], words: usize, a: usize, b: usize) -> u32 {
    let mut j = 0u32;
    for (&a_word, &b_word) in bits[a].iter().zip(&bits[b]).take(words) {
        j = j.saturating_add((a_word & b_word).count_ones());
    }
    j
}

/// Qualifying pairs: joint>=3, Jaccard>=1/2, lift>=3/2.
/// Jaccard: joint / union ; union = sa + sb - joint
/// lift: joint * N / (sa * sb) >= 3/2  →  2 * joint * N >= 3 * sa * sb  (u128)
fn qualifying_pairs(
    members: &[&str],
    bits: &[Vec<u64>],
    support: &[u32],
    words: usize,
    n_commits: usize,
) -> Vec<Pair> {
    let mcount = members.len();
    // Exact pre-filter: joint(a,b) <= min(support[a], support[b]); a member with
    // support below MIN_JOINT can never reach the joint gate, so it is dropped
    // before the quadratic scan without changing any qualifying pair. The
    // highest-support survivors are then capped so a window touching thousands
    // of files stays bounded.
    let mut candidates: Vec<usize> = (0..mcount)
        .filter(|&i| support[i] >= CO_CHANGE_MIN_JOINT)
        .collect();
    candidates.sort_by(|&x, &y| {
        support[y]
            .cmp(&support[x])
            .then_with(|| members[x].cmp(members[y]))
    });
    candidates.truncate(CO_CHANGE_MAX_PAIR_MEMBERS);
    candidates.sort_unstable();
    let mut pairs: Vec<Pair> = Vec::new();
    for (pi, &a) in candidates.iter().enumerate() {
        for &b in &candidates[pi + 1..] {
            // a < b by construction (candidates ascending by index).
            let j = pair_joint(bits, words, a, b);
            if j < CO_CHANGE_MIN_JOINT {
                continue;
            }
            let sa = support[a];
            let sb = support[b];
            if sa == 0 || sb == 0 {
                continue;
            }
            let union = sa + sb - j;
            if union == 0 {
                continue;
            }
            // Jaccard >= 1/2  ⇔  2*j >= union
            if 2 * j < union {
                continue;
            }
            // directional supports >= 3/5  ⇔  5*j >= 3*sa and 5*j >= 3*sb
            if 5 * u64::from(j) < 3 * u64::from(sa) || 5 * u64::from(j) < 3 * u64::from(sb) {
                continue;
            }
            // lift >= 3/2  ⇔  2 * j * N >= 3 * sa * sb
            let lhs = 2u128 * u128::from(j) * u128::from(n_commits as u32);
            let rhs = 3u128 * u128::from(sa) * u128::from(sb);
            if lhs < rhs {
                continue;
            }
            let jaccard_bp = ((u64::from(j) * 10_000) / u64::from(union)) as u32;
            // lift_bp = joint * N * 10000 / (sa * sb)
            let lift_bp = ((u128::from(j) * u128::from(n_commits as u32) * 10_000u128)
                / (u128::from(sa) * u128::from(sb))) as u32;
            pairs.push(Pair {
                a,
                b,
                joint: j,
                jaccard_bp,
                lift_bp,
            });
        }
    }

    // Sort qualifying pairs: desc joint, desc jaccard_bp, desc lift_bp, asc (id,id)
    pairs.sort_by(|p, q| {
        q.joint
            .cmp(&p.joint)
            .then_with(|| q.jaccard_bp.cmp(&p.jaccard_bp))
            .then_with(|| q.lift_bp.cmp(&p.lift_bp))
            .then_with(|| members[p.a].cmp(members[q.a]))
            .then_with(|| members[p.b].cmp(members[q.b]))
    });
    pairs
}

/// Complete-link clustering, greedy by sorted pairs, max 8, min 2.
/// Unclaimed pairs seed; a free member attaches when every cross-pair
/// qualifies; two claimed clusters merge when every cross-pair qualifies.
fn complete_link_clusters(
    members: &[&str],
    pairs: &[Pair],
    bits: &[Vec<u64>],
    words: usize,
    n_commits: usize,
) -> Vec<Vec<usize>> {
    let mcount = members.len();
    // Fast pair lookup for complete-link checks.
    let mut pair_ok: BTreeSet<(usize, usize)> = BTreeSet::new();
    for p in pairs {
        pair_ok.insert((p.a, p.b));
    }
    let ctx = LinkCtx {
        pair_ok: &pair_ok,
        bits,
        words,
        n_commits,
    };

    let mut claimed: Vec<bool> = vec![false; mcount];
    let mut clusters: Vec<Vec<usize>> = Vec::new();

    for p in pairs {
        match (claimed[p.a], claimed[p.b]) {
            (false, false) => {
                claimed[p.a] = true;
                claimed[p.b] = true;
                clusters.push(vec![p.a, p.b]);
            }
            (true, true) => {
                try_merge_clusters(&mut clusters, p.a, p.b, &ctx);
            }
            (true, false) | (false, true) => {
                let (free, bound) = if claimed[p.a] { (p.b, p.a) } else { (p.a, p.b) };
                if try_attach_member(&mut clusters, free, bound, &ctx) {
                    claimed[free] = true;
                }
            }
        }
    }
    clusters
}

fn try_merge_clusters(clusters: &mut Vec<Vec<usize>>, a: usize, b: usize, ctx: &LinkCtx<'_>) {
    let ia = clusters.iter().position(|c| c.contains(&a));
    let ib = clusters.iter().position(|c| c.contains(&b));
    let (Some(ia), Some(ib)) = (ia, ib) else {
        return;
    };
    if ia == ib {
        return;
    }
    let (lo, hi) = if ia < ib { (ia, ib) } else { (ib, ia) };
    if clusters[lo].len() + clusters[hi].len() > 8 {
        return;
    }
    // every cross-pair must qualify
    if !all_cross_pairs_ok(&clusters[lo], &clusters[hi], ctx.pair_ok) {
        return;
    }
    let mut cand = clusters[lo].clone();
    cand.extend_from_slice(&clusters[hi]);
    if !cluster_cohesion_ok(ctx.bits, ctx.words, &cand, ctx.n_commits) {
        return;
    }
    // merge hi into lo, remove hi
    let moved = clusters.remove(hi);
    // lo index may have shifted if hi < lo — we ordered lo < hi so lo stable
    clusters[lo].extend(moved);
}

fn all_cross_pairs_ok(left: &[usize], right: &[usize], pair_ok: &BTreeSet<(usize, usize)>) -> bool {
    for &x in left {
        for &y in right {
            let (a, b) = if x < y { (x, y) } else { (y, x) };
            if !pair_ok.contains(&(a, b)) {
                return false;
            }
        }
    }
    true
}

fn try_attach_member(
    clusters: &mut [Vec<usize>],
    free: usize,
    bound: usize,
    ctx: &LinkCtx<'_>,
) -> bool {
    let Some(ci) = clusters.iter().position(|c| c.contains(&bound)) else {
        return false;
    };
    if clusters[ci].len() >= 8 {
        return false;
    }
    for &m in &clusters[ci] {
        let (lo, hi) = if free < m { (free, m) } else { (m, free) };
        if !ctx.pair_ok.contains(&(lo, hi)) {
            return false;
        }
    }
    let mut cand = clusters[ci].clone();
    cand.push(free);
    if !cluster_cohesion_ok(ctx.bits, ctx.words, &cand, ctx.n_commits) {
        return false;
    }
    clusters[ci].push(free);
    true
}

fn render_debt_clusters(ctx: &RenderCtx<'_>, clusters: &[Vec<usize>]) -> Vec<DebtCluster> {
    // Also re-check cohesion on seeded pairs (always true for 2 if pair_ok).
    // Emit DebtCluster rows.
    let mut out = Vec::new();
    let n_commits = ctx.n_commits;
    for members_idx in clusters {
        if members_idx.len() < 2 {
            continue;
        }
        if !cluster_cohesion_ok(ctx.bits, ctx.words, members_idx, n_commits) {
            continue;
        }
        let mut members_idx = members_idx.clone();
        members_idx.sort_unstable();
        // joint_support for impact: for pairs use pair joint; for larger use
        // intersection popcount of all members.
        let joint_support = cluster_joint(ctx.bits, ctx.words, &members_idx);
        let mut subject_ids: Vec<String> = members_idx
            .iter()
            .map(|&i| ctx.members[i].to_string())
            .collect();
        subject_ids.sort();
        subject_ids.dedup();

        // Display paths: lexically sorted CodeFile names.
        let mut display: Vec<String> = subject_ids
            .iter()
            .map(|id| super::super::node_name(ctx.snap, id))
            .collect();
        display.sort();
        let paths = display.join(", ");
        // cohesion % from generalized Jaccard of the full set
        let cohesion_pct = cluster_jaccard_pct(ctx.bits, ctx.words, &members_idx, n_commits);
        let message = format!(
            "{paths} change together in {joint_support}/{n_commits} sampled commits (cohesion {cohesion_pct}%)"
        );
        let impact = joint_support.saturating_mul((members_idx.len() as u32).saturating_sub(1));
        let cluster_id = debt_cluster_id("co_change", &subject_ids);
        out.push(DebtCluster {
            kind: "co_change".into(),
            message,
            impact,
            confirm: CO_CHANGE_CONFIRM.into(),
            cluster_id,
            subject_ids,
        });
    }
    out
}

fn cluster_joint(bits: &[Vec<u64>], words: usize, members: &[usize]) -> u32 {
    if members.is_empty() {
        return 0;
    }
    let mut acc = bits[members[0]].clone();
    for &m in &members[1..] {
        for w in 0..words {
            acc[w] &= bits[m][w];
        }
    }
    acc.iter().map(|w| w.count_ones()).sum()
}

fn cluster_union_pop(bits: &[Vec<u64>], words: usize, members: &[usize]) -> u32 {
    if members.is_empty() {
        return 0;
    }
    let mut acc = vec![0u64; words];
    for &m in members {
        for w in 0..words {
            acc[w] |= bits[m][w];
        }
    }
    acc.iter().map(|w| w.count_ones()).sum()
}

fn cluster_cohesion_ok(
    bits: &[Vec<u64>],
    words: usize,
    members: &[usize],
    _n_commits: usize,
) -> bool {
    let j = cluster_joint(bits, words, members);
    if j < CO_CHANGE_MIN_JOINT {
        return false;
    }
    let u = cluster_union_pop(bits, words, members);
    if u == 0 {
        return false;
    }
    // generalized Jaccard >= 1/2  ⇔  2*j >= u
    2 * j >= u
}

fn cluster_jaccard_pct(
    bits: &[Vec<u64>],
    words: usize,
    members: &[usize],
    _n_commits: usize,
) -> u32 {
    let j = cluster_joint(bits, words, members);
    let u = cluster_union_pop(bits, words, members);
    if u == 0 {
        return 0;
    }
    ((u64::from(j) * 100) / u64::from(u)) as u32
}

#[cfg(test)]
mod tests {
    use super::super::debt_cluster_id;
    use super::super::git::{GitChange, GitCommit, GitStatus};
    use super::super::DebtCluster;
    use super::co_change_from_raw;
    use crate::model::{Node, NodeType, TruthClass};
    use crate::store::{Identity, Snapshot};
    use std::collections::BTreeMap;

    fn codefile(id: &str, path: &str) -> Node {
        Node {
            id: id.into(),
            node_type: NodeType::CodeFile,
            name: path.into(),
            description: String::new(),
            status: String::new(),
            truth_class: TruthClass::Derived,
            body: serde_json::json!({}),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn empty_snap(nodes: Vec<Node>) -> Snapshot {
        Snapshot {
            facts: Vec::new(),
            evidence: Vec::new(),
            identity: Identity {
                graph_id: "g".into(),
                name: "t".into(),
                schema_version: crate::SCHEMA_VERSION,
                observed: false,
            },
            nodes,
            edges: Vec::new(),
            facets: Vec::new(),
            tags: Vec::new(),
            config: BTreeMap::new(),
        }
    }

    /// Snapshot with the given primary files plus `filler_n` mapped filler paths.
    fn snap_with_fillers(primaries: &[(&str, &str)], filler_n: usize) -> Snapshot {
        let mut nodes: Vec<Node> = primaries
            .iter()
            .map(|(id, path)| codefile(id, path))
            .collect();
        for i in 0..filler_n {
            nodes.push(codefile(&format!("nf{i}"), &format!("filler/{i}.rs")));
        }
        empty_snap(nodes)
    }

    fn touch(path: &str) -> GitChange {
        GitChange {
            status: GitStatus::Modify,
            path: path.into(),
            other: None,
        }
    }

    fn del(path: &str) -> GitChange {
        GitChange {
            status: GitStatus::Delete,
            path: path.into(),
            other: None,
        }
    }

    fn ren(old: &str, new: &str) -> GitChange {
        GitChange {
            status: GitStatus::Rename,
            path: new.into(),
            other: Some(old.into()),
        }
    }

    fn commit(changes: Vec<GitChange>) -> GitCommit {
        GitCommit { changes }
    }

    /// joint co-occurs of a+b, then solo_a, solo_b, then filler single-file commits.
    fn history_with_pair(
        joint: usize,
        solo_a: usize,
        solo_b: usize,
        filler: usize,
        path_a: &str,
        path_b: &str,
    ) -> Vec<GitCommit> {
        let mut out = Vec::new();
        let mut n = 0usize;
        for _ in 0..joint {
            n += 1;
            out.push(commit(vec![touch(path_a), touch(path_b)]));
        }
        for _ in 0..solo_a {
            n += 1;
            out.push(commit(vec![touch(path_a)]));
        }
        for _ in 0..solo_b {
            n += 1;
            out.push(commit(vec![touch(path_b)]));
        }
        for i in 0..filler {
            n += 1;
            out.push(commit(vec![touch(&format!("filler/{i}.rs"))],
            ));
        }
        out
    }

    #[test]
    fn detects_repeated_pair() {
        // 4 joint + 1 solo each + 6 filler = 12 analyzable commits.
        // joint=4, sa=5, sb=5, N=12
        // Jaccard 4/6 >= 1/2; dir 4/5 >= 3/5; lift 4*12/(5*5)=1.92 >= 1.5
        let snap = snap_with_fillers(&[("na", "a.rs"), ("nb", "b.rs")], 6);
        let commits = history_with_pair(4, 1, 1, 6, "a.rs", "b.rs");
        let clusters = co_change_from_raw(&snap, &commits);
        assert_eq!(clusters.len(), 1, "got {clusters:?}");
        assert_eq!(clusters[0].kind, "co_change");
        assert_eq!(clusters[0].subject_ids, vec!["na".to_string(), "nb".into()]);
        assert_eq!(clusters[0].impact, 4); // joint * (2-1)
        assert!(clusters[0].message.contains("a.rs, b.rs"));
        assert!(
            clusters[0].message.contains("4/12"),
            "{}",
            clusters[0].message
        );
        assert_eq!(
            clusters[0].cluster_id,
            debt_cluster_id("co_change", &["na".into(), "nb".into()])
        );
    }

    #[test]
    fn filters_low_joint_noise() {
        // joint only 2 → below MIN_JOINT=3
        let snap = snap_with_fillers(&[("na", "a.rs"), ("nb", "b.rs")], 6);
        let commits = history_with_pair(2, 2, 2, 6, "a.rs", "b.rs");
        let clusters = co_change_from_raw(&snap, &commits);
        assert!(
            clusters.is_empty(),
            "joint=2 must not produce a cluster, got {clusters:?}"
        );
    }

    #[test]
    fn filters_bulk_commit_over_fifty_raw_paths() {
        let snap = snap_with_fillers(&[("na", "a.rs"), ("nb", "b.rs")], 6);
        let mut commits = history_with_pair(4, 0, 0, 6, "a.rs", "b.rs");
        // Bulk commit with 51 paths including a+b — must be ignored entirely.
        let mut bulk: Vec<GitChange> = (0..49).map(|i| touch(&format!("bulk/{i}.rs"))).collect();
        bulk.push(touch("a.rs"));
        bulk.push(touch("b.rs"));
        assert!(bulk.len() > 50);
        commits.insert(0, commit(bulk));
        let clusters = co_change_from_raw(&snap, &commits);
        assert_eq!(clusters.len(), 1, "got {clusters:?}");
        assert!(
            clusters[0].message.contains("4/"),
            "bulk commit must not count toward joint: {}",
            clusters[0].message
        );
    }

    #[test]
    fn filters_low_lift_ubiquitous_files() {
        // Isolate lift: 9×{a,b} + 3 fillers → N=12, sa=sb=j=9.
        // Jaccard=1, dir=1, but lift = 9*12/(9*9) = 1.33 < 1.5 → filtered by lift alone.
        let snap = snap_with_fillers(&[("na", "a.rs"), ("nb", "b.rs")], 3);
        let mut commits = Vec::new();
        for i in 0..9 {
            commits.push(commit(vec![touch("a.rs"), touch("b.rs")]));
        }
        for i in 0..3 {
            commits.push(commit(vec![touch(&format!("filler/{i}.rs"))],
            ));
        }
        let clusters = co_change_from_raw(&snap, &commits);
        assert!(
            clusters.is_empty(),
            "lift < 3/2 must not produce a cluster, got {clusters:?}"
        );
    }

    #[test]
    fn no_transitive_chaining_without_ac() {
        // AB and BC both qualify as pairs; AC does not (low Jaccard).
        // Complete-link must refuse ABC.
        // 5×{a,b,c} + 5×{a,b} + 5×{b,c} + 8 fillers = N=23
        // jab=10, jbc=10, jac=5; sa=10, sb=15, sc=10
        // AB/BC pass gates; AC Jaccard 5/15 < 1/2.
        let snap = snap_with_fillers(&[("na", "a.rs"), ("nb", "b.rs"), ("nc", "c.rs")], 8);
        let mut commits = Vec::new();
        for i in 0..5 {
            commits.push(commit(vec![touch("a.rs"), touch("b.rs"), touch("c.rs")],
            ));
        }
        for i in 0..5 {
            commits.push(commit(vec![touch("a.rs"), touch("b.rs")],
            ));
        }
        for i in 0..5 {
            commits.push(commit(vec![touch("b.rs"), touch("c.rs")],
            ));
        }
        for i in 0..8 {
            commits.push(commit(vec![touch(&format!("filler/{i}.rs"))],
            ));
        }
        let clusters = co_change_from_raw(&snap, &commits);
        for c in &clusters {
            let has_a = c.subject_ids.iter().any(|s| s == "na");
            let has_b = c.subject_ids.iter().any(|s| s == "nb");
            let has_c = c.subject_ids.iter().any(|s| s == "nc");
            assert!(
                !(has_a && has_b && has_c),
                "must not form ABC cluster: {c:?}"
            );
        }
        // At least one pairwise cluster should fire (AB or BC).
        assert!(
            !clusters.is_empty(),
            "expected at least one pairwise cluster, got none"
        );
    }

    #[test]
    fn rename_lineage_collapses_and_delete_is_not_touch() {
        // Current node owns new.rs. History (newest→oldest): mid→new, mid touches,
        // old→mid, old touches. D never counts as a member touch.
        let mut nodes = vec![codefile("n_new", "new.rs"), codefile("n_other", "other.rs")];
        for i in 0..5 {
            nodes.push(codefile(&format!("nz{i}"), &format!("z{i}.rs")));
        }
        let snap = empty_snap(nodes);
        let mut commits = Vec::new();
        // Rename events ride with other so they count as joint, not solo support.
        commits.push(commit(vec![ren("mid.rs", "new.rs"), touch("other.rs")],
        ));
        for i in 0..4 {
            commits.push(commit(vec![touch("mid.rs"), touch("other.rs")],
            ));
        }
        commits.push(commit(vec![ren("old.rs", "mid.rs"), touch("other.rs")],
        ));
        for i in 0..3 {
            commits.push(commit(vec![touch("old.rs"), touch("other.rs")],
            ));
        }
        // delete-only: bulk-noise only, not a touch (empty after map → dropped)
        commits.push(commit(vec![del("new.rs")]));
        for i in 0..5 {
            commits.push(commit(vec![touch(&format!("z{i}.rs"))]));
        }
        // joint: r2 + 4 mid + r1 + 3 old = 9; N = 9 + 5 fillers = 14
        // lift = 9*14/(9*9) ≈ 1.56 >= 1.5
        let clusters = co_change_from_raw(&snap, &commits);
        assert_eq!(clusters.len(), 1, "got {clusters:?}");
        assert_eq!(
            clusters[0].subject_ids,
            vec!["n_new".to_string(), "n_other".into()]
        );
        assert!(
            clusters[0].message.contains("new.rs, other.rs"),
            "{}",
            clusters[0].message
        );
        assert!(
            clusters[0].message.contains("9/14"),
            "rename collapse joint expected 9/14: {}",
            clusters[0].message
        );
    }

    #[test]
    fn deterministic_ids_and_order_under_reversed_input() {
        let snap = snap_with_fillers(
            &[
                ("na", "a.rs"),
                ("nb", "b.rs"),
                ("nc", "c.rs"),
                ("nd", "d.rs"),
            ],
            4,
        );
        let mut commits = Vec::new();
        for i in 0..4 {
            commits.push(commit(vec![touch("a.rs"), touch("b.rs")],
            ));
        }
        for i in 0..4 {
            commits.push(commit(vec![touch("c.rs"), touch("d.rs")],
            ));
        }
        for i in 0..4 {
            commits.push(commit(vec![touch(&format!("filler/{i}.rs"))],
            ));
        }

        let mut rev = commits.clone();
        rev.reverse();
        for c in &mut rev {
            c.changes.reverse();
        }

        let mut c1 = co_change_from_raw(&snap, &commits);
        let mut c2 = co_change_from_raw(&snap, &rev);
        let sort = |v: &mut Vec<DebtCluster>| {
            v.sort_by(|a, b| {
                b.impact
                    .cmp(&a.impact)
                    .then_with(|| a.kind.cmp(&b.kind))
                    .then_with(|| a.cluster_id.cmp(&b.cluster_id))
            });
        };
        sort(&mut c1);
        sort(&mut c2);
        assert_eq!(c1.len(), 2, "expected two pairs, got {c1:?}");
        assert_eq!(c1.len(), c2.len());
        for (a, b) in c1.iter().zip(c2.iter()) {
            assert_eq!(a.cluster_id, b.cluster_id);
            assert_eq!(a.subject_ids, b.subject_ids);
            assert_eq!(a.impact, b.impact);
            assert_eq!(a.message, b.message);
        }
    }

    #[test]
    fn duplicate_path_codefiles_are_omitted_from_mapping() {
        let mut nodes = vec![
            codefile("n1", "dup.rs"),
            codefile("n2", "dup.rs"),
            codefile("n3", "solo.rs"),
        ];
        for i in 0..6 {
            nodes.push(codefile(&format!("nz{i}"), &format!("z{i}.rs")));
        }
        let snap = empty_snap(nodes);
        let mut commits = Vec::new();
        for i in 0..5 {
            commits.push(commit(vec![touch("dup.rs"), touch("solo.rs")],
            ));
        }
        for i in 0..6 {
            commits.push(commit(vec![touch(&format!("z{i}.rs"))]));
        }
        let clusters = co_change_from_raw(&snap, &commits);
        assert!(
            clusters.is_empty(),
            "duplicate path must be omitted, got {clusters:?}"
        );
    }
}
