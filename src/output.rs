use serde::Serialize;

pub struct Printer {
    pub json: bool,
}

impl Printer {
    pub fn new(json: bool) -> Self {
        Self { json }
    }

    pub fn print_json<T: Serialize>(&self, value: &T) {
        // Compact, not pretty: the consumer is an LLM agent (pretty-printed
        // indentation is pure token spend; jq re-pretties for humans).
        let rendered = serde_json::to_string(value).unwrap_or_else(|e| {
            serde_json::to_string(&serde_json::json!({ "error": e.to_string() }))
                .expect("serializing JSON error object cannot fail")
        });
        println!("{rendered}");
    }
}

// ---------------------------------------------------------------------------
// Graph pulse — compact one-line situational awareness
// ---------------------------------------------------------------------------

/// Human-friendly relative time from an RFC3339 timestamp ("2h ago", "3d ago").
fn rel_time(rfc3339: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(rfc3339) {
        Ok(t) => {
            let secs = chrono::Utc::now()
                .signed_duration_since(t.with_timezone(&chrono::Utc))
                .num_seconds()
                .max(0);
            if secs < 60 {
                "just now".to_string()
            } else if secs < 3600 {
                format!("{}m ago", secs / 60)
            } else if secs < 86_400 {
                format!("{}h ago", secs / 3600)
            } else {
                format!("{}d ago", secs / 86_400)
            }
        }
        Err(_) => rfc3339.to_string(),
    }
}

/// One axis of the 360° line: "n/m" plus a check when closed, "—" when the
/// axis has no surface yet (never a vacuous 100%).
fn fmt_axis(a: &crate::db::queries::CoverageAxis) -> String {
    if a.total == 0 {
        "—".to_string()
    } else if a.done() {
        format!("{}/{} ✓", a.covered, a.total)
    } else {
        format!("{}/{}", a.covered, a.total)
    }
}

/// The five axes joined as one compact line — shared by the human pulse
/// (with the "360°: " prefix) and the JSON pulse (`coverage` field).
pub fn coverage_line(c: &crate::db::queries::Coverage360) -> String {
    format!(
        "grounded {} · realized {} · explored {} · measured {} · proven {}",
        fmt_axis(&c.grounded_files),
        fmt_axis(&c.realized_leaves),
        fmt_axis(&c.explored_pairs),
        fmt_axis(&c.measured_pairs),
        fmt_axis(&c.proven_leaves),
    )
}

/// The 360° coverage vector as one line — every vantage point counted, so the
/// driving LLM always sees which dimension is weakest without asking.
pub fn fmt_coverage(c: &crate::db::queries::Coverage360) -> String {
    format!("360°: {}", coverage_line(c))
}

/// One-line graph pulse for an LLM's quick look (shown as a footer).
/// Returns TWO lines: the pulse + the 360° coverage vector (callers print with
/// a two-space indent; the embedded newline carries the same indent).
pub fn fmt_pulse(s: &crate::db::queries::GraphState) -> String {
    let synced = if s.last_synced.is_empty() {
        "never synced".to_string()
    } else {
        format!("synced {}", rel_time(&s.last_synced))
    };
    let unexplored = if s.unexplored_pairs > 0 {
        format!(" · {} unexplored", s.unexplored_pairs)
    } else {
        String::new()
    };
    // Two completeness axes: vertical is binding (the spine), horizontal optional.
    let vert = if s.vertically_complete { "✓" } else { "✗" };
    let horiz = if s.horizontally_explored { "✓" } else { "○" };
    let ident = if s.graph_name.is_empty() {
        "graph".to_string()
    } else if s.custody == "observed" {
        format!("graph '{}' (observed)", s.graph_name)
    } else {
        format!("graph '{}'", s.graph_name)
    };
    format!(
        "{}: {} intents · {} edges ({} unresolved){} · {} codefiles · {} · vertical {} horizontal {} · phase={}\n  {}",
        ident, s.intents, s.total_edges, s.unresolved_edges, unexplored, s.codefiles, synced, vert, horiz, s.phase,
        fmt_coverage(&s.coverage)
    )
}

