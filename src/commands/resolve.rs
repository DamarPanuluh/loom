use anyhow::Result;

use crate::db::GraphReadRepository;

pub(crate) fn resolve_intent_with_db(db: &dyn GraphReadRepository, key: &str) -> Result<String> {
    // An empty/blank key (a common unset-variable accident) would `contains("")`-
    // match every intent and dump the whole list — refuse it cleanly instead.
    if key.trim().is_empty() {
        anyhow::bail!(
            "An intent identifier can't be empty — pass an id or a name fragment. `loom intent list` shows them."
        );
    }
    let intents = db.list_intents(None, None)?;
    // Delegate the id/exact/fragment matching + ambiguity contract to the
    // query-layer resolver so the matching logic and its error messages live in
    // one place; map its honest Ok(None) channel to the shared not-found message.
    match crate::db::queries::try_resolve_intent_from_list(&intents, key)? {
        Some(id) => Ok(id),
        None => anyhow::bail!(crate::db::queries::no_intent_match_message(key)),
    }
}

pub(crate) fn resolve_validation_with_db(
    db: &dyn GraphReadRepository,
    key: &str,
) -> Result<String> {
    if key.trim().is_empty() {
        anyhow::bail!(
            "A validation identifier can't be empty — pass an id or a name fragment. `loom validation list` shows them."
        );
    }
    let validations = db.list_validations()?;
    if validations.iter().any(|validation| validation.id == key) {
        return Ok(key.to_string());
    }
    let lower = key.to_lowercase();
    let exact: Vec<_> = validations
        .iter()
        .filter(|validation| validation.name.to_lowercase() == lower)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0].id.clone());
    }
    let matches: Vec<_> = validations
        .iter()
        .filter(|validation| validation.name.to_lowercase().contains(&lower))
        .collect();
    match matches.len() {
        1 => Ok(matches[0].id.clone()),
        0 => anyhow::bail!("{}", crate::types::no_validation_match_message(key)),
        _ => anyhow::bail!(
            "'{}' is ambiguous — matches {} validations. Use the id (`loom validation list`).",
            key,
            matches.len()
        ),
    }
}
