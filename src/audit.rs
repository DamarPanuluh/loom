//! Audit — does this graph's own record look like it was earned?
//!
//! Plane: statistical detection over asserted facts and the journal. Unlike the
//! advisory debt feed (INV-3), these findings are not merely reported: the
//! `sound` rung counts them (via `audit_subjects`), so an open audit finding
//! gates the rung until it is triaged to a settling verdict. They route through
//! ordinary triage like any other finding.
//!
//! Contract — **built from the incident, then run on ourselves.** Every check
//! below is a signature loom's own graph carried:
//!
//! - 30 ratifications sharing one journal minute, 9 more paced 25–40 seconds
//!   apart. Nobody reads and judges 30 behaviors in a minute.
//! - 39 of 51 ratifications with no journal entry behind them at all — the
//!   facet was written directly, past the gate that was supposed to be the
//!   only way in.
//! - 54 of 59 proofs whose "passing" verdict cited prose about a run that loom
//!   never performed.
//!
//! The point is not that these are now impossible — the evidence spine makes
//! most of them impossible going forward. The point is that a graph can be
//! IMPORTED, or carried forward, or written by a version of loom without these
//! guards, and a tool whose whole claim is falsifiability has to be able to
//! turn that claim on its own records.

use crate::model::{Claim, NodeType, TargetKind, Verification};
use crate::store::Store;
use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Below this many served packets, the efficacy ratio is a coincidence with a
/// percent sign. Reported anyway — with the caveat attached, because a hidden
/// number gets estimated and an estimated one gets quoted.
pub const EFFICACY_MIN_SAMPLE: usize = 20;

/// Writes by one actor inside one minute that stop looking like judgment.
pub const BURST_THRESHOLD: usize = 10;

/// Append-only journal event for a human's disposition of a historical audit
/// incident. This is deliberately not a batch authorization event: accepting
/// history does not authorize, ratify, or relabel the underlying judgments.
pub const INCIDENT_EVENT: &str = "audit_incident_disposition";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditIncidentDisposition {
    AcceptedHistoricalIncident,
}

/// Exact, human-gated disclosure of one historical judgment burst.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditIncident {
    pub schema_version: u32,
    pub kind: String,
    pub subject: String,
    pub actor: String,
    pub minute: String,
    pub claim: crate::batch_auth::BatchClaim,
    pub subjects: Vec<String>,
    pub subject_digest: String,
    pub incident_digest: String,
    pub disposition: AuditIncidentDisposition,
    pub reason: String,
    pub human_decision: crate::ratification::HumanDecision,
}

impl AuditIncident {
    pub fn accept(
        bucket: &JudgmentBurstBucket,
        reason: impl Into<String>,
        human_decision: crate::ratification::HumanDecision,
    ) -> Result<Self> {
        let reason = reason.into();
        if crate::model::is_placeholder(&reason) {
            anyhow::bail!("incident acceptance requires a substantive --reason");
        }
        let mut subjects = bucket.subjects.clone();
        subjects.sort();
        subjects.dedup();
        let subject_digest = crate::batch_auth::subject_digest(&subjects);
        let incident_digest = judgment_burst_incident_digest(
            &bucket.actor,
            &bucket.minute,
            bucket.claim,
            &subject_digest,
        );
        Ok(Self {
            schema_version: 1,
            kind: "judgment_burst".into(),
            subject: bucket.id(),
            actor: bucket.actor.clone(),
            minute: bucket.minute.clone(),
            claim: bucket.claim,
            subjects,
            subject_digest,
            incident_digest,
            disposition: AuditIncidentDisposition::AcceptedHistoricalIncident,
            reason,
            human_decision,
        })
    }

    pub fn digest_holds(&self) -> bool {
        self.schema_version == 1
            && self.kind == "judgment_burst"
            && self.subject == format!("{}@{}", self.actor, self.minute)
            && crate::batch_auth::subject_digest(&self.subjects) == self.subject_digest
            && judgment_burst_incident_digest(
                &self.actor,
                &self.minute,
                self.claim,
                &self.subject_digest,
            ) == self.incident_digest
    }

