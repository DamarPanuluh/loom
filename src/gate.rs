//! The enforcement layer that makes the role skeleton real.
//!
//! Two kinds of gate, both applied at the command boundary (the only write
//! surface — the LLM never writes GQL):
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

/// Enforce that `agent` is allowed to perform `action`.
///
/// - No declared role → solo mode, always allowed.
/// - Declared but unknown role → error (a typo must not bypass the lanes).
/// - Declared role outside `allowed` → lane violation, with a pointer to the
///   offender's own work queue.
pub fn enforce_lane(action: &str, allowed: &[&str], agent: &str) -> Result<()> {
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
    if !allowed.contains(&r) {
        let own_queue = mode_for_role(r)
            .map(|m| format!(" Your queue: `loom next --mode {m}`."))
            .unwrap_or_default();
        anyhow::bail!(
            "Lane violation: '{agent}' cannot {action} — that is {lanes} work. \
             Hand it off to that agent.{own_queue}",
            lanes = allowed
                .iter()
                .map(|a| format!("`{a}`"))
                .collect::<Vec<_>>()
                .join("/"),
        );
    }
    Ok(())
}

/// Resolve the acting agent (explicit flag → $LOOM_AGENT → "llm") and enforce
/// the lane in one step. Returns the agent string for provenance stamping.
pub fn acting_in_lane(action: &str, allowed: &[&str], explicit: Option<&str>) -> Result<String> {
    let agent = crate::agent::acting(explicit);
    enforce_lane(action, allowed, &agent)?;
    Ok(agent)
}

// ---------------------------------------------------------------------------
// Evidence gates
// ---------------------------------------------------------------------------

/// Minimum length (chars) for a substantive criterion/evidence/notes value.
pub const MIN_SUBSTANTIVE_LEN: usize = 10;

/// Inputs that read as "I filled the slot" rather than "I inspected the code".
pub const PLACEHOLDERS: &[&str] = &[
    "todo", "tbd", "n/a", "na", "none", "null", "unknown", "x", "xxx", "...",
    "criterion", "evidence", "notes", "<text>", "<criterion>", "<evidence>",
    "<notes>", "<why unrelated>", "?", "-",
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
        if l.len() < 3 || l.chars().any(char::is_whitespace) || !(l.contains('/') || l.contains('.')) {
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
        assert!(enforce_lane("ground an edge", &[role::ANALYZER], "llm").is_ok());
        assert!(enforce_lane("confirm an intent", &[role::VALIDATOR], "human").is_ok());
    }

    #[test]
    fn declared_role_is_held_to_its_lane() {
        // In lane.
        assert!(enforce_lane("ground an edge", &[role::ANALYZER], "llm:analyzer").is_ok());
        assert!(
            enforce_lane("mark a lifecycle", &[role::BUILDER, role::FIXER], "llm:fixer").is_ok()
        );
        // Out of lane.
        let err = enforce_lane("ground an edge", &[role::ANALYZER], "llm:builder")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Lane violation"), "got: {err}");
        assert!(err.contains("loom next --mode build"), "got: {err}");
    }

    #[test]
    fn unknown_role_is_rejected_not_bypassed() {
        let err = enforce_lane("ground an edge", &[role::ANALYZER], "llm:analyser")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Unknown agent role"), "got: {err}");
    }

    #[test]
    fn bare_known_role_is_rejected_not_solo_mode() {
        let err = enforce_lane("ground an edge", &[role::ANALYZER], "builder")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Use `llm:builder`"), "got: {err}");
        assert!(enforce_lane("ground an edge", &[role::ANALYZER], "alice").is_ok());
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
        assert_eq!(compose_evidence(&[], "found it in the parser").unwrap(), "found it in the parser");
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
            let err = compose_evidence(&[bad.into()], "e").unwrap_err().to_string();
            assert!(err.contains("file anchor"), "got: {err}");
        }
    }
}