/// The JSON pulse — the same situational awareness the two human pulse lines
/// carry, structured. This is the `graph_state` payload field on every
/// command EXCEPT `loom status --json`, which returns the full GraphState
/// (the tier-2 deep view: identity, version, custody, per-axis counts).
pub fn pulse_json(s: &crate::db::queries::GraphState) -> serde_json::Value {
    let mut o = serde_json::Map::new();
    if !s.graph_name.is_empty() {
        let ident = if s.custody == "observed" {
            format!("{} (observed)", s.graph_name)
        } else {
            s.graph_name.clone()
        };
        o.insert("graph".into(), ident.into());
    }
    o.insert("phase".into(), s.phase.clone().into());
    o.insert("next_action".into(), s.next_action.clone().into());
    o.insert("vertical".into(), s.vertically_complete.into());
    o.insert("horizontal".into(), s.horizontally_explored.into());
    o.insert("intents".into(), s.intents.into());
    o.insert("codefiles".into(), s.codefiles.into());
    o.insert("edges".into(), s.total_edges.into());
    o.insert("unresolved".into(), s.unresolved_edges.into());
    if s.unexplored_pairs > 0 {
        o.insert("unexplored".into(), s.unexplored_pairs.into());
    }
    o.insert(
        "synced".into(),
        if s.last_synced.is_empty() { "never".to_string() } else { rel_time(&s.last_synced) }.into(),
    );
    o.insert("coverage".into(), coverage_line(&s.coverage).into());
    serde_json::Value::Object(o)
}

// ---------------------------------------------------------------------------
// The LLM-driver output contract (shared by every command)
//
// loom is driven almost exclusively by LLM agents over long horizons, where
// every output is the prompt for the agent's next decision and the only
// memory that survives context compaction. Three invariants:
//
//   1. ANCHOR AFTER MUTATION — a state-changing command ends with the next
//      command + the two-line pulse (human), or `next_step` + `graph_state`
//      fields (json). An agent should never need a separate `loom status`
//      call to know where it stands.
//   2. PARITY — whatever guidance human mode prints, json mode carries.
//      Orchestrated agents run --json; hints that exist only in println!
//      leave them blind.
//   3. BOUNDED — any list that scales with graph size is capped with an
//      explicit "+N more" marker carrying the retrieval command. Flooding
//      the context window evicts the agent's own plan.
//   4. SURFACE, THEN DIG — payloads embed PROJECTIONS (the fields the next
//      decision needs), never full records: work items carry *Surface types,
//      anchors carry `pulse_json` (not the full GraphState), and every
//      elision names the runnable command that retrieves the rest
//      (`loom intent show`, `loom edge show`, `loom note list`,
//      `loom status --json`). Token spend is part of the contract.
// ---------------------------------------------------------------------------

/// Default cap for a variable-length section rendered inside another
/// command's output (notes on a work item, groundings on a show view, …).
pub const SECTION_CAP: usize = 10;

/// Default `--limit` for inventory list commands. 0 = unlimited.
pub const LIST_LIMIT: usize = 50;

/// The standard truncation marker: `… +N more — <how to fetch the rest>`.
/// Returns None when nothing was elided. `fetch_cmd` MUST be a runnable
/// command, not prose — the marker is an affordance, not an apology.
pub fn more_marker(total: usize, shown: usize, fetch_cmd: &str) -> Option<String> {
    (total > shown).then(|| format!("… +{} more — {}", total - shown, fetch_cmd))
}

/// Bound a list in place honoring the `--limit` convention (0 = all).
/// Returns the pre-truncation total for the caller's marker/count fields.
pub fn apply_limit<T>(items: &mut Vec<T>, limit: usize) -> usize {
    let total = items.len();
    if limit > 0 && total > limit {
        items.truncate(limit);
    }
    total
}

/// Human-mode mutation footer: the next command + the two-line pulse.
/// Call as the LAST lines of a mutating command's human branch.
pub fn print_anchor(db: &dyn crate::db::LoomDb, next_step: &str) -> anyhow::Result<()> {
    let gs = crate::db::queries::graph_state(db)?;
    println!("  → Next: {next_step}");
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}

/// JSON-mode mutation anchor: merge `next_step` + `graph_state` into the
/// command's payload (same data the human branch prints — parity by
/// construction). Non-object payloads are returned untouched.
pub fn with_anchor(
    mut v: serde_json::Value,
    db: &dyn crate::db::LoomDb,
    next_step: &str,
) -> anyhow::Result<serde_json::Value> {
    if let Some(obj) = v.as_object_mut() {
        obj.insert("next_step".into(), serde_json::Value::String(next_step.to_string()));
        obj.insert(
            "graph_state".into(),
            pulse_json(&crate::db::queries::graph_state(db)?),
        );
    }
    Ok(v)
}

// ---------------------------------------------------------------------------
// Human-readable Display helpers
// ---------------------------------------------------------------------------

