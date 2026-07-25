//! Derived-plane persistence primitives — the store half of `sync`.
//!
//! Plane: engine (persistence), owning the derived truth class. Derived nodes
//! and edges get deterministic content-addressed ids and sentinel timestamps,
//! so `wipe_derived` + rebuild is byte-identical (INV-2). `stale_edge` is the
//! ripple's only way to re-open a settled asserted verdict — it records the
//! cause and writes no verdict of its own. Nothing here mints random ids for
//! derived data or touches asserted statuses (INV-5). Asserted debt promotions
//! (`add_promoted_debt_finding`) also live here as a separate write path: they
//! never convert statistical signals into edges/nodes of the derived plane.

use super::*;
use anyhow::Context;

/// Inputs for promoting a live debt cluster into one asserted Finding.
///
/// Callers must recompute the feed and pass the live cluster fields; the write
/// boundary re-checks the deterministic cluster id so provenance cannot be forged.
#[derive(Debug, Clone)]
pub(crate) struct DebtPromotionInput<'a> {
    pub cluster_id: &'a str,
    pub kind: &'a str,
    pub message: &'a str,
    pub impact: u32,
    pub confirm: &'a str,
    pub subject_ids: &'a [String],
    /// Canonical CodeFile names matching `subject_ids` order for a single
    /// subject, or sorted when multiple (co_change).
    pub subject_names: &'a [String],
    pub evidence: &'a str,
    pub confidence: f64,
}

/// Result of an idempotent debt-promotion write.
#[derive(Debug, Clone)]
pub(crate) struct DebtPromotionResult {
    pub finding: Node,
    pub created: bool,
}

/// Normalized debt-promotion write inputs after boundary checks.
struct NormalizedDebtPromotion<'a> {
    finding_id: String,
    evidence: &'a str,
}

/// Recompute cluster id, enforce c-prefix shape, trim/validate evidence and confidence.
fn normalize_debt_promotion<'a>(
    input: &DebtPromotionInput<'a>,
) -> Result<NormalizedDebtPromotion<'a>> {
    let expected = crate::signal::debt_cluster_id(input.kind, input.subject_ids);
    if expected != input.cluster_id {
        bail!(
            "debt promotion cluster_id '{}' does not match recomputed id '{}' for kind '{}' — callers cannot bind arbitrary provenance",
            input.cluster_id,
            expected,
            input.kind
        );
    }
    if input.cluster_id.len() < 2 || !input.cluster_id.starts_with('c') {
        bail!(
            "debt promotion cluster_id '{}' is not a c-prefixed cluster id",
            input.cluster_id
        );
    }

    let evidence = input.evidence.trim();
    if evidence.is_empty() || crate::model::is_placeholder(evidence) {
        bail!(
            "debt promote requires substantive evidence (not a placeholder like '…' or '<evidence>')"
        );
    }
    if !input.confidence.is_finite() || !(0.0..=1.0).contains(&input.confidence) {
        bail!("debt promote confidence must be a finite value between 0.0 and 1.0");
    }

    Ok(NormalizedDebtPromotion {
        finding_id: format!("p{}", &input.cluster_id[1..]),
        evidence,
    })
}

/// Deterministic Finding body: kind/source/evidence/impact/confidence + debt_cluster
/// snapshot, with `file` (single) or `files` (multi) when subject names exist.
fn promoted_debt_finding_body(input: &DebtPromotionInput<'_>, evidence: &str) -> serde_json::Value {
    let mut body = serde_json::json!({
        "kind": input.kind,
        "source": "debt_promotion",
        "evidence": evidence,
        "impact": input.impact,
        "confidence": input.confidence,
        "debt_cluster": {
            "id": input.cluster_id,
            "kind": input.kind,
            "message": input.message,
            "impact": input.impact,
            "confirm": input.confirm,
            "subject_ids": input.subject_ids,
        },
    });
    match input.subject_names.len() {
        0 => {}
        1 => {
            body["file"] = serde_json::Value::String(input.subject_names[0].clone());
        }
        _ => {
            body["files"] = serde_json::Value::Array(
                input
                    .subject_names
                    .iter()
                    .map(|n| serde_json::Value::String(n.clone()))
                    .collect(),
            );
        }
    }
    body
}

