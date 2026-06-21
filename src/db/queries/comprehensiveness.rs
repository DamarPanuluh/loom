//! Comprehensiveness — the COVERAGE half of "production ready" (the QUALITY half
//! is `fully_proven`). Does the intent graph CAPTURE everything the code should
//! do? Comprehensiveness can't be a pure count of what's IN the graph; loom ships
//! a CANONICAL rubric of dimensions (`rubric_teaching`) and the LLM instantiates
//! it per repo. The five dimensions:
//!
//! - **entrypoint** (mechanical): every externally-public symbol is grounded /
//!   accepted / adjudicated. Reuses symbol_accountability (its `required` is the
//!   honest denominator) and GATES `fully_proven` (G7), computed from the snapshot.
//! - **boundary** (mechanical, DISK scan): every file importing an external
//!   service has a boundary intent. Needs raw imports the snapshot doesn't keep,
//!   so it lives in `loom complete` (`boundary_scan_from_disk`), not a badge gate.
//! - **invariant**: every coded intent measured — already `Coverage360.measured_pairs`.
//! - **journey** (cognitive ledger): every `user_visible` leaf owes a passing saga.
//! - **behavioral** (cognitive ledger): every realized `happy` leaf owes a realized
//!   sad/fallback/edge_case sibling.
//!
//! The crux honesty law across the cognitive dimensions: RECORD ≠ DISCHARGE — a
//! recorded-but-unfulfilled placeholder is binding debt, never a satisfaction.

use std::collections::HashMap;
use std::path::Path;

use crate::db::queries::stats::CoverageAxis;
use crate::db::queries::symbol_accountability::SymbolAccountabilityReport;
use crate::db::queries::QuerySnapshot;

/// True if a path is a DOCUMENTATION artifact (a spec/contract), not code.
pub(crate) fn is_doc_file(path: &str) -> bool {
    matches!(
        path.rsplit('.')
            .next()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("md" | "markdown" | "mdx" | "rst" | "adoc" | "asciidoc" | "txt" | "org")
    )
}

/// Intents marked `lifecycle=implemented` whose groundings are ALL documentation
/// files — a SPEC marked as BUILT. The doc is the CONTRACT (what the system must
/// match); the realization is real code. loom pushes the LLM to either build the
/// code (and reground), or drop these to `lifecycle=planned`. This is the
/// generalization of "mockup is contract, not realization" to docs/specs, and the
/// guard against a docs-repo masquerading as an implemented system (the pulse case).
pub fn doc_only_realizations(snapshot: &QuerySnapshot) -> Vec<String> {
    use std::collections::HashMap;
    // intent_id -> (has any grounding, all groundings are docs)
    let mut g: HashMap<&str, (bool, bool)> = HashMap::new();
    for im in &snapshot.implements {
        let e = g.entry(im.intent_id.as_str()).or_insert((false, true));
        e.0 = true;
        e.1 &= is_doc_file(&im.codefile_path);
    }
    let mut out: Vec<String> = snapshot
        .intents
        .iter()
        .filter(|i| i.lifecycle == "implemented")
        .filter(|i| {
            g.get(i.id.as_str())
                .is_some_and(|(has, all_doc)| *has && *all_doc)
        })
        .map(|i| i.name.clone())
        .collect();
    out.sort();
    out
}

/// Entrypoint coverage from an already-computed symbol-accountability report:
/// `required` public symbols minus the still-`actionable_gaps`. Denominator-honest
/// — a public symbol can't leave the denominator by being un-owned.
pub fn entrypoint_coverage(report: &SymbolAccountabilityReport) -> CoverageAxis {
    let required = report.summary.required as i64;
    let gaps = report.summary.actionable_gaps as i64;
    CoverageAxis {
        covered: (required - gaps).max(0),
        total: required,
    }
}

/// A cognitive-dimension ledger: how many of the ENUMERATED items are DISCHARGED.
/// The crux honesty invariant — RECORD ≠ DISCHARGE — lives here: an item is
/// discharged only by WORKING graph state (a realized sibling, a passing saga),
/// never by a recorded-but-unfulfilled placeholder. `owed` names the open gaps.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    pub enumerated: usize,
    pub discharged: usize,
    pub owed: Vec<String>,
}

