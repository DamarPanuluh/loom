//! Graph integrity checks for `loom doctor`.
//!
//! SQLite enforces the core shape with tables and foreign keys; loom still
//! verifies graph-level invariants here against the single declared vocabulary
//! in `crate::db::schema`.

use anyhow::Result;
use std::collections::HashSet;

use crate::db::schema;
use crate::types::{
    AbstractionLevel, CodeFile, Delegation, GovernsKind, Hypothesis, HypothesisStatus, Ignore,
    Intent, IntentStatus, Note, NoteKind, RelationKind, ServesEdge, Severity, TargetsEdge,
    ValidationResult, ValidationType, VocabTerm,
};

use super::snapshot::QuerySnapshot;
use super::GraphMeta;

/// Outcome of a full integrity scan.
#[derive(Debug)]
pub struct DoctorReport {
    pub expected_version: String,
    pub found_version: String,
    pub version_ok: bool,
    /// (label, count)
    pub node_counts: Vec<(String, i64)>,
    /// (edge type, count)
    pub edge_counts: Vec<(String, i64)>,
    /// Human-readable problems; empty == healthy.
    pub issues: Vec<String>,
    /// Advisory observations — worth knowing, never failing (doctor stays
    /// scriptable: exit code reflects `issues` only).
    pub hints: Vec<String>,
}

impl DoctorReport {
    pub fn healthy(&self) -> bool {
        self.version_ok && self.issues.is_empty()
    }
}

pub struct DoctorInputs<'a> {
    pub found_version: String,
    pub meta: Option<GraphMeta>,
    pub node_counts: Vec<(String, i64)>,
    pub edge_counts: Vec<(String, i64)>,
    pub missing_node_props: Vec<(String, String, i64)>,
    pub missing_edge_props: Vec<(String, String, i64)>,
    pub intents: Vec<Intent>,
    pub hypotheses: Vec<Hypothesis>,
    pub vocab_terms: Vec<VocabTerm>,
    pub target_edges: Vec<TargetsEdge>,
    pub serves_edges: Vec<ServesEdge>,
    pub edge_ids: std::collections::HashSet<String>,
    pub notes: &'a [Note],
}

/// Map-vs-territory reconciliation — what the graph DECLARES vs what's on DISK.
/// doctor's graph-internal pass can't see this; the read path folds the same
/// on-disk walk `loom coverage` does into the integrity check so unmapped real
/// files, drifted content, and phantom registrations block green instead of
/// laundering a healthy compass. Pure over the inputs the caller gathers (it
/// has the repo root for the walk + content hashes); no disk access here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiskReconciliation {
    /// Real source files on disk that are not grounded, not registered, not
    /// ignored, and not delegated — the map is MISSING them.
    pub unaccounted_files: Vec<String>,
    /// Registered codefiles whose current disk content hash no longer matches
    /// the recorded hash — the territory drifted since the last sync.
    pub drifted_codefiles: Vec<String>,
    /// Registered codefiles whose path no longer exists on disk — phantom map
    /// entries (the file was deleted or moved).
    pub missing_codefiles: Vec<String>,
}

impl DiskReconciliation {
    pub fn issue_count(&self) -> usize {
        self.unaccounted_files.len() + self.drifted_codefiles.len() + self.missing_codefiles.len()
    }
}

/// Reconcile the disk file list against the registered codefiles. `disk_hash`
/// returns the current content hash of a (repo-relative) path, or `None` if the
/// file can't be read (missing/unreadable) — supplied by the caller, which
/// holds the repo root. A codefile with an EMPTY recorded hash (grandfathered,
/// pre-hash) is never flagged as drifted, so this doesn't over-fire on legacy
/// registrations — only a NON-empty recorded hash that no longer matches is a
/// drift.
pub fn disk_reconciliation_from_parts(
    disk: &[String],
    codefiles: &[CodeFile],
    ignores: &[Ignore],
    delegations: &[Delegation],
    disk_hash: &dyn Fn(&str) -> Option<String>,
) -> DiskReconciliation {
    let registered: HashSet<&str> = codefiles.iter().map(|c| c.path.as_str()).collect();
    let ignore_pats: Vec<glob::Pattern> = ignores
        .iter()
        .filter_map(|i| glob::Pattern::new(&i.pattern).ok())
        .collect();
    let deleg_pats: Vec<glob::Pattern> = delegations
        .iter()
        .filter_map(|d| glob::Pattern::new(&d.pattern).ok())
        .collect();
    let is_ignored = |p: &str| ignore_pats.iter().any(|pat| pat.matches(p));
    let is_delegated = |p: &str| deleg_pats.iter().any(|pat| pat.matches(p));

    let mut unaccounted = Vec::new();
    for f in disk {
        if registered.contains(f.as_str()) || is_ignored(f) || is_delegated(f) {
            continue;
        }
        unaccounted.push(f.clone());
    }
    let mut drifted = Vec::new();
    let mut missing = Vec::new();
    for cf in codefiles {
        match disk_hash(&cf.path) {
            None => missing.push(cf.path.clone()),
            Some(h) => {
                if !cf.content_hash.is_empty() && cf.content_hash != h {
                    drifted.push(cf.path.clone());
                }
            }
        }
    }
    unaccounted.sort();
    drifted.sort();
    missing.sort();
    DiskReconciliation {
        unaccounted_files: unaccounted,
        drifted_codefiles: drifted,
        missing_codefiles: missing,
    }
}

