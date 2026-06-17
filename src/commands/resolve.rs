use anyhow::Result;

use crate::db::GraphReadRepository;

pub(crate) fn resolve_intent_with_db(db: &dyn GraphReadRepository, key: &str) -> Result<String> {
    let intents = db.list_intents(None, None)?;
    if intents.iter().any(|intent| intent.id == key) {
        return Ok(key.to_string());
    }
    let lower = key.to_lowercase();
    let exact: Vec<_> = intents
        .iter()
        .filter(|intent| intent.name.to_lowercase() == lower)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0].id.clone());
    }
    if exact.len() > 1 {
        anyhow::bail!(
            "Intent name '{}' is not unique ({} intents carry it) — use the id. `loom intent list` to see them.",
            key,
            exact.len()
        );
    }
    let matches: Vec<_> = intents
        .iter()
        .filter(|intent| intent.name.to_lowercase().contains(&lower))
        .collect();
    match matches.len() {
        1 => Ok(matches[0].id.clone()),
        0 => anyhow::bail!(
            "No intent matches '{}' (by id, exact name, or name fragment). Run `loom intent list`.",
            key
        ),
        _ => {
            let total = matches.len();
            let shown = matches
                .iter()
                .take(10)
                .map(|intent| format!("'{}'", intent.name))
                .collect::<Vec<_>>()
                .join(", ");
            if total > 10 {
                anyhow::bail!(
                    "'{}' is ambiguous — it matches: {} … +{} more — narrow the fragment or `loom find \"{}\"`.",
                    key,
                    shown,
                    total - 10,
                    key
                );
            }
            anyhow::bail!(
                "'{}' is ambiguous — it matches: {}. Narrow the fragment or use an id.",
                key,
                shown
            );
        }
    }
}

pub(crate) fn resolve_validation_with_db(
    db: &dyn GraphReadRepository,
    key: &str,
) -> Result<String> {
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
        0 => anyhow::bail!(
            "No validation matches '{}' (by id, name, or fragment). Run `loom validation list`.",
            key
        ),
        _ => anyhow::bail!(
            "'{}' is ambiguous — matches {} validations. Use the id (`loom validation list`).",
            key,
            matches.len()
        ),
    }
}
