//! `assert_fact` — the ONE path by which an asserted fact enters the graph.
//!
//! Plane: the write boundary. Sole writer of the `fact` and `evidence` tables
//! and of the edge-status projection they drive.
//!
//! Contract: before this module there was one gated path (`record_verdict`) and
//! a ring of ungated primitives around it — `set_node_status`, `set_facet`,
//! `record_finding_verdict`, `ratify_intent_as`, and an import path that wrote
//! raw SQL past all of them. Anything that wanted to skip the gate simply used a
//! different door. Now every asserted claim funnels through [`Store::assert_fact`],
//! which:
//!
//! 1. resolves the subject and checks the claim fits it,
//! 2. checks the state vocabulary,
//! 3. enforces authority (INV-8 for ratification, INV-7 lane gate otherwise),
//! 4. materializes and re-checks every anchor the caller cited,
//! 5. refuses the write if the resulting strength is below the floor for this
//!    claim — naming the command that would produce the missing anchor,
//! 6. writes fact + evidence + projection in one transaction.
//!
//! A caller supplies [`CitedEvidence`], which has no `Run` variant. The only
//! route to a `verified` fact is to let loom run something.

use super::{now, Store};
use crate::anchor;
use crate::evidence::{level, CitedEvidence, Evidence, EvidenceRow, Fact, RunRecord, StaleReason};
use crate::model::{Claim, InspectionStatus, NodeType, StaleCause, TargetKind, Verification};
use crate::registry;
use crate::Result;
use anyhow::{anyhow, bail};
use rusqlite::OptionalExtension;
use std::collections::BTreeSet;
use std::str::FromStr;

/// What one full re-verification pass found.
#[derive(Debug, Default, Clone, Copy)]
pub struct Reverified {
    /// Facts whose anchors no longer hold at the strength they claimed.
    pub demoted: usize,
    /// Facts whose file changed under them and which still stand — the payoff
    /// of anchoring to a symbol or a span rather than to a whole file.
    pub spared: usize,
    /// Validation nodes reset to `not_run` because their proof's anchor broke.
    pub validations_reset: usize,
    /// Evidence spans whose stored coordinates moved while their content
    /// stood — re-anchored in place and journaled, no re-verdict demanded.
    pub reanchored: usize,
}

/// How one anchor fared against the working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AnchorFate {
    /// Still says what it said, where it said it.
    Holds,
    /// Same content, new coordinates: the span moved inside its file, or its
    /// file was deleted and exactly one registered successor holds the body.
    /// The verdict stands; the stored stamp is re-anchored and the move
    /// journaled. Line numbers are display metadata, not identity.
    Moved {
        file: String,
        start: usize,
        end: usize,
    },
    Broken(StaleCause),
}

/// One recorded move: where a span was cited, where its content now lives.
/// Journaled as `evidence_reanchor` so the graph's history shows the move
/// without demanding a fresh verdict for it.
#[derive(Debug, Clone)]
struct Reanchor {
    from_file: String,
    from_start: usize,
    from_end: usize,
    symbol: String,
    to_file: String,
    to_start: usize,
    to_end: usize,
}

/// Lazily-loaded `(path, content)` pairs of every registered codefile still
/// on disk — the declared successors a deleted citation may re-anchor into.
/// Loaded at most once per pass, and only when a cited file has disappeared,
/// so a routine sync never pays for it.
#[derive(Default)]
struct SuccessorCache(Option<Vec<(String, String)>>);

impl SuccessorCache {
    fn get(&mut self, store: &Store) -> &[(String, String)] {
        if self.0.is_none() {
            let mut files = Vec::new();
            if let Ok(codefiles) = store.codefiles() {
                for node in codefiles {
                    if let Ok(content) = std::fs::read_to_string(store.root().join(&node.name)) {
                        files.push((node.name, content));
                    }
                }
            }
            self.0 = Some(files);
        }
        self.0.as_deref().unwrap_or(&[])
    }
}

/// Widen a grounding's citations to the whole file.
///
/// A realizing grounding that names no symbol claims the FILE, so its spans
/// carry the file's hash: a citation surviving verbatim inside a rewritten file
/// must stop holding the claim up. Ids are content-addressed, so widening the
/// scope changes identity — they are recomputed here.
fn widen_to_file_scope(
    root: &std::path::Path,
    file: &str,
    fact_id: &str,
    rows: &mut [EvidenceRow],
) {
    let Ok(content) = std::fs::read_to_string(root.join(file)) else {
        return;
    };
    // Hashed over the joined LINES, exactly as `span_intact` recomputes it —
    // hashing the raw bytes would differ by the trailing newline and expire
    // every file-scoped grounding on the very next sync.
    let whole = crate::artifact::fingerprint(&content.lines().collect::<Vec<_>>().join("\n"));
    for row in rows.iter_mut() {
        if let Evidence::Span(span) = &mut row.payload {
            if span.file == file {
                span.file_hash = whole.clone();
            }
        }
    }
    for row in rows.iter_mut() {
        row.id = EvidenceRow::id_for(fact_id, &row.payload);
    }
}

/// Does this anchor point into any of `changed`?
fn touches(payload: &Evidence, changed: &BTreeSet<String>) -> bool {
    match payload {
        Evidence::Run(run) => run.covered.keys().any(|f| changed.contains(f)),
        Evidence::Span(span) => changed.contains(&span.file),
        Evidence::Journal { .. } | Evidence::Claim { .. } => false,
    }
}

/// What a fact is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    Edge(String),
    Node(String),
}

impl Subject {
    fn kind(&self) -> TargetKind {
        match self {
            Subject::Edge(_) => TargetKind::Edge,
            Subject::Node(_) => TargetKind::Node,
        }
    }

    fn id(&self) -> &str {
        match self {
            Subject::Edge(id) | Subject::Node(id) => id,
        }
    }
}

/// One assertion, presented at the boundary.
pub struct Assertion<'a> {
    pub subject: Subject,
    pub claim: Claim,
    pub state: &'a str,
    pub criterion: &'a str,
    pub confidence: f64,
    pub asserted_by: &'a str,
    /// Caller-supplied anchors. No `Run` variant exists on this type.
    pub cited: Vec<CitedEvidence>,
    /// Machine-observed evidence. `pub(crate)` so only in-process runner call
    /// sites can attach one — a caller cannot express this field at all.
    pub(crate) run: Option<Box<RunRecord>>,
    /// A host conversation carried an explicit human answer and the current
    /// agent is only recording it. Crate-private so generic callers cannot
    /// turn an ordinary LLM assertion into human authority.
    pub(crate) mediated_human_decision: bool,
    /// One-shot escape used ONLY by `loom carry-forward`, which imports a legacy
    /// graph whose facts were never anchored. Records them honestly below the
    /// floor rather than pretending they meet it.
    pub(crate) below_floor: Option<StaleCause>,
    /// Batch provenance. Empty `batch_id` means an individual judgment.
    pub(crate) decision_mode: crate::model::DecisionMode,
    pub(crate) batch_id: &'a str,
}

impl<'a> Assertion<'a> {
    /// An assertion carrying only what a caller may supply.
    pub fn new(subject: Subject, claim: Claim, state: &'a str, asserted_by: &'a str) -> Self {
        Assertion {
            subject,
            claim,
            state,
            criterion: "",
            confidence: 0.0,
            asserted_by,
            cited: Vec::new(),
            run: None,
            mediated_human_decision: false,
            below_floor: None,
            decision_mode: crate::model::DecisionMode::Individual,
            batch_id: "",
        }
    }

    pub fn criterion(mut self, criterion: &'a str) -> Self {
        self.criterion = criterion;
        self
    }

    pub fn confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence;
        self
    }

    pub fn cited(mut self, cited: Vec<CitedEvidence>) -> Self {
        self.cited = cited;
        self
    }

    /// Attach an observation loom made. Crate-internal by construction, so a
    /// caller cannot express this field at all.
    // Wired up as each anchoring floor is raised to `verified` — the proof lane
    // first, then the locator and pre-screen probes.
    #[allow(dead_code)]
    pub fn observed(mut self, run: RunRecord) -> Self {
        self.run = Some(Box::new(run));
        self
    }

    pub(crate) fn mediated_human_decision(mut self) -> Self {
        self.mediated_human_decision = true;
        self
    }

    pub(crate) fn batch(mut self, batch_id: &'a str) -> Self {
        self.decision_mode = crate::model::DecisionMode::Batch;
        self.batch_id = batch_id;
        self
    }
}

/// A fact plus the evidence behind it — what callers need to judge strength.
#[derive(Debug, Clone)]
pub struct FactView {
    pub fact: Fact,
    pub evidence: Vec<EvidenceRow>,
}

