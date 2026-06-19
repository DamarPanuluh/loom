//! The enforcement layer that makes the role skeleton real.
//!
//! Two kinds of gate, both applied at the command boundary (the only write
//! surface — the LLM never writes storage queries):
//!
//! **Lane gates.** Schema v3 declared an owning role per field, but ownership
//! was advisory: any agent could write any field. Here it becomes a contract.
//! An agent that *declares* a role (`LOOM_AGENT=llm:analyzer` or an explicit
//! `--by`/`--inspected-by`/`--author` flag) is held to that role's lane — a
//! builder cannot ground edges, an analyzer cannot confirm intents, nobody but
//! quality can issue a GOVERNS verdict. A roleless agent (`llm`, `human`) is
//! solo mode: it passes every lane, because a single agent driving the whole
//! loop is still a supported way to run loom. The point of the lanes is
//! separation of duties *when duties are separated* — many limited agents
//! lifting together, no one of them able to green-light its own work.
//!
//! **Evidence gates.** The graph is only as trustworthy as its weakest
//! criterion. These reject the degenerate inputs that let an agent fake
//! progress: empty or placeholder criteria/evidence, independence claims with
//! no recorded why, confidence outside [0, 1].

use std::path::Path;

use anyhow::Result;

use crate::db::schema::{role, ROLES};

// ---------------------------------------------------------------------------
// Lane gates
// ---------------------------------------------------------------------------

/// Extract the declared role from an acting-agent string:
/// `llm:analyzer` → `Some("analyzer")`; bare `llm` / `human` → `None` (solo mode).
pub fn role_of(agent: &str) -> Option<&str> {
    agent
        .split_once(':')
        .map(|(_, r)| r.trim())
        .filter(|r| !r.is_empty())
}
pub(crate) fn known_bare_role(agent: &str) -> Option<&'static str> {
    let bare = agent.trim();
    if bare.contains(':') {
        return None;
    }
    ROLES.iter().copied().find(|r| r.eq_ignore_ascii_case(bare))
}

