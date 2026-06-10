//! Intent node queries.

use anyhow::{Context, Result};
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::Intent;

use super::row::{col_map, get, i64_val, str_val};

pub fn insert_intent(db: &dyn LoomDb, intent: &Intent) -> Result<()> {
    let q = format!(
        "INSERT (:Intent {{id: '{id}', name: '{name}', description: '{desc}', \
         abstraction_level: '{level}', domain: '{domain}', source_refs: '{refs}', \
         status: '{status}', aspect: '{aspect}', lifecycle: '{lifecycle}', \
         created_at: '{created}', updated_at: '{updated}'}})",
        id        = esc(&intent.id),
        name      = esc(&intent.name),
        desc      = esc(&intent.description),
        level     = esc(&intent.abstraction_level),
        domain    = esc(&intent.domain),
        refs      = esc(&intent.source_refs),
        status    = esc(&intent.status),
        aspect    = esc(&intent.aspect),
        lifecycle = esc(&intent.lifecycle),
        created   = esc(&intent.created_at),
        updated   = esc(&intent.updated_at),
    );
    db.execute(&q)?;
    Ok(())
}

/// Resolve an intent key — exact id, exact name (case-insensitive), or a
/// unique name fragment — to the intent's id. The natural key a driver has in
/// hand is the *name*; forcing UUIDs taxed every command with a lookup
/// round-trip (dogfood finding). Ambiguity is an error that lists the
/// candidates, so resolution is never a guess.
pub fn resolve_intent(db: &dyn LoomDb, key: &str) -> Result<String> {
    let intents = list_intents(db, None, None)?;
    if intents.iter().any(|i| i.id == key) {
        return Ok(key.to_string());
    }
    let kl = key.to_lowercase();
    let exact: Vec<_> = intents.iter().filter(|i| i.name.to_lowercase() == kl).collect();
    if exact.len() == 1 {
        return Ok(exact[0].id.clone());
    }
    if exact.len() > 1 {
        anyhow::bail!(
            "Intent name '{}' is not unique ({} intents carry it) — use the id. `loom intent list` to see them.",
            key, exact.len()
        );
    }
    let subs: Vec<_> = intents.iter().filter(|i| i.name.to_lowercase().contains(&kl)).collect();
    match subs.len() {
        1 => Ok(subs[0].id.clone()),
        0 => anyhow::bail!(
            "No intent matches '{}' (by id, exact name, or name fragment). Run `loom intent list`.",
            key
        ),
        _ => anyhow::bail!(
            "'{}' is ambiguous — it matches: {}. Narrow the fragment or use an id.",
            key,
            subs.iter().map(|i| format!("'{}'", i.name)).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// Append a path to an intent's `source_refs` (the canonical-source list: code
/// AND docs — contracts, ADRs, design notes). Idempotent: re-adding an existing
/// ref is a no-op. Returns false when the intent doesn't exist.
pub fn add_source_ref(db: &dyn LoomDb, id: &str, path: &str, updated_at: &str) -> Result<bool> {
    let Some(intent) = get_intent(db, id)? else {
        return Ok(false);
    };
    let mut refs = parse_source_refs(&intent)?;
    if !refs.iter().any(|r| r == path) {
        refs.push(path.to_string());
        set_source_refs(db, id, &refs, updated_at)?;
    }
    Ok(true)
}

/// Remove a path from an intent's `source_refs`. Returns Ok(None) when the
/// intent doesn't exist, Ok(Some(false)) when the ref wasn't present.
pub fn remove_source_ref(
    db: &dyn LoomDb,
    id: &str,
    path: &str,
    updated_at: &str,
) -> Result<Option<bool>> {
    let Some(intent) = get_intent(db, id)? else {
        return Ok(None);
    };
    let mut refs = parse_source_refs(&intent)?;
    let before = refs.len();
    refs.retain(|r| r != path);
    if refs.len() == before {
        return Ok(Some(false));
    }
    set_source_refs(db, id, &refs, updated_at)?;
    Ok(Some(true))
}

fn set_source_refs(db: &dyn LoomDb, id: &str, refs: &[String], updated_at: &str) -> Result<()> {
    db.execute(&format!(
        "MATCH (n:Intent {{id: '{}'}}) SET n.source_refs = '{}', n.updated_at = '{}'",
        esc(id),
        esc(&serde_json::to_string(refs)?),
        esc(updated_at)
    ))?;
    Ok(())
}

fn parse_source_refs(intent: &Intent) -> Result<Vec<String>> {
    serde_json::from_str(&intent.source_refs)
        .with_context(|| format!("Intent '{}' has malformed source_refs JSON", intent.id))
}

/// Set an intent's lifecycle (planned | implemented | needs_change).
pub fn set_intent_lifecycle(
    db: &dyn LoomDb,
    id: &str,
    lifecycle: &str,
    updated_at: &str,
) -> Result<bool> {
    let Some(prev) = get_intent(db, id)? else {
        return Ok(false);
    };
    db.execute(&format!(
        "MATCH (n:Intent {{id: '{}'}}) SET n.lifecycle = '{}', n.updated_at = '{}'",
        esc(id), esc(lifecycle), esc(updated_at)
    ))?;
    // Lifecycle changes are the intent-level recurrence signal (an intent
    // that keeps returning to needs_change is a hotspot of trouble).
    super::note::record_transition(db, "intent", id, &prev.lifecycle, lifecycle, "loom", updated_at)?;
    Ok(true)
}

pub fn get_intent(db: &dyn LoomDb, id: &str) -> Result<Option<Intent>> {
    let q = format!(
        "MATCH (n:Intent {{id: '{}'}}) \
         RETURN n.id, n.name, n.description, n.abstraction_level, n.domain, \
                n.source_refs, n.status, n.aspect, n.lifecycle, n.created_at, n.updated_at",
        esc(id)
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().first().map(|row| row_to_intent(row, &cols)))
}

pub fn list_intents(
    db: &dyn LoomDb,
    status_filter: Option<&str>,
    level_filter: Option<&str>,
) -> Result<Vec<Intent>> {
    let mut conditions = Vec::new();
    if let Some(s) = status_filter {
        conditions.push(format!("n.status = '{}'", esc(s)));
    }
    if let Some(l) = level_filter {
        conditions.push(format!("n.abstraction_level = '{}'", esc(l)));
    }
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };
    let q = format!(
        "MATCH (n:Intent){} \
         RETURN n.id, n.name, n.description, n.abstraction_level, n.domain, \
                n.source_refs, n.status, n.aspect, n.lifecycle, n.created_at, n.updated_at \
         ORDER BY n.name",
        where_clause
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| row_to_intent(row, &cols)).collect())
}

/// Intents that participate in computation: everything except `deprecated`.
/// THE RETIREMENT CONTRACT: a retired intent is INVISIBLE TO COMPUTATION,
/// VISIBLE TO HISTORY — its node, edges, and notes remain (the record), but
/// queues, coverage axes, centrality, the N×N grid, ripple, and completeness
/// must not count it. Every consumer that means "the live design" calls this.
pub fn list_active_intents(db: &dyn LoomDb) -> Result<Vec<Intent>> {
    Ok(list_intents(db, None, None)?
        .into_iter()
        .filter(|i| i.status != "deprecated")
        .collect())
}

/// What retiring this intent breaks — computed BEFORE the retire so the
/// command can report the triggered work, and re-checkable any time after.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RetireFallout {
    /// Children whose parent is now retired — re-parent (`loom edge hierarchy`)
    /// or retire them too; until then they read as roots.
    pub orphaned_children: Vec<String>,
    /// Files this intent was the ONLY active owner of — they become unreached
    /// (vertical gap) until re-grounded under a successor or ignored.
    pub solely_grounded_files: Vec<String>,
    /// Validations whose only active intent was this one — dangling specs.
    pub dangling_validations: Vec<String>,
    /// RELATES_TO edges that leave every computation with this retirement.
    pub edges_leaving_computation: usize,
}