impl FactView {
    pub fn verification(&self) -> Verification {
        self.fact.verification
    }

    /// Whether this fact may satisfy a maturity rung.
    pub fn counts(&self) -> bool {
        self.fact.verification.counts()
    }
}

impl Store {
    /// Record an asserted fact. See the module header for the full contract.
    pub fn assert_fact(&self, a: Assertion<'_>) -> Result<FactView> {
        // ---- 1. subject resolves, and the claim fits it ----------------------
        let (edge_kind, role, mut shape) = self.resolve_subject(&a)?;

        // ---- 2. state vocabulary --------------------------------------------
        check_state(a.claim, a.state)?;

        // ---- 3. authority ----------------------------------------------------
        match a.claim {
            // INV-8: ratification authority is human-only, and it is SYMMETRIC.
            // Saying a behavior is not wanted is the same kind of act as saying
            // it is — an agent that could reject could delete the product by
            // rejecting everything, and the rejection is absolute afterwards.
            //
            // Demoting to `needs_reconfirmation` or `unratified` is not an act
            // of authority but a loss of standing, which sync performs whenever
            // meaning drifts; requiring a human there would mean stale
            // wantedness could only be noticed by the person it was hidden from.
            // A mediated_human_decision carries the person's explicit answer;
            // the current lane is the recorder, not the authority.
            Claim::Ratification
                if matches!(a.state, "ratified" | "rejected") && !a.mediated_human_decision =>
            {
                self.require_human_authority()?
            }
            Claim::Ratification => {}
            // INV-7: the lane that owns this edge kind owns its verdict.
            Claim::Verdict => {
                if let Some(kind) = edge_kind {
                    self.check_lane(registry::spec(kind).owner)?;
                }
            }
            Claim::Adjudication | Claim::Observation => {}
        }

        // ---- 4. hygiene ------------------------------------------------------
        // `is_placeholder` is retained as a LINT, not the gate. It rejects "TBD"
        // and accepts any sentence; the floor below is what actually decides.
        if anchor::is_settling(a.state)
            && crate::model::is_placeholder(a.criterion)
            && matches!(a.claim, Claim::Verdict)
        {
            bail!(
                "a settled verdict needs a criterion stating what would falsify it \
                 (got {:?})",
                a.criterion
            );
        }
        if !(0.0..=1.0).contains(&a.confidence) || !a.confidence.is_finite() {
            bail!("confidence must be within 0.0..=1.0 (got {})", a.confidence);
        }

        // ---- 5. materialize anchors -----------------------------------------
        let (fact_id, rows, recorded_at) =
            self.materialize_anchors(&a, edge_kind, role, &mut shape)?;
        // ---- 6. the floor ----------------------------------------------------
        let strength = level(&rows);
        let floor = anchor::required_for(a.claim, edge_kind, role, a.state, shape);
        if a.below_floor.is_none() && strength.rank() < floor.required.rank() {
            bail!(
                "'{}' needs {} evidence but this is only {} — {}",
                a.state,
                floor.required.as_str(),
                strength.as_str(),
                floor.remedy
            );
        }
        let stale = a
            .below_floor
            .map(|cause| StaleReason::new(cause, Vec::new(), recorded_at.clone()));

        // A ratification DEMOTION changes standing, not authorship: the human
        // who said yes still said yes, and overwriting them with "sync" would
        // erase the provenance the demotion exists to protect.
        let (asserted_by, asserted_at) = if a.claim == Claim::Ratification && a.state != "ratified"
        {
            match self.fact_by_id(&fact_id)? {
                Some(prior) => (prior.fact.asserted_by, prior.fact.asserted_at),
                None => (a.asserted_by.to_string(), recorded_at.clone()),
            }
        } else {
            (a.asserted_by.to_string(), recorded_at.clone())
        };

        let fact = Fact {
            id: fact_id.clone(),
            subject_kind: a.subject.kind(),
            subject_id: a.subject.id().to_string(),
            claim: a.claim,
            state: a.state.to_string(),
            criterion: a.criterion.to_string(),
            verification: strength,
            confidence: a.confidence,
            asserted_by,
            asserted_at,
            decision_mode: a.decision_mode,
            batch_id: a.batch_id.to_string(),
            stale,
        };

        // ---- 7. idempotence --------------------------------------------------
        if let Some(view) = self.unchanged(&fact, &rows)? {
            return Ok(view);
        }

        // ---- 8. write, one transaction --------------------------------------
        self.write_fact(&fact, &rows)?;
        Ok(FactView {
            fact,
            evidence: rows,
        })
    }

    /// Persist a fact, replace its evidence set, and project its state.
    fn write_fact(&self, fact: &Fact, rows: &[EvidenceRow]) -> Result<()> {
        let stale = fact
            .stale
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?
            .unwrap_or_default();
        // One transaction: the fact row, its wholesale-replaced evidence set, and
        // the projection either all land or none do. A failure between the
        // evidence DELETE and the re-INSERT would otherwise leave a fact with no
        // anchors — silently unverified — until the next write. `maybe_tx` yields
        // `None` under an outer `begin()` batch, which then owns the atomicity.
        let tx = self.maybe_tx()?;
        self.conn.execute(
            "INSERT INTO fact (id,subject_kind,subject_id,claim,state,criterion,verification,\
                               confidence,asserted_by,asserted_at,stale,decision_mode,batch_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
             ON CONFLICT(id) DO UPDATE SET
                state=excluded.state, criterion=excluded.criterion,
                verification=excluded.verification, confidence=excluded.confidence,
                asserted_by=excluded.asserted_by, asserted_at=excluded.asserted_at,
                stale=excluded.stale, decision_mode=excluded.decision_mode,
                batch_id=excluded.batch_id",
            rusqlite::params![
                fact.id,
                fact.subject_kind.as_str(),
                fact.subject_id,
                fact.claim.as_str(),
                fact.state,
                fact.criterion,
                fact.verification.as_str(),
                fact.confidence,
                fact.asserted_by,
                fact.asserted_at,
                stale,
                fact.decision_mode.as_str(),
                fact.batch_id,
            ],
        )?;
        // The evidence set is replaced wholesale: an assertion's anchors are
        // exactly what its author cited this time, never an accumulation that
        // could keep a fact alive on a citation nobody stands behind.
        self.conn
            .execute("DELETE FROM evidence WHERE fact_id = ?1", [&fact.id])?;
        for row in rows {
            self.conn.execute(
                "INSERT INTO evidence (id,fact_id,payload,kind,recorded_at,holds,expiry_reason)
                 VALUES (?1,?2,?3,?4,?5,1,'')
                 ON CONFLICT(id) DO NOTHING",
                rusqlite::params![
                    row.id,
                    row.fact_id,
                    serde_json::to_string(&row.payload)?,
                    row.payload.kind().as_str(),
                    row.recorded_at,
                ],
            )?;
        }
        // `project` writes through `self.conn`, which is the same connection the
        // transaction is open on, so its edge-status update participates in this
        // atomic unit.
        self.project(fact)?;
        if let Some(tx) = tx {
            tx.commit()?;
        }
        Ok(())
    }