/// JOURNEY coverage (cognitive): every `user_visible` leaf intent owes an
/// end-to-end SAGA that actually RAN passing (discriminating). The LLM's cognitive
/// job — deciding which flows are journeys — is encoded by `visibility=user_visible`;
/// loom mechanically checks the discharge. A `saga add` that never ran is
/// enumerated-but-owed, never discharged.
pub fn journey_ledger_from_snapshot(snapshot: &QuerySnapshot) -> Ledger {
    let parents: std::collections::HashSet<&str> =
        snapshot.hierarchy.iter().map(|(p, _)| p.as_str()).collect();
    // intent_id -> does it have a PASSING discriminating saga?
    let saga_by_id: HashMap<&str, ()> = snapshot
        .validations
        .iter()
        .filter(|v| {
            v.validation_type == "saga"
                && v.last_result == "passed"
                && v.discrimination_status == "discriminating"
        })
        .map(|v| (v.id.as_str(), ()))
        .collect();
    let mut discharged_intents: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for ve in &snapshot.validates {
        if saga_by_id.contains_key(ve.validation_id.as_str()) {
            discharged_intents.insert(ve.intent_id.as_str());
        }
    }
    let mut ledger = Ledger::default();
    for i in &snapshot.intents {
        let is_leaf = !parents.contains(i.id.as_str());
        if is_leaf && i.visibility == "user_visible" {
            ledger.enumerated += 1;
            if discharged_intents.contains(i.id.as_str()) {
                ledger.discharged += 1;
            } else {
                ledger.owed.push(i.name.clone());
            }
        }
    }
    ledger
}

/// BEHAVIORAL coverage (cognitive): every realized `aspect=happy` leaf owes a
/// sibling that covers a non-happy aspect (sad / fallback / edge_case) — and that
/// sibling must itself be a realized implemented leaf (RECORD ≠ DISCHARGE: a
/// `planned` sad-path is owed, not discharged). Encodes "you haven't designed the
/// feature until you've designed its failure".
pub fn behavioral_ledger_from_snapshot(snapshot: &QuerySnapshot) -> Ledger {
    let non_happy = |a: &str| matches!(a, "sad" | "fallback" | "edge_case");
    let parent_of: HashMap<&str, &str> = snapshot
        .hierarchy
        .iter()
        .map(|(p, c)| (c.as_str(), p.as_str()))
        .collect();
    let parents: std::collections::HashSet<&str> =
        snapshot.hierarchy.iter().map(|(p, _)| p.as_str()).collect();
    let realized: std::collections::HashSet<&str> = snapshot
        .with_current_code
        .iter()
        .map(|s| s.as_str())
        .collect();
    // parent -> set of non-happy realized child aspects present.
    let mut parent_has_nonhappy: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for i in &snapshot.intents {
        if non_happy(&i.aspect) && i.lifecycle == "implemented" && realized.contains(i.id.as_str())
        {
            if let Some(p) = parent_of.get(i.id.as_str()) {
                parent_has_nonhappy.insert(p);
            }
        }
    }
    let mut ledger = Ledger::default();
    for i in &snapshot.intents {
        let is_leaf = !parents.contains(i.id.as_str());
        if is_leaf && i.aspect == "happy" && realized.contains(i.id.as_str()) {
            ledger.enumerated += 1;
            let covered = parent_of
                .get(i.id.as_str())
                .is_some_and(|p| parent_has_nonhappy.contains(p));
            if covered {
                ledger.discharged += 1;
            } else {
                ledger.owed.push(i.name.clone());
            }
        }
    }
    ledger
}

/// Import substrings that signal a network/external-service client (open list).
/// Excludes EMBEDDED stores (rusqlite/sqlite) — not an outbound boundary.
const OUTBOUND_IMPORT_SIGNALS: &[&str] = &[
    "reqwest",
    "hyper",
    "ureq",
    "isahc",
    "sqlx",
    "tokio_postgres",
    "tokio-postgres",
    "aws_sdk",
    "aws-sdk",
    "rusoto",
    "lapin",
    "rdkafka",
    "mongodb",
    "tonic",
    "axum",
    "actix_web",
    "actix-web",
    "rocket",
    "warp",
    "requests",
    "httpx",
    "urllib",
    "aiohttp",
    "boto3",
    "net/http",
    "database/sql",
    "axios",
    "node-fetch",
    "psycopg",
    "pymongo",
    "redis",
];

