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

/// Cap `text` at `limit` BYTES, keeping both ends and disclosing the cut.
///
/// The tail is as load-bearing as the head for machine output — a test runner
/// prints its verdict last — so a head-only clip drops exactly the line a
/// reader needs. Cuts are moved to char boundaries, and each end is trimmed so
/// the marker does not land against ragged whitespace.
///
/// Bytes, not characters, because callers here are bounding captured process
/// output against a byte budget; [`ellipsize`] is the character-cap rule for
/// rendered text.
pub fn bounded_head_tail(text: &str, limit: usize, marker: &str) -> String {
    if text.len() <= limit {
        return text.trim().to_string();
    }
    let half = limit / 2;
    let mut head = half;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - half;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!(
        "{}\n{marker}\n{}",
        text[..head].trim_end(),
        text[tail..].trim_start()
    )
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
    fn head_and_tail_survive_a_bounded_cut() {
        // The tail is the half a head-only clip would drop, and it is the half
        // a runner verdict lives in.
        let text = format!("HEAD{}TAIL", "x".repeat(200));
        let out = bounded_head_tail(&text, 40, "...[cut]...");
        assert!(out.starts_with("HEAD"), "head kept: {out}");
        assert!(out.ends_with("TAIL"), "tail kept: {out}");
        assert!(out.contains("...[cut]..."), "cut disclosed: {out}");
        assert!(out.len() < text.len());
    }

    #[test]
    fn text_within_the_budget_is_only_trimmed() {
        assert_eq!(bounded_head_tail("  short  ", 100, "...[cut]..."), "short");
    }

    /// The budget is in bytes, so a naive slice would land inside a multi-byte
    /// character and panic. Both cuts move to a char boundary first.
    #[test]
    fn a_byte_budget_never_splits_a_character() {
        let text = "日本語".repeat(50);
        let out = bounded_head_tail(&text, 41, "...[cut]...");
        assert!(out.contains("...[cut]..."));
        assert!(out.chars().count() > 0);
    }

    #[test]
    fn a_zero_cap_is_all_marker() {
        assert_eq!(ellipsize("abc", 0), "…");
        assert_eq!(ellipsize("", 0), "");
    }
}
