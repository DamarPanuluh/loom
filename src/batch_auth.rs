//! Batch authorization — sealed envelopes that account for judgment-shaped
//! bulk writes without pretending each write was an independent judgment.
//!
//! Plane: soundness provenance. A `judgment_burst` is unexplained judgment
//! compression. An envelope does not waive the detector; it records that one
//! authorized batch decision, with an evidenced predicate, covered an exact
//! subject set before or during execution.
//!
//! Facts retain `decision_mode=batch` and `batch_id` so history never claims
//! individual inspection occurred. Timestamp replay is never a remedy.

use crate::journal::{self, Entry, Origin};
use crate::model::{is_placeholder, Claim};
use crate::store::Store;
use crate::Result;
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

/// Journal event name for a sealed batch authorization.
pub const EVENT: &str = "batch_authorization";

pub use crate::model::DecisionMode;

/// What kind of judgment-shaped operation the envelope authorizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchClaim {
    Ratification,
    Adjudication,
}

impl BatchClaim {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ratification => "ratification",
            Self::Adjudication => "adjudication",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "ratification" => Ok(Self::Ratification),
            "adjudication" => Ok(Self::Adjudication),
            other => bail!("unknown batch claim '{other}' (use ratification|adjudication)"),
        }
    }

    pub fn matches_fact(self, claim: Claim) -> bool {
        Claim::from(self) == claim
    }
}

impl From<BatchClaim> for Claim {
    fn from(claim: BatchClaim) -> Self {
        match claim {
            BatchClaim::Ratification => Claim::Ratification,
            BatchClaim::Adjudication => Claim::Adjudication,
        }
    }
}

impl TryFrom<Claim> for BatchClaim {
    type Error = anyhow::Error;

    fn try_from(claim: Claim) -> Result<Self> {
        match claim {
            Claim::Ratification => Ok(Self::Ratification),
            Claim::Adjudication => Ok(Self::Adjudication),
            other => bail!("claim '{}' is not batch-authorizable", other.as_str()),
        }
    }
}

/// Sealed authorization covering an exact set of judgment writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchAuthorization {
    pub claim: BatchClaim,
    /// Permitted operation (`ratify`, `reject`, `verdict`, …).
    pub operation: String,
    /// Exact subject ids, sorted at seal time.
    pub subjects: Vec<String>,
    /// Immutable digest of the sorted subject set.
    pub subject_digest: String,
    pub authority: String,
    pub executor: String,
    pub decision_mode: DecisionMode,
    /// Batch-level criterion (the shared predicate or portfolio decision).
    pub criterion: String,
    /// Contemporaneous local evidence references. Only `journal:<id>` citations
    /// to locally appended entries can satisfy validation; imported journal
    /// history and prose remain descriptive context only.
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub time_start: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub time_end: String,
    /// Required when the batch claims mechanical routing safety.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub routing_class: String,
    /// Host-mediated human answer when authority is human product judgment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_decision: Option<crate::ratification::HumanDecision>,
}

impl BatchAuthorization {
    /// Build and validate a seal over `subjects`.
    pub fn seal(
        claim: BatchClaim,
        operation: impl Into<String>,
        subjects: impl IntoIterator<Item = impl Into<String>>,
        authority: impl Into<String>,
        executor: impl Into<String>,
        criterion: impl Into<String>,
        evidence: Vec<String>,
    ) -> Result<Self> {
        let mut subjects: Vec<String> = subjects.into_iter().map(Into::into).collect();
        subjects.sort();
        subjects.dedup();
        if subjects.is_empty() {
            bail!("a batch authorization must name at least one subject");
        }
        let subject_digest = subject_digest(&subjects);
        let criterion = criterion.into();
        if is_placeholder(&criterion) {
            bail!("batch criterion must be substantive — name the shared predicate or portfolio decision");
        }
        if evidence.is_empty() {
            bail!("batch authorization requires contemporaneous evidence references");
        }
        if evidence.iter().all(|e| is_placeholder(e)) {
            bail!("batch evidence must cite contemporaneous records, not placeholder prose");
        }
        let authority = authority.into();
        let operation = operation.into();
        if claim == BatchClaim::Ratification && !authority_is_human(&authority) {
            bail!("ratification batches require human authority (got '{authority}')");
        }
        Ok(Self {
            claim,
            operation,
            subjects,
            subject_digest,
            authority,
            executor: executor.into(),
            decision_mode: DecisionMode::Batch,
            criterion,
            evidence,
            command_id: String::new(),
            time_start: String::new(),
            time_end: String::new(),
            routing_class: String::new(),
            human_decision: None,
        })
    }