/// BOUNDARY coverage (mechanical, DISK scan): every source file whose RAW import
/// lines name an external network/service client owes an owning intent with a
/// declared boundary. Done here (not the snapshot) because the graph persists only
/// internal-resolved imports; re-reading source is acceptable for a dedicated
/// command. Anchored to real import lines — can't be satisfied by a vibe; a repo
/// with no outbound code reads total=0 (rendered "—", never vacuous 100%).
pub fn boundary_scan_from_disk(
    root: &Path,
    snapshot: &QuerySnapshot,
) -> (CoverageAxis, Vec<String>) {
    let mut owners: HashMap<&str, Vec<&str>> = HashMap::new();
    for im in &snapshot.implements {
        owners
            .entry(im.codefile_path.as_str())
            .or_default()
            .push(im.intent_id.as_str());
    }
    let boundary_by_intent: HashMap<&str, &str> = snapshot
        .intents
        .iter()
        .map(|i| (i.id.as_str(), i.boundary.as_str()))
        .collect();

    let mut total = 0i64;
    let mut covered = 0i64;
    let mut owed = Vec::new();
    for cf in &snapshot.codefiles {
        let Ok(content) = std::fs::read_to_string(root.join(&cf.path)) else {
            continue;
        };
        let outbound = content.lines().any(|line| {
            let l = line.trim();
            // A real import statement only — NOT a bare string-literal line (the
            // earlier Go-block-import heuristic falsely matched Rust string
            // literals containing common words like "requests"/"redis").
            let is_import = l.starts_with("use ")
                || l.starts_with("import ")
                || l.starts_with("from ")
                || l.contains("require(");
            is_import && {
                let lower = l.to_lowercase();
                OUTBOUND_IMPORT_SIGNALS.iter().any(|s| lower.contains(s))
            }
        });
        if !outbound {
            continue;
        }
        total += 1;
        let has_boundary = owners
            .get(cf.path.as_str())
            .into_iter()
            .flatten()
            .any(|id| boundary_by_intent.get(id).is_some_and(|b| !b.is_empty()));
        if has_boundary {
            covered += 1;
        } else {
            owed.push(cf.path.clone());
        }
    }
    (CoverageAxis { covered, total }, owed)
}

/// The CANONICAL completeness rubric — the invariant dimensions, served by both
/// `loom complete --teach` and `loom guide`. loom names the dimensions; the LLM
/// instantiates them for THIS repo (which flows are journeys, which sad paths
/// matter). The maxim that defeats the shallow-checklist launder: RECORD is not
/// DISCHARGE.
pub fn rubric_teaching() -> &'static str {
    "\
COMPLETENESS RUBRIC — the COVERAGE half of production-ready (fully_proven is the QUALITY half).
loom names the canonical dimensions; YOU instantiate them for this repo. The law that makes it
honest: RECORD ≠ DISCHARGE — recording a planned intent or a `saga add` is BINDING DEBT, never a
satisfaction; a dimension clears only when the placeholder becomes realized + proven graph state.

  1. ENTRYPOINT (mechanical, FORCED) — every public symbol is grounded/accepted/adjudicated.
     done_when: `loom coverage` 0 actionable symbol gaps. Gates fully_proven (G7).
  2. BOUNDARY (mechanical, FORCED) — every file that imports an external service/client has an
     owning intent with `--boundary outbound` (or inbound for a served surface).
     done_when: `loom complete` boundary covered == total.
  3. INVARIANT (mechanical) — every coded intent is measured under >=1 GOVERNS rule.
     done_when: measured_pairs covered == total (`loom next --mode quality` empty).
  4. JOURNEY (cognitive) — every `--visibility user_visible` leaf has a SAGA that RAN passing.
     You decide which flows are journeys (set visibility); loom checks the run. `saga add` alone
     is OWED, not discharged. done_when: every user_visible leaf has a passing discriminating saga.
  5. BEHAVIORAL (cognitive) — every realized `--aspect happy` leaf has a realized sibling covering
     a sad / fallback / edge_case aspect, or a decision note with a falsifiable `reopen-when:`.
     done_when: no happy leaf without its failure-path sibling. A `planned` sibling is OWED.

To FULFILL a gap: create the intent (`loom intent add --aspect sad …` / `--visibility user_visible`),
ground it, prove it, and for journeys `loom saga add` then `loom saga run`. Then `loom complete` re-checks."
}