/// The `loom next` mode that serves a role's lane — used in lane-violation
/// errors to point the agent back at its own queue.
pub fn mode_for_role(r: &str) -> Option<&'static str> {
    match r {
        role::BUILDER => Some("build"),
        role::ANALYZER => Some("discovery"),
        role::FIXER => Some("fix"),
        role::VALIDATOR => Some("validate"),
        role::QUALITY => Some("quality"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The lane table — the single source of truth for who may do what.
//
// Each `Lane` pairs a command action with the roles allowed to perform it. Call
// sites reference a named `lane::CONST` instead of inlining `(action, &[roles])`,
// so the membership map lives in ONE place: `enforce_lane` renders an offender's
// whole lane from it, `actions_for_role` inverts it, and a ratchet test keeps
// `LANES` in sync with the consts. Mirrors the `role`+`ROLES` idiom above.
// ---------------------------------------------------------------------------

/// A command action paired with the roles permitted to perform it.
#[derive(Debug, PartialEq, Eq)]
pub struct Lane {
    pub action: &'static str,
    pub allowed: &'static [&'static str],
}

pub mod lane {
    use super::Lane;
    use crate::db::schema::role;

    // analyzer + fixer — grounding/repairing inspectable edges
    pub const GROUND_RELATES_TO: Lane = Lane {
        action: "ground a RELATES_TO edge",
        allowed: &[role::ANALYZER, role::FIXER],
    };
    pub const ISSUE_RELATES_TO: Lane = Lane {
        action: "record an issue on a RELATES_TO edge",
        allowed: &[role::ANALYZER, role::FIXER],
    };
    pub const GROUND_SERVES: Lane = Lane {
        action: "ground a SERVES edge",
        allowed: &[role::ANALYZER, role::FIXER],
    };

    // analyzer — discovery verdicts
    pub const INDEPENDENT_RELATES_TO: Lane = Lane {
        action: "confirm two intents independent",
        allowed: &[role::ANALYZER],
    };
    pub const ISSUE_SERVES: Lane = Lane {
        action: "issue a SERVES edge",
        allowed: &[role::ANALYZER],
    };
    pub const INDEPENDENT_SERVES: Lane = Lane {
        action: "mark SERVES independent",
        allowed: &[role::ANALYZER],
    };
    pub const PROVE_HYPOTHESIS: Lane = Lane {
        action: "prove a hypothesis",
        allowed: &[role::ANALYZER],
    };

    // fixer — repair
    pub const FIX_RELATES_TO: Lane = Lane {
        action: "mark a repaired RELATES_TO edge passing",
        allowed: &[role::FIXER],
    };

    // quality — the green gate
    pub const GOVERNS_VERDICT: Lane = Lane {
        action: "record a GOVERNS verdict",
        allowed: &[role::QUALITY],
    };
    pub const APPLY_RULE_EDGE: Lane = Lane {
        action: "apply a quality rule (GOVERNS)",
        allowed: &[role::QUALITY],
    };
    pub const APPLY_RULE: Lane = Lane {
        action: "apply a quality rule",
        allowed: &[role::QUALITY],
    };
    pub const ADD_RULE: Lane = Lane {
        action: "add a quality rule",
        allowed: &[role::QUALITY],
    };
    pub const SEED_RULES: Lane = Lane {
        action: "seed a rule pack",
        allowed: &[role::QUALITY],
    };

    // builder — construction
    pub const IMPLEMENT: Lane = Lane {
        action: "create an IMPLEMENTS edge",
        allowed: &[role::BUILDER],
    };
    pub const UNIMPLEMENT: Lane = Lane {
        action: "remove an IMPLEMENTS edge",
        allowed: &[role::BUILDER],
    };
    pub const HIERARCHY: Lane = Lane {
        action: "create a HIERARCHY edge",
        allowed: &[role::BUILDER],
    };
    pub const INTENT_SOURCE: Lane = Lane {
        action: "edit an intent's source refs",
        allowed: &[role::BUILDER],
    };
    pub const INTENT_TAG: Lane = Lane {
        action: "edit an intent's tags",
        allowed: &[role::BUILDER],
    };
    pub const ADD_INTENT: Lane = Lane {
        action: "add an intent",
        allowed: &[role::BUILDER],
    };
    pub const UPDATE_INTENT: Lane = Lane {
        action: "update an intent",
        allowed: &[role::BUILDER],
    };
    pub const DELETE_INTENT: Lane = Lane {
        action: "delete an intent",
        allowed: &[role::BUILDER],
    };
    pub const RETIRE_INTENT: Lane = Lane {
        action: "retire an intent",
        allowed: &[role::BUILDER],
    };
    pub const ADOPT_HYPOTHESIS: Lane = Lane {
        action: "adopt a hypothesis",
        allowed: &[role::BUILDER],
    };
    pub const REJECT_HYPOTHESIS: Lane = Lane {
        action: "reject a hypothesis",
        allowed: &[role::BUILDER],
    };
    pub const ADD_CODEFILE: Lane = Lane {
        action: "register a code file",
        allowed: &[role::BUILDER],
    };
    pub const REMOVE_CODEFILE: Lane = Lane {
        action: "remove a code file",
        allowed: &[role::BUILDER],
    };
    pub const SET_LAYER_ORDER: Lane = Lane {
        action: "declare the architecture layer order",
        allowed: &[role::BUILDER],
    };
    pub const CLEAR_LAYER_ORDER: Lane = Lane {
        action: "clear the architecture layer order",
        allowed: &[role::BUILDER],
    };
    pub const ADD_PERSONA: Lane = Lane {
        action: "add a persona",
        allowed: &[role::BUILDER],
    };
    pub const ADD_VOCAB: Lane = Lane {
        action: "register a vocab term",
        allowed: &[role::BUILDER],
    };
    pub const MERGE_VOCAB: Lane = Lane {
        action: "merge vocab terms",
        allowed: &[role::BUILDER],
    };
    pub const ADD_DELEGATION: Lane = Lane {
        action: "delegate a subtree",
        allowed: &[role::BUILDER],
    };
    pub const REMOVE_DELEGATION: Lane = Lane {
        action: "remove a subtree delegation",
        allowed: &[role::BUILDER],
    };
    pub const SPAWN_JOURNEY_INTENTS: Lane = Lane {
        action: "spawn planned intents from a journey",
        allowed: &[role::BUILDER],
    };
    pub const POPULATE_GRAPH: Lane = Lane {
        action: "populate derived graph structure",
        allowed: &[role::BUILDER],
    };

    // validator — proof
    pub const CONFIRM_INTENT: Lane = Lane {
        action: "confirm an intent",
        allowed: &[role::VALIDATOR],
    };
    pub const MARK_VALIDATION: Lane = Lane {
        action: "mark a validation result",
        allowed: &[role::VALIDATOR],
    };
    pub const RUN_VALIDATIONS: Lane = Lane {
        action: "run validations",
        allowed: &[role::VALIDATOR],
    };
    pub const RUN_SAGA: Lane = Lane {
        action: "run a saga proof",
        allowed: &[role::VALIDATOR],
    };

    // builder + fixer
    pub const SET_LIFECYCLE: Lane = Lane {
        action: "set an intent lifecycle",
        allowed: &[role::BUILDER, role::FIXER],
    };

    // builder + validator — validations & sagas
    pub const ADD_VALIDATION: Lane = Lane {
        action: "add a validation",
        allowed: &[role::BUILDER, role::VALIDATOR],
    };
    pub const UPDATE_VALIDATION: Lane = Lane {
        action: "update a validation definition",
        allowed: &[role::BUILDER, role::VALIDATOR],
    };
    pub const DELETE_VALIDATION: Lane = Lane {
        action: "delete a validation",
        allowed: &[role::BUILDER, role::VALIDATOR],
    };
    pub const LINK_VALIDATION: Lane = Lane {
        action: "link a validation (VALIDATES)",
        allowed: &[role::BUILDER, role::VALIDATOR],
    };
    pub const ADD_SAGA: Lane = Lane {
        action: "register a saga proof",
        allowed: &[role::BUILDER, role::VALIDATOR],
    };
}

/// Every lane, for iteration (inversion, rendering, the ratchet test). MUST list
/// each `lane::` const exactly once — `lane_table_is_the_single_source` guards it.
pub const LANES: &[&Lane] = &[
    &lane::GROUND_RELATES_TO,
    &lane::ISSUE_RELATES_TO,
    &lane::GROUND_SERVES,
    &lane::INDEPENDENT_RELATES_TO,
    &lane::ISSUE_SERVES,
    &lane::INDEPENDENT_SERVES,
    &lane::PROVE_HYPOTHESIS,
    &lane::FIX_RELATES_TO,
    &lane::GOVERNS_VERDICT,
    &lane::APPLY_RULE_EDGE,
    &lane::APPLY_RULE,
    &lane::ADD_RULE,
    &lane::SEED_RULES,
    &lane::IMPLEMENT,
    &lane::UNIMPLEMENT,
    &lane::HIERARCHY,
    &lane::INTENT_SOURCE,
    &lane::INTENT_TAG,
    &lane::ADD_INTENT,
    &lane::UPDATE_INTENT,
    &lane::DELETE_INTENT,
    &lane::RETIRE_INTENT,
    &lane::ADOPT_HYPOTHESIS,
    &lane::REJECT_HYPOTHESIS,
    &lane::ADD_CODEFILE,
    &lane::REMOVE_CODEFILE,
    &lane::SET_LAYER_ORDER,
    &lane::CLEAR_LAYER_ORDER,
    &lane::ADD_PERSONA,
    &lane::ADD_VOCAB,
    &lane::MERGE_VOCAB,
    &lane::ADD_DELEGATION,
    &lane::REMOVE_DELEGATION,
    &lane::SPAWN_JOURNEY_INTENTS,
    &lane::POPULATE_GRAPH,
    &lane::CONFIRM_INTENT,
    &lane::MARK_VALIDATION,
    &lane::RUN_VALIDATIONS,
    &lane::RUN_SAGA,
    &lane::SET_LIFECYCLE,
    &lane::ADD_VALIDATION,
    &lane::UPDATE_VALIDATION,
    &lane::DELETE_VALIDATION,
    &lane::LINK_VALIDATION,
    &lane::ADD_SAGA,
];

/// All lanes a role may perform — inverts `LANES`. Used by the lane-violation
/// error (to show the offender its whole lane) and, later, `loom guide --role`.
pub fn actions_for_role(role: &str) -> Vec<&'static Lane> {
    LANES
        .iter()
        .copied()
        .filter(|l| l.allowed.contains(&role))
        .collect()
}