/// True when `existing` is an asserted Finding from this promotion path for `cluster_id`.
fn is_matching_debt_promotion(existing: &Node, cluster_id: &str) -> bool {
    existing.node_type == NodeType::Finding
        && existing.truth_class == TruthClass::Asserted
        && existing.body.get("source").and_then(|v| v.as_str()) == Some("debt_promotion")
        && existing
            .body
            .get("debt_cluster")
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str())
            == Some(cluster_id)
}

impl Store {
    // ---- ring 2: structural plane (sync + derived data) ------------------

    /// All CodeFile nodes.
    pub fn codefiles(&self) -> Result<Vec<Node>> {
        self.list_nodes(Some(NodeType::CodeFile), usize::MAX)
    }

    /// Edges filtered by any combination of kind / from / to. Used by the
    /// sync ripple to find what a changed file or intent invalidates.
    pub fn edges_with(
        &self,
        kind: Option<EdgeKind>,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Result<Vec<Edge>> {
        let mut sql = format!("SELECT {EDGE_COLS} FROM edge_view WHERE 1=1");
        let mut args: Vec<String> = Vec::new();
        if let Some(k) = kind {
            sql.push_str(&format!(" AND kind=?{}", args.len() + 1));
            args.push(k.as_str().to_string());
        }
        if let Some(f) = from {
            sql.push_str(&format!(" AND from_id=?{}", args.len() + 1));
            args.push(f.to_string());
        }
        if let Some(t) = to {
            sql.push_str(&format!(" AND to_id=?{}", args.len() + 1));
            args.push(t.to_string());
        }
        sql.push_str(" ORDER BY id");
        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> =
            args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(refs.as_slice(), row_to_edge)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Re-open an asserted edge whose dependency changed (sync ripple). Moves a
    /// settled verdict to `needs_reverification` and records the concrete cause
    /// on the edge as the `stale_cause` facet. Distinct from `record_verdict`
    /// (it writes no fresh verdict) and from `set_derived_status` (derived only).
    /// Returns true if the edge was re-opened.
    pub fn stale_edge(&self, edge_id: &str, cause: &str) -> Result<bool> {
        let cause = cause.trim();
        if cause.is_empty() {
            bail!("stale_edge requires a cause");
        }
        let edge = self
            .get_edge(edge_id)?
            .ok_or_else(|| anyhow!("no edge '{edge_id}'"))?;
        if edge.truth_class != TruthClass::Asserted {
            return Ok(false);
        }
        match edge.status {
            InspectionStatus::Passing
            | InspectionStatus::Failing
            | InspectionStatus::Independent => {
                self.write_edge_status(edge_id, InspectionStatus::NeedsReverification.as_str())?;
                self.set_facet(
                    edge_id,
                    TargetKind::Edge,
                    "stale_cause",
                    cause,
                    TruthClass::Derived,
                )?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Re-open only a previously passing asserted edge. Journey boundary failures
    /// use this narrower transition for never-reached steps: stale green proofs
    /// must be rechecked, but uninspected or already-failing edges carry useful
    /// state and are left untouched.
    pub fn stale_passing_edge(&self, edge_id: &str) -> Result<bool> {
        let edge = self
            .get_edge(edge_id)?
            .ok_or_else(|| anyhow!("no edge '{edge_id}'"))?;
        if edge.truth_class != TruthClass::Asserted || edge.status != InspectionStatus::Passing {
            return Ok(false);
        }
        self.write_edge_status(edge_id, InspectionStatus::NeedsReverification.as_str())?;
        Ok(true)
    }

    /// Reset a Validation to `not_run` as a deterministic sync consequence.
    /// Sync-derived invalidation is not an authored state transition, so it uses
    /// the derived timestamp sentinel to preserve INV-2 byte-identical exports
    /// across repeated recomputes. User-visible status changes must keep using
    /// [`set_node_status`].
    pub fn reset_validation_status_for_sync(&self, id: &str) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE node SET status='not_run',updated_at=?2 WHERE id=?1 AND node_type=?3",
            params![id, DERIVED_TS, NodeType::Validation.as_str()],
        )?;
        if n == 0 {
            bail!("no validation node '{id}'");
        }
        Ok(())
    }

    /// Set a node's status directly for asserted/user-visible transitions.
    /// A repeated status is a no-op so rerunning an unchanged proof does not
    /// create meaningless timestamp and export churn.
    pub fn set_node_status(&self, id: &str, status: &str) -> Result<()> {
        let node = self
            .get_node(id)?
            .ok_or_else(|| anyhow!("no node '{id}'"))?;
        if node.status == status {
            return Ok(());
        }
        let now = now(&self.conn)?;
        self.conn.execute(
            "UPDATE node SET status=?2,updated_at=?3 WHERE id=?1",
            params![id, status, now],
        )?;
        Ok(())
    }

    /// Add (or refresh) a derived node with a deterministic, content-addressed
    /// id and a fixed sentinel timestamp, so wipe+rebuild is byte-identical
    /// (INV-2). Sync-owned: derived truth class, never an asserted verdict.
    pub fn add_derived_node(
        &self,
        node_type: NodeType,
        det_key: &str,
        name: &str,
        description: &str,
        status: &str,
        body: serde_json::Value,
    ) -> Result<Node> {
        if !registry::node_allows_truth_class(node_type, TruthClass::Derived) {
            bail!(
                "'{node_type}' does not allow derived nodes — use add_node, not add_derived_node"
            );
        }
        let tc = TruthClass::Derived;
        let id = derived_id(&[node_type.as_str(), det_key]);
        self.conn.execute(
            "INSERT INTO node(id,node_type,name,description,status,truth_class,body,created_at,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)
             ON CONFLICT(id) DO UPDATE SET name=?3,description=?4,status=?5,body=?7",
            params![id, node_type.as_str(), name, description, status, tc.as_str(), body.to_string(), DERIVED_TS],
        )?;
        self.get_node(&id)?
            .ok_or_else(|| anyhow!("derived node vanished"))
    }

    /// Deterministic id a derived node of this type/key would get. Lets
    /// read-side views join durable adjudications against live-computed
    /// signals before (or after) sync materializes the node.
    pub fn derived_node_id(node_type: NodeType, det_key: &str) -> String {
        derived_id(&[node_type.as_str(), det_key])
    }

    /// Validate an edge against the registry: truth-class allowed for the kind,
    /// both endpoints exist, and their node types match the kind's spec. Shared
    /// by `add_edge` (asserted) and `add_derived_edge` (derived) so deterministic
    /// ids never weaken edge-kind integrity.
    pub(super) fn validate_edge_endpoints(
        &self,
        kind: EdgeKind,
        from_id: &str,
        to_id: &str,
        truth_class: TruthClass,
    ) -> Result<()> {
        let spec = registry::spec(kind);
        if !spec.allows_truth_class(truth_class) {
            bail!("edge kind '{kind}' does not allow truth_class '{truth_class}'");
        }
        let from = self
            .get_node(from_id)?
            .ok_or_else(|| anyhow!("from node '{from_id}' does not exist"))?;
        let to = self
            .get_node(to_id)?
            .ok_or_else(|| anyhow!("to node '{to_id}' does not exist"))?;
        if from.node_type != spec.from {
            bail!(
                "edge '{kind}' requires from-node type '{}', got '{}'",
                spec.from,
                from.node_type
            );
        }
        if to.node_type != spec.to {
            bail!(
                "edge '{kind}' requires to-node type '{}', got '{}'",
                spec.to,
                to.node_type
            );
        }
        Ok(())
    }

    /// Add (or refresh) a derived edge with a deterministic id. Sync-owned.
    pub fn add_derived_edge(&self, kind: EdgeKind, from_id: &str, to_id: &str) -> Result<Edge> {
        self.validate_edge_endpoints(kind, from_id, to_id, TruthClass::Derived)?;
        let id = derived_id(&["edge", kind.as_str(), from_id, to_id]);
        self.conn.execute(
            "INSERT INTO edge(id,from_id,to_id,kind,truth_class,status,created_at,updated_at)
             VALUES (?1,?2,?3,?4,'derived','current',?5,?5)
             ON CONFLICT(id) DO NOTHING",
            params![id, from_id, to_id, kind.as_str(), DERIVED_TS],
        )?;
        self.get_edge(&id)?
            .ok_or_else(|| anyhow!("derived edge vanished"))
    }

    /// Upsert a built-in seed node (e.g. a structural CodeRule) with a stable,
    /// content-addressed id and sentinel timestamp, so built-ins are identical
    /// across machines. Asserted truth class (a norm, not a derived occurrence).
    pub fn upsert_builtin_node(
        &self,
        node_type: NodeType,
        det_key: &str,
        name: &str,
        description: &str,
        body: serde_json::Value,
    ) -> Result<Node> {
        let id = derived_id(&["builtin", node_type.as_str(), det_key]);
        self.conn.execute(
            "INSERT INTO node(id,node_type,name,description,status,truth_class,body,created_at,updated_at)
             VALUES (?1,?2,?3,?4,'',  'asserted', ?5,?6,?6)
             ON CONFLICT(id) DO UPDATE SET description=?4, body=?5",
            params![id, node_type.as_str(), name, description, body.to_string(), DERIVED_TS],
        )?;
        self.get_node(&id)?
            .ok_or_else(|| anyhow!("builtin node vanished"))
    }

    /// Delete derived nodes + derived edges (Findings, flags, assesses, derived
    /// exposes). Run every sync before re-deriving findings.
    pub fn wipe_derived_graph(&self) -> Result<()> {
        // Derived edges first (some hang off asserted nodes); derived nodes then
        // cascade their remaining edges via FK.
        self.conn
            .execute("DELETE FROM edge WHERE truth_class='derived'", [])?;
        self.conn
            .execute("DELETE FROM node WHERE truth_class='derived'", [])?;
        Ok(())
    }

    /// Delete only the STRUCTURAL derived findings (and their flags/assesses
    /// edges), leaving external scan diagnostics (`status = external_diagnostic`)
    /// intact. `sync` rebuilds structural findings every run, but scan
    /// diagnostics are a SEPARATE derived plane owned by `loom scan run` — a
    /// routine sync must not destroy them (H-6). Asserted adjudication facets on
    /// deterministic finding ids survive and re-attach on rebuild.
    pub fn wipe_structural_findings(&self) -> Result<()> {
        self.conn.execute(
            "DELETE FROM edge WHERE truth_class='derived' AND from_id IN
                (SELECT id FROM node WHERE truth_class='derived' AND node_type='finding'
                    AND status != 'external_diagnostic')",
            [],
        )?;
        self.conn.execute(
            "DELETE FROM node WHERE truth_class='derived' AND node_type='finding'
                AND status != 'external_diagnostic'",
            [],
        )?;
        Ok(())
    }

    /// Delete specific derived Finding nodes and their incident derived edges.
    ///
    /// This is the scan adapter convergence primitive: callers validate adapter
    /// scope, then ask the store to remove only disappeared derived findings.
    /// The full id set is validated before any delete so a bad id cannot leave a
    /// partial cleanup.
    pub fn remove_derived_findings(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        let mut node_ids = std::collections::BTreeSet::new();
        let mut incident_edges = std::collections::BTreeSet::new();
        for id in ids {
            if !node_ids.insert(id.clone()) {
                continue;
            }
            let node = self
                .get_node(id)?
                .ok_or_else(|| anyhow!("no finding node '{id}'"))?;
            if node.node_type != NodeType::Finding || node.truth_class != TruthClass::Derived {
                bail!("'{id}' is not a derived finding");
            }
            for edge in self.edges_with(None, Some(id), None)? {
                if edge.truth_class != TruthClass::Derived {
                    bail!("'{id}' has non-derived incident edge '{}'", edge.id);
                }
                incident_edges.insert(edge.id);
            }
            for edge in self.edges_with(None, None, Some(id))? {
                if edge.truth_class != TruthClass::Derived {
                    bail!("'{id}' has non-derived incident edge '{}'", edge.id);
                }
                incident_edges.insert(edge.id);
            }
        }

        let tx = self.conn.unchecked_transaction()?;
        for edge_id in &incident_edges {
            tx.execute(
                "DELETE FROM facet WHERE target_id=?1 AND target_kind='edge'",
                params![edge_id],
            )?;
            tx.execute(
                "DELETE FROM tag WHERE target_id=?1 AND target_kind='edge'",
                params![edge_id],
            )?;
            tx.execute("DELETE FROM edge WHERE id=?1", params![edge_id])?;
        }
        for node_id in &node_ids {
            tx.execute(
                "DELETE FROM facet WHERE target_id=?1 AND target_kind='node'",
                params![node_id],
            )?;
            tx.execute(
                "DELETE FROM tag WHERE target_id=?1 AND target_kind='node'",
                params![node_id],
            )?;
            tx.execute("DELETE FROM node WHERE id=?1", params![node_id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Delete all derived facets (language, loc, content_hash, …).
    pub fn wipe_derived_facets(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM facet WHERE truth_class='derived'", [])?;
        Ok(())
    }

    /// Delete the ENTIRE derived plane — nodes, edges, and facets. The INV-2
    /// operation: after this, a `sync` rebuilds a byte-identical derived plane
    /// (and, because no prior content_hash remains, ripples nothing).
    pub fn wipe_derived(&self) -> Result<()> {
        self.wipe_derived_graph()?;
        self.wipe_derived_facets()?;
        Ok(())
    }
    /// Persist a durable adjudication verdict on a derived finding.
    ///
    /// Findings are rebuilt on every sync, but their ids are deterministic. Store
    /// the operator's judgment as an asserted facet on that stable id so it
    /// survives derived graph wipes, while stamping the current codefile hash so
    /// a future file edit can falsify the judgment.
    pub fn record_finding_verdict(
        &self,
        finding_id: &str,
        verdict: &str,
        reason: &str,
        evidence: &str,
    ) -> Result<()> {
        // The judgment itself is a FACT: it goes through the write boundary,
        // where it picks up its anchors and its floor. The reason and the
        // anchor are separate on purpose: a judge who writes a good sentence
        // has still not said where they looked, and only the second one is
        // re-checkable. An empty `evidence` falls back to the reason so a
        // citation written into the sentence still counts.
        let anchor = if evidence.is_empty() {
            reason
        } else {
            evidence
        };
        let cited = crate::evidence::cite(self.root(), anchor)?;
        self.assert_fact(
            crate::store::Assertion::new(
                crate::store::Subject::Node(finding_id.to_string()),
                crate::model::Claim::Adjudication,
                verdict,
                "llm",
            )
            .criterion(reason)
            .confidence(1.0)
            .cited(cited),
        )?;
        // The hash/metric stamp is DERIVED bookkeeping, not a claim: it records
        // what the world looked like when the judgment was made, so a resolving
        // verdict can be band-staled instead of reopening on every byte change.
        // Kept as a separate derived facet precisely so it cannot be mistaken
        // for — or used to forge — the judgment.
        let hash = self.finding_codefile_hash(finding_id)?.unwrap_or_default();
        let metric = self.finding_metric(finding_id)?;
        let mut stamp = serde_json::json!({ "hash": hash, "at": now(&self.conn)? });
        if let Some(m) = metric {
            stamp["metric"] = serde_json::json!(m);
        }
        self.set_facet(
            finding_id,
            TargetKind::Node,
            "adjudication_stamp",
            &stamp.to_string(),
            TruthClass::Derived,
        )
    }

    /// Observable metric the finding is about (file loc, symbol complexity, …),
    /// when the finding body or description carries one. Used to band-stale
    /// resolving adjudications instead of reopening on every content-hash bump.
    pub fn finding_metric(&self, finding_id: &str) -> Result<Option<u64>> {
        let Some(finding) = self.get_node(finding_id)? else {
            return Ok(None);
        };
        if let Some(m) = finding.body.get("metric").and_then(|v| v.as_u64()) {
            return Ok(Some(m));
        }
        // Parse a leading integer from the detail line ("1200 lines (> 600)").
        let mut num = String::new();
        for ch in finding.description.chars() {
            if ch.is_ascii_digit() {
                num.push(ch);
            } else if !num.is_empty() {
                break;
            }
        }
        if !num.is_empty() {
            return Ok(Some(num.parse::<u64>().with_context(|| {
                format!("invalid numeric metric '{num}' on finding '{finding_id}'")
            })?));
        }
        // oversized_file fallback: current codefile loc facet.
        if finding.body.get("kind").and_then(|v| v.as_str()) == Some("oversized_file") {
            if let Some(flags) = self
                .edges_with(Some(EdgeKind::Flags), Some(finding_id), None)?
                .into_iter()
                .next()
            {
                if let Some(loc) = self.get_facet(&flags.to_id, TargetKind::Node, "loc")? {
                    return Ok(Some(loc.parse::<u64>().with_context(|| {
                        format!("invalid loc facet '{loc}' on codefile '{}'", flags.to_id)
                    })?));
                }
            }
        }
        Ok(None)
    }

    /// Current content hash of the codefile flagged by a finding.
    pub fn finding_codefile_hash(&self, finding_id: &str) -> Result<Option<String>> {
        if let Some(flags) = self
            .edges_with(Some(EdgeKind::Flags), Some(finding_id), None)?
            .into_iter()
            .next()
        {
            return self.get_facet(&flags.to_id, TargetKind::Node, "content_hash");
        }
        let Some(finding) = self.get_node(finding_id)? else {
            return Ok(None);
        };
        if finding.node_type != NodeType::Finding || finding.truth_class != TruthClass::Asserted {
            return Ok(None);
        }
        let Some(file) = finding.body.get("file").and_then(|v| v.as_str()) else {
            return Ok(None);
        };
        let codefile = self.resolve_node(file, Some(NodeType::CodeFile))?;
        self.get_facet(&codefile.id, TargetKind::Node, "content_hash")
    }

    /// Intents that own (implement) the codefile a finding flags. Cohesion
    /// evidence for triage: one or two cohesive intents reads as justified
    /// length; many unrelated ones reads as a file that needs splitting.
    pub fn finding_owner_intents(&self, finding_id: &str) -> Result<Vec<Node>> {
        let codefile_id = if let Some(flags) = self
            .edges_with(Some(EdgeKind::Flags), Some(finding_id), None)?
            .into_iter()
            .next()
        {
            Some(flags.to_id)
        } else {
            let Some(finding) = self.get_node(finding_id)? else {
                return Ok(Vec::new());
            };
            if finding.node_type != NodeType::Finding || finding.truth_class != TruthClass::Asserted
            {
                None
            } else if let Some(file) = finding.body.get("file").and_then(|v| v.as_str()) {
                Some(self.resolve_node(file, Some(NodeType::CodeFile))?.id)
            } else {
                None
            }
        };
        let Some(codefile_id) = codefile_id else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for e in self.realizing_implementers(&codefile_id)? {
            if let Some(n) = self.get_node(&e.from_id)? {
                if n.node_type == NodeType::Intent {
                    out.push(n);
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out.dedup_by(|a, b| a.id == b.id);
        Ok(out)
    }

    /// Resolve a finding by exact id or unique id-prefix.
    ///
    /// Finding listings print short id prefixes; verdict writes must accept those
    /// without falling back to names or fragments.
    pub fn resolve_finding(&self, key: &str) -> Result<Node> {
        if let Some(n) = self.get_node(key)? {
            if n.node_type == NodeType::Finding {
                return Ok(n);
            }
        }
        let prefix = format!("{key}%");
        let matches = self.find_nodes_by(
            "id LIKE ?1",
            params![prefix],
            Some(NodeType::Finding.as_str()),
        )?;
        match matches.len() {
            0 => bail!("no finding matches '{key}'"),
            1 => Ok(matches.into_iter().next().expect("len == 1 by match arm")),
            n => bail!("ambiguous finding prefix '{key}': {n} match"),
        }
    }

    /// Read a derived facet value on a node (e.g. content_hash).
    pub fn get_facet(
        &self,
        target_id: &str,
        target_kind: TargetKind,
        key: &str,
    ) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM facet WHERE target_id=?1 AND target_kind=?2 AND key=?3",
                params![target_id, target_kind.as_str(), key],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(Into::into)
    }

    /// All facets on a target (node or edge), sorted by key. `edge show` reads
    /// this so corrections that live on facets (locator, role, stale_cause,
    /// superseded_by) are visible, not just the bare edge row (M-3).
    pub fn facets_of(
        &self,
        target_id: &str,
        target_kind: TargetKind,
    ) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT key,value FROM facet WHERE target_id=?1 AND target_kind=?2 ORDER BY key",
        )?;
        let rows = stmt.query_map(params![target_id, target_kind.as_str()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---- ring 5: vocab + layer order ----------------------------------------
    // ---- ring 5: vocab + layer order -------------------------------------

    /// Register a vocabulary term (idempotent).
    pub fn add_vocab_term(&self, term: &str, description: &str) -> Result<()> {
        let now = now(&self.conn)?;
        self.conn.execute(
            "INSERT INTO tag_vocabulary(term,description,created_at) VALUES (?1,?2,?3)
             ON CONFLICT(term) DO UPDATE SET description=?2",
            params![term, description, now],
        )?;
        Ok(())
    }

    /// All registered vocabulary terms.
    pub fn list_vocab(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT term,description FROM tag_vocabulary ORDER BY term")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Whether a term is registered.
    pub fn vocab_has(&self, term: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM tag_vocabulary WHERE term=?1",
                params![term],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }
    /// Count stored facts by truth class across the node, edge, and facet
    /// tables — the raw inputs to the derived-floor balance the maturity ladder
    /// reports. Returns `(derived, asserted)`.
    pub fn truth_class_census(&self) -> Result<(usize, usize)> {
        let (mut derived, mut asserted) = (0usize, 0usize);
        for table in ["node", "edge", "facet"] {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT truth_class, COUNT(*) FROM {table} GROUP BY truth_class"
            ))?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            for row in rows {
                let (tc, n) = row?;
                match tc.as_str() {
                    "derived" => derived += n as usize,
                    "asserted" => asserted += n as usize,
                    _ => {}
                }
            }
        }
        Ok((derived, asserted))
    }

    /// Set a meta key (e.g. the layer order JSON).
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=?2",
            params![key, value],
        )?;
        Ok(())
    }

    /// Remove a meta key if present. Used by config resets so the key reverts to
    /// its "absent = shipped default" fallback rather than a pinned value.
    pub fn remove_meta(&self, key: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM meta WHERE key=?1", params![key])?;
        Ok(())
    }

    /// Read a meta key.
    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key=?1", params![key], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .map_err(Into::into)
    }

    /// Coverage-exclusion globs recorded via `loom ignore add`. These files are
    /// deliberately outside the tracked surface: an unowned file matching one of
    /// them is not a coverage gap. Malformed entries are skipped rather than
    /// failing the read.
    pub fn ignore_globs(&self) -> Result<Vec<String>> {
        let raw = match self.get_meta("ignores")? {
            Some(v) => v,
            None => return Ok(Vec::new()),
        };
        let list: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(list
            .into_iter()
            .filter_map(|r| {
                r.get("glob")
                    .and_then(|g| g.as_str())
                    .map(|s| s.to_string())
            })
            .collect())
    }

    /// Promote a live debt cluster into one asserted Finding with deterministic
    /// id `p` + cluster digest. Idempotent when evidence/confidence match;
    /// fails closed on id collision or conflicting replay. Never writes edges,
    /// facets, or statistical truth (INV-3).
    pub(crate) fn add_promoted_debt_finding(
        &self,
        input: DebtPromotionInput<'_>,
    ) -> Result<DebtPromotionResult> {
        let normalized = normalize_debt_promotion(&input)?;
        let body = promoted_debt_finding_body(&input, normalized.evidence);
        let inserted = self.insert_promoted_debt_finding_row(
            &normalized.finding_id,
            &input,
            normalized.evidence,
            &body,
        )?;

        if inserted == 1 {
            let finding = self.get_node(&normalized.finding_id)?.ok_or_else(|| {
                anyhow!(
                    "promoted finding '{}' vanished after insert",
                    normalized.finding_id
                )
            })?;
            return Ok(DebtPromotionResult {
                finding,
                created: true,
            });
        }

        self.existing_promoted_debt_result(&normalized.finding_id, &input, normalized.evidence)
    }

    /// INSERT ON CONFLICT DO NOTHING for a promoted debt Finding.
    fn insert_promoted_debt_finding_row(
        &self,
        finding_id: &str,
        input: &DebtPromotionInput<'_>,
        evidence: &str,
        body: &serde_json::Value,
    ) -> Result<usize> {
        let now = now(&self.conn)?;
        let inserted = self.conn.execute(
            "INSERT INTO node(id,node_type,name,description,status,truth_class,body,created_at,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)
             ON CONFLICT(id) DO NOTHING",
            params![
                finding_id,
                NodeType::Finding.as_str(),
                input.message,
                evidence,
                input.kind,
                TruthClass::Asserted.as_str(),
                body.to_string(),
                now
            ],
        )?;
        Ok(inserted)
    }

    /// Collision / idempotency / conflicting-payload checks after ON CONFLICT.
    fn existing_promoted_debt_result(
        &self,
        finding_id: &str,
        input: &DebtPromotionInput<'_>,
        evidence: &str,
    ) -> Result<DebtPromotionResult> {
        let existing = self
            .get_node(finding_id)?
            .ok_or_else(|| anyhow!("promoted finding '{finding_id}' missing after conflict"))?;

        if !is_matching_debt_promotion(&existing, input.cluster_id) {
            bail!("debt promotion id '{finding_id}' collides with an unrelated node");
        }

        let stored_evidence = existing
            .body
            .get("evidence")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let stored_confidence = existing.body.get("confidence").and_then(|v| v.as_f64());
        if stored_evidence != evidence || stored_confidence != Some(input.confidence) {
            bail!(
                "debt cluster '{}' is already promoted as finding '{}' with different evidence or confidence",
                input.cluster_id,
                finding_id
            );
        }

        Ok(DebtPromotionResult {
            finding: existing,
            created: false,
        })
    }
}