    pub fn with_command_id(mut self, id: impl Into<String>) -> Self {
        self.command_id = id.into();
        self
    }

    pub fn with_time_bounds(mut self, start: impl Into<String>, end: impl Into<String>) -> Self {
        self.time_start = start.into();
        self.time_end = end.into();
        self
    }

    pub fn with_routing_class(mut self, class: impl Into<String>) -> Self {
        self.routing_class = class.into();
        self
    }

    pub fn with_human_decision(mut self, decision: crate::ratification::HumanDecision) -> Self {
        self.human_decision = Some(decision);
        self
    }

    /// Digest still matches the sealed subject list.
    pub fn digest_holds(&self) -> bool {
        subject_digest(&self.subjects) == self.subject_digest
    }

    /// Exact set match against a live burst bucket.
    pub fn covers_subjects(&self, subjects: &[String]) -> bool {
        let mut sorted: Vec<String> = subjects.to_vec();
        sorted.sort();
        sorted.dedup();
        self.digest_holds() && self.subjects == sorted
    }
}

/// Stable digest over a sorted subject id set.
pub fn subject_digest(subjects: &[String]) -> String {
    let mut sorted = subjects.to_vec();
    sorted.sort();
    sorted.dedup();
    format!(
        "b{}",
        crate::store::fnv_hex_digest(&sorted.iter().map(String::as_str).collect::<Vec<_>>())
    )
}

fn authority_is_human(authority: &str) -> bool {
    authority == "human" || authority.starts_with("human:")
}

/// Append a sealed envelope to the journal. `target_id` is the subject digest.
pub fn append_envelope(root: &Path, envelope: &BatchAuthorization) -> Result<Entry> {
    if !envelope.digest_holds() {
        bail!("batch subject digest does not match the sealed subject list");
    }
    let payload = serde_json::to_value(envelope)?;
    journal::append(root, EVENT, &envelope.subject_digest, payload)
}

/// Parse a journal entry as a batch authorization envelope.
pub fn parse_entry(entry: &Entry) -> Result<Option<BatchAuthorization>> {
    if entry.event != EVENT {
        return Ok(None);
    }
    let envelope: BatchAuthorization = serde_json::from_value(entry.payload.clone())
        .with_context(|| format!("parsing batch_authorization journal entry {}", entry.id))?;
    Ok(Some(envelope))
}

/// Load local batch authorization envelopes from the journal. Imported rows
/// remain readable as history but are not candidate authority.
pub fn load_envelopes(root: &Path) -> Result<Vec<(Entry, BatchAuthorization)>> {
    let (entries, _) = journal::read_counting(root)?;
    let mut out = Vec::new();
    for entry in entries {
        if entry.origin != Origin::Local {
            continue;
        }
        if let Some(env) = parse_entry(&entry)? {
            out.push((entry, env));
        }
    }
    Ok(out)
}

/// Why a candidate envelope fails to cover a burst.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeReject {
    DigestMismatch,
    ClaimMismatch,
    MissingEvidence,
    EvidenceUnresolved(String),
    EvidenceNotContemporaneous(String),
    RetrospectiveEnvelope,
    ImportedEnvelope,
    ImportedEvidence(String),
    WrongAuthority(String),
    MechanicalLabeledJudgment,
    DigestCorrupt,
    ProseOnlyEvidence,
}

