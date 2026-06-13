//! Intent node queries.

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::Intent;

use super::row::{col_map, get, str_val};

pub fn insert_intent(db: &dyn LoomDb, intent: &Intent) -> Result<()> {
    // Param-bound: name/description/domain/layer are agent-written free text.
    db.execute_with_params(
        "INSERT (:Intent {id: $id, name: $name, description: $desc, \
         abstraction_level: $level, domain: $domain, layer: $layer, source_refs: $refs, \
         status: $status, aspect: $aspect, tags: $tags, visibility: $vis, \
         lifecycle: $lifecycle, created_at: $created, updated_at: $updated})",
        {
            let mut p = super::row::sparams(&[
                ("id", &intent.id),
                ("name", &intent.name),
                ("desc", &intent.description),
                ("level", &intent.abstraction_level),
                ("domain", &intent.domain),
                ("layer", &intent.layer),
                ("status", &intent.status),
                ("aspect", &intent.aspect),
                ("vis", &intent.visibility),
                ("lifecycle", &intent.lifecycle),
                ("created", &intent.created_at),
                ("updated", &intent.updated_at),
            ]);
            p.insert("refs".into(), super::row::list_param(&intent.source_refs));
            p.insert("tags".into(), super::row::list_param(&intent.tags));
            p
        },
    )?;
    Ok(())
}

/// Resolve an intent key — exact id, exact name (case-insensitive), or a
/// unique name fragment — to the intent's id. The natural key a driver has in
/// hand is the *name*; forcing UUIDs taxed every command with a lookup
/// round-trip (dogfood finding). Ambiguity is an error that lists the
/// candidates, so resolution is never a guess.
pub fn resolve_intent(db: &dyn LoomDb, key: &str) -> Result<String> {
    try_resolve_intent(db, key)?.ok_or_else(|| {
        anyhow::anyhow!(
            "No intent matches '{}' (by id, exact name, or name fragment). Run `loom intent list`.",
            key
        )
    })
}

/// Resolution with an honest "nothing matches" channel: Ok(None) ONLY when no
/// intent matches by id, exact name, or fragment. Ambiguity stays an error —
/// a caller that creates on None (the journey-first saga entrance) must never
/// mint a twin because a fragment matched two names.
pub fn try_resolve_intent(db: &dyn LoomDb, key: &str) -> Result<Option<String>> {
    let intents = list_intents(db, None, None)?;
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
            key, exact.len()
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
                    key, shown, total - 10, key
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
    let mut p = super::row::sparams(&[("id", id), ("updated", updated_at)]);
    p.insert("refs".into(), super::row::list_param(refs));
    db.execute_with_params(
        "MATCH (n:Intent {id: $id}) SET n.source_refs = $refs, n.updated_at = $updated",
        p,
    )?;
    Ok(())
}

fn parse_source_refs(intent: &Intent) -> Result<Vec<String>> {
    // Native list since schema v5 — kept as a function so call sites read the
    // same; the malformed-JSON failure class is gone by construction.
    Ok(intent.source_refs.clone())
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
        esc(id),
        esc(lifecycle),
        esc(updated_at)
    ))?;
    // Lifecycle changes are the intent-level recurrence signal (an intent
    // that keeps returning to needs_change is a hotspot of trouble).
    super::note::record_transition(
        db,
        "intent",
        id,
        &prev.lifecycle,
        lifecycle,
        "loom",
        updated_at,
    )?;
    Ok(true)
}