pub fn check_graph_from_parts(
    query_snapshot: &QuerySnapshot,
    inputs: DoctorInputs<'_>,
) -> Result<DoctorReport> {
    let mut issues = Vec::new();
    let mut hints = Vec::new();

    // 1. Schema version.
    let expected_version = schema::SCHEMA_VERSION.to_string();
    let version_ok = inputs.found_version == expected_version;
    if !version_ok {
        issues.push(format!(
            "schema version mismatch: graph is '{}', this loom expects '{}' \
             (no in-place upgrade — re-export from the older loom, then `loom init . && loom import`)",
            inputs.found_version, expected_version
        ));
    }

    // Identity + custody on the meta sentinel (federation).
    if let Some(m) = &inputs.meta {
        if !m.custody.is_empty() && !matches!(m.custody.as_str(), "owned" | "observed") {
            issues.push(format!(
                "LoomMeta has invalid custody '{}' (valid: owned, observed)",
                m.custody
            ));
        }
        if m.graph_id.is_empty() {
            hints.push(
                "this graph has no identity (pre-federation) — run `loom init .` to backfill \
                 graph_id/graph_name so other looms can reference it"
                    .to_string(),
            );
        }
    }

    // 2. Node counts + required-property presence.
    let node_counts = inputs.node_counts;
    for (lbl, p, missing) in &inputs.missing_node_props {
        issues.push(format!("{missing} {lbl} node(s) missing property '{p}'"));
    }

    // 3. Edge counts + required-property presence.
    let edge_counts = inputs.edge_counts;
    for (etype, p, missing) in &inputs.missing_edge_props {
        issues.push(format!("{missing} {etype} edge(s) missing property '{p}'"));
    }

    // 4. Value validity for constrained fields (reliable full scans).
    let registered_terms: std::collections::HashSet<&str> =
        inputs.vocab_terms.iter().map(|t| t.name.as_str()).collect();
    for i in &inputs.intents {
        if i.id.is_empty() {
            issues.push(format!("Intent '{}' has an empty id", i.name));
        }
        if i.status.parse::<IntentStatus>().is_err() {
            issues.push(format!("Intent {} has invalid status '{}'", i.id, i.status));
        }
        if i.abstraction_level.parse::<AbstractionLevel>().is_err() {
            issues.push(format!(
                "Intent {} has invalid abstraction_level '{}'",
                i.id, i.abstraction_level
            ));
        }
        // A SET criterion is held to the same substantive-evidence floor as edge
        // criteria (no placeholders, ≥10 chars). An EMPTY criterion is "not yet
        // stated" — additive field, not a doctor failure — so only flag a present
        // but vacuous one (a placeholder that launders as a real criterion).
        if !i.criterion.trim().is_empty() && crate::gate::is_vacuous(&i.criterion) {
            issues.push(format!(
                "Intent '{}' has a vacuous criterion '{}' — state the ONE falsifiable thing it is \
                 done/correct by, or clear it (`loom intent update <id> --criterion \"…\" --reason …`)",
                i.name, i.criterion
            ));
        }
        // Tags: within the cap, every term registered. Native list since v5
        // (malformed-JSON is impossible by construction; absent reads empty).
        match super::vocab::parse_tags(i) {
            Err(_) => {
                issues.push(format!("Intent {} has unreadable tags", i.id));
            }
            Ok(tags) => {
                if tags.len() > super::vocab::MAX_TAGS_PER_INTENT {
                    issues.push(format!(
                        "Intent '{}' carries {} tags (max {}) — tag spam makes everything \
                         collide and kills the duplicate-responsibility signal",
                        i.name,
                        tags.len(),
                        super::vocab::MAX_TAGS_PER_INTENT
                    ));
                }
                for t in &tags {
                    if !registered_terms.contains(t.as_str()) {
                        issues.push(format!(
                            "Intent '{}' is tagged '{}' but no such VocabTerm is registered \
                             (register it: loom vocab add {} --why \"…\"; or retag the intent)",
                            i.name, t, t
                        ));
                    }
                }
            }
        }
    }
    for r in &query_snapshot.rules {
        if r.severity.parse::<Severity>().is_err() {
            issues.push(format!(
                "QualityRule {} has invalid severity '{}'",
                r.id, r.severity
            ));
        }
        // Taxonomy: the norm category (the GOVERNS kind) must be in the closed
        // vocab; "" is allowed (uncategorized).
        if !r.kind.is_empty() && r.kind.parse::<GovernsKind>().is_err() {
            issues.push(format!(
                "QualityRule '{}' has unknown kind '{}' (valid: {}, or empty)",
                r.name,
                r.kind,
                GovernsKind::ALL
                    .iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    // Taxonomy: every relationship kind on a RELATES_TO edge must be in the
    // closed RelationKind vocab.
    let mut weak_only: Vec<String> = Vec::new();
    for e in &query_snapshot.relates {
        for k in &e.kinds {
            if k.parse::<RelationKind>().is_err() {
                issues.push(format!(
                    "RELATES_TO edge {} → {} has unknown kind '{}' (valid: {})",
                    e.from_name,
                    e.to_name,
                    k,
                    RelationKind::ALL
                        .iter()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        // Epistemic: a passing/failing verdict whose every relationship kind is
        // weak (concept similarity, not proven coupling) is a weak claim.
        if matches!(e.inspection_status.as_str(), "passing" | "failing")
            && !e.kinds.is_empty()
            && e.kinds.iter().all(|k| {
                k.parse::<RelationKind>()
                    .map(|rk| rk.trust_weight() == "weak")
                    .unwrap_or(false)
            })
        {
            weak_only.push(format!(
                "{} → {} [{}]",
                e.from_name,
                e.to_name,
                e.kinds.join(", ")
            ));
        }
    }
    if !weak_only.is_empty() {
        let shown: Vec<&String> = weak_only.iter().take(5).collect();
        hints.push(format!(
            "epistemic: {} passing/failing relationship(s) are grounded ONLY by weak kinds \
             (shares_vocab/same_domain/doc_reference — concept similarity, not proven coupling); \
             sample with `loom next --mode review`{}{}",
            weak_only.len(),
            if shown.is_empty() { "" } else { ": " },
            shown
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    for v in &query_snapshot.validations {
        if !v.last_result.is_empty() && v.last_result.parse::<ValidationResult>().is_err() {
            issues.push(format!(
                "Validation {} has invalid last_result '{}'",
                v.id, v.last_result
            ));
        }
        if v.validation_type.parse::<ValidationType>().is_err() {
            issues.push(format!(
                "Validation {} has invalid validation_type '{}' (valid: test, assertion, benchmark, manual_check, saga)",
                v.id, v.validation_type
            ));
        }
    }
    // Hypothesis plane: status vocabulary + the evidence audit behind every
    // proof verdict, and the proposer≠prover contract (when roles are declared).
    for h in &inputs.hypotheses {
        if h.status.parse::<HypothesisStatus>().is_err() {
            issues.push(format!(
                "Hypothesis {} has invalid status '{}'",
                h.id, h.status
            ));
            continue;
        }
        if matches!(h.status.as_str(), "supported" | "refuted") {
            if crate::gate::is_vacuous(&h.evidence) {
                issues.push(format!(
                    "Hypothesis '{}' is '{}' but its evidence is empty/vacuous ('{}')",
                    h.name,
                    h.status,
                    h.evidence.trim()
                ));
            }
            if h.last_inspected.trim().is_empty() {
                issues.push(format!(
                    "Hypothesis '{}' is '{}' but last_inspected is empty — \
                     the verdict has no inspection timestamp",
                    h.name, h.status
                ));
            }
        }
        if h.status != "proposed"
            && crate::gate::role_of(&h.author).is_some()
            && crate::gate::role_of(&h.inspected_by).is_some()
            && h.author == h.inspected_by
        {
            issues.push(format!(
                "Hypothesis '{}' was proposed AND proven by '{}' — \
                 separation of duties is broken (proposer must not be the prover)",
                h.name, h.author
            ));
        }
    }

    // 5. Note validity + referential integrity (dangling targets).
    let intent_ids: std::collections::HashSet<String> =
        inputs.intents.iter().map(|i| i.id.clone()).collect();
    let hypothesis_ids: std::collections::HashSet<String> =
        inputs.hypotheses.iter().map(|h| h.id.clone()).collect();
    for n in inputs.notes {
        if let Err(e) = n.kind.parse::<NoteKind>() {
            issues.push(format!(
                "Note {} has invalid kind '{}' — {} (likely written by a different loom version; \
                 `loom note list --limit 0 --json` locates it, and a fresh `loom note add --kind <valid>` \
                 preserves the text under a valid kind)",
                n.id, n.kind, e
            ));
        }
        if n.target_kind == "intent" && !intent_ids.contains(&n.target_id) {
            issues.push(format!(
                "Note {} targets missing intent '{}' — `loom note prune` removes notes whose target no longer exists",
                n.id, n.target_id
            ));
        }
        if n.target_kind == "hypothesis" && !hypothesis_ids.contains(&n.target_id) {
            issues.push(format!(
                "Note {} targets missing hypothesis '{}' — `loom note prune` removes notes whose target no longer exists",
                n.id, n.target_id
            ));
        }
        if n.target_kind == "edge" && !inputs.edge_ids.contains(&n.target_id) {
            issues.push(format!("Note {} targets missing edge '{}' — `loom note prune` removes notes whose target no longer exists", n.id, n.target_id));
        }
    }

    // 5b. Smell-adjudication quality — the rubber-stamp audit. The write-time
    // gate blocks NEW batch-stamps; this surfaces a pre-existing backlog of
    // findings ruled away on vacuous or templated rationales.
    audit_smell_adjudications(inputs.notes, &mut issues, &mut hints);

    audit_inspectable_edges(
        query_snapshot,
        &inputs.target_edges,
        &inputs.serves_edges,
        &mut issues,
        &mut hints,
    )?;
    // evidence behind a verdict, and provenance lanes (a verdict recorded by an
    // out-of-lane role is a separation-of-duties breach — the whole point of
    // the role system is that no agent green-lights its own work).

    // 7. HIERARCHY tree well-formedness. These are *structural* violations (the
    // spine isn't a tree), not progress — so they belong in doctor. The other
    // completeness facts (unrealized leaves / unreached files) are progress and
    // are surfaced by `loom report` / the status compass instead.
    let vc = super::completeness::vertical_completeness_from_snapshot(query_snapshot);
    for name in &vc.multi_parent {
        issues.push(format!(
            "Intent '{}' has more than one HIERARCHY parent — the hierarchy must be a tree.",
            name
        ));
    }
    if vc.cycle {
        issues.push(
            "HIERARCHY contains a cycle — the hierarchy must be an acyclic tree.".to_string(),
        );
    }

    Ok(DoctorReport {
        expected_version,
        found_version: inputs.found_version,
        version_ok,
        node_counts,
        edge_counts,
        issues,
        hints,
    })
}

/// A claim-shaped record extracted from any inspectable edge, so one audit
/// covers RELATES_TO / IMPLEMENTS / GOVERNS uniformly.
struct EdgeClaim {
    etype: &'static str,
    label: String, // "a → b" for the issue message
    status: String,
    criterion: String,
    confidence: f64,
    evidence: String,
    last_inspected: String,
    notes: String,
    inspected_by: String,
}

/// A cluster of this many smell rulings sharing one template is the
/// batch-rubber-stamp signature (one rationale pasted across findings).
const SMELL_TEMPLATE_CLUSTER_MIN: usize = 3;

/// Audit smell-adjudication decision notes for the rubber-stamp pattern. A
/// vacuous ruling that suppresses a finding without inspecting it is an ISSUE
/// (same floor as a vacuous criterion). A CLUSTER of rulings that reuse one
/// template across many findings is a HINT — the green gate is `loom smells`,
/// not doctor; this is the provenance read that says "these read as stamped,
/// not inspected." The write-time gate (`gate::require_distinct_smell_ruling`)
/// blocks NEW batch-stamps; this surfaces a pre-existing backlog to re-audit.
fn audit_smell_adjudications(notes: &[Note], issues: &mut Vec<String>, hints: &mut Vec<String>) {
    // Only the NEWEST decision note per smell target is the ACTIVE adjudication
    // (mirrors `last_decision` in smells.rs). Auditing the full note HISTORY
    // would make a rubber-stamp unrecoverable: re-auditing — superseding a
    // templated ruling with a genuine one, the exact remedy this hint names —
    // must be able to clear it. `note prune` never removes a live-target
    // decision note, so history-based auditing would be a permanent scar.
    let mut active: std::collections::HashMap<&str, &Note> = std::collections::HashMap::new();
    for n in notes
        .iter()
        .filter(|n| n.kind == "decision" && n.target_kind == "smell")
    {
        active
            .entry(n.target_id.as_str())
            .and_modify(|e| {
                if n.created_at > e.created_at {
                    *e = n;
                }
            })
            .or_insert(n);
    }
    let mut rulings: Vec<(&str, &str)> = active
        .values()
        .map(|n| (n.target_id.as_str(), n.text.as_str()))
        .collect();
    rulings.sort_by(|a, b| a.0.cmp(b.0)); // deterministic clustering + sample
    if rulings.is_empty() {
        return;
    }
    // A finding ruled away on placeholder/too-short text — never an inspection.
    for (target, text) in &rulings {
        if crate::gate::is_vacuous(text)
            || text.trim().chars().count() < crate::gate::MIN_SMELL_RULING_LEN
        {
            issues.push(format!(
                "Smell adjudication on '{target}' is vacuous/too short ('{got}') — a finding ruled \
                 deliberate must name the decomposition considered and why it is wrong HERE; \
                 re-inspect and re-rule, or let the finding re-open",
                got = text.trim(),
            ));
        }
    }
    // Template clusters: group rulings that reuse each other's wording. Greedy
    // single-pass clustering is enough to surface the pattern (doctor is not a
    // hot path, and the hint only needs to name the size + a sample).
    let mut clustered = vec![false; rulings.len()];
    for i in 0..rulings.len() {
        if clustered[i] {
            continue;
        }
        let members: Vec<usize> = (i..rulings.len())
            .filter(|&j| {
                !clustered[j]
                    && (j == i
                        || crate::gate::smell_rulings_are_templated(rulings[i].1, rulings[j].1))
            })
            .collect();
        if members.len() >= SMELL_TEMPLATE_CLUSTER_MIN {
            for &m in &members {
                clustered[m] = true;
            }
            let sample = rulings[i].1.trim();
            let sample: String = if sample.chars().count() > 100 {
                format!("{}…", sample.chars().take(100).collect::<String>())
            } else {
                sample.to_string()
            };
            hints.push(format!(
                "{} smell adjudications share one ruling template (e.g. \"{sample}\") — that \
                 uniformity is the signature of batch rubber-stamping, not per-finding inspection. \
                 Re-audit each on its own code (`loom smells --json`); a real ruling is true only of \
                 its own finding.",
                members.len(),
            ));
        }
    }
}

/// Audit every inspectable edge for: valid inspection_status, confidence in
/// [0,1], a substantive criterion behind any passing/failing verdict
/// (RELATES_TO/GOVERNS — IMPLEMENTS starts passing-by-construction with an
/// empty criterion), a meaningful confidence + timestamp behind any verdict,
/// recorded reasoning behind `independent`, and provenance lanes
/// (`inspected_by` role must be the owning role for that edge type).
fn audit_inspectable_edges(
    snapshot: &QuerySnapshot,
    targets: &[TargetsEdge],
    serves: &[ServesEdge],
    issues: &mut Vec<String>,
    hints: &mut Vec<String>,
) -> Result<()> {
    let mut claims: Vec<EdgeClaim> = Vec::new();
    for e in &snapshot.relates {
        claims.push(EdgeClaim {
            etype: schema::edge::RELATES_TO,
            label: format!("{} → {}", e.from_name, e.to_name),
            status: e.inspection_status.clone(),
            criterion: e.criterion.clone(),
            confidence: e.confidence,
            evidence: e.evidence.clone(),
            last_inspected: e.last_inspected.clone(),
            notes: e.notes.clone(),
            inspected_by: e.inspected_by.clone(),
        });
    }
    for e in &snapshot.implements {
        claims.push(EdgeClaim {
            etype: schema::edge::IMPLEMENTS,
            label: format!("{} → {}", e.intent_name, e.codefile_path),
            status: e.inspection_status.clone(),
            criterion: e.criterion.clone(),
            confidence: e.confidence,
            evidence: e.evidence.clone(),
            last_inspected: e.last_inspected.clone(),
            notes: e.notes.clone(),
            inspected_by: e.inspected_by.clone(),
        });
    }
    for e in &snapshot.governs {
        claims.push(EdgeClaim {
            etype: schema::edge::GOVERNS,
            label: format!("{} → {}", e.rule_name, e.intent_name),
            status: e.inspection_status.clone(),
            criterion: e.criterion.clone(),
            confidence: e.confidence,
            evidence: e.evidence.clone(),
            last_inspected: e.last_inspected.clone(),
            notes: e.notes.clone(),
            inspected_by: e.inspected_by.clone(),
        });
    }
    for e in targets {
        claims.push(EdgeClaim {
            etype: schema::edge::TARGETS,
            label: format!("{} → {}", e.hypothesis_name, e.intent_name),
            status: e.inspection_status.clone(),
            criterion: e.criterion.clone(),
            confidence: e.confidence,
            evidence: e.evidence.clone(),
            last_inspected: e.last_inspected.clone(),
            notes: e.notes.clone(),
            inspected_by: e.inspected_by.clone(),
        });
    }
    for e in serves {
        claims.push(EdgeClaim {
            etype: schema::edge::SERVES,
            label: format!("{} → {}", e.persona_name, e.intent_name),
            status: e.inspection_status.clone(),
            criterion: e.criterion.clone(),
            confidence: e.confidence,
            evidence: e.evidence.clone(),
            last_inspected: e.last_inspected.clone(),
            notes: e.notes.clone(),
            inspected_by: e.inspected_by.clone(),
        });
    }
    // VALIDATES carries only a status — audit its vocabulary too.
    for e in &snapshot.validates {
        if !matches!(
            e.inspection_status.as_str(),
            "uninspected" | "passing" | "failing" | "needs_reverification"
        ) {
            issues.push(format!(
                "VALIDATES edge {} → {} has invalid inspection_status '{}'",
                e.validation_name, e.intent_name, e.inspection_status
            ));
        }
    }

    // Solo-mode observation: when every recorded verdict is bare `llm`/`human`
    // (no role declared), separation of duties rests on one agent's discipline.
    // Legitimate (solo driving is supported), so a hint — not an issue.
    let verdicts = claims.iter().filter(|c| c.status != "uninspected").count();
    if verdicts > 0
        && !claims
            .iter()
            .filter(|c| c.status != "uninspected")
            .any(|c| crate::gate::role_of(&c.inspected_by).is_some())
    {
        hints.push(format!(
            "all {verdicts} verdict(s) were recorded in solo mode (bare llm/human) — \
             for real separation of duties, declare roles per agent \
             (LOOM_AGENT=llm:analyzer|quality|…); see `loom guide`"
        ));
    }

    // Epistemic-health accumulation: confidence per (author, edge-type) on real
    // verdicts (per-etype so a uniform GOVERNS batch isn't diluted by RELATES_TO
    // spread), carrying evidence so copied evidence counts as one trust unit.
    let mut conf_by_author: std::collections::HashMap<(String, &'static str), Vec<(f64, String)>> =
        std::collections::HashMap::new();
    for c in &claims {
        // `independent` is valid on RELATES_TO (confirmed unrelated), on
        // GOVERNS (measured — the rule does not apply to this intent), and on
        // TARGETS (checked — this intent turns out not to be affected).
        let independent_ok = matches!(
            c.etype,
            x if x == schema::edge::RELATES_TO
                || x == schema::edge::GOVERNS
                || x == schema::edge::TARGETS
                || x == schema::edge::SERVES
        );
        let valid_status = matches!(
            c.status.as_str(),
            "uninspected" | "passing" | "failing" | "needs_reverification"
        ) || (independent_ok && c.status == "independent");
        if !valid_status {
            issues.push(format!(
                "{} edge {} has invalid inspection_status '{}'",
                c.etype, c.label, c.status
            ));
        }
        if !(0.0..=1.0).contains(&c.confidence) || c.confidence.is_nan() {
            issues.push(format!(
                "{} edge {} has confidence {} outside [0.0, 1.0]",
                c.etype, c.label, c.confidence
            ));
        }
        // A verdict with no substantive criterion is unfalsifiable — the graph
        // looks inspected without having been inspected.
        if c.etype != schema::edge::IMPLEMENTS
            && matches!(c.status.as_str(), "passing" | "failing")
            && crate::gate::is_vacuous(&c.criterion)
        {
            issues.push(format!(
                "{} edge {} is '{}' but its criterion is empty/vacuous ('{}')",
                c.etype,
                c.label,
                c.status,
                c.criterion.trim()
            ));
        }
        // A verdict whose confidence is still the 0.0 default, or whose
        // last_inspected was never stamped, reads as "inspected" without having
        // been inspected. IMPLEMENTS is exempt: it starts passing-by-construction
        // (a structural assertion, not a verdict) with exactly those defaults.
        // `independent` carries no confidence slot (its why lives in notes/
        // evidence), so the confidence check applies to passing/failing only.
        if c.etype != schema::edge::IMPLEMENTS {
            if matches!(c.status.as_str(), "passing" | "failing") && c.confidence == 0.0 {
                issues.push(format!(
                    "{} edge {} is '{}' with confidence 0.0 — the uninspected default \
                     leaked into a verdict (re-record it with a real --confidence)",
                    c.etype, c.label, c.status
                ));
            }
            if matches!(c.status.as_str(), "passing" | "failing" | "independent")
                && c.last_inspected.trim().is_empty()
            {
                issues.push(format!(
                    "{} edge {} is '{}' but last_inspected is empty — \
                     the verdict has no inspection timestamp",
                    c.etype, c.label, c.status
                ));
            }
        }
        if c.status == "independent" {
            // The why lives in `notes` for RELATES_TO (unrelated) and SERVES
            // (intent doesn't serve this persona — `persona serve … independent
            // --notes`), and in `evidence` for GOVERNS (rule doesn't apply).
            if (c.etype == schema::edge::RELATES_TO || c.etype == schema::edge::SERVES)
                && crate::gate::is_vacuous(&c.notes)
            {
                issues.push(format!(
                    "{} edge {} is 'independent' but records no why (notes: '{}')",
                    c.etype,
                    c.label,
                    c.notes.trim()
                ));
            }
            if c.etype == schema::edge::GOVERNS && crate::gate::is_vacuous(&c.evidence) {
                issues.push(format!(
                    "GOVERNS edge {} is 'independent' (rule doesn't apply) but records no why (evidence: '{}')",
                    c.label, c.evidence.trim()
                ));
            }
        }
        // Provenance lane: a verdict stamped by an out-of-lane role.
        if c.status != "uninspected" {
            if let Some(r) = crate::gate::role_of(&c.inspected_by) {
                // Same source of truth as the write-time gate (gate.rs), so the
                // audit can't drift from what the gate actually enforces.
                let allowed = crate::gate::inspector_roles_for_edge(c.etype);
                if !allowed.contains(&r) {
                    issues.push(format!(
                        "{} edge {} was inspected by '{}' — out of lane (expected {}); \
                         separation of duties is broken",
                        c.etype,
                        c.label,
                        c.inspected_by,
                        allowed.join(" or "),
                    ));
                }
            } else if let Some(r) = crate::gate::known_bare_role(&c.inspected_by) {
                issues.push(format!(
                    "{} edge {} was inspected by '{}' — bare known role '{}' has no provenance prefix; \
                     use 'llm:{}' so lane gates can enforce separation of duties",
                    c.etype, c.label, c.inspected_by, r, r,
                ));
            }
            // NOTE: a bare `llm` inspector (role_of/known_bare_role both None) is
            // NOT flagged here — that is the legitimate solo-mode provenance the
            // gate accepts by design. A laundered bare-llm batch is caught instead
            // by the per-(author, edge-type) concentration detector below.
        }
        if c.etype != schema::edge::IMPLEMENTS
            && matches!(c.status.as_str(), "passing" | "failing")
            && !c.inspected_by.trim().is_empty()
        {
            conf_by_author
                .entry((c.inspected_by.clone(), c.etype))
                .or_default()
                .push((c.confidence, c.evidence.clone()));
        }
    }
    hints.extend(confidence_concentration_hints(&conf_by_author));
    hints.extend(unresolved_locator_hints(
        &snapshot.implements,
        &snapshot.codefiles,
    ));
    Ok(())
}

/// A locator that is purely a line / range anchor (digits, `-`, `:`) — it names
/// a position, not a symbol, so it's exempt from symbol resolution.
fn is_line_anchor(loc: &str) -> bool {
    !loc.is_empty()
        && loc
            .chars()
            .all(|c| c.is_ascii_digit() || c == '-' || c == ':')
}

/// Is `loc` a symbol-style anchor (a code identifier / `fn name` / `Type::method`)
/// rather than a prose/text anchor? A bare token (no internal whitespace) always
/// is; a multi-word locator is symbol-style only if it leads with a code keyword.
/// A prose anchor ("mockup is contract, not realization") isn't naming a symbol,
/// so it must not be judged for symbol resolution.
fn is_symbol_style_locator(loc: &str) -> bool {
    if !loc.contains(char::is_whitespace) {
        return true;
    }
    matches!(
        loc.split_whitespace().next().unwrap_or(""),
        "fn" | "pub"
            | "def"
            | "struct"
            | "impl"
            | "const"
            | "static"
            | "type"
            | "enum"
            | "trait"
            | "macro"
            | "func"
            | "class"
            | "async"
            | "unsafe"
            | "extern"
            | "mod"
    )
}

/// HINT (#2 honesty): a passing/failing IMPLEMENTS edge whose symbol-style locator
/// does NOT resolve to any extracted symbol in the file — a grounding that reads
/// green but anchors nothing (the symbol was renamed or moved). Only HIGH-grade
/// (tree-sitter) files are judged: their fact set is COMPLETE (methods included),
/// so non-resolution is real drift. A low/none/ungraded file's facts are
/// incomplete or absent, so non-resolution there is expected — judging it would
/// just be noise (e.g. loom's own pre-grade graph, or a method-level locator
/// whose facts predate method extraction). This precision is exactly what the
/// per-file `extractor_grade` exists for. Prose / line-anchor / file-level
/// locators are exempt. Advisory, sampled — never an issue. Reuses `fact_is_grounded`.
fn unresolved_locator_hints(
    implements: &[crate::types::Implements],
    codefiles: &[crate::types::CodeFile],
) -> Vec<String> {
    use crate::db::queries::symbol_accountability::fact_is_grounded;
    let facts_by_path: std::collections::HashMap<&str, &Vec<crate::types::SymbolFact>> = codefiles
        .iter()
        .filter(|cf| cf.extractor_grade == "high" && !cf.symbol_facts.is_empty())
        .map(|cf| (cf.path.as_str(), &cf.symbol_facts))
        .collect();
    let mut unresolved: Vec<String> = Vec::new();
    for im in implements {
        if !matches!(im.inspection_status.as_str(), "passing" | "failing") {
            continue;
        }
        let loc = im.locator.trim();
        if loc.is_empty() || is_line_anchor(loc) || !is_symbol_style_locator(loc) {
            continue;
        }
        let Some(facts) = facts_by_path.get(im.codefile_path.as_str()) else {
            continue; // not a high-grade file with a complete fact set → can't judge
        };
        let one = [im.locator.clone()];
        if !facts.iter().any(|f| fact_is_grounded(f, &one)) {
            unresolved.push(format!(
                "'{}' @ {} '{loc}'",
                im.intent_name, im.codefile_path
            ));
        }
    }
    if unresolved.is_empty() {
        return Vec::new();
    }
    unresolved.sort();
    let shown = unresolved
        .iter()
        .take(5)
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    let more = unresolved.len().saturating_sub(5);
    vec![format!(
        "epistemic: {} passing/failing IMPLEMENTS locator(s) name a symbol NOT in the \
         (tree-sitter-extracted) file's facts — a grounding that reads green but anchors nothing \
         (symbol renamed/moved — re-ground or `loom sync`). Sample: {shown}{}",
        unresolved.len(),
        if more > 0 {
            format!("; … +{more} more")
        } else {
            String::new()
        }
    )]
}

/// Epistemic-health detector: a single author whose passing/failing verdicts
/// cluster at one confidence value is the corrupt-batch signature (uniform 0.9
/// with no per-edge judgement). Returns advisory hints, NEVER issues — doctor
/// can be structurally healthy yet warn a lane deserves a review sample.
/// Calibrated to stay silent on legitimate spread: loom's own analyzer/quality
/// lanes span 9–13 distinct confidence values, so the `distinct <= 2` guard
/// keeps them quiet while a uniform-0.9 scout (1 distinct value) trips it.
fn confidence_concentration_hints(
    conf_by_author: &std::collections::HashMap<(String, &'static str), Vec<(f64, String)>>,
) -> Vec<String> {
    /// Below this, a distribution is too small to judge.
    const MIN_VERDICTS: usize = 20;
    // (author, etype, n, mode_conf, distinct_conf, distinct_evidence)
    let mut concentrated: Vec<(String, &'static str, usize, f64, usize, usize)> = Vec::new();
    for ((author, etype), entries) in conf_by_author {
        if entries.len() < MIN_VERDICTS {
            continue;
        }
        let mut buckets: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        for (cf, _) in entries {
            *buckets.entry((cf * 100.0).round() as u64).or_default() += 1;
        }
        let distinct = buckets.len();
        let mode_count = buckets.values().copied().max().unwrap_or(0);
        let mode_key = buckets
            .iter()
            .max_by_key(|(_, n)| **n)
            .map(|(k, _)| *k)
            .unwrap_or(0);
        let distinct_evidence = entries
            .iter()
            .map(|(_, e)| e.trim())
            .collect::<std::collections::HashSet<_>>()
            .len();
        // Per (author, edge-type): near-uniform confidence (<=2 distinct, mode
        // >=90%) OR copied evidence (a handful of strings behind many verdicts).
        let near_uniform = distinct <= 2 && mode_count * 10 >= entries.len() * 9;
        let copied_evidence = distinct_evidence * 5 <= entries.len();
        if near_uniform || copied_evidence {
            concentrated.push((
                author.clone(),
                etype,
                entries.len(),
                mode_key as f64 / 100.0,
                distinct,
                distinct_evidence,
            ));
        }
    }
    concentrated.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| a.0.cmp(&b.0))
            .then_with(|| a.1.cmp(b.1))
    });
    concentrated
        .into_iter()
        .map(|(author, etype, n, conf, distinct, evidence)| {
            format!(
                "epistemic: '{author}' recorded {n} {etype} passing/failing verdict(s) at \
                 near-uniform confidence (~{conf:.2}, {distinct} distinct value(s)) backed by only \
                 {evidence} distinct evidence string(s) — a lane that rates everything the same with \
                 copied evidence is the laundering smell; sample it with `loom next --mode review`"
            )
        })
        .collect()
}

#[cfg(test)]
mod epistemic_tests {
    use super::confidence_concentration_hints;
    use std::collections::HashMap;

    type ConfMap = HashMap<(String, &'static str), Vec<(f64, String)>>;

    #[test]
    fn flags_uniform_confidence_lane_but_spares_spread() {
        let mut m: ConfMap = HashMap::new();
        // A uniform-0.9 scout across 25 GOVERNS verdicts (distinct evidence each)
        // → the near-uniform corrupt-batch smell.
        m.insert(
            ("llm:scout".into(), "GOVERNS"),
            (0..25).map(|i| (0.9, format!("evidence {i}"))).collect(),
        );
        // A well-spread analyzer (5 distinct conf, distinct evidence over 40) → quiet.
        m.insert(
            ("llm:analyzer".into(), "RELATES_TO"),
            (0..40)
                .map(|i| (0.5 + (i % 5) as f64 * 0.1, format!("evidence {i}")))
                .collect(),
        );
        // Uniform but too few verdicts to judge → quiet.
        m.insert(
            ("llm:fixer".into(), "RELATES_TO"),
            (0..10).map(|i| (0.9, format!("evidence {i}"))).collect(),
        );

        let hints = confidence_concentration_hints(&m);
        assert_eq!(
            hints.len(),
            1,
            "only the uniform lane should flag: {hints:?}"
        );
        assert!(hints[0].contains("llm:scout"), "got: {hints:?}");
        assert!(
            hints[0].contains("GOVERNS"),
            "names the edge type: {hints:?}"
        );
    }

    #[test]
    fn flags_copied_evidence_even_with_spread_confidence() {
        let mut m: ConfMap = HashMap::new();
        // 30 verdicts, SPREAD confidence (not near-uniform) but only 2 evidence
        // strings behind them → the copied-evidence laundering signal.
        m.insert(
            ("llm:scout".into(), "GOVERNS"),
            (0..30)
                .map(|i| {
                    (
                        0.5 + (i % 6) as f64 * 0.08,
                        if i % 2 == 0 { "A".into() } else { "B".into() },
                    )
                })
                .collect(),
        );
        let hints = confidence_concentration_hints(&m);
        assert_eq!(
            hints.len(),
            1,
            "copied evidence should flag even with spread confidence: {hints:?}"
        );
        assert!(hints[0].contains("distinct evidence"), "got: {hints:?}");
    }

    // HINT: a passing/failing IMPLEMENTS locator that names a symbol NOT in a
    // HIGH-grade file's extracted facts anchors nothing. Real symbol / file-level
    // / line-anchor / prose locators are exempt, and a non-high-grade file (whose
    // fact set is incomplete) is never judged — so the check stays quiet on loom's
    // own legitimately ungraded graph and only fires on real drift.
    #[test]
    fn unresolved_locator_flags_fabricated_symbol() {
        use crate::types::{CodeFile, Implements, SymbolFact};
        let fact = SymbolFact {
            label: "fn real_sym".into(),
            name: "real_sym".into(),
            kind: "fn".into(),
            visibility: "public".into(),
            line_start: 1,
            line_end: 2,
            is_test: false,
            string_literals: vec![],
            panic_marker_count: 0,
            panic_markers: vec![],
            body_hash: String::new(),
            shape_hash: String::new(),
        };
        let cf = CodeFile {
            id: "cf1".into(),
            path: "src/a.rs".into(),
            language: "rust".into(),
            last_modified: String::new(),
            imports: vec![],
            symbols: vec![],
            symbol_facts: vec![fact],
            content_hash: String::new(),
            extractor_grade: "high".into(),
        };
        let im = |loc: &str| Implements {
            id: format!("imp:{loc}"),
            intent_id: "i".into(),
            codefile_id: "cf1".into(),
            intent_name: "intent".into(),
            codefile_path: "src/a.rs".into(),
            inspection_status: "passing".into(),
            criterion: String::new(),
            confidence: 0.0,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: String::new(),
            locator: loc.into(),
            notes: String::new(),
            created_at: "t".into(),
        };
        // A symbol-style locator NOT in a HIGH-grade file → flagged.
        let flagged =
            super::unresolved_locator_hints(&[im("fn ghost_sym")], std::slice::from_ref(&cf));
        assert_eq!(flagged.len(), 1, "{flagged:?}");
        assert!(flagged[0].contains("anchors nothing"), "{flagged:?}");

        // Exempt: real symbol, file-level (empty), line anchor, AND a prose anchor
        // (it never named a symbol — a real loom pattern that was false-flagged).
        let clean = super::unresolved_locator_hints(
            &[
                im("fn real_sym"),
                im(""),
                im("42-50"),
                im("mockup is contract, not realization"),
            ],
            std::slice::from_ref(&cf),
        );
        assert!(clean.is_empty(), "{clean:?}");

        // A non-high-grade file is never judged (incomplete fact set) — quiet even
        // on a fabricated locator. This is what keeps loom's own ungraded graph quiet.
        let low = CodeFile {
            extractor_grade: "low".into(),
            ..cf.clone()
        };
        assert!(
            super::unresolved_locator_hints(&[im("fn ghost_sym")], std::slice::from_ref(&low))
                .is_empty(),
            "a low/ungraded file must not be judged"
        );
    }
}

#[cfg(test)]
mod disk_reconciliation_tests {
    use super::{disk_reconciliation_from_parts, DiskReconciliation};
    use crate::types::{CodeFile, Delegation, Ignore};

    fn cf(path: &str, hash: &str) -> CodeFile {
        CodeFile {
            id: format!("cf:{path}"),
            path: path.to_string(),
            language: "rust".to_string(),
            last_modified: String::new(),
            imports: Vec::new(),
            symbols: Vec::new(),
            symbol_facts: Vec::new(),
            content_hash: hash.to_string(),
            extractor_grade: String::new(),
        }
    }
    fn ignore(pattern: &str) -> Ignore {
        Ignore {
            id: format!("ig:{pattern}"),
            pattern: pattern.to_string(),
            reason: "test".to_string(),
            author: "test".to_string(),
            created_at: "t".to_string(),
        }
    }
    fn delegation(pattern: &str) -> Delegation {
        Delegation {
            id: format!("dg:{pattern}"),
            pattern: pattern.to_string(),
            target: "child/loom.graph.json".to_string(),
            export_hash: String::new(),
            seam_intents: Vec::new(),
            author: "test".to_string(),
            created_at: "t".to_string(),
        }
    }

    // FALSE-GREEN [map-vs-territory-reconcile-on-read]: the disk-vs-graph
    // reconciliation must surface the three real gaps (unmapped, drifted,
    // missing) WITHOUT over-firing on legitimate cases (ignored files,
    // delegated subtrees, and grandfathered registrations with no recorded
    // hash are all fine — only the laundered SHAPE is a problem).
    #[test]
    fn reconciles_unmapped_drifted_missing_without_over_firing() {
        let disk = vec![
            "src/mapped.rs".to_string(),   // registered + grounded → fine
            "src/drifted.rs".to_string(),  // registered, hash mismatch → drifted
            "src/legacy.rs".to_string(),   // registered, EMPTY hash → grandfathered, NOT drifted
            "src/ignored.rs".to_string(),  // matches an ignore glob → excluded
            "sub/child.rs".to_string(),    // matches a delegation → excluded
            "src/phantom.rs".to_string(),  // NOT on disk (disk_hash None) — registered → missing
            "src/unmapped.rs".to_string(), // real, not registered/ignored/delegated → unmapped
        ];
        let codefiles = vec![
            cf("src/mapped.rs", "h-mapped"),
            cf("src/drifted.rs", "h-old"),
            cf("src/legacy.rs", ""),
            cf("src/phantom.rs", "h-phantom"),
        ];
        let ignores = vec![ignore("src/ignored.rs")];
        let delegations = vec![delegation("sub/**")];
        // disk_hash: phantom returns None (file gone); mapped matches its
        // recorded hash (not drifted); drifted returns a NEW hash (drifted);
        // legacy's disk content is irrelevant — its recorded hash is EMPTY so
        // it's grandfathered and never flagged drifted.
        let disk_hash = |p: &str| match p {
            "src/phantom.rs" => None,
            "src/mapped.rs" => Some("h-mapped".to_string()),
            "src/drifted.rs" => Some("h-new".to_string()),
            _ => Some("h-disk".to_string()),
        };
        let rec =
            disk_reconciliation_from_parts(&disk, &codefiles, &ignores, &delegations, &disk_hash);

        assert_eq!(rec.unaccounted_files, vec!["src/unmapped.rs"]);
        assert_eq!(rec.drifted_codefiles, vec!["src/drifted.rs"]);
        assert!(
            !rec.drifted_codefiles.contains(&"src/legacy.rs".to_string()),
            "an EMPTY recorded hash is grandfathered, not drifted: {:?}",
            rec.drifted_codefiles
        );
        assert_eq!(rec.missing_codefiles, vec!["src/phantom.rs"]);
        assert_eq!(rec.issue_count(), 3);
    }

    #[test]
    fn empty_disk_and_empty_codefiles_is_clean() {
        let rec = disk_reconciliation_from_parts(&[], &[], &[], &[], &|_| None);
        assert_eq!(rec, DiskReconciliation::default());
        assert_eq!(rec.issue_count(), 0);
    }
}

#[cfg(test)]
mod smell_adjudication_audit_tests {
    use super::audit_smell_adjudications;
    use crate::types::Note;

    fn smell_note(target: &str, text: &str, created_at: &str) -> Note {
        Note {
            id: format!("n:{target}:{created_at}"),
            kind: "decision".to_string(),
            text: text.to_string(),
            author: "llm".to_string(),
            target_kind: "smell".to_string(),
            target_id: target.to_string(),
            resolution: String::new(),
            audience: String::new(),
            created_at: created_at.to_string(),
        }
    }

    const TEMPLATE: &str =
        "Deliberate: this file serves several subcommand intents in one cohesive command module surface.";

    fn genuine(n: usize) -> &'static str {
        [
            "Audited intent.rs: a flat match over eight Intent lifecycle verbs, each a linear resolve-mutate-render arm.",
            "Audited rule.rs: dispatch add/seed/verdict/list/detect/show, expanding const pack tables into governable rules.",
            "Audited saga.rs: add/run/diagnose/list/show over consumer-plane proofs; the runner invokes diagnose on failure.",
        ][n]
    }

    // The defect this guards: `audit_smell_adjudications` must audit the ACTIVE
    // adjudication (newest decision note per target), not the full note HISTORY.
    // A rubber-stamp superseded by a genuine per-finding ruling — the exact
    // remedy the hint prescribes — must clear; else re-auditing could never
    // satisfy the audit, since `note prune` won't remove a live-target note.
    #[test]
    fn audits_active_adjudication_not_superseded_history() {
        let targets = [
            "tangled_file:a.rs",
            "tangled_file:b.rs",
            "tangled_file:c.rs",
        ];
        let mut notes = Vec::new();
        for (i, t) in targets.iter().enumerate() {
            notes.push(smell_note(t, TEMPLATE, "2026-01-01T00:00:00Z")); // old rubber-stamp
            notes.push(smell_note(t, genuine(i), "2026-02-01T00:00:00Z")); // genuine re-audit
        }
        let (mut issues, mut hints) = (Vec::new(), Vec::new());
        audit_smell_adjudications(&notes, &mut issues, &mut hints);
        assert!(
            issues.is_empty(),
            "genuine active rulings are not vacuous: {issues:?}"
        );
        assert!(
            hints.is_empty(),
            "superseded templated history must NOT cluster — only active distinct rulings count: {hints:?}"
        );
    }

    // The flip side: if the ACTIVE (newest) ruling is the templated one,
    // re-stamping over a genuine note must STILL flag the cluster.
    #[test]
    fn flags_templated_active_rulings() {
        let targets = [
            "tangled_file:a.rs",
            "tangled_file:b.rs",
            "tangled_file:c.rs",
        ];
        let mut notes = Vec::new();
        for (i, t) in targets.iter().enumerate() {
            notes.push(smell_note(t, genuine(i), "2026-01-01T00:00:00Z")); // genuine first
            notes.push(smell_note(t, TEMPLATE, "2026-02-01T00:00:00Z")); // then rubber-stamped
        }
        let (mut issues, mut hints) = (Vec::new(), Vec::new());
        audit_smell_adjudications(&notes, &mut issues, &mut hints);
        assert_eq!(
            hints.len(),
            1,
            "3 templated active rulings must cluster into one hint: {hints:?}"
        );
    }
}
