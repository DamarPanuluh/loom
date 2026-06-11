//! Graph integrity checks for `loom doctor`.
//!
//! grafeo is schema-optional, so loom verifies its own invariants here against
//! the single declared vocabulary in `crate::db::schema`. Everything uses
//! reliable query paths: node/edge counts, `IS NULL` presence predicates (which
//! — unlike relationship *equality* matching — work on edges too), and Rust-side
//! value validation.

use anyhow::Result;

use crate::db::schema::{self, prop, EDGE_TYPES, NODE_LABELS};
use crate::db::LoomDb;
use crate::types::{
    AbstractionLevel, HypothesisStatus, IntentStatus, NoteKind, Severity, ValidationResult,
};

use super::row::i64_val;
use super::snapshot::QuerySnapshot;
use super::{list_all_targets, list_hypotheses, list_intents, list_notes};

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

fn count(db: &dyn LoomDb, query: &str) -> Result<i64> {
    let r = db.execute(query)?;
    Ok(r.rows().first().map(|row| i64_val(&row[0])).unwrap_or(0))
}

fn count_nodes(db: &dyn LoomDb, lbl: &str) -> Result<i64> {
    count(db, &format!("MATCH (n:{lbl}) RETURN count(n) AS c"))
}

fn count_edges(db: &dyn LoomDb, etype: &str) -> Result<i64> {
    count(db, &format!("MATCH ()-[r:{etype}]->() RETURN count(r) AS c"))
}

/// Nodes of `lbl` missing property `p` (present as NULL / never written).
fn nodes_missing_prop(db: &dyn LoomDb, lbl: &str, p: &str) -> Result<i64> {
    count(db, &format!("MATCH (n:{lbl}) WHERE n.{p} IS NULL RETURN count(n) AS c"))
}

/// Edges of `etype` missing property `p`.
fn edges_missing_prop(db: &dyn LoomDb, etype: &str, p: &str) -> Result<i64> {
    count(db, &format!("MATCH ()-[r:{etype}]->() WHERE r.{p} IS NULL RETURN count(r) AS c"))
}