/// Render the offender's whole lane for a violation message — capped so a wide
/// lane (builder owns 21 actions) teaches without becoming a wall.
fn render_lane_summary(role: &str) -> String {
    let mine = actions_for_role(role);
    if mine.is_empty() {
        return String::new();
    }
    const SHOW: usize = 4;
    let shown = mine
        .iter()
        .take(SHOW)
        .map(|l| format!("`{}`", l.action))
        .collect::<Vec<_>>()
        .join(", ");
    let more = if mine.len() > SHOW {
        format!(" (+{} more)", mine.len() - SHOW)
    } else {
        String::new()
    };
    format!(" Your lane: {shown}{more}.")
}

/// Enforce that `agent` is allowed to perform `lane`'s action.
///
/// - No declared role → solo mode, always allowed.
/// - Declared but unknown role → error (a typo must not bypass the lanes).
/// - Declared role outside the lane → violation, with a pointer to the offender's
///   own work queue and a capped view of its whole lane.
pub fn enforce_lane(lane: &Lane, agent: &str) -> Result<()> {
    let Some(r) = role_of(agent) else {
        if let Some(r) = known_bare_role(agent) {
            anyhow::bail!(
                "Agent '{agent}' names the known role '{r}' without a provenance prefix. \
                 Use `llm:{r}` (or `human:{r}` for human provenance) so lane gates can enforce separation of duties; \
                 use bare 'llm'/'human' only for solo mode."
            );
        }
        return Ok(()); // solo mode
    };
    if !ROLES.contains(&r) {
        anyhow::bail!(
            "Unknown agent role '{r}' (acting as '{agent}'). Valid roles: {roles}. \
             Set LOOM_AGENT=llm:<role>, or use a bare 'llm'/'human' for solo mode.",
            roles = ROLES.join(", "),
        );
    }
    if !lane.allowed.contains(&r) {
        let own_queue = mode_for_role(r)
            .map(|m| format!(" Your queue: `loom next --mode {m}`."))
            .unwrap_or_default();
        let lane_summary = render_lane_summary(r);
        let action = lane.action;
        let lanes = lane
            .allowed
            .iter()
            .map(|a| format!("`{a}`"))
            .collect::<Vec<_>>()
            .join("/");
        anyhow::bail!(
            "Lane violation: '{agent}' cannot {action} — that is {lanes} work. \
             Hand it off to that agent.{own_queue}{lane_summary}"
        );
    }
    Ok(())
}