pub fn get_intent(db: &dyn LoomDb, id: &str) -> Result<Option<Intent>> {
    let q = format!(
        "MATCH (n:Intent {{id: '{}'}}) \
         RETURN n.id, n.name, n.description, n.abstraction_level, n.domain, n.layer, \
                n.source_refs, n.status, n.aspect, n.tags, n.visibility, \
                n.lifecycle, n.created_at, n.updated_at",
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
         RETURN n.id, n.name, n.description, n.abstraction_level, n.domain, n.layer, \
                n.source_refs, n.status, n.aspect, n.tags, n.visibility, \
                n.lifecycle, n.created_at, n.updated_at \
         ORDER BY n.name",
        where_clause
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result
        .rows()
        .iter()
        .map(|row| row_to_intent(row, &cols))
        .collect())
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
    let mut owners: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for im in super::implements::list_all_implements(db)? {
        owners
            .entry(im.codefile_path)
            .or_default()
            .push(im.intent_id);
    }
    let mut solely_grounded_files: Vec<String> = owners
        .into_iter()
        .filter(|(_, os)| os.contains(&id.to_string()) && !os.iter().any(|o| active.contains(o)))
        .map(|(p, _)| p)
        .collect();
    solely_grounded_files.sort();

    // Validations whose every linked intent is retired once this one goes.
    let mut linked: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut vname: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for e in super::validates::list_all_validates(db)? {
        linked
            .entry(e.validation_id.clone())
            .or_default()
            .push(e.intent_id);
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
///
/// Retirement RIPPLES like a redefinition (the codefile analogy: a changed
/// file stales the claims earned against it): every verified RELATES_TO
/// verdict touching this intent was earned in a world where it existed, so
/// `passing`/`independent` flip to needs_reverification with a sync-flip note.
/// The edges themselves leave computation with the retired end (history), but
/// the flip notes are the churn signal `align_candidates` counts — the living
/// neighbours become drift suspects and surface in `loom next --mode align`
/// for the user to re-affirm. One hop, like sync. Other edge kinds stay: the
/// retired intent's own IMPLEMENTS/GOVERNS/VALIDATES leave computation with
/// it, and `retire_fallout` already reports what they strand.
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
        esc(id),
        esc(now)
    ))?;
    super::note::record_transition(db, "intent", id, &prev.status, "deprecated", "loom", now)?;
    let cause = format!("intent '{}' retired", prev.name);
    for edge in super::relates_to::edges_for_intent(db, id)? {
        if edge.inspection_status == "passing" || edge.inspection_status == "independent" {
            db.execute(&format!(
                "MATCH (a:Intent {{id: '{from}'}})-[r:RELATES_TO]->(b:Intent {{id: '{to}'}}) \
                 SET r.inspection_status = 'needs_reverification'",
                from = esc(&edge.from_id),
                to = esc(&edge.to_id),
            ))?;
            super::note::record_sync_flip(
                db,
                "edge",
                &edge.id,
                &edge.inspection_status,
                "needs_reverification",
                &cause,
                now,
            )?;
        }
    }
    let text = match replaced_by {
        Some(s) => format!("retired: {reason} — replaced by intent {s}"),
        None => format!("retired: {reason}"),
    };
    super::note::insert_note(
        db,
        &crate::types::Note {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "decision".into(),
            text,
            author: "loom".into(),
            target_kind: "intent".into(),
            target_id: id.to_string(),
            audience: String::new(),
            created_at: now.to_string(),
        },
    )?;
    Ok(true)
}

pub fn confirm_intent(db: &dyn LoomDb, id: &str, updated_at: &str) -> Result<bool> {
    let check = db.execute(&format!(
        "MATCH (n:Intent {{id: '{}'}}) RETURN n.id",
        esc(id)
    ))?;
    if check.rows().is_empty() {
        return Ok(false);
    }
    db.execute(&format!(
        "MATCH (n:Intent {{id: '{}'}}) SET n.status = 'confirmed', n.updated_at = '{}'",
        esc(id),
        esc(updated_at)
    ))?;
    Ok(true)
}