    /// Resolve the subject, check the claim fits it, and read the SHAPE the
    /// floor needs — whether a proof is runnable, whether a finding flags a
    /// file, whether a relationship has code on both ends.
    ///
    /// Step 1 of `assert_fact`, extracted. Every one of these is derived HERE,
    /// at the one place that can see the whole subject, rather than trusted
    /// from the caller: a caller that could declare its own claim unanchorable
    /// would have a way around the floor.
    fn resolve_subject(
        &self,
        a: &Assertion<'_>,
    ) -> Result<(
        Option<crate::model::EdgeKind>,
        Option<crate::model::GroundingRole>,
        anchor::Shape,
    )> {
        let mut shape = anchor::Shape::default();
        let (edge_kind, role) = match (&a.subject, a.claim) {
            (Subject::Edge(id), Claim::Verdict) => {
                let edge = self
                    .get_edge(id)?
                    .ok_or_else(|| anyhow!("no edge '{id}'"))?;
                if edge.truth_class != crate::model::TruthClass::Asserted {
                    bail!("edge '{id}' is derived — sync owns its status, not a verdict");
                }
                // A proof's floor depends on whether loom CAN run it.
                shape.runnable_proof = edge.kind == crate::model::EdgeKind::Validates
                    && self
                        .get_node(&edge.from_id)?
                        .map(|v| {
                            let command =
                                v.body.get("command").and_then(|c| c.as_str()).unwrap_or("");
                            let ty = v
                                .body
                                .get("type")
                                .and_then(|c| c.as_str())
                                .unwrap_or("test");
                            // A journey is proven by `loom journey run`, not by
                            // the validation runner — which reports `Manual`
                            // rather than pretending. Classing it runnable here
                            // would demand a Run nothing in this path produces.
                            !command.trim().is_empty() && !matches!(ty, "manual_check" | "journey")
                        })
                        .unwrap_or(false);
                // A relationship can only be anchored where both ends have
                // source to point at. Computed here, at the one place that can
                // see the whole subject, rather than trusted from the caller.
                if !matches!(
                    edge.kind,
                    crate::model::EdgeKind::Implements
                        | crate::model::EdgeKind::Validates
                        | crate::model::EdgeKind::Governs
                        | crate::model::EdgeKind::Hierarchy
                ) {
                    shape.endpoints_realized =
                        !self.realizing_groundings(&edge.from_id)?.is_empty()
                            && !self.realizing_groundings(&edge.to_id)?.is_empty();
                }
                (Some(edge.kind), Some(self.grounding_role(id)?))
            }
            (Subject::Edge(_), other) => {
                bail!("claim '{}' is about a node, not an edge", other.as_str())
            }
            (Subject::Node(id), claim) => {
                let node = self
                    .get_node(id)?
                    .ok_or_else(|| anyhow!("no node '{id}'"))?;
                match claim {
                    Claim::Ratification => {
                        if !matches!(node.node_type, NodeType::Intent | NodeType::Pattern) {
                            bail!(
                                "only an intent or pattern can be ratified; '{id}' is a {}",
                                node.node_type
                            );
                        }
                        if node.status == "deprecated" {
                            bail!("cannot ratify a deprecated node — reactivate it first");
                        }
                    }
                    Claim::Adjudication | Claim::Observation => {
                        if node.node_type != NodeType::Finding {
                            bail!(
                                "only a finding can be adjudicated; '{id}' is a {}",
                                node.node_type
                            );
                        }
                        // Whether the judge had anywhere to look. A finding that
                        // flags a live codefile can be judged from the code; a
                        // smell about an intent flags nothing on disk, so
                        // demanding a span would only produce invented ones.
                        shape.flagged_file = self.finding_codefile_hash(id)?.is_some();
                    }
                    Claim::Verdict => bail!("a verdict is about an edge, not a node"),
                }
                (None, None)
            }
        };
        Ok((edge_kind, role, shape))
    }

    /// The grounding this assertion is about: its edge and the file it names.
    ///
    /// `Some` only for a SETTLING verdict on an `implements` edge whose target
    /// file is still in the graph — the one case that mints a grounding probe.
    /// Flattens four levels of `if let` that together asked one question.
    fn grounding_subject(
        &self,
        a: &Assertion<'_>,
        edge_kind: Option<crate::model::EdgeKind>,
    ) -> Result<Option<(crate::model::Edge, crate::model::Node)>> {
        if a.run.is_some()
            || edge_kind != Some(crate::model::EdgeKind::Implements)
            || !anchor::is_settling(a.state)
        {
            return Ok(None);
        }
        let Subject::Edge(edge_id) = &a.subject else {
            return Ok(None);
        };
        let Some(edge) = self.get_edge(edge_id)? else {
            return Ok(None);
        };
        let Some(file) = self.get_node(&edge.to_id)? else {
            return Ok(None);
        };
        Ok(Some((edge, file)))
    }

    /// The quality rule this assertion measures, and the files it governs.
    ///
    /// `Some` only for a settling verdict on a `governs` edge — the one case
    /// that mints a pattern scan. Same flattening as `grounding_subject`.
    fn scannable_rule_subject(
        &self,
        a: &Assertion<'_>,
        edge_kind: Option<crate::model::EdgeKind>,
    ) -> Result<Option<(crate::model::Node, Vec<String>)>> {
        if a.run.is_some()
            || edge_kind != Some(crate::model::EdgeKind::Governs)
            || !anchor::is_settling(a.state)
        {
            return Ok(None);
        }
        let Subject::Edge(edge_id) = &a.subject else {
            return Ok(None);
        };
        let Some(edge) = self.get_edge(edge_id)? else {
            return Ok(None);
        };
        let Some(rule) = self.get_node(&edge.from_id)? else {
            return Ok(None);
        };
        // A rule is measured against the code the behavior LIVES in, not the
        // test that verifies it: `.unwrap()` in a test is idiomatic, so
        // scanning `verifies` groundings made a passing verdict unreachable for
        // any intent proved by a Rust test.
        let files = self.files_realizing(&edge.to_id)?;
        Ok(Some((rule, files)))
    }

