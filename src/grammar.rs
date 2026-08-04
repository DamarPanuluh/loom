//! Statement grammar — shared write-gate vocabulary and sentence predicates.
//!
//! Plane: pure grammar. This module owns the finite statement vocabularies and
//! lexical predicates used by write boundaries and `loom schema`; it performs
//! no storage or CLI orchestration.

/// Allowed intent altitude labels.
pub const LEVELS: &[&str] = &["system", "component", "feature", "cross_cutting"];

/// Allowed scenario labels for an intent.
pub const ASPECTS: &[&str] = &["happy", "sad", "fallback", "edge_case"];

/// Allowed audience labels for an intent.
pub const VISIBILITIES: &[&str] = &["user_visible", "internal"];

/// Lifecycle states accepted when creating or updating an active intent.
pub const ACTIVE_LIFECYCLES: &[&str] = &["planned", "implemented", "needs_change"];

/// Every intent lifecycle, including the retired state accepted by readers.
pub const ALL_LIFECYCLES: &[&str] = &["planned", "implemented", "needs_change", "deprecated"];

/// Ratification states. A missing facet reads as `unratified` (INV-8).
/// The states a HUMAN may assert. `de_facto` is absent on purpose: it is
/// derived from evidence, so there is no path from caller input to it.
pub const RATIFICATION_STATES: &[&str] =
    &["unratified", "ratified", "rejected", "needs_reconfirmation"];

/// Whole-field filler tokens that cannot stand in for evidence or a reason.
pub const PLACEHOLDER_TOKENS: &[&str] = &[
    "…",
    "...",
    ". . .",
    "todo",
    "tbd",
    "tba",
    "n/a",
    "na",
    "none",
    "-",
    "--",
    ".",
    "?",
    "???",
    "xxx",
    "fixme",
    "placeholder",
];

/// Whether a criterion / evidence / reason field is a non-substantive
/// placeholder. This checks the whole field, not a substring: real evidence
/// may legitimately contain an ellipsis.
pub fn is_placeholder(s: &str) -> bool {
    // Strip whitespace and quote/backtick wrappers only — NOT angle brackets,
    // so a whole-field `<reason>` hole remains detectable below.
    let raw = s
        .trim()
        .trim_matches(|c: char| matches!(c, '\'' | '"' | '`'))
        .trim();
    if raw.is_empty() {
        return true;
    }
    // A field that IS `<…>` (or `[…]`) is an unfilled write-back hole.
    if (raw.starts_with('<') && raw.ends_with('>')) || (raw.starts_with('[') && raw.ends_with(']'))
    {
        return true;
    }
    PLACEHOLDER_TOKENS.contains(&raw.to_ascii_lowercase().as_str())
}

/// A validation name with a hole left where a retired proof-level token was
/// excised immediately before a trailing `proof`.
///
/// The retired L0–L6 token was removed from some names without rejoining words,
/// leaving fingerprints such as `grades  proof` or `empty  proof`. Only a
/// double-space immediately before a final `proof` counts — a mid-phrase
/// double space (`checks payment  retry policy`) is legitimate whitespace, not
/// this corruption.
pub fn excised_proof_level_name(name: &str) -> bool {
    let name = name.trim_end();
    let Some(proof_at) = name.rfind("proof") else {
        return false;
    };
    if proof_at + "proof".len() != name.len() {
        return false;
    }
    name[..proof_at].ends_with("  ")
}

/// Does this name look like a code symbol rather than a behavior? Behaviors
/// read as phrases; symbols are single tokens such as `capture_payment`,
/// `runWithSqlite`, `Store::open`, or `handle()`.
pub fn looks_like_symbol(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || name.contains(' ') {
        return false;
    }
    name.contains('_') || name.contains("::") || name.contains('(') || has_internal_caps(name)
}

fn has_internal_caps(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    chars
        .windows(2)
        .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::{excised_proof_level_name, is_placeholder, looks_like_symbol};

    #[test]
    fn excised_proof_level_names_leave_a_double_space_hole() {
        assert!(excised_proof_level_name(
            "a proof whose command cannot run grades  proof"
        ));
        assert!(excised_proof_level_name(
            "an offset past the end returns an empty  proof"
        ));
        assert!(!excised_proof_level_name(
            "a proof whose command cannot run grades S0"
        ));
        assert!(!excised_proof_level_name("typed CLI dispatch contracts"));
        // Mid-phrase double space is not the excision fingerprint.
        assert!(!excised_proof_level_name("checks payment  retry policy"));
    }

    #[test]
    fn rejects_whole_field_placeholders() {
        for placeholder in [
            "",
            "  ",
            "…",
            "...",
            "<...>",
            "TODO",
            "tbd",
            "n/a",
            "-",
            ".",
            "???",
            "'…'",
            "<reason>",
            "<what was built>",
            "<symbol>",
            "[fill me]",
        ] {
            assert!(is_placeholder(placeholder), "should reject {placeholder:?}");
        }
    }

    #[test]
    fn accepts_substantive_text_even_with_ellipsis() {
        for text in [
            "src/store/edges.rs:110 gates empty evidence",
            "test output: assertion failed at line 42 …",
            "no auth check before delete_user()",
        ] {
            assert!(!is_placeholder(text), "should accept {text:?}");
        }
    }

    #[test]
    fn recognizes_symbol_like_names() {
        assert!(looks_like_symbol("capture_payment"));
        assert!(looks_like_symbol("runWithSqlite"));
        assert!(looks_like_symbol("Store::open"));
        assert!(looks_like_symbol("handle()"));
        assert!(!looks_like_symbol("payment can be captured"));
    }
}