impl EnvelopeReject {
    pub fn as_str(&self) -> String {
        match self {
            Self::DigestMismatch => "covers a different subject set".into(),
            Self::ClaimMismatch => "claim type does not match the burst facts".into(),
            Self::MissingEvidence => "missing contemporaneous evidence".into(),
            Self::EvidenceUnresolved(r) => format!("evidence does not resolve: {r}"),
            Self::EvidenceNotContemporaneous(r) => {
                format!("evidence is not contemporaneous: {r}")
            }
            Self::RetrospectiveEnvelope => {
                "authorization envelope was written after the final fact assertion".into()
            }
            Self::ImportedEnvelope => {
                "authorization envelope is imported, not a local authorization act".into()
            }
            Self::ImportedEvidence(r) => {
                format!("evidence is imported, not a local journal act: {r}")
            }
            Self::WrongAuthority(a) => format!("wrong authority: {a}"),
            Self::MechanicalLabeledJudgment => {
                "labels judgment work as mechanical without a routing class".into()
            }
            Self::DigestCorrupt => "subject digest does not match its sealed list".into(),
            Self::ProseOnlyEvidence => {
                "evidence is prose acknowledgment only — not contemporaneous proof".into()
            }
        }
    }
}

/// The journal provenance and exact burst boundary checked against an envelope.
#[derive(Debug, Clone, Copy)]
pub struct CoverContext<'a> {
    pub envelope_ts: &'a str,
    pub envelope_origin: Origin,
    pub subjects: &'a [String],
    pub claim: BatchClaim,
    pub burst_minute: &'a str,
    pub latest_assertion_millis: i64,
}