    pub fn matches(&self, bucket: &JudgmentBurstBucket) -> bool {
        let mut subjects = bucket.subjects.clone();
        subjects.sort();
        subjects.dedup();
        self.digest_holds()
            && self.actor == bucket.actor
            && self.minute == bucket.minute
            && self.claim == bucket.claim
            && self.subjects == subjects
    }
}

fn judgment_burst_incident_digest(
    actor: &str,
    minute: &str,
    claim: crate::batch_auth::BatchClaim,
    subject_digest: &str,
) -> String {
    format!(
        "i{}",
        crate::store::fnv_hex_digest(&[
            "judgment_burst",
            actor,
            minute,
            claim.as_str(),
            subject_digest,
        ])
    )
}

pub fn parse_incident_entry(entry: &crate::journal::Entry) -> Result<Option<AuditIncident>> {
    if entry.event != INCIDENT_EVENT {
        return Ok(None);
    }
    let incident: AuditIncident = serde_json::from_value(entry.payload.clone())?;
    if entry.target_id != incident.incident_digest || !incident.digest_holds() {
        anyhow::bail!("audit incident {} has a corrupt digest binding", entry.id);
    }
    Ok(Some(incident))
}

/// Every well-formed disclosure, including imported history. Callers decide
/// whether local authority is required for a particular use.
pub fn incident_entries(store: &Store) -> Result<Vec<(crate::journal::Entry, AuditIncident)>> {
    let mut out = Vec::new();
    for entry in crate::journal::read(store.root())? {
        if let Some(incident) = parse_incident_entry(&entry)? {
            out.push((entry, incident));
        }
    }
    Ok(out)
}