pub fn retire_fallout(db: &dyn LoomDb, id: &str) -> Result<RetireFallout> {
    let name_of: std::collections::HashMap<String, String> = list_intents(db, None, None)?
        .into_iter()
        .map(|i| (i.id.clone(), i.name))
        .collect();
    let active: std::collections::HashSet<String> = list_active_intents(db)?
        .into_iter()
        .filter(|i| i.id != id)
        .map(|i| i.id)
        .collect();

    let mut orphaned_children: Vec<String> = super::hierarchy::list_all_hierarchy(db)?
        .into_iter()
        .filter(|(p, c)| p == id && active.contains(c))
        .map(|(_, c)| name_of.get(&c).cloned().unwrap_or(c))
        .collect();
    orphaned_children.sort();

    // Files where this intent is the only ACTIVE owner.
    let mut owners: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for im in super::implements::list_all_implements(db)? {
        owners.entry(im.codefile_path).or_default().push(im.intent_id);
    }
    let mut solely_grounded_files: Vec<String> = owners
        .into_iter()
        .filter(|(_, os)| os.contains(&id.to_string()) && !os.iter().any(|o| active.contains(o)))
        .map(|(p, _)| p)
        .collect();
    solely_grounded_files.sort();

    // Validations whose every linked intent is retired once this one goes.
    let mut linked: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let mut vname: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for e in super::validates::list_all_validates(db)? {
        linked.entry(e.validation_id.clone()).or_default().push(e.intent_id);
        vname.insert(e.validation_id, e.validation_name);
    }
    let mut dangling_validations: Vec<String> = linked
        .into_iter()
        .filter(|(_, is)| is.contains(&id.to_string()) && !is.iter().any(|i| active.contains(i)))
        .map(|(v, _)| vname.get(&v).cloned().unwrap_or(v))
        .collect();
    dangling_validations.sort();

    let edges_leaving_computation = super::relates_to::list_relates_to(db, None)?
        .into_iter()
        .filter(|e| e.from_id == id || e.to_id == id)
        .count();

    Ok(RetireFallout {
        orphaned_children,
        solely_grounded_files,
        dangling_validations,
        edges_leaving_computation,
    })
}