    /// Pattern hits contradict a `passing` verdict — unless the author cited
    /// the hit and said why it is not what the rule means.
    ///
    /// The patterns are declared prompt hints that "do not replace adjudicated
    /// quality verdicts" (`packs.rs`), so they must not override adjudication
    /// outright. But an unexamined `passing` beside a hit is exactly the
    /// unearned green this gate exists to stop. Both hold if the exemption is
    /// per hit: cite the span, and the verdict stands for that one.
    ///
    /// This is a HIGHER bar than a blanket pass, not a lower one — every hit
    /// must be answered individually. And because a citation is stamped with a
    /// hash of the lines it names, the exemption expires the moment that code
    /// changes; you cannot cite once and coast.
    fn refuse_passing_over_hits(
        &self,
        a: &Assertion<'_>,
        rule_name: &str,
        probe: Option<&RunRecord>,
        hits: &[crate::prescan::PreScreenHit],
    ) -> Result<()> {
        let Some(run) = probe else { return Ok(()) };
        if run.exit_code == 0 || a.state != "passing" {
            return Ok(());
        }
        // Read the spans the caller already cited, not the raw prose: by this
        // point each is stamped with a hash of the lines it names, which is
        // what makes the exemption expire when that code changes.
        let cited: Vec<&crate::evidence::SpanStamp> = a
            .cited
            .iter()
            .filter_map(|c| match c {
                CitedEvidence::Span(s) => Some(s),
                _ => None,
            })
            .collect();
        // A hit is answered three ways: cited in this verdict's evidence,
        // or suppressed once as a hit-level adjudication — keyed by the
        // matched text's content hash, so the judgment follows the text
        // wherever it moves and expires when the text changes.
        let mut unanswered = Vec::new();
        let mut suppressed = 0usize;
        for h in hits {
            let cited_here = cited
                .iter()
                .any(|s| s.file == h.path && s.start <= h.line && h.line <= s.end);
            if cited_here {
                continue;
            }
            if self.is_hit_suppressed(rule_name, &h.excerpt)? {
                suppressed += 1;
                continue;
            }
            unanswered.push(h);
        }
        if unanswered.is_empty() {
            return Ok(());
        }
        let listed = unanswered
            .iter()
            .map(|h| format!("{}:{} {}\n    {}", h.path, h.line, h.pattern, h.excerpt))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "'{}' found {} hit(s) this verdict does not answer — a passing \
             verdict contradicts them:\n{}\n\nRecord `failing`, cite each span \
             above in --evidence and say why it is not what the rule means, or \
             suppress a false positive once and durably with `loom rule \
             suppress '{}' --excerpt '<matched text>' --reason '<why>'. \
             {} hit(s) already answered ({} suppressed).",
            rule_name,
            unanswered.len(),
            listed,
            rule_name,
            hits.len() - unanswered.len(),
            suppressed,
        )
    }

    /// Turn what the caller cited into evidence rows, and mint loom's own
    /// probes for the claims loom can check itself.
    ///
    /// Step 5 of `assert_fact`, extracted: it was 137 of that function's 338
    /// lines and held all of its deep nesting, which is why loom flagged the
    /// chokepoint as long, complex AND deeply nested at once. The SQL stays in
    /// this module, so the one-door invariant is untouched.
    ///
    /// Returns the fact id and the rows; `shape` is filled in as the probes run
    /// (a rule is only `scannable` if its scan actually ran).
    fn materialize_anchors(
        &self,
        a: &Assertion<'_>,
        edge_kind: Option<crate::model::EdgeKind>,
        role: Option<crate::model::GroundingRole>,
        shape: &mut anchor::Shape,
    ) -> Result<(String, Vec<EvidenceRow>, String)> {
        let fact_id = Fact::id_for(a.subject.kind(), a.subject.id(), a.claim);
        let mut rows: Vec<EvidenceRow> = Vec::new();
        let recorded_at = now(&self.conn)?;
        for cited in a.cited.clone() {
            let payload = cited.into_evidence();
            rows.push(EvidenceRow {
                id: EvidenceRow::id_for(&fact_id, &payload),
                fact_id: fact_id.clone(),
                payload,
                recorded_at: recorded_at.clone(),
                holds: true,
                expiry_reason: None,
            });
        }
        // loom checks the grounding claim itself: does the locator still name a
        // live symbol in that file? A worker asserting "the behavior lives here"
        // no longer has to be believed — and no longer has to be doubted either.
        let mut probe: Option<RunRecord> = None;
        if let Some((edge, file)) = self.grounding_subject(a, edge_kind)? {
            let locator = self.get_facet(&edge.id, TargetKind::Edge, "locator")?;
            let realizes = role == Some(crate::model::GroundingRole::Realizes);
            // A realizing grounding that names NO symbol claims the whole file.
            // Widen its citations to say so, so a span surviving verbatim inside
            // a rewritten file stops holding the claim up.
            if realizes && locator.as_deref().map(str::trim).unwrap_or("").is_empty() {
                widen_to_file_scope(&self.root, &file.name, &fact_id, &mut rows);
            }
            probe = if realizes {
                // A locator narrows the claim to one symbol and earns
                // symbol-scoped sparing; no locator leaves it file-wide, and the
                // anchor says so rather than quietly borrowing the narrower
                // scope from a span that happens to sit in the file.
                crate::runner::locator_probe(&self.root, &file.name, locator.as_deref())
                    .filter(|r| r.exit_code == 0)
            } else {
                // "This file USES the behavior." Only the seam leaving
                // falsifies it.
                crate::runner::seam_probe(&self.root, &file.name, locator.as_deref())
                    .filter(|r| r.exit_code == 0)
            };
        }
        if edge_kind == Some(crate::model::EdgeKind::Exemplar) {
            let edge = self
                .get_edge(a.subject.id())?
                .ok_or_else(|| anyhow!("no exemplar edge"))?;
            let file = self
                .get_node(&edge.to_id)?
                .ok_or_else(|| anyhow!("missing exemplar file"))?;
            let locator = self.get_facet(&edge.id, TargetKind::Edge, "locator")?;
            probe = locator.as_deref().and_then(|locator| {
                crate::runner::unique_locator_probe(&self.root, &file.name, locator)
            });
        }

        // A quality verdict on a rule that carries patterns is checkable the
        // same way: loom runs the scan itself. This is what lets an ABSENCE
        // count as evidence — nothing to cite, but something to re-run.
        if probe.is_none() {
            if let Some((rule, files)) = self.scannable_rule_subject(a, edge_kind)? {
                let patterns: Vec<String> = rule
                    .body
                    .get("patterns")
                    .and_then(|v| v.as_array())
                    .map(|list| {
                        list.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let scanned =
                    crate::runner::prescreen_probe(&self.root, &rule.name, &patterns, &files);
                let hits = scanned
                    .as_ref()
                    .map(|(_, hits)| hits.clone())
                    .unwrap_or_default();
                probe = scanned.map(|(run, _)| run);
                // The floor follows what loom actually DID, not what it could
                // have tried. The scan fails closed on a file it cannot read,
                // and demanding `verified` from a scan that never happened
                // would refuse an honest verdict for loom's own inability to
                // look.
                shape.scannable_rule = probe.is_some();
                self.refuse_passing_over_hits(a, &rule.name, probe.as_ref(), &hits)?;
            }
        }

        // The run — whether the caller's or loom's own probe — becomes the row
        // that makes this fact `verified`. Only `crate::runner` mints one, so
        // there is no path from caller input to this branch.
        if let Some(run) = a.run.clone().map(|b| *b).or(probe) {
            let payload = Evidence::Run(run);
            rows.push(EvidenceRow {
                id: EvidenceRow::id_for(&fact_id, &payload),
                fact_id: fact_id.clone(),
                payload,
                recorded_at: recorded_at.clone(),
                holds: true,
                expiry_reason: None,
            });
        }

        Ok((fact_id, rows, recorded_at))
    }

    /// The already-recorded answer, when this assertion changes nothing.
    ///
    /// Step 7 of `assert_fact`, extracted. Re-asserting an identical fact must
    /// be a byte-identical no-op, or `loom export --check` drifts on every sync
    /// and the committed graph stops being a stable artifact.
    fn unchanged(&self, fact: &Fact, rows: &[EvidenceRow]) -> Result<Option<FactView>> {
        let fact_id = fact.id.as_str();
        // A byte-identical re-assertion is a no-op: it must not bump the edge's
        // `updated_at` (which is exported, so a repeat would dirty
        // `loom.graph.json` and make `export --check` useless in CI). Evidence
        // ids are content-addressed, so "the same anchors" is an id-set
        // comparison rather than a deep one.
        if let Some(existing) = self.fact_by_id(fact_id)? {
            let same_claim = existing.fact.state == fact.state
                && existing.fact.criterion == fact.criterion
                && existing.fact.confidence.to_bits() == fact.confidence.to_bits()
                && existing.fact.asserted_by == fact.asserted_by
                && existing.fact.verification == fact.verification
                && existing.fact.decision_mode == fact.decision_mode
                && existing.fact.batch_id == fact.batch_id
                && existing.fact.stale.as_ref().map(|s| s.cause)
                    == fact.stale.as_ref().map(|s| s.cause);
            let before: std::collections::BTreeSet<&str> =
                existing.evidence.iter().map(|r| r.id.as_str()).collect();
            let after: std::collections::BTreeSet<&str> =
                rows.iter().map(|r| r.id.as_str()).collect();
            if same_claim && before == after {
                return Ok(Some(existing));
            }
        }
        Ok(None)
    }

    /// Push a fact's state onto the row that indexes it. `edge.status` stays a
    /// real column because the queues page on it; it is written ONLY here.
    fn project(&self, fact: &Fact) -> Result<()> {
        match fact.claim {
            Claim::Verdict => {
                let status = InspectionStatus::from_str(&fact.state)?;
                self.write_edge_status(&fact.subject_id, status.as_str())?;
            }
            // Node-level projections (validation status, finding adjudication,
            // ratification) land as the caller's own node write for now; the
            // status narrowing that makes them projections-only comes with the
            // side-door closure.
            Claim::Adjudication | Claim::Observation | Claim::Ratification => {}
        }
        Ok(())
    }

    /// Write an edge's status column. **The only place that SQL exists.**
    ///
    /// `edge.status` stays a real column because the queues page on it, which
    /// makes it the one piece of asserted truth living outside the `fact` table.
    /// Funnelling every writer through here is what lets a test assert, by
    /// scanning the source, that no other module can move it — the check that
    /// would have caught `restore_inner` writing raw SQL past every gate.
    ///
    /// Callers: `assert_fact` (asserted verdicts), `expire_fact` (anchors
    /// broke), `set_derived_status` (INV-5, sync-owned), and sync's staling.
    pub(crate) fn write_edge_status(&self, edge_id: &str, status: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE edge SET status = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![status, now(&self.conn)?, edge_id],
        )?;
        Ok(())
    }

    /// The fact for one subject and claim, with its evidence.
    pub fn fact(&self, subject: &Subject, claim: Claim) -> Result<Option<FactView>> {
        let id = Fact::id_for(subject.kind(), subject.id(), claim);
        self.fact_by_id(&id)
    }

    pub fn fact_by_id(&self, id: &str) -> Result<Option<FactView>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,subject_kind,subject_id,claim,state,criterion,verification,\
                    confidence,asserted_by,asserted_at,stale,decision_mode,batch_id \
             FROM fact WHERE id = ?1",
        )?;
        let fact = stmt
            .query_row([id], |r| Ok(row_to_fact(r)))
            .optional_row()?;
        let Some(fact) = fact else { return Ok(None) };
        let fact = fact?;
        let evidence = self.evidence_for(&fact.id)?;
        Ok(Some(FactView { fact, evidence }))
    }

    /// Every evidence row behind one fact.
    pub fn evidence_for(&self, fact_id: &str) -> Result<Vec<EvidenceRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,fact_id,payload,recorded_at,holds,expiry_reason FROM evidence \
             WHERE fact_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([fact_id], |r| {
            let payload: String = r.get("payload")?;
            let holds: i64 = r.get("holds")?;
            let reason: String = r.get("expiry_reason")?;
            Ok(EvidenceRow {
                id: r.get("id")?,
                fact_id: r.get("fact_id")?,
                payload: serde_json::from_str(&payload).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
                recorded_at: r.get("recorded_at")?,
                holds: holds != 0,
                expiry_reason: StaleCause::from_str(&reason).ok(),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Every stored fact, for audit and export.
    pub fn all_facts(&self) -> Result<Vec<Fact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,subject_kind,subject_id,claim,state,criterion,verification,\
                    confidence,asserted_by,asserted_at,stale,decision_mode,batch_id \
             FROM fact ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| Ok(row_to_fact(r)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    /// Stamp existing facts as covered by a batch envelope without rewriting
    /// `asserted_at` / `asserted_by` — timestamp replay is never a remedy.
    pub fn stamp_batch_ids(
        &self,
        subject_ids: &[String],
        claim: Claim,
        batch_id: &str,
    ) -> Result<usize> {
        let tx = self.maybe_tx()?;
        let mut n = 0;
        for subject_id in subject_ids {
            let changed = self.conn.execute(
                "UPDATE fact SET decision_mode = 'batch', batch_id = ?1 \
                 WHERE subject_id = ?2 AND claim = ?3",
                rusqlite::params![batch_id, subject_id, claim.as_str()],
            )?;
            n += changed;
        }
        if let Some(tx) = tx {
            tx.commit()?;
        }
        Ok(n)
    }

    /// Re-check every anchor in the graph against the working tree, recompute
    /// each fact's strength, and re-open whatever was demoted.
    ///
    /// This is the uniform replacement for the hand-written ripple matrix: one
    /// question — "does the thing this fact points at still say what it said?" —
    /// asked of every anchor, rather than a per-edge-kind decision table. It is
    /// also what makes import safe: an export can claim `verified`, but the
    /// claim only survives if the run's covered files still hash the same HERE.
    ///
    /// `changed` is the set of CodeFile PATHS whose content moved this run. It
    /// is used only to tell two indistinguishable outcomes apart in the report:
    /// a fact nothing touched, and a fact whose file DID change while its anchor
    /// survived — the second is the sparing that makes a large repo workable,
    /// and it is worth counting.
    pub fn reverify_all(&self, changed: &BTreeSet<String>) -> Result<Reverified> {
        let facts = self.all_facts()?;
        let mut out = Reverified::default();
        // One transaction for the whole pass: re-verification touches every
        // fact's evidence rows, verification column, and (on expiry) the edge
        // status + stale_cause facet + proof reset. A crash mid-pass would
        // otherwise leave the graph half-re-verified — some edges re-opened,
        // their sibling facts still claiming stale strength. `maybe_tx` composes
        // with an outer batch when one is open (`restore` runs it after its own
        // commit; `sync` outside any tx), so this never nests.
        let tx = self.maybe_tx()?;
        let mut successors = SuccessorCache::default();
        // Re-anchors are journaled only AFTER the transaction commits — the
        // journal must never record a move the store rolled back.
        let mut reanchors: Vec<(String, String, Reanchor)> = Vec::new();
        for fact in facts {
            let rows = self.evidence_for(&fact.id)?;
            let mut checked = Vec::with_capacity(rows.len());
            for mut row in rows {
                match self.recheck(&fact.subject_id, &row.payload, &mut successors) {
                    AnchorFate::Holds => {
                        row.holds = true;
                        row.expiry_reason = None;
                        self.conn.execute(
                            "UPDATE evidence SET holds = 1, expiry_reason = '' WHERE id = ?1",
                            [&row.id],
                        )?;
                    }
                    AnchorFate::Moved { file, start, end } => {
                        row.holds = true;
                        row.expiry_reason = None;
                        let Evidence::Span(from) = &row.payload else {
                            bail!("AnchorFate::Moved from a non-span anchor");
                        };
                        let reanchor = Reanchor {
                            from_file: from.file.clone(),
                            from_start: from.start,
                            from_end: from.end,
                            symbol: from.symbol.clone(),
                            to_file: file,
                            to_start: start,
                            to_end: end,
                        };
                        let mut payload = row.payload.clone();
                        if let Evidence::Span(span) = &mut payload {
                            span.file = reanchor.to_file.clone();
                            span.start = reanchor.to_start;
                            span.end = reanchor.to_end;
                        }
                        // Ids are content-addressed, so new coordinates change
                        // identity — recomputed here, as widen_to_file_scope
                        // does for scope. A row already carrying the recomputed
                        // identity makes this one redundant: merge, don't
                        // violate the primary key.
                        let new_id = EvidenceRow::id_for(&fact.id, &payload);
                        let collision = new_id != row.id
                            && self
                                .conn
                                .query_row(
                                    "SELECT 1 FROM evidence WHERE id = ?1",
                                    [&new_id],
                                    |_| Ok(()),
                                )
                                .optional()?
                                .is_some();
                        if collision {
                            self.conn
                                .execute("DELETE FROM evidence WHERE id = ?1", [&row.id])?;
                            reanchors.push((fact.id.clone(), String::new(), reanchor));
                            continue; // the surviving twin already holds
                        }
                        self.conn.execute(
                            "UPDATE evidence SET id = ?1, payload = ?2, holds = 1, expiry_reason = ''
                             WHERE id = ?3",
                            rusqlite::params![new_id, serde_json::to_string(&payload)?, row.id],
                        )?;
                        reanchors.push((fact.id.clone(), new_id.clone(), reanchor));
                        row.id = new_id;
                        row.payload = payload;
                        out.reanchored += 1;
                    }
                    AnchorFate::Broken(cause) => {
                        row.holds = false;
                        row.expiry_reason = Some(cause);
                        self.conn.execute(
                            "UPDATE evidence SET holds = 0, expiry_reason = ?1 WHERE id = ?2",
                            rusqlite::params![cause.as_str(), row.id],
                        )?;
                    }
                }
                checked.push(row);
            }
            let strength = level(&checked);
            if strength.rank() < fact.verification.rank() {
                out.demoted += 1;
            } else if !changed.is_empty()
                && strength.counts()
                && checked.iter().any(|r| touches(&r.payload, changed))
            {
                // The file moved under it and the claim still stands.
                out.spared += 1;
            }
            // A fact that has fallen below what counts is no longer settled
            // truth, so the claim RE-OPENS and its lane serves it again.
            // Demoting the fact while leaving the edge green would leave the
            // graph reporting a verdict it no longer stands behind — the exact
            // shape this whole spine exists to make impossible.
            // Re-open on EXPIRED, not on "stopped counting". Expired means every
            // anchor this fact had is gone (or it never had one) — the vacuous
            // case. Facts whose floor is `claimed` by design (an `independent`
            // relationship, a finding that flags no file) carry a Claim row that
            // never breaks, so they settle once and stay settled instead of
            // being re-opened on every sync.
            //
            // Keyed off the CURRENT edge status rather than the fact's previous
            // verification, because an import can arrive carrying a passing edge
            // whose fact never counted here — and that edge has to re-open too.
            let settled = self
                .get_edge(&fact.subject_id)?
                .map(|e| {
                    matches!(
                        e.status,
                        InspectionStatus::Passing
                            | InspectionStatus::Failing
                            | InspectionStatus::Independent
                    )
                })
                .unwrap_or(false);
            if fact.claim == Claim::Verdict && strength == Verification::Expired && settled {
                self.write_edge_status(
                    &fact.subject_id,
                    InspectionStatus::NeedsReverification.as_str(),
                )?;
                // The typed reason is the source of truth; this facet is its
                // rendering, so `loom next` can say WHY in one line without
                // every reader learning the type.
                let why = checked
                    .iter()
                    .find(|r| !r.holds)
                    .map(describe)
                    .unwrap_or_else(|| "anchor missing".to_string());
                self.set_facet(
                    &fact.subject_id,
                    TargetKind::Edge,
                    "stale_cause",
                    &why,
                    crate::model::TruthClass::Derived,
                )?;
                out.validations_reset += self.reset_proof_if_any(&fact.subject_id)?;
            }

            if strength != fact.verification {
                let cause = checked
                    .iter()
                    .find_map(|r| r.expiry_reason)
                    .unwrap_or(StaleCause::AnchorMissing);
                let subjects: Vec<String> = checked
                    .iter()
                    .filter(|r| !r.holds)
                    .map(|r| r.id.clone())
                    .collect();
                let reason = StaleReason::new(cause, subjects, now(&self.conn)?);
                self.conn.execute(
                    "UPDATE fact SET verification = ?1, stale = ?2 WHERE id = ?3",
                    rusqlite::params![
                        strength.as_str(),
                        if strength.counts() {
                            String::new()
                        } else {
                            serde_json::to_string(&reason)?
                        },
                        fact.id
                    ],
                )?;
            }
        }
        if let Some(tx) = tx {
            tx.commit()?;
        }
        // The store's word is final; now the journal records what moved. A
        // re-anchored verdict was NOT re-inspected — the journal entry is the
        // audit trail that lets a reviewer tell a content-identical move from
        // a silent rewrite.
        for (fact_id, evidence_id, r) in reanchors {
            crate::journal::append(
                &self.root,
                crate::evidence::REANCHOR_EVENT,
                if evidence_id.is_empty() {
                    &fact_id
                } else {
                    &evidence_id
                },
                serde_json::json!({
                    "fact_id": fact_id,
                    "evidence_id": evidence_id,
                    "symbol": r.symbol,
                    "from": { "file": r.from_file, "start": r.from_start, "end": r.from_end },
                    "to": { "file": r.to_file, "start": r.to_start, "end": r.to_end },
                }),
            )?;
        }
        Ok(out)
    }
}

/// The trailing `[body-fingerprint]` of a locator probe's detail line — the
/// part that is identity. Everything before it (kind, name, FILE, match
/// count) is display metadata: a symbol that crossed files with its body
/// intact is the same subject, not a redefinition.
fn locator_body_fingerprint(detail: &str) -> Option<&str> {
    let detail = detail.trim_end();
    let close = detail.strip_suffix(']')?;
    let open = close.rfind('[')?;
    let fp = &close[open + 1..];
    (!fp.is_empty() && fp.chars().all(|c| c.is_ascii_hexdigit())).then_some(fp)
}

/// One broken anchor, in a sentence a worker can act on.
fn describe(row: &EvidenceRow) -> String {
    let what = match &row.payload {
        Evidence::Run(run) => run.command.clone(),
        Evidence::Span(span) => format!("{}:{}-{}", span.file, span.start, span.end),
        Evidence::Journal { r#ref } => format!("journal:{ref}", ref = r#ref),
        Evidence::Claim { .. } => "recorded rationale".to_string(),
    };
    match row.expiry_reason {
        Some(cause) => format!("{} no longer holds ({})", what, cause.as_str()),
        None => format!("{what} no longer holds"),
    }
}

impl Store {
    /// Reset the Validation behind a re-opened proof edge, if there is one.
    ///
    /// A proof whose anchor broke is not merely a stale edge — the Validation
    /// node itself has to read `not_run`, or `loom status` reports a passing
    /// proof behind a re-opened claim. Returns 1 when a proof that was actually
    /// standing came down; one already at `not_run` is unchanged, and counting
    /// it would overstate how much re-verification a sync created.
    fn reset_proof_if_any(&self, edge_id: &str) -> Result<usize> {
        let Some(edge) = self.get_edge(edge_id)? else {
            return Ok(0);
        };
        if edge.kind != crate::model::EdgeKind::Validates {
            return Ok(0);
        }
        let was_proven = self
            .get_node(&edge.from_id)?
            .map(|n| n.status != "not_run")
            .unwrap_or(false);
        self.reset_validation_status_for_sync(&edge.from_id)?;
        Ok(usize::from(was_proven))
    }

    /// The current command of the validation behind a verdict's subject edge, if
    /// any. A `validates` edge runs from a Validation node to the intent it
    /// proves; the recorded run's command must still match this or the proof is
    /// stale. Any miss (edge/node gone, not a Validation, no command) is `None`
    /// — a lookup must never fail the whole reverify pass.
    fn validation_command_for(&self, edge_id: &str) -> Option<String> {
        let edge = self.get_edge(edge_id).ok().flatten()?;
        let node = self.get_node(&edge.from_id).ok().flatten()?;
        if node.node_type != NodeType::Validation {
            return None;
        }
        node.body
            .get("command")
            .and_then(|c| c.as_str())
            .map(str::to_string)
    }

    /// Does this anchor still hold? `subject_id` is the fact's subject, used
    /// only to resolve a validation Run's current command so a rewritten
    /// command re-opens its proof. `successors` feeds the deleted-file search:
    /// a span whose file is gone re-anchors into the ONE registered codefile
    /// still holding its content, when there is exactly one.
    fn recheck(
        &self,
        subject_id: &str,
        payload: &Evidence,
        successors: &mut SuccessorCache,
    ) -> AnchorFate {
        match payload {
            // Prose cannot rot mechanically — and never counts, so nothing turns
            // on it either way.
            Evidence::Claim { .. } => AnchorFate::Holds,
            // A LOCATOR run asserts "this symbol is here", so it expires when
            // the symbol stops resolving — not when an unrelated line in the
            // same file moves. Comparing file hashes would re-open every
            // grounding in a file on any edit, destroying the symbol-scoped
            // sparing that makes a large repo workable. Re-running the probe is
            // both cheaper to reason about and exactly what the claim means.
            // A seam claim survives content churn by design, so its anchor is
            // re-run rather than hashed. Comparing file hashes here would
            // re-open every consumer grounding on any edit to the file, which
            // is exactly the claim it does NOT make.
            Evidence::Run(run) if run.producer == crate::model::RunProducer::Seam => {
                let Some((locator, file)) = run
                    .command
                    .strip_prefix("seam '")
                    .and_then(|r| r.split_once("' in "))
                else {
                    // The command prose does not match the shape this arm mints.
                    // recheck's Holds means "holds", so a `?` here would make an
                    // unparseable (or crafted/imported) seam run immortal. Fail
                    // closed: an anchor we cannot re-resolve is a broken anchor.
                    return AnchorFate::Broken(StaleCause::AnchorMissing);
                };
                match std::fs::read_to_string(self.root.join(file)) {
                    // The consumer surface itself is gone.
                    Err(_) => AnchorFate::Broken(StaleCause::SpanFileDeleted),
                    Ok(content) => {
                        if crate::runner::seam_present(&content, locator) {
                            AnchorFate::Holds
                        } else {
                            AnchorFate::Broken(StaleCause::SeamGone)
                        }
                    }
                }
            }
            Evidence::Run(run) if run.producer == crate::model::RunProducer::Locator => {
                // The locator claim belongs to the EDGE, not to the file path
                // recorded when it was minted: after `edge retarget` (a file
                // rename/split) the edge's CURRENT target is where the symbol
                // must resolve — the recorded path is provenance, not
                // identity. Same body under the new path → Holds (the move is
                // journaled by the retarget itself); a body that changed in
                // the move → SubjectRedefined, an honest re-open.
                let file = self
                    .get_edge(subject_id)
                    .ok()
                    .flatten()
                    .and_then(|e| self.get_node(&e.to_id).ok().flatten())
                    .map(|n| n.name)
                    .filter(|name| self.root.join(name).is_file())
                    .unwrap_or_else(|| run.covered.keys().next().cloned().unwrap_or_default());
                let locator = run
                    .command
                    .strip_prefix("resolve '")
                    .and_then(|r| r.split_once("' in "))
                    .map(|(l, _)| l.to_string());
                match crate::runner::locator_probe(&self.root, &file, locator.as_deref()) {
                    // Same symbol, same body: the claim is untouched by whatever
                    // else moved in the file. This is the symbol-scoped sparing
                    // that keeps a large repo workable.
                    Some(fresh) if fresh.exit_code == 0 && fresh.stdout_hash == run.stdout_hash => {
                        AnchorFate::Holds
                    }
                    // The symbol's BODY fingerprint is the identity; the file
                    // path inside the probe's detail line is display metadata.
                    // A retargeted edge probes its new target, so the full
                    // output differs even when the body crossed intact —
                    // compare the bracketed body fingerprint before declaring
                    // a redefinition. Anchors recorded before excerpts existed
                    // fall back to the strict full-output compare above.
                    Some(fresh) if fresh.exit_code == 0 => {
                        match (
                            locator_body_fingerprint(&run.stdout_excerpt),
                            locator_body_fingerprint(&fresh.stdout_excerpt),
                        ) {
                            (Some(recorded), Some(current)) if recorded == current => {
                                AnchorFate::Holds
                            }
                            _ => AnchorFate::Broken(StaleCause::SubjectRedefined),
                        }
                    }
                    Some(_) => AnchorFate::Broken(StaleCause::AnchorMissing),
                    None => AnchorFate::Broken(StaleCause::SpanFileDeleted),
                }
            }
            Evidence::Run(run) => {
                // A validation's recorded run also anchors the COMMAND that
                // produced it: rewriting the command means the old passing run
                // no longer proves what the validation now asks, so the claim
                // re-opens. Only a plain Command run is compared — a Journey
                // composes its command with a base URL/env the recorder cannot
                // reproduce, so its recorded command legitimately differs from
                // the node's. Any resolution failure falls through to the
                // covered-file check rather than inventing staleness.
                if run.producer == crate::model::RunProducer::Command {
                    if let Some(cmd) = self.validation_command_for(subject_id) {
                        if cmd != run.command {
                            return AnchorFate::Broken(StaleCause::RunCommandChanged);
                        }
                    }
                }
                match crate::runner::covered_intact(&self.root, run) {
                    Some(_) => AnchorFate::Broken(StaleCause::RunCoveredFileChanged),
                    None => AnchorFate::Holds,
                }
            }
            Evidence::Span(stamp) => match std::fs::read_to_string(self.root.join(&stamp.file)) {
                Ok(content) => match crate::evidence::span_fate(stamp, &stamp.file, &content) {
                    crate::evidence::SpanFate::Intact => AnchorFate::Holds,
                    crate::evidence::SpanFate::Moved { start, end } => AnchorFate::Moved {
                        file: stamp.file.clone(),
                        start,
                        end,
                    },
                    crate::evidence::SpanFate::ScopeChanged => {
                        AnchorFate::Broken(StaleCause::ScopeFileChanged)
                    }
                    crate::evidence::SpanFate::Rewritten => {
                        AnchorFate::Broken(StaleCause::SpanRewritten)
                    }
                },
                // The cited file is gone. Before re-opening the claim, look for
                // the body in the registered codefiles still on disk: exactly
                // one match is a declared successor and the verdict crosses
                // with its evidence; zero or two-plus matches fail closed.
                Err(_) => match crate::evidence::find_elsewhere(stamp, successors.get(self)) {
                    Some((file, start, end)) => AnchorFate::Moved { file, start, end },
                    None => AnchorFate::Broken(StaleCause::SpanFileDeleted),
                },
            },
            Evidence::Journal { r#ref } => match crate::journal::exists(&self.root, r#ref) {
                Ok(true) => AnchorFate::Holds,
                _ => AnchorFate::Broken(StaleCause::JournalMissing),
            },
        }
    }

    /// Re-open a fact whose anchor broke. Replaces `stale_edge`: the cause is
    /// typed, so the router reads a `Rework` class instead of grepping prose.
    pub fn expire_fact(&self, fact_id: &str, reason: StaleReason) -> Result<bool> {
        let Some(view) = self.fact_by_id(fact_id)? else {
            return Ok(false);
        };
        if view.fact.claim == Claim::Verdict {
            // Only a settled verdict can go stale; an unrun one already is.
            if !matches!(
                InspectionStatus::from_str(&view.fact.state)?,
                InspectionStatus::Passing
                    | InspectionStatus::Failing
                    | InspectionStatus::Independent
            ) {
                return Ok(false);
            }
            self.write_edge_status(
                &view.fact.subject_id,
                InspectionStatus::NeedsReverification.as_str(),
            )?;
        }
        self.conn.execute(
            "UPDATE fact SET verification = 'expired', stale = ?1 WHERE id = ?2",
            rusqlite::params![serde_json::to_string(&reason)?, fact_id],
        )?;
        Ok(true)
    }
}

fn row_to_fact(r: &rusqlite::Row) -> Result<Fact> {
    let stale: String = r.get("stale")?;
    Ok(Fact {
        id: r.get("id")?,
        subject_kind: TargetKind::from_str(&r.get::<_, String>("subject_kind")?)?,
        subject_id: r.get("subject_id")?,
        claim: Claim::from_str(&r.get::<_, String>("claim")?)?,
        state: r.get("state")?,
        criterion: r.get("criterion")?,
        verification: Verification::from_str(&r.get::<_, String>("verification")?)?,
        confidence: r.get("confidence")?,
        asserted_by: r.get("asserted_by")?,
        asserted_at: r.get("asserted_at")?,
        decision_mode: crate::model::DecisionMode::parse(&r.get::<_, String>("decision_mode")?),
        batch_id: r.get("batch_id")?,
        stale: if stale.is_empty() {
            None
        } else {
            serde_json::from_str(&stale).ok()
        },
    })
}

/// Claim-specific state vocabulary. Rejecting an unknown state here is what
/// stops a typo becoming a silently-unqueryable fact.
fn check_state(claim: Claim, state: &str) -> Result<()> {
    match claim {
        Claim::Verdict => {
            let status = InspectionStatus::from_str(state)?;
            if status == InspectionStatus::Current {
                bail!("'current' is a derived status — sync owns it, not an assertion");
            }
            Ok(())
        }
        Claim::Adjudication => {
            const VERDICTS: &[&str] = &[
                "needed",
                "justified",
                "rejected",
                "deferred",
                "blocked",
                "duplicate",
                "resolved",
            ];
            if !VERDICTS.contains(&state) {
                bail!(
                    "unknown finding verdict '{state}' (use {})",
                    VERDICTS.join("|")
                );
            }
            Ok(())
        }
        Claim::Ratification => {
            if !crate::grammar::RATIFICATION_STATES.contains(&state) {
                bail!(
                    "unknown ratification state '{state}' (use {})",
                    crate::grammar::RATIFICATION_STATES.join("|")
                );
            }
            Ok(())
        }
        Claim::Observation => Ok(()),
    }
}

/// `query_row` returns `QueryReturnedNoRows`; make that an `Option` instead.
trait OptionalRow<T> {
    fn optional_row(self) -> Result<Option<T>>;
}

impl<T> OptionalRow<T> for rusqlite::Result<T> {
    fn optional_row(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// Convenience for the common case: a verdict about an edge.
pub fn edge_verdict(edge_id: &str) -> Subject {
    Subject::Edge(edge_id.to_string())
}

impl Store {
    /// The human-readable justification behind an edge's verdict: the prose the
    /// author wrote, which is now ONE kind of evidence rather than the whole of
    /// it. Empty when the verdict rests only on anchors loom produced.
    pub fn verdict_prose(&self, edge_id: &str) -> Result<String> {
        let Some(view) = self.fact(&Subject::Edge(edge_id.to_string()), Claim::Verdict)? else {
            return Ok(String::new());
        };
        Ok(view
            .evidence
            .iter()
            .find_map(|r| match &r.payload {
                Evidence::Claim { text } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default())
    }

    /// How strongly this edge's verdict is anchored. `Expired` when there is no
    /// verdict at all — an unrecorded claim is not a weak claim, it is absent.
    pub fn edge_verification(&self, edge_id: &str) -> Result<Verification> {
        Ok(self
            .fact(&Subject::Edge(edge_id.to_string()), Claim::Verdict)?
            .map(|v| v.fact.verification)
            .unwrap_or(Verification::Expired))
    }

    /// Like [`Store::edge_verification`] but keeps the absent case distinct:
    /// `None` means no verdict fact was ever recorded (a settled status
    /// standing on nothing), whereas `Some(Expired)` is a recorded verdict
    /// whose anchors have all since broken. Auditors that must tell "never
    /// recorded" from "went stale" use this; `edge_verification` collapses both
    /// to `Expired`.
    pub fn edge_verdict_verification(&self, edge_id: &str) -> Result<Option<Verification>> {
        Ok(self
            .fact(&Subject::Edge(edge_id.to_string()), Claim::Verdict)?
            .map(|v| v.fact.verification))
    }
}

impl Store {
    /// An intent's wantedness. Absence reads as `unratified` — wantedness is
    /// never presumed, and a graph that has never been asked has not said yes.
    pub fn ratification(&self, intent_id: &str) -> Result<String> {
        let journal = crate::journal::read(self.root())?;
        self.ratification_with_journal(intent_id, &journal)
    }

    pub(crate) fn ratification_with_journal(
        &self,
        intent_id: &str,
        journal: &[crate::journal::Entry],
    ) -> Result<String> {
        let Some(view) = self.fact(&Subject::Node(intent_id.to_string()), Claim::Ratification)?
        else {
            return Ok("unratified".into());
        };
        if !matches!(view.fact.state.as_str(), "ratified" | "rejected") {
            return Ok(view.fact.state);
        }
        let event = if view.fact.state == "rejected" {
            "rejection"
        } else {
            "ratification"
        };
        let node = self.get_node(intent_id)?;
        let journal_stands = view.evidence.iter().any(|row| {
            let Evidence::Journal { r#ref } = &row.payload else {
                return false;
            };
            row.holds
                && journal.iter().any(|entry| {
                    let presence = entry.payload.get("presence").and_then(|v| v.as_str());
                    let mediated_stands = presence != Some("host-mediated")
                        || entry
                            .payload
                            .get("human_decision")
                            .and_then(|decision| decision.get("mode").zip(decision.get("response")))
                            .and_then(|(mode, response)| Some((mode.as_str()?, response.as_str()?)))
                            .is_some_and(|(mode, response)| {
                                mode == "mediated"
                                    && !response.trim().is_empty()
                                    && !crate::model::is_placeholder(response)
                            });
                    entry.origin == crate::journal::Origin::Local
                        && entry.id == *r#ref
                        && entry.target_id == intent_id
                        && entry.event == event
                        && entry.payload.get("ratified_by").and_then(|v| v.as_str())
                            == Some("human")
                        && presence == Some(view.fact.criterion.as_str())
                        && mediated_stands
                        && node.as_ref().is_none_or(|node| {
                            node.node_type != NodeType::Pattern
                                || entry.payload.get("pattern_body") == Some(&node.body)
                        })
                })
        });
        if view.fact.asserted_by == "human" && view.fact.verification.counts() && journal_stands {
            Ok(view.fact.state)
        } else {
            Ok("needs_reconfirmation".into())
        }
    }

    /// Who recorded the ratification, and when.
    pub fn ratified_by(&self, intent_id: &str) -> Result<Option<(String, String)>> {
        Ok(self
            .fact(&Subject::Node(intent_id.to_string()), Claim::Ratification)?
            .map(|v| (v.fact.asserted_by, v.fact.asserted_at)))
    }

    /// The authority channel recorded with the ratification (`tty+challenge`
    /// for a direct decision, `host-mediated` when an LLM recorded the human's
    /// answer). Stored as the fact's criterion.
    pub fn ratified_presence(&self, intent_id: &str) -> Result<Option<String>> {
        Ok(self
            .fact(&Subject::Node(intent_id.to_string()), Claim::Ratification)?
            .map(|v| v.fact.criterion)
            .filter(|c| !c.is_empty()))
    }
}

/// Insert facts + evidence imported from a snapshot, inside the caller's
/// transaction.
///
/// This lives in the chokepoint module ON PURPOSE. Import is the one path that
/// legitimately writes facts it did not derive, and when that SQL lived in
/// `facets.rs` it was a second door with no gate behind it — the door the
/// chokepoint test exists to find. Keeping it here means every statement that
/// can move asserted truth is in one reviewable file, and the strength each
/// fact lands at is not the exporter's claim: `verification` is forced to
/// `claimed` here, and `reverify_all` re-earns it against the local tree
/// immediately after the transaction commits.
pub(super) fn insert_imported(
    tx: &rusqlite::Transaction<'_>,
    facts: &[Fact],
    evidence: &[EvidenceRow],
) -> Result<()> {
    // Ids are recomputed here, never trusted. An imported id produced by a
    // different scheme (or hand-edited) would not collide on `id` but WOULD
    // collide on the fact table's UNIQUE(subject_kind,subject_id,claim), wedging
    // every future assert_fact for that subject. Recompute canonically and remap
    // every evidence row's fact_id through the same table.
    let mut fact_id_remap: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // Validate the complete batch before the first INSERT: a malformed later
    // fact must not leave earlier rows staged in the caller's transaction.
    for fact in facts {
        if crate::journal::stamp_millis(&fact.asserted_at).is_none() {
            bail!(
                "cannot import {} fact '{}' for {} '{}': invalid asserted_at timestamp {:?}",
                fact.claim.as_str(),
                fact.id,
                fact.subject_kind.as_str(),
                fact.subject_id,
                fact.asserted_at
            );
        }
    }
    for fact in facts {
        let canonical = Fact::id_for(fact.subject_kind, &fact.subject_id, fact.claim);
        fact_id_remap.insert(fact.id.clone(), canonical.clone());
        tx.execute(
            "INSERT INTO fact (id,subject_kind,subject_id,claim,state,criterion,\
                               verification,confidence,asserted_by,asserted_at,stale,\
                               decision_mode,batch_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            rusqlite::params![
                canonical,
                fact.subject_kind.as_str(),
                fact.subject_id,
                fact.claim.as_str(),
                fact.state,
                fact.criterion,
                Verification::Claimed.as_str(),
                fact.confidence,
                fact.asserted_by,
                fact.asserted_at,
                "",
                fact.decision_mode.as_str(),
                fact.batch_id,
            ],
        )?;
    }
    for row in evidence {
        // A Run is loom's own observation; nothing in a plaintext export the
        // author fully controls lets us re-check that it really ran (covered
        // hashes are a FRESHNESS check, not an AUTHENTICITY one, and the author
        // has every one of those hashes). Importing it verbatim would let an
        // edited graph.json mint `verified` for a command that never executed.
        // Downgrade it to the prose it actually is: recorded, but never counting.
        // Verified is re-earned only by a local run.
        //
        // EXCEPT loom's own probes: a Locator/Seam run is re-resolved against
        // the live tree by `recheck` (the probe is re-run and its hash
        // compared), and a Prescreen/Detector run is re-checked through its
        // covered-file hashes. A forged probe hash cannot survive that — the
        // fresh loom re-derives the truth. Only an externally-executed Command
        // (or a Journey composing one) needs the downgrade: loom cannot prove
        // from the export that the command ever ran.
        let payload = match &row.payload {
            crate::evidence::Evidence::Run(run)
                if matches!(
                    run.producer,
                    crate::model::RunProducer::Command | crate::model::RunProducer::Journey
                ) =>
            {
                crate::evidence::Evidence::Claim {
                    text: format!("imported run (unverified): {}", run.command),
                }
            }
            other => other.clone(),
        };
        let fact_id = fact_id_remap
            .get(&row.fact_id)
            .cloned()
            .unwrap_or_else(|| row.fact_id.clone());
        let id = crate::evidence::EvidenceRow::id_for(&fact_id, &payload);
        tx.execute(
            "INSERT INTO evidence (id,fact_id,payload,kind,recorded_at,holds,expiry_reason)
             VALUES (?1,?2,?3,?4,?5,1,\'\')",
            rusqlite::params![
                id,
                fact_id,
                serde_json::to_string(&payload)?,
                payload.kind().as_str(),
                row.recorded_at,
            ],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod imported_tests {
    use super::*;

    struct Tmp(std::path::PathBuf);

    impl Tmp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "loom-ratification-origin-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn imported_fact(asserted_at: &str) -> Fact {
        Fact {
            id: "exported-fact-id".into(),
            subject_kind: TargetKind::Node,
            subject_id: "subject-123".into(),
            claim: Claim::Adjudication,
            state: "justified".into(),
            criterion: String::new(),
            verification: Verification::Cited,
            confidence: 1.0,
            asserted_by: "human".into(),
            asserted_at: asserted_at.into(),
            decision_mode: crate::model::DecisionMode::Individual,
            batch_id: String::new(),
            stale: None,
        }
    }

    #[test]
    fn imported_ratification_and_rejection_journal_rows_have_no_local_authority() {
        for (state, event) in [("ratified", "ratification"), ("rejected", "rejection")] {
            let tmp = Tmp::new();
            let store = Store::init(&tmp.0, Some("origin test"), false).unwrap();
            let intent = store
                .add_node(
                    NodeType::Intent,
                    &format!("locally {state} behavior"),
                    "a behavior whose decision authority is under test",
                    "planned",
                    serde_json::json!({}),
                )
                .unwrap();
            if state == "ratified" {
                store
                    .ratify_intent(&intent.id, "the local human wants this", "test fixture")
                    .unwrap();
            } else {
                store
                    .reject_intent(
                        &intent.id,
                        "the local human does not want this",
                        "test fixture",
                    )
                    .unwrap();
            }

            let journal = crate::journal::read(&tmp.0).unwrap();
            assert!(journal.iter().any(|entry| {
                entry.event == event && entry.origin == crate::journal::Origin::Local
            }));
            assert_eq!(
                store
                    .ratification_with_journal(&intent.id, &journal)
                    .unwrap(),
                state,
                "a local direct {event} must count"
            );

            let imported = journal
                .into_iter()
                .map(|mut entry| {
                    entry.origin = crate::journal::Origin::Imported;
                    entry
                })
                .collect::<Vec<_>>();
            assert_eq!(
                store
                    .ratification_with_journal(&intent.id, &imported)
                    .unwrap(),
                "needs_reconfirmation",
                "an imported {event} must not confer local authority"
            );
        }
    }

    #[test]
    fn insert_imported_rejects_invalid_asserted_at_before_sql() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        let fact = imported_fact("2026-02-29T07:10:25Z");
        let error = insert_imported(&tx, &[fact], &[]).unwrap_err().to_string();
        assert!(error.contains("subject-123"), "{error}");
        assert!(error.contains("exported-fact-id"), "{error}");
        assert!(error.contains("2026-02-29T07:10:25Z"), "{error}");
        assert!(
            !error.contains("no such table"),
            "validation must happen before SQL: {error}"
        );
    }
}
