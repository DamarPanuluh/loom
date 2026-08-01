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

use crate::model::{Claim, NodeType, Verification};
use crate::store::Store;
use crate::Result;
use serde::Serialize;
use std::collections::BTreeMap;

/// Below this many served packets, the efficacy ratio is a coincidence with a
/// percent sign. Reported anyway — with the caveat attached, because a hidden
/// number gets estimated and an estimated one gets quoted.
pub const EFFICACY_MIN_SAMPLE: usize = 20;

/// Writes by one actor inside one minute that stop looking like judgment.
pub const BURST_THRESHOLD: usize = 10;

#[derive(Debug, Clone, Serialize)]
pub struct AuditFinding {
    pub kind: &'static str,
    pub subject: String,
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
    out.extend(bursts(store)?);
    out.extend(unanchored_settled_facts(store)?);
    if corrupt > 0 {
        out.push(AuditFinding {
            kind: "journal_corruption",
            subject: crate::journal::path(store.root()).display().to_string(),
            detail: format!(
                "{corrupt} journal line(s) failed to parse — most likely a truncated \
                 final record from an interrupted append. They are skipped, not read \
                 as evidence, so the intact history above them still counts."
            ),
            remedy: "inspect the tail of .loom/journal/events.jsonl; the append-only \
                     record above the damaged line is unaffected"
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
    let ratified_targets: std::collections::BTreeSet<&str> = entries
        .iter()
        .filter(|e| matches!(e.event.as_str(), "ratification" | "rejection"))
        .map(|e| e.target_id.as_str())
        .collect();
    let mut out = Vec::new();
    for intent in store.list_nodes(Some(NodeType::Intent), usize::MAX)? {
        let state = store.ratification(&intent.id)?;
        if state != "ratified" && state != "rejected" {
            continue;
        }
        let has_entry = ratified_targets.contains(intent.id.as_str());
        if !has_entry {
            out.push(AuditFinding {
                kind: "unjournaled_ratification",
                subject: intent.id.clone(),
                detail: format!(
                    "'{}' is {state} with no journal entry behind it — the act was never recorded",
                    intent.name
                ),
                remedy: format!(
                    "re-ratify it deliberately (`loom intent ratify {}`), or reject it",
                    &intent.id[..8.min(intent.id.len())]
                ),
            });
        }
    }
    Ok(out)
}

/// Many asserted writes by one actor inside one minute.
///
/// Statistical, and reported as such: a legitimate bulk import looks like this
/// too. What it says is "these were not individually judged", which is exactly
/// what the 30-at-19:20 cluster meant.
fn bursts(store: &Store) -> Result<Vec<AuditFinding>> {
    let mut buckets: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for fact in store.all_facts()? {
        if fact.claim != Claim::Ratification && fact.claim != Claim::Adjudication {
            continue;
        }
        // Minute precision: the timestamps are ISO-8601, so truncating at the
        // colon before seconds is the whole grouping key.
        let minute: String = fact.asserted_at.chars().take(16).collect();
        buckets
            .entry((fact.asserted_by.clone(), minute))
            .or_default()
            .push(fact.subject_id.clone());
    }
    let mut out = Vec::new();
    for ((actor, minute), subjects) in buckets {
        if subjects.len() < BURST_THRESHOLD {
            continue;
        }
        out.push(AuditFinding {
            kind: "judgment_burst",
            subject: format!("{actor}@{minute}"),
            detail: format!(
                "{} judgments by '{actor}' inside one minute ({minute}) — \
                 too fast to have been made one at a time",
                subjects.len()
            ),
            remedy: "re-open them and judge them individually, or record that this was \
                     a bulk import and what authorized it"
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
        out.push(AuditFinding {
            kind: "unanchored_claim",
            subject: fact.subject_id.clone(),
            detail: format!(
                "{} is '{}' with no surviving anchor",
                fact.claim.as_str(),
                fact.state
            ),
            remedy: "re-establish it with evidence loom can re-check, or withdraw it".into(),
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
        let iso = epoch_millis("2026-07-25T07:10:25.553Z").expect("ISO parses");
        let millis = epoch_millis("1784963425553").expect("epoch parses");
        assert_eq!(iso, millis, "the same instant in both formats");
        // And ordering survives the conversion.
        assert!(epoch_millis("2026-07-25T07:10:26.000Z") > epoch_millis("1784963425553"));
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

/// Milliseconds since the epoch, from either stamp format loom writes.
///
/// A shared clock is the precondition for comparing two planes at all, and
/// loom has two formats because the journal predates a time dependency it
/// still does not want.
fn epoch_millis(stamp: &str) -> Option<i64> {
    if let Ok(millis) = stamp.parse::<i64>() {
        return Some(millis);
    }
    // `YYYY-MM-DDTHH:MM:SS.sssZ` — parsed by hand for the same reason.
    let (date, rest) = stamp.split_once('T')?;
    let mut d = date.split('-');
    let (y, mo, da): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let time = rest.trim_end_matches('Z');
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = hms.split(':');
    let (h, mi, sec): (i64, i64, i64) = (
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
    );
    // Days since the epoch via a civil-date conversion (Howard Hinnant's), so
    // month lengths and leap years are handled rather than approximated.
    let y_adj = if mo <= 2 { y - 1 } else { y };
    let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
    let yoe = y_adj - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let millis: i64 = frac
        .chars()
        .chain(std::iter::repeat('0'))
        .take(3)
        .collect::<String>()
        .parse()
        .unwrap_or(0);
    Some(((days * 86_400 + h * 3600 + mi * 60 + sec) * 1000) + millis)
}

pub fn efficacy(store: &Store) -> Result<Efficacy> {
    // When each subject first reached a re-checkable state.
    let mut settled_at: BTreeMap<String, String> = BTreeMap::new();
    for fact in store.all_facts()? {
        if !fact.verification.counts() {
            continue;
        }
        let at = fact.asserted_at.clone();
        settled_at
            .entry(fact.subject_id.clone())
            .and_modify(|e| {
                if at < *e {
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
                        if at < e {
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
                .and_then(|at| epoch_millis(at))
                .zip(epoch_millis(&entry.ts))
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