/// Resolve the acting agent (explicit flag → $LOOM_AGENT → "llm") and enforce
/// the lane in one step. Returns the agent string for provenance stamping.
pub fn acting_in_lane(lane: &Lane, explicit: Option<&str>) -> Result<String> {
    let agent = crate::agent::acting(explicit);
    enforce_lane(lane, &agent)?;
    Ok(agent)
}

// ---------------------------------------------------------------------------
// Evidence gates
// ---------------------------------------------------------------------------

/// Minimum length (chars) for a substantive criterion/evidence/notes value.
pub const MIN_SUBSTANTIVE_LEN: usize = 10;

/// Inputs that read as "I filled the slot" rather than "I inspected the code".
pub const PLACEHOLDERS: &[&str] = &[
    "todo",
    "tbd",
    "n/a",
    "na",
    "none",
    "null",
    "unknown",
    "x",
    "xxx",
    "...",
    "criterion",
    "evidence",
    "notes",
    "<text>",
    "<criterion>",
    "<evidence>",
    "<notes>",
    "<why unrelated>",
    "?",
    "-",
];

/// True when a recorded value is empty or a known placeholder — used by both
/// the write-time gates here and the `loom doctor` audit.
pub fn is_vacuous(value: &str) -> bool {
    let v = value.trim().to_lowercase();
    v.is_empty() || v.chars().count() < MIN_SUBSTANTIVE_LEN || PLACEHOLDERS.contains(&v.as_str())
}