/// Validate that a journal envelope entry may close the burst described by
/// `context`. Imported envelopes are history and never local authority.
pub fn validate_cover(
    store: &Store,
    envelope: &BatchAuthorization,
    context: CoverContext<'_>,
) -> Result<(), EnvelopeReject> {
    let CoverContext {
        envelope_ts,
        envelope_origin,
        subjects,
        claim,
        burst_minute,
        latest_assertion_millis,
    } = context;
    if envelope_origin != Origin::Local {
        return Err(EnvelopeReject::ImportedEnvelope);
    }
    if !envelope.digest_holds() {
        return Err(EnvelopeReject::DigestCorrupt);
    }
    if !envelope.covers_subjects(subjects) {
        return Err(EnvelopeReject::DigestMismatch);
    }
    if envelope.claim != claim {
        return Err(EnvelopeReject::ClaimMismatch);
    }
    let Some(envelope_millis) = journal::stamp_millis(envelope_ts) else {
        return Err(EnvelopeReject::RetrospectiveEnvelope);
    };
    if envelope_millis > latest_assertion_millis {
        return Err(EnvelopeReject::RetrospectiveEnvelope);
    }
    if envelope.evidence.is_empty() {
        return Err(EnvelopeReject::MissingEvidence);
    }
    if claim == BatchClaim::Ratification && !authority_is_human(&envelope.authority) {
        return Err(EnvelopeReject::WrongAuthority(envelope.authority.clone()));
    }
    // Mechanical batches must name the routing classification that made them
    // batch-safe. Calling judgment work "mechanical" without that class fails.
    if envelope.routing_class.is_empty()
        && envelope.authority.contains("mechanical")
        && claim == BatchClaim::Adjudication
    {
        return Err(EnvelopeReject::MechanicalLabeledJudgment);
    }
    if claim == BatchClaim::Ratification && envelope.routing_class == "mechanical" {
        return Err(EnvelopeReject::MechanicalLabeledJudgment);
    }

    let mut any_contemporaneous = false;
    let mut all_prose = true;
    for raw in &envelope.evidence {
        if is_placeholder(raw) {
            continue;
        }
        match classify_evidence_ref(raw) {
            EvidenceRefKind::Prose => {}
            EvidenceRefKind::Unresolved => {
                return Err(EnvelopeReject::EvidenceUnresolved(raw.clone()));
            }
            EvidenceRefKind::Journal => {
                all_prose = false;
                let evidence_entry = resolve_journal_ref(store, raw)?;
                if evidence_entry.origin != Origin::Local {
                    return Err(EnvelopeReject::ImportedEvidence(raw.clone()));
                }
                if is_contemporaneous(
                    &evidence_entry.ts,
                    envelope_ts,
                    burst_minute,
                    latest_assertion_millis,
                ) {
                    any_contemporaneous = true;
                } else {
                    return Err(EnvelopeReject::EvidenceNotContemporaneous(raw.clone()));
                }
            }
        }
    }
    if all_prose {
        return Err(EnvelopeReject::ProseOnlyEvidence);
    }
    if !any_contemporaneous {
        return Err(EnvelopeReject::MissingEvidence);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum EvidenceRefKind {
    Journal,
    /// Looks like a machine reference, but has no authentic resolver.
    Unresolved,
    Prose,
}

fn classify_evidence_ref(raw: &str) -> EvidenceRefKind {
    let trimmed = raw.trim();
    if trimmed.starts_with("journal:") {
        return EvidenceRefKind::Journal;
    }
    if trimmed.starts_with("validation:")
        || trimmed.starts_with("run:")
        || trimmed.starts_with("command:")
        || trimmed.starts_with("pkt-")
        || trimmed.starts_with("import:")
        || trimmed.starts_with("apply:")
    {
        return EvidenceRefKind::Unresolved;
    }
    EvidenceRefKind::Prose
}

fn resolve_journal_ref(store: &Store, raw: &str) -> Result<Entry, EnvelopeReject> {
    let id = raw
        .trim()
        .strip_prefix("journal:")
        .expect("classified journal reference")
        .trim();
    if id.is_empty() {
        return Err(EnvelopeReject::EvidenceUnresolved(raw.into()));
    }
    let (entries, _) = journal::read_counting(store.root())
        .map_err(|_| EnvelopeReject::EvidenceUnresolved(raw.into()))?;
    let entry = entries
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| EnvelopeReject::EvidenceUnresolved(raw.into()))?;
    if journal::stamp_millis(&entry.ts).is_none() {
        return Err(EnvelopeReject::EvidenceUnresolved(raw.into()));
    }
    Ok(entry)
}

/// Contemporaneous means: evidence timestamp is at or before the envelope, the
/// envelope is at or before the exact final fact assertion, and the records are
/// within or before the burst minute window — never a note written afterward.
fn is_contemporaneous(
    evidence_ts: &str,
    envelope_ts: &str,
    burst_minute: &str,
    latest_assertion_millis: i64,
) -> bool {
    let Some(burst_minute) = normalized_minute(burst_minute) else {
        return false;
    };
    let Some(envelope_minute) = journal::minute_key(envelope_ts) else {
        return false;
    };
    // A seal written after the burst is retrospective acknowledgment, not
    // authorization. The exact millisecond boundary matters inside the same
    // minute: a seal after the final fact is too late.
    let Some(envelope_millis) = journal::stamp_millis(envelope_ts) else {
        return false;
    };
    if envelope_minute > burst_minute || envelope_millis > latest_assertion_millis {
        return false;
    }

    // Every accepted evidence reference has its own journal timestamp.
    let Some(evidence_minute) = journal::minute_key(evidence_ts) else {
        return false;
    };
    let Some(evidence_millis) = journal::stamp_millis(evidence_ts) else {
        return false;
    };
    evidence_millis <= envelope_millis && evidence_minute <= burst_minute
}

fn normalized_minute(stamp_or_minute: &str) -> Option<String> {
    journal::minute_key(stamp_or_minute)
        .or_else(|| journal::minute_key(&format!("{stamp_or_minute}:00.000Z")))
}

/// Find a valid covering envelope for a burst bucket.
pub fn covering_envelope(
    store: &Store,
    subjects: &[String],
    claim: BatchClaim,
    actor: &str,
    minute: &str,
    batch_ids: &BTreeSet<String>,
    latest_assertion_millis: i64,
) -> Result<Option<String>> {
    let envelopes = load_envelopes(store.root())?;
    // Prefer envelopes named by the facts' batch_id.
    let mut candidates: Vec<&(Entry, BatchAuthorization)> = envelopes
        .iter()
        .filter(|(e, _)| e.origin == Origin::Local && batch_ids.contains(&e.id))
        .collect();
    if candidates.is_empty() {
        let digest = subject_digest(subjects);
        candidates = envelopes
            .iter()
            .filter(|(entry, env)| entry.origin == Origin::Local && env.subject_digest == digest)
            .collect();
    }
    for (entry, env) in candidates {
        // Executor / actor alignment: the burst actor should match the
        // envelope executor, or the envelope authority for human bursts.
        let actor_ok = env.executor == actor
            || env.authority == actor
            || (actor == "human" && authority_is_human(&env.authority));
        if !actor_ok {
            continue;
        }
        if validate_cover(
            store,
            env,
            CoverContext {
                envelope_ts: &entry.ts,
                envelope_origin: entry.origin,
                subjects,
                claim,
                burst_minute: minute,
                latest_assertion_millis,
            },
        )
        .is_ok()
        {
            return Ok(Some(entry.id.clone()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TmpRoot(PathBuf);

    impl TmpRoot {
        fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "loom-batch-auth-{}-{nanos}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TmpRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn envelope() -> BatchAuthorization {
        let subjects = vec!["finding-1".to_string()];
        BatchAuthorization {
            claim: BatchClaim::Adjudication,
            operation: "verdict".into(),
            subject_digest: subject_digest(&subjects),
            subjects,
            authority: "llm:analyzer".into(),
            executor: "llm:analyzer".into(),
            decision_mode: DecisionMode::Batch,
            criterion: "shared reviewed predicate".into(),
            evidence: vec!["journal:fixture-entry".into()],
            command_id: String::new(),
            time_start: String::new(),
            time_end: String::new(),
            routing_class: "reviewed".into(),
            human_decision: None,
        }
    }

    fn validate(
        store: &Store,
        envelope: &BatchAuthorization,
        envelope_ts: &str,
        burst_minute: &str,
    ) -> Result<(), EnvelopeReject> {
        validate_cover(
            store,
            envelope,
            CoverContext {
                envelope_ts,
                envelope_origin: Origin::Local,
                subjects: &envelope.subjects,
                claim: BatchClaim::Adjudication,
                burst_minute,
                latest_assertion_millis: journal::stamp_millis(envelope_ts).unwrap_or(i64::MIN),
            },
        )
    }

    #[test]
    fn fabricated_machine_prefixes_are_unresolved() {
        let tmp = TmpRoot::new();
        let store = Store::init(tmp.path(), Some("batch-auth"), false).unwrap();
        let now = journal::now_iso();
        let minute = journal::minute_key(&now).unwrap();

        for fabricated in [
            "validation:fake",
            "run:fake",
            "command:fake",
            "pkt-fake",
            "import:fake",
            "apply:fake",
        ] {
            let mut envelope = envelope();
            envelope.evidence = vec![fabricated.into()];
            assert_eq!(
                validate(&store, &envelope, &now, &minute),
                Err(EnvelopeReject::EvidenceUnresolved(fabricated.into())),
                "{fabricated} must not borrow authenticity from envelope time"
            );
        }
    }

    #[test]
    fn journal_reference_requires_a_real_entry_with_a_valid_timestamp() {
        let tmp = TmpRoot::new();
        let store = Store::init(tmp.path(), Some("batch-auth"), false).unwrap();
        let mut missing = envelope();
        missing.evidence = vec!["journal:missing".into()];
        let now = journal::now_iso();
        let minute = journal::minute_key(&now).unwrap();
        assert_eq!(
            validate(&store, &missing, &now, &minute),
            Err(EnvelopeReject::EvidenceUnresolved("journal:missing".into()))
        );

        let invalid = Entry {
            id: "invalid-stamp".into(),
            ts: "not-a-timestamp".into(),
            actor: "test".into(),
            event: "test_evidence".into(),
            target_id: "finding-1".into(),
            payload: serde_json::json!({}),
            origin: Origin::Local,
        };
        journal::restore_entries(store.root(), &[invalid]).unwrap();
        let mut invalid_ref = envelope();
        invalid_ref.evidence = vec!["journal:invalid-stamp".into()];
        assert_eq!(
            validate(&store, &invalid_ref, &now, &minute),
            Err(EnvelopeReject::EvidenceUnresolved(
                "journal:invalid-stamp".into()
            ))
        );
    }

    #[test]
    fn real_journal_reference_passes_only_contemporaneously() {
        let tmp = TmpRoot::new();
        let store = Store::init(tmp.path(), Some("batch-auth"), false).unwrap();
        let evidence = journal::append(
            store.root(),
            "test_evidence",
            "finding-1",
            serde_json::json!({ "result": "reviewed" }),
        )
        .unwrap();
        let mut envelope = envelope();
        envelope.evidence = vec![journal::reference(&evidence)];
        let minute = journal::minute_key(&evidence.ts).unwrap();

        assert_eq!(validate(&store, &envelope, &evidence.ts, &minute), Ok(()));

        let evidence_millis = journal::stamp_millis(&evidence.ts).unwrap();
        let earlier_envelope = (evidence_millis - 1).to_string();
        assert_eq!(
            validate(&store, &envelope, &earlier_envelope, &minute),
            Err(EnvelopeReject::EvidenceNotContemporaneous(
                journal::reference(&evidence)
            ))
        );
    }

    #[test]
    fn batch_claim_converts_to_and_from_fact_claim() {
        for batch in [BatchClaim::Ratification, BatchClaim::Adjudication] {
            let fact = Claim::from(batch);
            assert_eq!(BatchClaim::try_from(fact).unwrap(), batch);
            assert!(batch.matches_fact(fact));
        }
        assert!(BatchClaim::try_from(Claim::Verdict).is_err());
    }

    #[test]
    fn exact_final_assertion_boundary_accepts_before_and_during_but_rejects_after() {
        let latest = 1_784_963_459_000;
        assert!(is_contemporaneous(
            "1784963425553",
            "1784963425553",
            "2026-07-25T07:10",
            latest,
        ));
        assert!(is_contemporaneous(
            "1784963425553",
            &latest.to_string(),
            "2026-07-25T07:10",
            latest,
        ));
        assert!(!is_contemporaneous(
            "1784963425553",
            &(latest + 1).to_string(),
            "2026-07-25T07:10",
            latest,
        ));
    }

    #[test]
    fn imported_envelope_and_evidence_are_not_authority() {
        let tmp = TmpRoot::new();
        let store = Store::init(tmp.path(), Some("batch-auth"), false).unwrap();
        let evidence = journal::append(
            store.root(),
            "test_evidence",
            "finding-1",
            serde_json::json!({ "result": "reviewed" }),
        )
        .unwrap();
        let mut envelope = envelope();
        envelope.evidence = vec![journal::reference(&evidence)];
        let minute = journal::minute_key(&evidence.ts).unwrap();
        let latest = journal::stamp_millis(&evidence.ts).unwrap();

        assert_eq!(
            validate_cover(
                &store,
                &envelope,
                CoverContext {
                    envelope_ts: &evidence.ts,
                    envelope_origin: Origin::Imported,
                    subjects: &envelope.subjects,
                    claim: BatchClaim::Adjudication,
                    burst_minute: &minute,
                    latest_assertion_millis: latest,
                },
            ),
            Err(EnvelopeReject::ImportedEnvelope)
        );

        let mut imported_evidence = evidence.clone();
        imported_evidence.id = "imported-evidence".into();
        journal::restore_entries(store.root(), std::slice::from_ref(&imported_evidence)).unwrap();
        envelope.evidence = vec![format!("journal:{}", imported_evidence.id)];
        assert_eq!(
            validate_cover(
                &store,
                &envelope,
                CoverContext {
                    envelope_ts: &evidence.ts,
                    envelope_origin: Origin::Local,
                    subjects: &envelope.subjects,
                    claim: BatchClaim::Adjudication,
                    burst_minute: &minute,
                    latest_assertion_millis: latest,
                },
            ),
            Err(EnvelopeReject::ImportedEvidence(format!(
                "journal:{}",
                imported_evidence.id
            )))
        );
    }

    #[test]
    fn covering_ignores_imported_authority_but_accepts_a_local_envelope() {
        let tmp = TmpRoot::new();
        let store = Store::init(tmp.path(), Some("batch-auth"), false).unwrap();
        let subjects = vec!["finding-1".to_string()];
        let imported_evidence = Entry {
            id: "imported-proof".into(),
            ts: "1784963425000".into(),
            actor: "llm:analyzer".into(),
            event: "test_evidence".into(),
            target_id: "finding-1".into(),
            payload: serde_json::json!({}),
            origin: Origin::Local,
        };
        let mut imported_envelope = envelope();
        imported_envelope.evidence = vec![journal::reference(&imported_evidence)];
        let imported_entry = Entry {
            id: "imported-envelope".into(),
            ts: "1784963426000".into(),
            actor: "llm:analyzer".into(),
            event: EVENT.into(),
            target_id: imported_envelope.subject_digest.clone(),
            payload: serde_json::to_value(&imported_envelope).unwrap(),
            origin: Origin::Local,
        };
        journal::restore_entries(
            store.root(),
            &[imported_evidence.clone(), imported_entry.clone()],
        )
        .unwrap();
        let imported_ids = [imported_entry.id.clone()].into_iter().collect();
        assert_eq!(
            covering_envelope(
                &store,
                &subjects,
                BatchClaim::Adjudication,
                "llm:analyzer",
                "2026-07-25T07:10",
                &imported_ids,
                1_784_963_426_000,
            )
            .unwrap(),
            None
        );

        let local_evidence = journal::append(
            store.root(),
            "test_evidence",
            "finding-1",
            serde_json::json!({}),
        )
        .unwrap();
        let mut local_envelope = envelope();
        local_envelope.evidence = vec![journal::reference(&local_evidence)];
        let local_entry = append_envelope(store.root(), &local_envelope).unwrap();
        let latest = journal::stamp_millis(&local_entry.ts).unwrap();
        let minute = journal::minute_key(&local_entry.ts).unwrap();
        assert_eq!(
            covering_envelope(
                &store,
                &subjects,
                BatchClaim::Adjudication,
                "llm:analyzer",
                &minute,
                &BTreeSet::new(),
                latest,
            )
            .unwrap(),
            Some(local_entry.id)
        );
    }

    #[test]
    fn a_later_epoch_envelope_is_not_contemporaneous() {
        assert!(is_contemporaneous(
            "1784963425553",
            "1784963425553",
            "2026-07-25T07:10",
            1_784_963_425_553,
        ));
        assert!(
            !is_contemporaneous(
                "1784963425553",
                "1784963480000",
                "2026-07-25T07:10",
                1_784_963_425_553,
            ),
            "an envelope written in a later minute must fail closed"
        );
    }
}
