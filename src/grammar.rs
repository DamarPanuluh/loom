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
pub const RATIFICATION_STATES: &[&str] = &["unratified", "ratified", "needs_reconfirmation"];

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
    use super::{is_placeholder, looks_like_symbol};

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