pub fn fmt_intent(i: &crate::types::Intent) -> String {
    let refs_str = if i.source_refs.is_empty() {
        "(none)".to_string()
    } else {
        i.source_refs.join(", ")
    };
    let aspect_line = if i.aspect.is_empty() {
        String::new()
    } else {
        format!("\n  aspect:      {}", i.aspect)
    };
    // Tags render only when present — untagged is not worth a line.
    let tags_line = if i.tags.is_empty() {
        String::new()
    } else {
        format!("\n  tags:        {}", i.tags.join(", "))
    };
    let lifecycle = if i.lifecycle.is_empty() { "implemented" } else { &i.lifecycle };
    format!(
        "  id:          {}\n  name:        {}\n  level:       {}\n  domain:      {}\n  status:      {}\n  lifecycle:   {}{}{}\n  description: {}\n  sources:     {}\n  created:     {}\n  updated:     {}",
        i.id, i.name, i.abstraction_level, i.domain, i.status, lifecycle, aspect_line, tags_line,
        i.description, refs_str, i.created_at, i.updated_at
    )
}

/// Human rendering of an IntentSurface — the work-item block. Mirrors the
/// JSON surface field-for-field (parity by construction); the full record
/// (timestamps, empty facets) is `loom intent show <id>`.
pub fn fmt_intent_surface(i: &crate::types::IntentSurface) -> String {
    let mut s = format!(
        "  id:          {}\n  name:        {}\n  level:       {}",
        i.id, i.name, i.level,
    );
    if !(i.domain.is_empty() || i.domain == "unknown") {
        s.push_str(&format!("\n  domain:      {}", i.domain));
    }
    s.push_str(&format!("\n  status:      {}\n  lifecycle:   {}", i.status, i.lifecycle));
    if !i.aspect.is_empty() {
        s.push_str(&format!("\n  aspect:      {}", i.aspect));
    }
    if !i.tags.is_empty() {
        s.push_str(&format!("\n  tags:        {}", i.tags.join(", ")));
    }
    s.push_str(&format!("\n  description: {}", i.description));
    if !i.sources.is_empty() {
        s.push_str(&format!("\n  sources:     {}", i.sources.join(", ")));
    }
    s
}

pub fn fmt_intent_row(i: &crate::types::Intent) -> String {
    format!(
        "  [{status:>20}]  {level:<15}  {name:<40}  {id}",
        status = i.status,
        level  = i.abstraction_level,
        name   = i.name,
        id     = i.id
    )
}

pub fn fmt_edge_row(e: &crate::types::RelatesTo) -> String {
    format!(
        "  [{status:<22}]  pri={pri:.2}  conf={conf:.2}  {from} → {to}  id={id}",
        status = e.inspection_status,
        pri    = e.priority_score,
        conf   = e.confidence,
        from   = e.from_name,
        to     = e.to_name,
        id     = e.id
    )
}

pub fn fmt_edge_detail(e: &crate::types::RelatesTo) -> String {
    let last = if e.last_inspected.is_empty() { "(never)" } else { &e.last_inspected };
    let by   = if e.inspected_by.is_empty()   { "(none)"  } else { &e.inspected_by };
    let ev   = if e.evidence.is_empty()        { "(none)"  } else { &e.evidence };
    let crit = if e.criterion.is_empty()       { "(none)"  } else { &e.criterion };
    let notes = if e.notes.is_empty()          { "(none)"  } else { &e.notes };
    // An unexplored pair has no materialised edge yet (id is empty) — say so
    // rather than printing a blank field that reads like a bug.
    let id = if e.id.is_empty() { "(not yet created — `loom edge explore` records it)" } else { &e.id };
    format!(
        "  id:                {}\n  from:              {} ({})\n  to:                {} ({})\n\
         \n  inspection_status: {}\n  criterion:         {}\n  evidence:          {}\
         \n  confidence:        {:.2}\n  priority:          {:.2}\
         \n  last_inspected:    {}\n  inspected_by:      {}\n  notes:             {}",
        id,
        e.from_name, e.from_id,
        e.to_name,   e.to_id,
        e.inspection_status,
        crit, ev,
        e.confidence, e.priority_score,
        last, by, notes,
    )
}

pub fn fmt_rule_row(r: &crate::types::QualityRule) -> String {
    format!(
        "  [{sev:<8}]  {name:<40}  {id}",
        sev  = r.severity,
        name = r.name,
        id   = r.id
    )
}

