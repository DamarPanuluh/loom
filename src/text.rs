//! Display truncation — one rule for capping rendered text.
//!
//! Plane: pure text. This module sits below every ring that renders: it knows
//! nothing about storage, the graph, or the CLI. It exists so a low ring can
//! cap an excerpt without importing the command plane — the inversion that
//! `src/proof.rs` carried when this rule lived in `commands`.

/// Cap `s` at `max` characters, marking any elision with `…`.
///
/// Counts characters, not bytes: the cap describes what a reader sees, and a
/// byte cap would split multi-byte text mid-character. Whitespace is left
/// alone — callers that need it stripped call `.trim()` at the site, so this
/// function has exactly one behavior and a caller can always tell which one it
/// asked for.
pub fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_shorter_than_the_cap_is_returned_unchanged() {
        assert_eq!(ellipsize("short", 60), "short");
    }

    #[test]
    fn text_exactly_at_the_cap_gains_no_marker() {
        assert_eq!(ellipsize("abcde", 5), "abcde");
    }

    #[test]
    fn overflow_keeps_max_characters_and_appends_the_marker() {
        assert_eq!(ellipsize("abcdef", 5), "abcde…");
    }

    /// The cap is in characters. A byte cap would slice these in half and
    /// panic; counting characters is the property the callers depend on.
    #[test]
    fn multibyte_text_is_capped_by_character_not_byte() {
        assert_eq!(ellipsize("ααααα", 3), "ααα…");
        assert_eq!(ellipsize("日本語のテキスト", 4), "日本語の…");
    }

    #[test]
    fn whitespace_is_preserved_because_trimming_is_the_callers_choice() {
        assert_eq!(ellipsize("  padded  ", 60), "  padded  ");
    }

    #[test]
    fn a_zero_cap_is_all_marker() {
        assert_eq!(ellipsize("abc", 0), "…");
        assert_eq!(ellipsize("", 0), "");
    }
}