/// Reject an empty/placeholder/too-short value for a required evidence field.
/// `field` is the flag name (e.g. "criterion"); `purpose` finishes the sentence
/// "it must state …" so the error teaches what a good value looks like.
pub fn require_substantive(field: &str, value: &str, purpose: &str) -> Result<()> {
    if is_vacuous(value) {
        anyhow::bail!(
            "--{field} must be substantive (≥{min} chars, not a placeholder): it must state {purpose}. \
             Got: '{got}'. A vacuous {field} makes the edge unfalsifiable — the graph would look \
             inspected without being inspected.",
            min = MIN_SUBSTANTIVE_LEN,
            got = value.trim(),
        );
    }
    Ok(())
}

/// Reject a confidence outside [0.0, 1.0].
pub fn require_confidence(confidence: f64) -> Result<()> {
    if !(0.0..=1.0).contains(&confidence) || confidence.is_nan() {
        anyhow::bail!(
            "--confidence must be between 0.0 and 1.0 (got {confidence}). \
             It is a probability that the recorded verdict is correct."
        );
    }
    Ok(())
}

/// Parse a locator line suffix: "299" → (299, 299), "299-340" → (299, 340).
/// `None` for a non-numeric suffix (a symbol locator like `fn foo`), which the
/// resolution gate treats as "existence-only" — symbol verification is the
/// code-aware layer's job, not this one.
fn parse_line_range(s: &str) -> Option<(usize, usize)> {
    let s = s.trim();
    match s.split_once('-') {
        Some((a, b)) => Some((a.trim().parse().ok()?, b.trim().parse().ok()?)),
        None => {
            let n = s.parse().ok()?;
            Some((n, n))
        }
    }
}

/// Reject an `--evidence-locator` that does not RESOLVE against the repo root:
/// the file must exist within the root, and a numeric `:line` / `:start-end`
/// range must fall within it. Syntax is already checked in `compose_evidence`;
/// this is the resolution gate, kept fs-side so the pure string composer stays
/// unit-testable. A fabricated or stale anchor (`@src/nope.rs:1-9`) must not be
/// laundered into a verdict as precise evidence — that is exactly how a
/// low-tier lane fakes "I looked at the code".
pub fn require_locators_resolve(root: &Path, locators: &[String]) -> Result<()> {
    for raw in locators {
        let l = raw.trim();
        let (path, range) = match l.split_once(':') {
            Some((p, rest)) => (p, Some(rest)),
            None => (l, None),
        };
        let Some(rel) = crate::repo::confine(root, Path::new(path)) else {
            anyhow::bail!(
                "--evidence-locator '{l}' escapes the repo root — anchor real code inside this repo."
            );
        };
        let content = std::fs::read_to_string(root.join(&rel)).map_err(|_| {
            anyhow::anyhow!(
                "--evidence-locator '{l}' points at '{path}', which is not a readable file in the repo. \
                 A fabricated or stale anchor cannot ground a verdict — cite a real file:line."
            )
        })?;
        if let Some(range) = range {
            if let Some((start, end)) = parse_line_range(range) {
                let total = content.lines().count().max(1);
                if start == 0 || end < start || start > total || end > total {
                    anyhow::bail!(
                        "--evidence-locator '{l}': line range '{range}' is outside '{path}' (1..={total}) — \
                         the anchor does not point at real lines."
                    );
                }
            }
        }
    }
    Ok(())
}