/// Run every integrity check and collect a report.
pub fn check_graph(db: &dyn LoomDb) -> Result<DoctorReport> {
    let mut issues = Vec::new();
    let mut hints = Vec::new();

    // 1. Schema version.
    let meta = db.execute(schema::CHECK_INITIALIZED)?;
    let found_version = meta
        .rows()
        .first()
        .map(|row| super::row::str_val(&row[0]))
        .unwrap_or_default();
    let expected_version = schema::SCHEMA_VERSION.to_string();
    let version_ok = found_version == expected_version;
    if !version_ok {
        issues.push(format!(
            "schema version mismatch: graph is '{}', this loom expects '{}' \
             (re-init or migrate)",
            found_version, expected_version
        ));
    }

    // Identity + custody on the meta sentinel (federation).
    if let Some(m) = super::meta::get_meta(db)? {
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
    let mut node_counts = Vec::new();
    for &lbl in NODE_LABELS {
        node_counts.push((lbl.to_string(), count_nodes(db, lbl)?));
        for &(p, _owner) in schema::required_node_props(lbl) {
            let missing = nodes_missing_prop(db, lbl, p)?;
            if missing > 0 {
                issues.push(format!("{missing} {lbl} node(s) missing property '{p}'"));
            }
        }
    }

    // 3. Edge counts + required-property presence.
    let mut edge_counts = Vec::new();
    for &etype in EDGE_TYPES {
        edge_counts.push((etype.to_string(), count_edges(db, etype)?));
        for &(p, _owner) in schema::required_edge_props(etype) {
            let missing = edges_missing_prop(db, etype, p)?;
            if missing > 0 {
                issues.push(format!("{missing} {etype} edge(s) missing property '{p}'"));
            }
        }
    }

    let intents = list_intents(db, None, None)?;
    let hypotheses = list_hypotheses(db, None)?;

    let query_snapshot = QuerySnapshot::load(db)?;

    // 4. Value validity for constrained fields (reliable full scans).
    let vocab_terms = super::vocab::list_vocab_terms(db)?;
    let registered_terms: std::collections::HashSet<&str> =
        vocab_terms.iter().map(|t| t.name.as_str()).collect();
    for i in &intents {
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
            issues.push(format!("QualityRule {} has invalid severity '{}'", r.id, r.severity));
        }
    }
    for v in &query_snapshot.validations {
        if !v.last_result.is_empty() && v.last_result.parse::<ValidationResult>().is_err() {
            issues.push(format!(
                "Validation {} has invalid last_result '{}'",
                v.id, v.last_result
            ));
        }
    }
    // Hypothesis plane: status vocabulary + the evidence audit behind every
    // proof verdict, and the proposer≠prover contract (when roles are declared).
    for h in &hypotheses {
        if h.status.parse::<HypothesisStatus>().is_err() {
            issues.push(format!("Hypothesis {} has invalid status '{}'", h.id, h.status));
            continue;
        }
        if matches!(h.status.as_str(), "supported" | "refuted") {
            if crate::gate::is_vacuous(&h.evidence) {
                issues.push(format!(
                    "Hypothesis '{}' is '{}' but its evidence is empty/vacuous ('{}')",
                    h.name, h.status, h.evidence.trim()
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
        intents.iter().map(|i| i.id.clone()).collect();
    let hypothesis_ids: std::collections::HashSet<String> =
        hypotheses.iter().map(|h| h.id.clone()).collect();
    let edge_ids = collect_edge_ids(db)?;
    for n in list_notes(db, None, None)? {
        if n.kind.parse::<NoteKind>().is_err() {
            issues.push(format!("Note {} has invalid kind '{}'", n.id, n.kind));
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
        if n.target_kind == "edge" && !edge_ids.contains(&n.target_id) {
            issues.push(format!("Note {} targets missing edge '{}' — `loom note prune` removes notes whose target no longer exists", n.id, n.target_id));
        }
    }

    audit_inspectable_edges(db, &query_snapshot, &mut issues, &mut hints)?;
    // evidence behind a verdict, and provenance lanes (a verdict recorded by an
    // out-of-lane role is a separation-of-duties breach — the whole point of
    // the role system is that no agent green-lights its own work).

    // 7. HIERARCHY tree well-formedness. These are *structural* violations (the
    // spine isn't a tree), not progress — so they belong in doctor. The other
    // completeness facts (unrealized leaves / unreached files) are progress and
    // are surfaced by `loom report` / the status compass instead.
    let vc = super::completeness::vertical_completeness(db)?;
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
        found_version,
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

/// Audit every inspectable edge for: valid inspection_status, confidence in
/// [0,1], a substantive criterion behind any passing/failing verdict
/// (RELATES_TO/GOVERNS — IMPLEMENTS starts passing-by-construction with an
/// empty criterion), a meaningful confidence + timestamp behind any verdict,
/// recorded reasoning behind `independent`, and provenance lanes
/// (`inspected_by` role must be the owning role for that edge type).
fn audit_inspectable_edges(
    db: &dyn LoomDb,
    snapshot: &QuerySnapshot,
    issues: &mut Vec<String>,
    hints: &mut Vec<String>,
) -> Result<()> {
    use crate::db::schema::role;

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
    for e in list_all_targets(db)? {
        claims.push(EdgeClaim {
            etype: schema::edge::TARGETS,
            label: format!("{} → {}", e.hypothesis_name, e.intent_name),
            status: e.inspection_status,
            criterion: e.criterion,
            confidence: e.confidence,
            evidence: e.evidence,
            last_inspected: e.last_inspected,
            notes: e.notes,
            inspected_by: e.inspected_by,
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

    for c in claims {
        // `independent` is valid on RELATES_TO (confirmed unrelated), on
        // GOVERNS (measured — the rule does not apply to this intent), and on
        // TARGETS (checked — this intent turns out not to be affected).
        let independent_ok = matches!(
            c.etype,
            x if x == schema::edge::RELATES_TO
                || x == schema::edge::GOVERNS
                || x == schema::edge::TARGETS
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
                c.etype, c.label, c.status, c.criterion.trim()
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
            // The why lives in `notes` for RELATES_TO (unrelated) and in
            // `evidence` for GOVERNS (rule doesn't apply — recorded by verdict).
            if c.etype == schema::edge::RELATES_TO && crate::gate::is_vacuous(&c.notes) {
                issues.push(format!(
                    "RELATES_TO edge {} is 'independent' but records no why (notes: '{}')",
                    c.label, c.notes.trim()
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
                let allowed: &[&str] = match c.etype {
                    schema::edge::GOVERNS => &[role::QUALITY],
                    _ => &[role::ANALYZER, role::FIXER],
                };
                if !allowed.contains(&r) {
                    issues.push(format!(
                        "{} edge {} was inspected by '{}' — out of lane (expected {}); \
                         separation of duties is broken",
                        c.etype, c.label, c.inspected_by,
                        allowed.join(" or "),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Collect every tracked edge id once for note referential-integrity checks.
/// The old per-note check rescanned every edge type for each edge-targeted
/// note; large histories made `loom doctor` and `loom next --all` feel heavy.
pub(super) fn collect_edge_ids(db: &dyn LoomDb) -> Result<std::collections::HashSet<String>> {
    // v4: edge identity is DERIVED from the endpoints (schema::edge_key) —
    // nothing stored on the edge. Endpoint node ids ARE the edge id.
    let mut ids = std::collections::HashSet::new();
    for &etype in EDGE_TYPES {
        let r = db.execute(&format!(
            "MATCH (a)-[r:{etype}]->(b) RETURN a.{id} AS f, b.{id} AS t",
            id = prop::ID
        ))?;
        for row in r.rows() {
            ids.insert(crate::db::schema::edge_key(
                etype,
                &super::row::str_val(&row[0]),
                &super::row::str_val(&row[1]),
            ));
        }
    }
    Ok(ids)
}
