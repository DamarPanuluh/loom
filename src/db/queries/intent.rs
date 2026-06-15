//! Intent analysis helpers.

use anyhow::Result;

use crate::types::Intent;

use super::snapshot::QuerySnapshot;

/// Resolve an intent key — exact id, exact name (case-insensitive), or a
/// unique name fragment — to the intent's id. Ambiguity is an error that lists
/// candidates, so resolution is never a guess.
pub fn resolve_intent_from_snapshot(snapshot: &QuerySnapshot, key: &str) -> Result<String> {
    try_resolve_intent_from_snapshot(snapshot, key)?.ok_or_else(|| {
        anyhow::anyhow!(
            "No intent matches '{}' (by id, exact name, or name fragment). Run `loom intent list`.",
            key
        )
    })
}

/// Resolution with an honest "nothing matches" channel: Ok(None) only when no
/// intent matches by id, exact name, or fragment. Ambiguity stays an error.
pub fn try_resolve_intent_from_snapshot(
    snapshot: &QuerySnapshot,
    key: &str,
) -> Result<Option<String>> {
    try_resolve_intent_from_list(&snapshot.intents, key)
}

fn try_resolve_intent_from_list(intents: &[Intent], key: &str) -> Result<Option<String>> {
    if intents.iter().any(|i| i.id == key) {
        return Ok(Some(key.to_string()));
    }

    let kl = key.to_lowercase();
    let exact: Vec<_> = intents
        .iter()
        .filter(|i| i.name.to_lowercase() == kl)
        .collect();
    if exact.len() == 1 {
        return Ok(Some(exact[0].id.clone()));
    }
    if exact.len() > 1 {
        anyhow::bail!(
            "Intent name '{}' is not unique ({} intents carry it) — use the id. `loom intent list` to see them.",
            key,
            exact.len()
        );
    }

    let subs: Vec<_> = intents
        .iter()
        .filter(|i| i.name.to_lowercase().contains(&kl))
        .collect();
    match subs.len() {
        1 => Ok(Some(subs[0].id.clone())),
        0 => Ok(None),
        _ => {
            let total = subs.len();
            let shown = subs
                .iter()
                .take(10)
                .map(|i| format!("'{}'", i.name))
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
            )
        }
    }
}

/// What retiring an intent breaks — computed before the retire so the command
/// can report the work it triggered.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RetireFallout {
    pub orphaned_children: Vec<String>,
    pub solely_grounded_files: Vec<String>,
    pub dangling_validations: Vec<String>,
    pub edges_leaving_computation: usize,
}

/// What one intent redefinition staled — the counts `loom intent update`
/// reports.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RedefinitionRipple {
    pub relates_to_flagged: usize,
    pub governs_flagged: usize,
    pub targets_flagged: usize,
    pub implements_flagged: usize,
    pub validations_invalidated: usize,
}