/// Fold `--evidence-locator` values (file/line anchors like
/// `src/db/queries/stats.rs:299-340`) into the stored evidence string with a
/// canonical, parseable `@<locator>` prefix — so a later reviewer lands on
/// the exact lines instead of re-deriving them from prose. Locators are
/// validated (path-shaped, no whitespace); the prose part is untouched.
/// No locators → the evidence passes through unchanged.
pub fn compose_evidence(locators: &[String], evidence: &str) -> Result<String> {
    if locators.is_empty() {
        return Ok(evidence.to_string());
    }
    let mut anchors = Vec::with_capacity(locators.len());
    for l in locators {
        let l = l.trim();
        if l.len() < 3
            || l.chars().any(char::is_whitespace)
            || !(l.contains('/') || l.contains('.'))
        {
            anyhow::bail!(
                "--evidence-locator must be a file anchor like `src/db/queries/stats.rs:299-340` \
                 (path, optionally `:line` or `:start-end`; no spaces). Got: '{l}'."
            );
        }
        anchors.push(format!("@{l}"));
    }
    let anchors = anchors.join(" ");
    Ok(if evidence.trim().is_empty() {
        anchors
    } else {
        format!("{anchors} — {evidence}")
    })
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a throwaway lane for the mechanics tests, so they don't couple to
    /// the production table's contents.
    fn mk_lane(action: &'static str, allowed: &'static [&'static str]) -> Lane {
        Lane { action, allowed }
    }

    #[test]
    fn role_parsing() {
        assert_eq!(role_of("llm:analyzer"), Some("analyzer"));
        assert_eq!(role_of("human:quality"), Some("quality"));
        assert_eq!(role_of("llm"), None);
        assert_eq!(role_of("human"), None);
        assert_eq!(role_of("llm:"), None);
    }

    #[test]
    fn solo_mode_passes_every_lane() {
        assert!(enforce_lane(&mk_lane("ground an edge", &[role::ANALYZER]), "llm").is_ok());
        assert!(enforce_lane(&mk_lane("confirm an intent", &[role::VALIDATOR]), "human").is_ok());
    }

    #[test]
    fn declared_role_is_held_to_its_lane() {
        // In lane.
        assert!(enforce_lane(
            &mk_lane("ground an edge", &[role::ANALYZER]),
            "llm:analyzer"
        )
        .is_ok());
        assert!(enforce_lane(
            &mk_lane("mark a lifecycle", &[role::BUILDER, role::FIXER]),
            "llm:fixer"
        )
        .is_ok());
        // Out of lane: the violation names the queue AND the offender's whole lane.
        let err = enforce_lane(&mk_lane("ground an edge", &[role::ANALYZER]), "llm:builder")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Lane violation"), "got: {err}");
        assert!(err.contains("loom next --mode build"), "got: {err}");
        assert!(err.contains("Your lane:"), "whole-lane view missing: {err}");
    }

    #[test]
    fn unknown_role_is_rejected_not_bypassed() {
        let err = enforce_lane(
            &mk_lane("ground an edge", &[role::ANALYZER]),
            "llm:analyser",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("Unknown agent role"), "got: {err}");
    }

    #[test]
    fn bare_known_role_is_rejected_not_solo_mode() {
        let err = enforce_lane(&mk_lane("ground an edge", &[role::ANALYZER]), "builder")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Use `llm:builder`"), "got: {err}");
        assert!(enforce_lane(&mk_lane("ground an edge", &[role::ANALYZER]), "alice").is_ok());
    }

    #[test]
    fn lane_table_is_the_single_source() {
        use std::collections::HashMap;
        // (1) No action maps to two different allowed-sets, and no entry repeats.
        let mut seen: HashMap<&str, &[&str]> = HashMap::new();
        for l in LANES {
            if let Some(prev) = seen.insert(l.action, l.allowed) {
                assert_eq!(
                    prev, l.allowed,
                    "action '{}' has conflicting allowed-sets",
                    l.action
                );
            }
        }
        assert_eq!(
            seen.len(),
            LANES.len(),
            "LANES has a duplicate action entry"
        );
        // (2) Every role named in a lane is a real agent role.
        for l in LANES {
            for r in l.allowed {
                assert!(
                    ROLES.contains(r),
                    "lane '{}' allows unknown role '{r}'",
                    l.action
                );
            }
        }
        // (3) Every agent role owns at least one lane.
        for r in ROLES {
            assert!(
                LANES.iter().any(|l| l.allowed.contains(r)),
                "role '{r}' has no lane"
            );
        }
        // (4) actions_for_role inverts LANES correctly.
        for r in ROLES {
            let got = actions_for_role(r).len();
            let want = LANES.iter().filter(|l| l.allowed.contains(r)).count();
            assert_eq!(got, want, "actions_for_role('{r}') disagrees with LANES");
        }
    }

    #[test]
    fn vacuous_values_are_rejected() {
        for bad in ["", "  ", "todo", "TBD", "<criterion>", "n/a", "short"] {
            assert!(is_vacuous(bad), "expected vacuous: '{bad}'");
            assert!(require_substantive("criterion", bad, "what passing looks like").is_err());
        }
        let good = "loom sync flags IMPLEMENTS edges of files whose mtime advanced";
        assert!(!is_vacuous(good));
        assert!(require_substantive("criterion", good, "what passing looks like").is_ok());
    }

    #[test]
    fn confidence_bounds() {
        assert!(require_confidence(0.0).is_ok());
        assert!(require_confidence(0.9).is_ok());
        assert!(require_confidence(1.0).is_ok());
        assert!(require_confidence(-0.1).is_err());
        assert!(require_confidence(7.3).is_err());
        assert!(require_confidence(f64::NAN).is_err());
    }

    #[test]
    fn compose_evidence_folds_locators() {
        // No locators → passthrough, byte-for-byte.
        assert_eq!(
            compose_evidence(&[], "found it in the parser").unwrap(),
            "found it in the parser"
        );
        // Locators prefix the prose with parseable anchors.
        assert_eq!(
            compose_evidence(&["src/a.rs:10-20".into()], "the call path exists").unwrap(),
            "@src/a.rs:10-20 — the call path exists"
        );
        assert_eq!(
            compose_evidence(&["src/a.rs:10-20".into(), "src/b.rs:5".into()], "").unwrap(),
            "@src/a.rs:10-20 @src/b.rs:5",
            "locators alone are a valid evidence body for ground"
        );
        // Non-path-shaped or spaced anchors are rejected with the format taught.
        for bad in ["x", "not a path", "noslashordot"] {
            let err = compose_evidence(&[bad.into()], "e")
                .unwrap_err()
                .to_string();
            assert!(err.contains("file anchor"), "got: {err}");
        }
    }

    #[test]
    fn locator_resolution_rejects_fabricated_anchors() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("loom-gate-loc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = std::fs::File::create(dir.join("real.rs")).unwrap();
        writeln!(f, "fn a() {{}}\nfn b() {{}}\nfn c() {{}}").unwrap(); // 3 lines

        // A real file resolves — bare, in-range, and (existence-only) symbol suffix.
        assert!(require_locators_resolve(&dir, &["real.rs".into()]).is_ok());
        assert!(require_locators_resolve(&dir, &["real.rs:1-3".into()]).is_ok());
        assert!(require_locators_resolve(&dir, &["real.rs:2".into()]).is_ok());
        assert!(require_locators_resolve(&dir, &["real.rs:funcname".into()]).is_ok());
        // No locators → passthrough.
        assert!(require_locators_resolve(&dir, &[]).is_ok());

        // A fabricated file is rejected (the core laundering vector).
        let err = require_locators_resolve(&dir, &["nope.rs:1-9".into()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a readable file"), "got: {err}");
        // A line range past EOF is rejected.
        assert!(require_locators_resolve(&dir, &["real.rs:9999".into()]).is_err());
        assert!(
            require_locators_resolve(&dir, &["real.rs:1-9999".into()]).is_err(),
            "end line past EOF must be rejected"
        );
        // An inverted range is rejected.
        assert!(require_locators_resolve(&dir, &["real.rs:3-1".into()]).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