/// Set who the behavior is for: "user_visible" | "internal" | "" (untriaged).
/// "" clears the ruling — `loom intent update --description` does exactly
/// that, because a redefined meaning's audience is unknown again. Returns
/// false when the intent doesn't exist.
pub fn set_intent_visibility(
    db: &dyn LoomDb,
    id: &str,
    visibility: &str,
    updated_at: &str,
) -> Result<bool> {
    if !matches!(visibility, "" | "user_visible" | "internal") {
        anyhow::bail!(
            "Invalid visibility '{visibility}'. Valid: user_visible (a capability the user can \
             see/feel) | internal (machinery serving other intents)."
        );
    }
    let check = db.execute(&format!(
        "MATCH (n:Intent {{id: '{}'}}) RETURN n.id",
        esc(id)
    ))?;
    if check.rows().is_empty() {
        return Ok(false);
    }
    db.execute(&format!(
        "MATCH (n:Intent {{id: '{}'}}) SET n.visibility = '{}', n.updated_at = '{}'",
        esc(id),
        esc(visibility),
        esc(updated_at)
    ))?;
    Ok(true)
}

/// Set an intent's architecture layer. This is metadata about where the
/// responsibility sits, not a semantic redefinition, so it does not stale
/// earned verdicts. Returns false when the intent doesn't exist.
pub fn set_intent_layer(db: &dyn LoomDb, id: &str, layer: &str, updated_at: &str) -> Result<bool> {
    if get_intent(db, id)?.is_none() {
        return Ok(false);
    }
    db.execute_with_params(
        "MATCH (n:Intent {id: $id}) SET n.layer = $layer, n.updated_at = $updated",
        super::row::sparams(&[("id", id), ("layer", layer), ("updated", updated_at)]),
    )?;
    Ok(true)
}

/// Record a confirmation EVENT — the freshness stamp the align queue ranks by.
/// Status alone can't carry freshness ("confirmed" is sticky; re-confirming is
/// a no-op on the node), so each ratification lands as an append-only note:
/// kind="confirm", target=the intent. "When did a human last re-affirm this
/// meaning?" = the newest such note. Notes travel in the export, so alignment
/// history survives a re-import — no schema field, no migration.
pub fn record_confirmation(db: &dyn LoomDb, id: &str, author: &str, now: &str) -> Result<()> {
    super::note::insert_note(
        db,
        &crate::types::Note {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "confirm".into(),
            text: "meaning re-affirmed".into(),
            author: author.to_string(),
            target_kind: "intent".into(),
            target_id: id.to_string(),
            audience: String::new(),
            created_at: now.to_string(),
        },
    )
}

/// Newest confirmation stamp for an intent (rfc3339), None = never confirmed.
/// `list_notes` returns newest LAST; confirm events are append-only, so the
/// tail is the latest ratification. Production reads the stamps in bulk
/// (`align_candidates`); this per-intent form remains as the contract's test
/// surface.
#[cfg(test)]
pub fn last_confirmed_at(db: &dyn LoomDb, intent_id: &str) -> Result<Option<String>> {
    Ok(
        super::note::list_notes(db, Some(intent_id), Some("confirm"))?
            .pop()
            .map(|n| n.created_at),
    )
}

/// Update an intent's name and/or description in place — design EVOLUTION,
/// distinct from retirement: same node, same id, same edge and note history;
/// only the meaning statement moves. Free text goes through $params
/// (agent/user-written). Returns false when the intent doesn't exist.
pub fn update_intent_meaning(
    db: &dyn LoomDb,
    id: &str,
    name: Option<&str>,
    description: Option<&str>,
    updated_at: &str,
) -> Result<bool> {
    if get_intent(db, id)?.is_none() {
        return Ok(false);
    }
    let mut sets = vec!["n.updated_at = $updated"];
    let mut pairs: Vec<(&str, &str)> = vec![("id", id), ("updated", updated_at)];
    if let Some(n) = name {
        sets.push("n.name = $name");
        pairs.push(("name", n));
    }
    if let Some(d) = description {
        sets.push("n.description = $desc");
        pairs.push(("desc", d));
    }
    db.execute_with_params(
        &format!("MATCH (n:Intent {{id: $id}}) SET {}", sets.join(", ")),
        super::row::sparams(&pairs),
    )?;
    Ok(true)
}