/// Retire an intent: status → deprecated, with the why (and the successor, if
/// any) recorded as notes. NOT a delete — deletion is for mistakes; retirement
/// is for design that was real and got superseded. Returns false if missing.
pub fn retire_intent(
    db: &dyn LoomDb,
    id: &str,
    reason: &str,
    replaced_by: Option<&str>,
    now: &str,
) -> Result<bool> {
    let Some(prev) = get_intent(db, id)? else {
        return Ok(false);
    };
    db.execute(&format!(
        "MATCH (n:Intent {{id: '{}'}}) SET n.status = 'deprecated', n.updated_at = '{}'",
        esc(id), esc(now)
    ))?;
    super::note::record_transition(db, "intent", id, &prev.status, "deprecated", "loom", now)?;
    let text = match replaced_by {
        Some(s) => format!("retired: {reason} — replaced by intent {s}"),
        None => format!("retired: {reason}"),
    };
    super::note::insert_note(db, &crate::types::Note {
        id: uuid::Uuid::new_v4().to_string(),
        kind: "decision".into(),
        text,
        author: "loom".into(),
        target_kind: "intent".into(),
        target_id: id.to_string(),
        audience: String::new(),
        created_at: now.to_string(),
    })?;
    Ok(true)
}

pub fn confirm_intent(db: &dyn LoomDb, id: &str, updated_at: &str) -> Result<bool> {
    let check = db.execute(&format!(
        "MATCH (n:Intent {{id: '{}'}}) RETURN n.id", esc(id)
    ))?;
    if check.rows().is_empty() {
        return Ok(false);
    }
    db.execute(&format!(
        "MATCH (n:Intent {{id: '{}'}}) SET n.status = 'confirmed', n.updated_at = '{}'",
        esc(id), esc(updated_at)
    ))?;
    Ok(true)
}

/// Hard-delete an intent: the node, every edge touching it, and any notes
/// targeting it. Returns false if the intent didn't exist.
pub fn delete_intent(db: &dyn LoomDb, id: &str) -> Result<bool> {
    let check = db.execute(&format!(
        "MATCH (n:Intent {{id: '{}'}}) RETURN n.id", esc(id)
    ))?;
    if check.rows().is_empty() {
        return Ok(false);
    }
    // DETACH DELETE removes the node and all edges connected to it.
    db.execute(&format!(
        "MATCH (n:Intent {{id: '{}'}}) DETACH DELETE n", esc(id)
    ))?;
    // Notes reference the intent by target_id (not a graph edge), so prune them.
    db.execute(&format!(
        "MATCH (note:Note) WHERE note.target_id = '{}' DETACH DELETE note", esc(id)
    ))?;
    Ok(true)
}

/// Return all intents that have zero VALIDATES edges pointing to them.
pub fn intents_without_validations(db: &dyn LoomDb) -> Result<Vec<Intent>> {
    let all = list_intents(db, None, None)?;
    let mut result = Vec::new();
    for intent in all {
        let q = format!(
            "MATCH ()-[e:VALIDATES]->(i:Intent {{id: '{}'}}) RETURN count(e) AS c",
            esc(&intent.id)
        );
        let r = db.execute(&q)?;
        let c = r.rows().first().map(|row| i64_val(&row[0])).unwrap_or(0);
        if c == 0 {
            result.push(intent);
        }
    }
    Ok(result)
}

fn row_to_intent(row: &[Value], cols: &HashMap<&str, usize>) -> Intent {
    Intent {
        id:               str_val(get(row, cols, "n.id")),
        name:             str_val(get(row, cols, "n.name")),
        description:      str_val(get(row, cols, "n.description")),
        abstraction_level:str_val(get(row, cols, "n.abstraction_level")),
        domain:           str_val(get(row, cols, "n.domain")),
        source_refs:      str_val(get(row, cols, "n.source_refs")),
        status:           str_val(get(row, cols, "n.status")),
        aspect:           str_val(get(row, cols, "n.aspect")),
        lifecycle:        str_val(get(row, cols, "n.lifecycle")),
        created_at:       str_val(get(row, cols, "n.created_at")),
        updated_at:       str_val(get(row, cols, "n.updated_at")),
    }
}