pub fn fmt_status(s: &crate::types::StatusReport) -> String {
    let pass_pct = s.validation_pass_rate * 100.0;
    format!(
        "Nodes:\n\
         \n  intents:                    {intents}\
         \n  code files:                 {codefiles}\
         \n  validations:                {validations}\
         \n\nEdges (all types combined):   {total_edges}\
         \n  uninspected:                {uninspected}\
         \n  passing:                    {passing}\
         \n  failing:                    {failing}\
         \n  independent:                {independent}\
         \n  needs_reverification:       {nrv}\
         \n\nQuality:\n\
         \n  open issues (failing):      {issues}\
         \n  intents without validation: {no_val}\
         \n  validation pass rate:       {pass:.1}%",
        intents     = s.total_intents,
        codefiles   = s.total_codefiles,
        validations = s.total_validations,
        total_edges = s.total_edges,
        uninspected = s.uninspected_edges,
        passing     = s.passing_edges,
        failing     = s.failing_edges,
        independent = s.independent_edges,
        nrv         = s.needs_reverification,
        issues      = s.open_issues,
        no_val      = s.intents_without_validations,
        pass        = pass_pct,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::LoomDb;

    #[test]
    fn fmt_intent_renders_refs_and_tags() {
        let intent = crate::types::Intent {
            id: "i".to_string(),
            name: "intent".to_string(),
            description: "description".to_string(),
            abstraction_level: "feature".to_string(),
            domain: "test".to_string(),
            source_refs: vec!["src/a.rs".to_string(), "docs/SPEC.md".to_string()],
            status: "proposed".to_string(),
            aspect: String::new(),
            tags: vec!["enforcement".to_string()],
            lifecycle: "implemented".to_string(),
            created_at: "t0".to_string(),
            updated_at: "t0".to_string(),
        };

        let rendered = fmt_intent(&intent);
        assert!(rendered.contains("src/a.rs, docs/SPEC.md"), "{rendered}");
        assert!(rendered.contains("tags:        enforcement"), "{rendered}");
    }

    #[test]
    fn more_marker_is_an_affordance_not_an_apology() {
        assert_eq!(more_marker(10, 10, "loom x"), None, "nothing elided, no marker");
        assert_eq!(more_marker(9, 10, "loom x"), None, "shown >= total, no marker");
        let m = more_marker(12, 10, "loom note list --edge e1").unwrap();
        assert!(m.contains("+2 more"), "{m}");
        assert!(m.contains("loom note list --edge e1"), "the marker must carry the runnable fetch: {m}");
    }

    #[test]
    fn apply_limit_honors_zero_as_unlimited() {
        let mut v: Vec<u32> = (0..7).collect();
        assert_eq!(apply_limit(&mut v, 0), 7);
        assert_eq!(v.len(), 7, "0 = all");
        assert_eq!(apply_limit(&mut v, 3), 7, "returns pre-truncation total");
        assert_eq!(v, vec![0, 1, 2], "keeps the head");
        assert_eq!(apply_limit(&mut v, 5), 3, "under the cap stays untouched");
    }

    #[test]
    fn anchor_json_carries_next_step_and_graph_state() {
        let db = crate::db::GrafeoDb::in_memory();
        db.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION, "t", "g", "anchor", "owned",
        )).unwrap();
        let v = with_anchor(serde_json::json!({"status": "ok"}), &db, "`loom next`").unwrap();
        assert_eq!(v["status"], "ok", "existing fields preserved");
        assert_eq!(v["next_step"], "`loom next`");
        assert!(v["graph_state"].get("phase").is_some(), "the pulse travels in json: {v}");
        // Non-object payloads pass through untouched (lists wrap themselves).
        let arr = with_anchor(serde_json::json!([1, 2]), &db, "x").unwrap();
        assert!(arr.is_array());
    }

    #[test]
    fn pulse_is_a_surface_not_the_full_state() {
        let db = crate::db::GrafeoDb::in_memory();
        db.execute(&crate::db::schema::insert_meta(
            crate::db::schema::SCHEMA_VERSION, "t", "g", "pulse", "owned",
        )).unwrap();
        let gs = crate::db::queries::graph_state(&db).unwrap();
        let p = pulse_json(&gs);
        // Everything the next decision needs travels…
        for k in [
            "phase", "next_action", "vertical", "horizontal", "intents",
            "codefiles", "edges", "unresolved", "synced", "coverage",
        ] {
            assert!(p.get(k).is_some(), "pulse must carry '{k}': {p}");
        }
        assert_eq!(p["graph"], "pulse", "identity is the human-form name");
        assert!(p["coverage"].is_string(), "coverage is the compact axis vector: {p}");
        // …and none of the deep-view fields `loom status --json` owns (tier-2).
        for k in [
            "version", "graph_id", "graph_name", "custody", "notes",
            "validations", "relates_to_edges", "implements_edges", "last_synced",
        ] {
            assert!(p.get(k).is_none(), "'{k}' is tier-2 — dig via `loom status --json`: {p}");
        }
        // Never synced reads as prose, not as an empty string.
        assert_eq!(p["synced"], "never");
    }
}
