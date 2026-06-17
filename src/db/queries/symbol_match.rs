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
