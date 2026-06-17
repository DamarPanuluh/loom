pub(crate) fn push_unique_nonempty(out: &mut Vec<String>, item: String) {
    if !item.is_empty() && !out.contains(&item) {
        out.push(item);
    }
}