/// What one redefinition staled — the counts `loom intent update` reports.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RedefinitionRipple {
    pub relates_to_flagged: usize,
    pub governs_flagged: usize,
    pub targets_flagged: usize,
    pub implements_flagged: usize,
    pub validations_invalidated: usize,
}

/// The semantic twin of the `loom sync` ripple. Sync flips claims when the
/// CODE under an intent changes; this flips them when the intent's MEANING
/// changes — every earned verdict touching it was earned against the old
/// wording, so green must be re-earned against the new one. One hop, like
/// sync. Differences from the sync flip set, deliberate:
/// - IMPLEMENTS flips here (sync only flips it on a missing locator): the
///   grounding claim is "this code does what the intent says" — redefinition
///   stales it even though the code is byte-identical.
/// - GOVERNS `independent` flips here (sync flips `passing` only): "this rule
///   doesn't apply" was judged against the old meaning; a code change can't
///   alter what an intent MEANS, but a redefinition is exactly that.
/// The cause string ends in "redefined", never " changed", so
/// `parse_sync_cause` returns None and the hot-FILE grouping in
/// `loom next --take` is never polluted with a non-file.
pub fn ripple_intent_redefinition(
    db: &dyn LoomDb,
    intent_id: &str,
    intent_name: &str,
    now: &str,
) -> Result<RedefinitionRipple> {
    let cause = format!("intent '{intent_name}' redefined");
    let mut r = RedefinitionRipple::default();

    // RELATES_TO: same flip set as sync (passing | independent).
    for edge in super::relates_to::edges_for_intent(db, intent_id)? {
        if edge.inspection_status == "passing" || edge.inspection_status == "independent" {
            db.execute(&format!(
                "MATCH (a:Intent {{id: '{from}'}})-[r:RELATES_TO]->(b:Intent {{id: '{to}'}}) \
                 SET r.inspection_status = 'needs_reverification'",
                from = esc(&edge.from_id),
                to = esc(&edge.to_id),
            ))?;
            super::note::record_sync_flip(
                db,
                "edge",
                &edge.id,
                &edge.inspection_status,
                "needs_reverification",
                &cause,
                now,
            )?;
            r.relates_to_flagged += 1;
        }
    }

    // GOVERNS: passing AND independent (see above — broader than sync).
    for g in super::governs::list_governs_for_intent(db, intent_id)? {
        if g.inspection_status == "passing" || g.inspection_status == "independent" {
            db.execute(&format!(
                "MATCH (r:QualityRule {{id: '{rid}'}})-[e:GOVERNS]->(i:Intent {{id: '{iid}'}}) \
                 SET e.inspection_status = 'needs_reverification'",
                rid = esc(&g.rule_id),
                iid = esc(intent_id),
            ))?;
            super::note::record_sync_flip(
                db,
                "edge",
                &g.id,
                &g.inspection_status,
                "needs_reverification",
                &cause,
                now,
            )?;
            r.governs_flagged += 1;
        }
    }

    // TARGETS: a supported hypothesis was proven against the old claim text.
    for t in super::targets::list_all_targets(db)? {
        if t.intent_id == intent_id && t.inspection_status == "passing" {
            db.execute(&format!(
                "MATCH (h:Hypothesis {{id: '{hid}'}})-[e:TARGETS]->(i:Intent {{id: '{iid}'}}) \
                 SET e.inspection_status = 'needs_reverification', e.notes = '{notes}'",
                hid = esc(&t.hypothesis_id),
                iid = esc(intent_id),
                notes = esc(&format!("stale: {cause}")),
            ))?;
            super::note::record_sync_flip(
                db,
                "edge",
                &t.id,
                "passing",
                "needs_reverification",
                &cause,
                now,
            )?;
            r.targets_flagged += 1;
        }
    }

    // IMPLEMENTS: the grounding claim itself (see above).
    for im in super::implements::list_implements_for_intent(db, intent_id)? {
        if im.inspection_status == "passing" {
            db.execute(&format!(
                "MATCH (i:Intent {{id: '{iid}'}})-[e:IMPLEMENTS]->(cf:CodeFile {{id: '{cfid}'}}) \
                 SET e.inspection_status = 'needs_reverification'",
                iid = esc(intent_id),
                cfid = esc(&im.codefile_id),
            ))?;
            super::note::record_sync_flip(
                db,
                "edge",
                &im.id,
                "passing",
                "needs_reverification",
                &cause,
                now,
            )?;
            r.implements_flagged += 1;
        }
    }

    // Linked proofs: passed runs proved the OLD acceptance contract. Skip
    // `blocked` (waiting on something external; flipping would erase the
    // recorded reason) and already-not_run — same skip set as sync.
    for edge in super::validates::list_validates_for_intent(db, intent_id)? {
        if let Some(v) = super::validation::get_validation(db, &edge.validation_id)? {
            if v.last_result != "not_run" && v.last_result != "blocked" && !v.last_result.is_empty()
            {
                db.execute(&format!(
                    "MATCH (v:Validation {{id: '{}'}}) SET v.last_result = 'not_run'",
                    esc(&v.id)
                ))?;
                r.validations_invalidated += 1;
            }
        }
    }

    Ok(r)
}