fn locally_accepted(bucket: &JudgmentBurstBucket, entries: &[crate::journal::Entry]) -> bool {
    entries.iter().any(|entry| {
        entry.origin == crate::journal::Origin::Local
            && parse_incident_entry(entry)
                .ok()
                .flatten()
                .is_some_and(|incident| incident.matches(bucket))
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgmentBurstBucket {
    pub actor: String,
    /// Worker profiles observed under this authorization identity. Empty means
    /// the facts predate profile capture or the executor did not declare one.
    pub profiles: BTreeSet<String>,
    pub minute: String,
    pub claim: crate::batch_auth::BatchClaim,
    pub subjects: Vec<String>,
    pub batch_ids: BTreeSet<String>,
    /// Exact latest valid fact assertion in this actor/minute/claim bucket.
    pub latest_assertion_millis: i64,
}

impl JudgmentBurstBucket {
    pub fn id(&self) -> String {
        format!("{}@{}", self.actor, self.minute)
    }

    pub fn for_key(
        store: &Store,
        actor: &str,
        minute: &str,
        claim: crate::batch_auth::BatchClaim,
    ) -> Result<Option<Self>> {
        let Some(minute) = normalized_minute(minute) else {
            return Ok(None);
        };
        Ok(Self::group(store)?.into_iter().find(|bucket| {
            bucket.actor == actor && bucket.minute == minute && bucket.claim == claim
        }))
    }

    pub fn group(store: &Store) -> Result<Vec<Self>> {
        group_judgment_facts(store.all_facts()?, |subject_id| {
            Ok(store.get_node(subject_id)?.is_some())
        })
    }
}

type JudgmentBurstKey = (String, String, crate::batch_auth::BatchClaim);
type JudgmentBurstState = (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>, i64);

fn group_judgment_facts<F>(
    facts: Vec<crate::evidence::Fact>,
    mut is_live: F,
) -> Result<Vec<JudgmentBurstBucket>>
where
    F: FnMut(&str) -> Result<bool>,
{
    let mut buckets: BTreeMap<JudgmentBurstKey, JudgmentBurstState> = BTreeMap::new();
    for fact in facts {
        let Ok(claim) = crate::batch_auth::BatchClaim::try_from(fact.claim) else {
            continue;
        };
        if !is_live(&fact.subject_id)? {
            continue;
        }
        let Some(asserted_millis) = crate::journal::stamp_millis(&fact.asserted_at) else {
            continue;
        };
        let Some(minute) = crate::journal::minute_key(&fact.asserted_at) else {
            continue;
        };
        let (subjects, profiles, batch_ids, latest_assertion_millis) = buckets
            .entry((fact.asserted_by, minute, claim))
            .or_insert_with(|| (BTreeSet::new(), BTreeSet::new(), BTreeSet::new(), i64::MIN));
        *latest_assertion_millis = (*latest_assertion_millis).max(asserted_millis);
        subjects.insert(fact.subject_id);
        if let Some(profile) = fact.asserted_profile {
            profiles.insert(profile);
        }
        if !fact.batch_id.is_empty() {
            batch_ids.insert(fact.batch_id);
        }
    }
    Ok(buckets
        .into_iter()
        .map(
            |((actor, minute, claim), (subjects, profiles, batch_ids, latest_assertion_millis))| {
                JudgmentBurstBucket {
                    actor,
                    profiles,
                    minute,
                    claim,
                    subjects: subjects.into_iter().collect(),
                    batch_ids,
                    latest_assertion_millis,
                }
            },
        )
        .collect())
}

fn normalized_minute(stamp_or_minute: &str) -> Option<String> {
    crate::journal::minute_key(stamp_or_minute)
        .or_else(|| crate::journal::minute_key(&format!("{stamp_or_minute}:00.000Z")))
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum AuditSubject {
    Node(String),
    Edge(String),
    Graph(String),
}

impl AuditSubject {
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Node(id) | Self::Edge(id) => Some(id),
            Self::Graph(_) => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditFinding {
    pub kind: &'static str,
    pub subject: AuditSubject,
    pub detail: String,
    /// What to do — every finding names its own remedy, because an audit that
    /// only accuses is a scoreboard.
    pub remedy: String,
}

/// Every self-fabrication signature in this graph.
pub fn run(store: &Store) -> Result<Vec<AuditFinding>> {
    let mut out = Vec::new();
    // Read the journal ONCE: unjournaled_ratifications used to re-parse the whole
    // file per ratified Intent, and audit::run is on the `loom status` path.
    let (entries, corrupt) = crate::journal::read_counting(store.root())?;
    out.extend(unjournaled_ratifications(store, &entries)?);
    out.extend(malformed_judgment_timestamps(store)?);
    out.extend(bursts(store, &entries)?);
    out.extend(unanchored_settled_facts(store)?);
    if corrupt > 0 {
        out.push(AuditFinding {
            kind: "journal_corruption",
            subject: AuditSubject::Graph(crate::journal::path(store.root()).display().to_string()),
            detail: format!(
                "{corrupt} journal line(s) failed to parse — most likely a truncated \
                 final record from an interrupted append. They are skipped, not read \
                 as evidence, so the intact history above them still counts."
            ),
            remedy: "inspect the tail of .loom/journal/events.jsonl and repair the damaged \
                     final line (the append-only record above it is unaffected); verify with \
                     loom audit --json — the finding must be absent"
                .into(),
        });
    }
    out.sort_by(|a, b| a.kind.cmp(b.kind).then(a.subject.cmp(&b.subject)));
    Ok(out)
}

/// A `ratified` fact with no journal entry behind it.
///
/// loom writes the entry BEFORE stamping the fact, so on a graph this version
/// produced the invariant holds by construction. A violation therefore means
/// one of two things, and both are worth knowing: the record predates the
/// spine, or something wrote past the boundary.
fn unjournaled_ratifications(
    store: &Store,
    entries: &[crate::journal::Entry],
) -> Result<Vec<AuditFinding>> {
    let mut out = Vec::new();
    for node_type in [NodeType::Intent, NodeType::Pattern] {
        for node in store.list_nodes(Some(node_type), usize::MAX)? {
            let Some(view) = store.fact(
                &crate::store::Subject::Node(node.id.clone()),
                Claim::Ratification,
            )?
            else {
                continue;
            };
            let state = view.fact.state;
            if state != "ratified" && state != "rejected" {
                continue;
            }
            let standing = store.ratification_with_journal(&node.id, entries)? == state;
            if !standing {
                let command = if node_type == NodeType::Pattern {
                    "pattern"
                } else {
                    "intent"
                };
                out.push(AuditFinding {
                    kind: "unjournaled_ratification",
                    subject: AuditSubject::Node(node.id.clone()),
                    detail: format!(
                        "'{}' is recorded as {state}, but lacks standing human authority and matching live journal evidence",
                        node.name
                    ),
                    remedy: format!(
                        "re-ratify it deliberately (`loom {command} ratify {}`)",
                        crate::model::short(&node.id)
                    ),
                });
            }
        }
    }
    Ok(out)
}

/// Judgment facts with timestamps that cannot participate in time-based audit.
///
/// These records may predate the strict import boundary or have been written
/// directly. They must be visible as corruption rather than merely omitted from
/// burst grouping.
fn malformed_judgment_timestamps(store: &Store) -> Result<Vec<AuditFinding>> {
    Ok(malformed_judgment_timestamp_findings(store.all_facts()?))
}

fn malformed_judgment_timestamp_findings(facts: Vec<crate::evidence::Fact>) -> Vec<AuditFinding> {
    let mut out = Vec::new();
    for fact in facts {
        let Ok(claim) = crate::batch_auth::BatchClaim::try_from(fact.claim) else {
            continue;
        };
        if crate::journal::stamp_millis(&fact.asserted_at).is_some() {
            continue;
        }
        let command = match claim {
            crate::batch_auth::BatchClaim::Ratification => "ratify or reject",
            crate::batch_auth::BatchClaim::Adjudication => "record its finding verdict",
        };
        out.push(AuditFinding {
            kind: "malformed_judgment_timestamp",
            subject: match fact.subject_kind {
                TargetKind::Node => AuditSubject::Node(fact.subject_id.clone()),
                TargetKind::Edge => AuditSubject::Edge(fact.subject_id.clone()),
            },
            detail: format!(
                "stored {} fact '{}' has invalid asserted_at timestamp {:?}; it cannot be grouped or time-audited",
                claim.as_str(),
                fact.id,
                fact.asserted_at
            ),
            remedy: format!(
                "re-record this judgment through the typed command ({command}) so loom stamps a canonical timestamp; then remove the malformed legacy fact only after preserving a backup"
            ),
        });
    }
    out.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.detail.cmp(&b.detail)));
    out
}

/// Many asserted writes by one actor inside one minute.
///
/// Statistical, and reported as such: unexplained judgment compression looks
/// like this. One sealed batch authorization envelope, or a union of trusted
/// human-gated sub-batch envelopes, covering exactly those subjects is batch
/// truth, not an exemption — the facts retain
/// `decision_mode=batch` and the burst is not reported.
fn bursts(store: &Store, entries: &[crate::journal::Entry]) -> Result<Vec<AuditFinding>> {
    let mut out = Vec::new();
    for bucket in JudgmentBurstBucket::group(store)? {
        if bucket.subjects.len() < BURST_THRESHOLD {
            continue;
        }
        if crate::batch_auth::covering_envelopes(
            store,
            &bucket.subjects,
            bucket.claim,
            &bucket.actor,
            &bucket.minute,
            &bucket.batch_ids,
            bucket.latest_assertion_millis,
        )?
        .is_some()
        {
            continue;
        }
        if locally_accepted(&bucket, entries) {
            continue;
        }
        out.push(AuditFinding {
            kind: "judgment_burst",
            subject: AuditSubject::Graph(bucket.id()),
            detail: format!(
                "{} {} judgments by '{}'{} inside one minute ({}) — \
                 too fast to have been made one at a time, and no sealed \
                 batch authorization set covers this exact subject union",
                bucket.subjects.len(),
                bucket.claim.as_str(),
                bucket.actor,
                if bucket.profiles.is_empty() {
                    " (executor profile unreported)".to_string()
                } else {
                    format!(
                        " (executor profile{}) {}",
                        if bucket.profiles.len() == 1 { "" } else { "s" },
                        bucket.profiles.iter().cloned().collect::<Vec<_>>().join(", ")
                    )
                },
                bucket.minute,
            ),
            remedy: "this burst can only be closed retrospectively by a HUMAN vouching over a trusted, digest-bound authorization record that predates the burst's final fact — a `batch_intent` journal record written by the human-gated batch path (`loom intent ratify --all`), carrying a recorded HumanDecision for this exact subject set (`loom audit attest-burst --authority human:<name> --evidence journal:<id>`). A self-asserted human authority citing an unrelated, machine-written, or forged record is never accepted, and adjudication bursts have no trusted human-gated batch record today. Re-judging the subjects is NOT a remedy: re-asserting the same judgment is a no-op, while changed re-judgments simply move the facts to the current minute (re-detecting the same burst when done in one pass) or — spread across separate minutes — would overwrite the original asserted_at/criterion/evidence, laundering exactly what this audit exists to surface. If no trusted bound record exists, accept it as a documented incident. For future bulk work, seal the batch authorization BEFORE the first write — `loom apply` adjudications and `loom intent ratify --all` do this automatically; the burst actor's own after-the-fact seal is never accepted"
                .into(),
        });
    }
    Ok(out)
}

/// A settled fact standing on nothing re-checkable.
///
/// The spine refuses these at write time now, so a hit means the fact arrived
/// some other way: an import, a carry-forward, or a graph written before the
/// floors existed.
fn unanchored_settled_facts(store: &Store) -> Result<Vec<AuditFinding>> {
    let mut out = Vec::new();
    for fact in store.all_facts()? {
        if !crate::anchor::is_settling(&fact.state) {
            continue;
        }
        if fact.verification != Verification::Expired {
            continue;
        }
        // A verdict whose edge has already been reopened is ordinary routed
        // remeasurement work, not an audit violation. The fact deliberately
        // retains its last observed state as history while the edge status is
        // the live claim. Counting both made one sync turn every stale edge
        // into a second Audit item and drowned genuine unanchored records.
        if fact.claim == Claim::Verdict
            && fact.subject_kind == TargetKind::Edge
            && store.get_edge(&fact.subject_id)?.is_some_and(|edge| {
                matches!(
                    edge.status,
                    crate::model::InspectionStatus::NeedsReverification
                        | crate::model::InspectionStatus::Uninspected
                )
            })
        {
            continue;
        }
        // The one documented dormant reference: an adjudication on a
        // deterministic derived Finding while sync has temporarily wiped that
        // Finding. Its evidence is still present and the same id reattaches on
        // rematerialization; absence is not a broken anchor.
        if fact.claim == Claim::Adjudication
            && fact.subject_kind == TargetKind::Node
            && store.get_node(&fact.subject_id)?.is_none()
            && crate::store::is_derived_node_id(&fact.subject_id)
        {
            continue;
        }
        out.push(AuditFinding {
            kind: "unanchored_claim",
            subject: match fact.subject_kind {
                TargetKind::Node => AuditSubject::Node(fact.subject_id.clone()),
                TargetKind::Edge => AuditSubject::Edge(fact.subject_id.clone()),
            },
            detail: format!(
                "{} is '{}' with no surviving anchor",
                fact.claim.as_str(),
                fact.state
            ),
            remedy: {
                let id8 = crate::model::short(&fact.subject_id);
                format!(
                    "re-record the claim through its typed verdict command with evidence loom \
                     can re-check (loom edge verdict {id8} <ground|issue|independent> \
                     --evidence '…' for edges; loom finding verdict {id8} <justified|rejected|…> \
                     --reason '…' --evidence '…' for findings), or withdraw it \
                     (loom edge remove {id8} --reason '…')"
                )
            },
        });
    }
    Ok(out)
}

/// The single actionable read path shared by status and the audit queue.
/// Self-audit findings lead because an unearned record outranks structural
/// cleanup. Doctor issues and unresolved smells follow in stable order.
pub fn backlog(store: &Store) -> Result<Vec<AuditFinding>> {
    let mut out = run(store)?;
    for finding in &mut out {
        let orphan = match &finding.subject {
            AuditSubject::Node(id) if store.get_node(id)?.is_none() => Some(id.clone()),
            AuditSubject::Edge(id) if store.get_edge(id)?.is_none() => Some(id.clone()),
            _ => None,
        };
        if let Some(id) = orphan {
            finding.detail = format!("{} (orphan audit subject: {id})", finding.detail);
            finding.subject = AuditSubject::Graph(id);
        }
    }
    for issue in crate::signal::doctor(store)? {
        let subject = issue
            .message
            .split_whitespace()
            .find(|token| token.len() == 32 && token.chars().all(|c| c.is_ascii_hexdigit()))
            .map(|id| {
                if store.get_node(id).ok().flatten().is_some() {
                    AuditSubject::Node(id.to_string())
                } else if store.get_edge(id).ok().flatten().is_some() {
                    AuditSubject::Edge(id.to_string())
                } else {
                    AuditSubject::Graph(issue.kind.clone())
                }
            })
            .unwrap_or_else(|| AuditSubject::Graph(issue.kind.clone()));
        out.push(AuditFinding {
            kind: "doctor_issue",
            subject,
            detail: issue.message,
            remedy: format!("`loom doctor` reports: {}", issue.kind),
        });
    }
    for smell in crate::signal::smells(store)? {
        if crate::signal::smell_has_resolving_adjudication(store, &smell.identity)? {
            continue;
        }
        let id = smell.identity.rsplit_once(':').map(|(_, id)| id);
        let subject = match id {
            Some(id) if store.get_node(id)?.is_some() => AuditSubject::Node(id.to_string()),
            Some(id) if store.get_edge(id)?.is_some() => AuditSubject::Edge(id.to_string()),
            _ => AuditSubject::Graph(smell.identity.clone()),
        };
        out.push(AuditFinding {
            kind: "smell",
            subject,
            detail: smell.message,
            remedy: smell.remedy,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two planes stamp time differently, so the comparison has to
    /// normalize. Getting this wrong reported 100% efficacy for every graph.
    #[test]
    fn both_timestamp_formats_land_on_one_clock() {
        let iso = crate::journal::stamp_millis("2026-07-25T07:10:25.553Z").expect("ISO parses");
        let millis = crate::journal::stamp_millis("1784963425553").expect("epoch parses");
        assert_eq!(iso, millis, "the same instant in both formats");
        // And ordering survives the conversion.
        assert!(
            crate::journal::stamp_millis("2026-07-25T07:10:26.000Z")
                > crate::journal::stamp_millis("1784963425553")
        );
    }

    #[test]
    fn burst_grouping_normalizes_time_filters_dead_subjects_and_sorts_membership() {
        let fact = |subject: &str, asserted_at: &str, batch_id: &str| crate::evidence::Fact {
            id: format!("fact-{subject}"),
            subject_kind: TargetKind::Node,
            subject_id: subject.into(),
            claim: Claim::Adjudication,
            state: "justified".into(),
            criterion: String::new(),
            verification: Verification::Cited,
            confidence: 1.0,
            asserted_by: "llm:analyzer".into(),
            asserted_profile: Some("loom-auditor".into()),
            asserted_at: asserted_at.into(),
            decision_mode: crate::model::DecisionMode::Batch,
            batch_id: batch_id.into(),
            stale: None,
        };
        let buckets = group_judgment_facts(
            vec![
                fact("b", "1784963425553", "batch-2"),
                fact("dead", "2026-07-25T07:10:01.000Z", "batch-dead"),
                fact("a", "2026-07-25T07:10:59.000Z", "batch-1"),
            ],
            |subject| Ok(subject != "dead"),
        )
        .unwrap();
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].minute, "2026-07-25T07:10");
        assert_eq!(
            buckets[0].profiles.iter().cloned().collect::<Vec<_>>(),
            vec!["loom-auditor"]
        );
        assert_eq!(buckets[0].subjects, vec!["a", "b"]);
        assert_eq!(
            buckets[0].batch_ids.iter().cloned().collect::<Vec<_>>(),
            vec!["batch-1", "batch-2"]
        );
    }

    #[test]
    fn malformed_judgment_timestamps_are_reported_without_disturbing_valid_groups() {
        let fact = |subject: &str, claim: Claim, asserted_at: &str| crate::evidence::Fact {
            id: format!("fact-{subject}"),
            subject_kind: TargetKind::Node,
            subject_id: subject.into(),
            claim,
            state: "justified".into(),
            criterion: String::new(),
            verification: Verification::Cited,
            confidence: 1.0,
            asserted_by: "llm:analyzer".into(),
            asserted_profile: None,
            asserted_at: asserted_at.into(),
            decision_mode: crate::model::DecisionMode::Individual,
            batch_id: String::new(),
            stale: None,
        };
        let facts = vec![
            fact("valid-b", Claim::Adjudication, "1784963425553"),
            fact("bad-z", Claim::Ratification, "2026-07-25T07:10:25"),
            fact("bad-day", Claim::Adjudication, "2026-02-29T07:10:25Z"),
            fact("valid-a", Claim::Adjudication, "2026-07-25T07:10:59Z"),
            fact("not-judgment", Claim::Verdict, "also-bad"),
        ];

        let findings = malformed_judgment_timestamp_findings(facts.clone());
        assert_eq!(findings.len(), 2);
        assert_eq!(
            findings
                .iter()
                .map(|finding| finding.subject.id().unwrap())
                .collect::<Vec<_>>(),
            vec!["bad-day", "bad-z"]
        );
        assert!(findings[0].detail.contains("2026-02-29T07:10:25Z"));
        assert!(findings[1].detail.contains("2026-07-25T07:10:25"));
        assert!(findings.iter().all(|finding| {
            finding.kind == "malformed_judgment_timestamp"
                && finding.remedy.contains("typed command")
        }));

        let buckets = group_judgment_facts(facts, |_| Ok(true)).unwrap();
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].subjects, vec!["valid-a", "valid-b"]);
        assert_eq!(buckets[0].minute, "2026-07-25T07:10");
        assert_eq!(buckets[0].latest_assertion_millis, 1_784_963_459_000);
    }

    #[test]
    fn burst_bucket_identity_and_membership_are_deterministic() {
        let bucket = JudgmentBurstBucket {
            actor: "llm:analyzer".into(),
            profiles: ["loom-auditor".to_string()].into_iter().collect(),
            minute: "2026-07-25T07:10".into(),
            claim: crate::batch_auth::BatchClaim::Adjudication,
            subjects: vec!["a".into(), "b".into()],
            batch_ids: ["batch-1".to_string()].into_iter().collect(),
            latest_assertion_millis: 1_784_963_459_000,
        };
        assert_eq!(bucket.id(), "llm:analyzer@2026-07-25T07:10");
        assert_eq!(bucket.subjects, vec!["a", "b"]);
        assert_eq!(
            bucket.batch_ids.into_iter().collect::<Vec<_>>(),
            vec!["batch-1"]
        );
    }

    #[test]
    fn the_burst_threshold_is_about_reading_speed() {
        // Ten judgments in sixty seconds is six seconds each, including
        // reading the behavior and its evidence. The number is a claim about
        // humans, not about the database.
        assert_eq!(BURST_THRESHOLD, 10);
    }
}

/// Did loom's context actually help?
///
/// The ratio of served packets whose target subsequently acquired a fact loom
/// could re-check. Derived from the append-only record on both sides: the
/// `packet_served` entries say what was handed out and when, and the fact table
/// says what was established afterwards.
///
/// Deliberately NOT self-reported. The obvious design asks the writer to cite
/// the packet it used, which is a claim about its own usefulness made by the
/// party with an interest in it — the same shape as an agent reporting that its
/// proof passed. Correlating timestamps is weaker evidence and honest evidence.
///
/// STATISTICAL: reported, never gated (INV-3). A low ratio can mean the packets
/// were unhelpful, or that the work they enabled has not landed yet.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Efficacy {
    pub served: usize,
    /// Packets whose target later gained a fact at `cited` or better.
    pub converted: usize,
    pub ratio: f64,
    /// The same split by packet kind, so `next` and `context` can be told apart.
    pub by_kind: BTreeMap<String, (usize, usize)>,
}

pub fn efficacy(store: &Store) -> Result<Efficacy> {
    // When each subject most recently reached a re-checkable state.
    //
    // Earliest-wins discarded re-verification: a packet about an already-
    // established target never converted even when later qualifying work
    // landed. Latest-wins credits any post-serve settle (still statistical —
    // timestamp correlation, not packet citation).
    let mut settled_at: BTreeMap<String, String> = BTreeMap::new();
    for fact in store.all_facts()? {
        if !fact.verification.counts() {
            continue;
        }
        let at = fact.asserted_at.clone();
        settled_at
            .entry(fact.subject_id.clone())
            .and_modify(|e| {
                if at > *e {
                    *e = at.clone();
                }
            })
            .or_insert(at);
    }
    // An edge's fact is about the edge; a packet is usually about a node. Map
    // each edge's endpoints to the edge's settle time so a packet about an
    // intent counts when that intent's grounding was established.
    let mut node_settled: BTreeMap<String, String> = settled_at.clone();
    for (subject, at) in &settled_at {
        if let Ok(Some(edge)) = store.get_edge(subject) {
            for endpoint in [edge.from_id, edge.to_id] {
                node_settled
                    .entry(endpoint)
                    .and_modify(|e| {
                        if at > e {
                            *e = at.clone();
                        }
                    })
                    .or_insert(at.clone());
            }
        }
    }

    let mut out = Efficacy::default();
    for entry in crate::journal::read(store.root())? {
        if entry.event != "packet_served" {
            continue;
        }
        let Some(packets) = entry.payload.get("packets").and_then(|v| v.as_array()) else {
            continue;
        };
        for p in packets {
            let kind = p
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let target = p.get("target").and_then(|v| v.as_str()).unwrap_or("");
            out.served += 1;
            let slot = out.by_kind.entry(kind).or_insert((0, 0));
            slot.0 += 1;
            // Settled AFTER this packet was served. Work that was already done
            // is not work the packet enabled.
            // Normalize both sides before comparing. The journal stamps UTC
            // epoch milliseconds; the fact table stamps ISO-8601 from SQLite.
            // Comparing them as strings is nonsense that happens to look like
            // an answer — "2026-…" sorts above "1784…" for every fact, which
            // would have reported 100% efficacy forever.
            if node_settled
                .get(target)
                .and_then(|at| crate::journal::stamp_millis(at))
                .zip(crate::journal::stamp_millis(&entry.ts))
                .map(|(settled, served)| settled > served)
                .unwrap_or(false)
            {
                out.converted += 1;
                slot.1 += 1;
            }
        }
    }
    out.ratio = if out.served == 0 {
        0.0
    } else {
        out.converted as f64 / out.served as f64
    };
    Ok(out)
}
