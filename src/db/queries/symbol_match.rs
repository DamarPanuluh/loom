pub fn symbol_identifier(symbol: &str) -> &str {
    symbol
        .split_whitespace()
        .last()
        .unwrap_or(symbol)
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
}

pub fn contains_identifier_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let bytes = haystack.as_bytes();
    let needle_bytes = needle.as_bytes();
    for (idx, _) in haystack.match_indices(needle) {
        let before = idx
            .checked_sub(1)
            .and_then(|i| bytes.get(i))
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
        let after_idx = idx + needle_bytes.len();
        let after = bytes
            .get(after_idx)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
        if !before && !after {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod identifier_word_tests {
    use super::contains_identifier_word;

    #[test]
    fn matches_whole_identifiers_not_sub_tokens() {
        // The exact symbol matches.
        assert!(contains_identifier_word("fn add_tax", "add_tax"));
        assert!(contains_identifier_word("pub fn add", "add"));
        // A sub-token must NOT match (the sync false-positive this guards): a
        // change to `add` must not re-open a grounding on `add_tax`.
        assert!(!contains_identifier_word("fn add_tax", "add"));
        assert!(!contains_identifier_word("fn tax_adder", "add"));
        assert!(!contains_identifier_word("fn readd", "add"));
        // `_` is part of the identifier.
        assert!(!contains_identifier_word("fn add_tax_v2", "tax"));
    }
}