/// Hard-delete an intent: the node, every edge touching it, and any notes
/// targeting it. Returns false if the intent didn't exist.
pub fn delete_intent(db: &dyn LoomDb, id: &str) -> Result<bool> {
    let check = db.execute(&format!(
        "MATCH (n:Intent {{id: '{}'}}) RETURN n.id",
        esc(id)
    ))?;
    if check.rows().is_empty() {
        return Ok(false);
    }
    // DETACH DELETE removes the node and all edges connected to it.
    db.execute(&format!(
        "MATCH (n:Intent {{id: '{}'}}) DETACH DELETE n",
        esc(id)
    ))?;
    // Notes reference the intent by target_id (not a graph edge), so prune them.
    db.execute(&format!(
        "MATCH (note:Note) WHERE note.target_id = '{}' DETACH DELETE note",
        esc(id)
    ))?;
    // The DETACH DELETE above also killed this intent's edges — prune the
    // notes attached to THOSE (derived edge keys embed the intent id), or
    // they dangle forever (the v3-era bug `loom note prune` cleans up).
    super::note::prune_edge_notes_touching(db, id)?;
    Ok(true)
}

/// Return all intents that have zero VALIDATES edges pointing to them.
pub fn intents_without_validations(db: &dyn LoomDb) -> Result<Vec<Intent>> {
    let validated: std::collections::HashSet<String> = super::validates::list_all_validates(db)?
        .into_iter()
        .map(|e| e.intent_id)
        .collect();
    Ok(list_intents(db, None, None)?
        .into_iter()
        .filter(|intent| !validated.contains(&intent.id))
        .collect())
}

fn row_to_intent(row: &[Value], cols: &HashMap<&str, usize>) -> Intent {
    Intent {
        id: str_val(get(row, cols, "n.id")),
        name: str_val(get(row, cols, "n.name")),
        description: str_val(get(row, cols, "n.description")),
        abstraction_level: str_val(get(row, cols, "n.abstraction_level")),
        domain: str_val(get(row, cols, "n.domain")),
        layer: str_val(get(row, cols, "n.layer")),
        source_refs: super::row::list_val(get(row, cols, "n.source_refs")),
        status: str_val(get(row, cols, "n.status")),
        aspect: str_val(get(row, cols, "n.aspect")),
        // Additive in v3, native list in v5: Null on intents from older
        // graphs reads as empty (= untagged); legacy JSON strings parse.
        tags: super::row::list_val(get(row, cols, "n.tags")),
        // Additive: Null on intents from older graphs reads as "" (untriaged).
        visibility: str_val(get(row, cols, "n.visibility")),
        lifecycle: str_val(get(row, cols, "n.lifecycle")),
        created_at: str_val(get(row, cols, "n.created_at")),
        updated_at: str_val(get(row, cols, "n.updated_at")),
    }
}
