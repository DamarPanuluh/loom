use serde::Serialize;

pub fn note_list_intent_command(id: &str) -> String {
    format!("loom note list --intent {id}")
}

pub fn intent_show_command(id: &str) -> String {
    format!("`loom intent show {id}`")
}

pub fn intent_not_found_list(id: &str) -> String {
    format!("Intent '{id}' not found.\nRun `loom intent list` to see available intents.")
}

pub fn intent_not_found_find(id: &str) -> String {
    format!("Intent '{id}' not found. Run `loom intent list` (or `loom find \"<words>\"`).")
}

pub const STATUS_RECHECK_NEXT_STEP: &str = "`loom status` re-checks the compass";

pub struct Printer {
    pub json: bool,
    /// When `Some`, JSON output is appended to this buffer instead of being
    /// printed to process stdout. Normal CLI execution leaves this as `None`;
    /// unit tests use capture mode for exact JSON assertions.
    capture: Option<std::cell::RefCell<String>>,
    /// Whether anything has been written through `emit_line` yet. The failure
    /// chokepoint (`print_error`) consults this so it only synthesizes an error
    /// envelope when the command produced NO output — commands like `doctor`,
    /// `batch` and `validate` print their full structured result and THEN return
    /// `Err` purely to set a non-zero exit; appending an envelope there would
    /// emit two JSON objects on stdout and break single-object parsers.
    emitted: std::cell::Cell<bool>,
}

impl Printer {
    /// Direct mode: writes straight to process stdout (unchanged behaviour).
    pub fn new(json: bool) -> Self {
        Self {
            json,
            capture: None,
            emitted: std::cell::Cell::new(false),
        }
    }

    /// Capturing mode for tests: every stdout write in JSON mode is folded into
    /// an internal buffer instead of hitting the terminal.
    #[cfg(test)]
    pub fn capturing(json: bool) -> Self {
        Self {
            json,
            capture: Some(std::cell::RefCell::new(String::new())),
            emitted: std::cell::Cell::new(false),
        }
    }

    /// The captured buffer. `None` for a direct-mode printer.
    #[cfg(test)]
    pub fn captured(&self) -> Option<String> {
        self.capture.as_ref().map(|c| c.borrow().clone())
    }

    /// Emit one line of already-rendered output: append it (plus a newline) to
    /// the capture buffer when capturing, else `println!` to stdout. The single
    /// stdout chokepoint — every other writer (`print_json`, the human anchor
    /// when capturing) routes through here so capture mode stays consistent.
    fn emit_line(&self, line: &str) {
        self.emitted.set(true);
        match &self.capture {
            Some(buf) => {
                let mut b = buf.borrow_mut();
                b.push_str(line);
                b.push('\n');
            }
            None => println!("{line}"),
        }
    }

    pub fn print_json<T: Serialize>(&self, value: &T) {
        // Compact, not pretty: the consumer is an LLM agent (pretty-printed
        // indentation is pure token spend; jq re-pretties for humans).
        let rendered = serde_json::to_string(value).unwrap_or_else(|e| {
            serde_json::to_string(&serde_json::json!({ "error": e.to_string() }))
                .expect("serializing JSON error object cannot fail")
        });
        self.emit_line(&rendered);
    }

    /// Render a command failure as a structured envelope in JSON mode. The
    /// success path of every command emits a JSON object; without this the
    /// failure path would surface only anyhow's plain-text `Error:` on stderr,
    /// leaving a `--json` driver with empty stdout and unparseable prose. Human
    /// mode is a deliberate no-op — `main()`'s anyhow Termination still prints
    /// the plain message to stderr and the non-zero exit code is preserved
    /// either way.
    pub fn print_error(&self, err: &anyhow::Error) {
        // Only synthesize an envelope when in JSON mode AND the command emitted
        // nothing — otherwise we would append a second JSON object after a
        // command that already printed its result (doctor/batch/validate report
        // then return Err for the exit code).
        if !self.json || self.emitted.get() {
            return;
        }
        let causes: Vec<String> = err.chain().map(|c| c.to_string()).collect();
        self.print_json(&serde_json::json!({
            "status": "error",
            "error": err.to_string(),
            "causes": causes,
            "exit_code": 1,
        }));
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

/// The ambient session role as a compact pulse token: `as fixer` when a role is
/// declared, `solo` otherwise. Pure — the env read lives in
/// [`crate::agent::session_role`]; this just formats. Stamping it on every
/// footer makes a silent drop to solo mode (lane enforcement off) visible.
pub fn fmt_role_stamp(role: Option<&str>) -> String {
    match role {
        Some(r) => format!("as {r}"),
        None => "solo".to_string(),
    }
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
    let horiz = if s.horizontally_explored {
        "✓"
    } else {
        "○"
    };
    let ident = if s.graph_name.is_empty() {
        "graph".to_string()
    } else if s.custody == "observed" {
        format!("graph '{}' (observed)", s.graph_name)
    } else {
        format!("graph '{}'", s.graph_name)
    };
    let stamp = fmt_role_stamp(crate::agent::session_role().as_deref());
    let base = format!(
        "{stamp} · {}: {} intents · {} edges ({} unresolved){} · {} codefiles · {} · vertical {} horizontal {} · phase={}\n  {}",
        ident, s.intents, s.total_edges, s.unresolved_edges, unexplored, s.codefiles, synced, vert, horiz, s.phase,
        fmt_coverage(&s.coverage)
    );
    if s.note_hygiene.is_empty() {
        base
    } else {
        format!("{base}\n  ⓘ {}", s.note_hygiene)
    }
}

/// The JSON pulse — the same situational awareness the two human pulse lines
/// carry, structured. This is the `graph_state` payload field on every
/// command EXCEPT `loom status --json`, which returns the full GraphState
/// (the tier-2 deep view: identity, version, custody, per-axis counts).
/// Deliberately NO `next_action`: the carrying payload's `next_step` (or the
/// human `→ Next:` line) is the guidance channel — repeating the compass
/// sentence inside every nested graph_state was pure token spend.
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
    // The ambient session role (or "solo") — parity with the human footer stamp.
    o.insert(
        "role".into(),
        crate::agent::session_role()
            .unwrap_or_else(|| "solo".to_string())
            .into(),
    );
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
        if s.last_synced.is_empty() {
            "never".to_string()
        } else {
            rel_time(&s.last_synced)
        }
        .into(),
    );
    o.insert("coverage".into(), coverage_line(&s.coverage).into());
    // Parity with the human pulse's note-hygiene line — present only when there
    // is something to teach (heavy note log).
    if !s.note_hygiene.is_empty() {
        o.insert("note_hygiene".into(), s.note_hygiene.clone().into());
    }
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

/// JSON-mode mutation anchor for backend-neutral repositories.
pub fn with_read_anchor(
    mut v: serde_json::Value,
    db: &dyn crate::db::GraphReadRepository,
    next_step: &str,
) -> anyhow::Result<serde_json::Value> {
    if let Some(obj) = v.as_object_mut() {
        let snapshot = db.query_snapshot()?;
        obj.insert(
            "next_step".into(),
            serde_json::Value::String(next_step.to_string()),
        );
        obj.insert(
            "graph_state".into(),
            pulse_json(&db.graph_state(&snapshot)?),
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
    // Visibility renders only when ruled — untriaged is not worth a line.
    let visibility_line = if i.visibility.is_empty() {
        String::new()
    } else {
        format!("\n  visibility:  {}", i.visibility)
    };
    // Boundary renders only when set — internal intents aren't worth a line.
    let boundary_line = if i.boundary.is_empty() {
        String::new()
    } else {
        format!("\n  boundary:    {}", i.boundary)
    };
    let lifecycle = if i.lifecycle.is_empty() {
        "implemented"
    } else {
        &i.lifecycle
    };
    let layer_line = if i.layer.is_empty() {
        String::new()
    } else {
        format!("\n  layer:       {}", i.layer)
    };
    format!(
        "  id:          {}\n  name:        {}\n  level:       {}\n  domain:      {}{}\n  status:      {}\n  lifecycle:   {}{}{}{}{}\n  description: {}\n  sources:     {}\n  created:     {}\n  updated:     {}",
        i.id, i.name, i.abstraction_level, i.domain, layer_line, i.status, lifecycle, aspect_line, tags_line,
        visibility_line, boundary_line, i.description, refs_str, i.created_at, i.updated_at
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
    if !i.layer.is_empty() {
        s.push_str(&format!("\n  layer:       {}", i.layer));
    }
    s.push_str(&format!(
        "\n  status:      {}\n  lifecycle:   {}",
        i.status, i.lifecycle
    ));
    if !i.aspect.is_empty() {
        s.push_str(&format!("\n  aspect:      {}", i.aspect));
    }
    // Boundary surfaces in the work item: the driver sees "this crosses into
    // the outside world" before touching the code, without re-deriving it.
    if !i.boundary.is_empty() {
        s.push_str(&format!("\n  boundary:    {}", i.boundary));
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
        level = i.abstraction_level,
        name = i.name,
        id = i.id
    )
}

pub fn fmt_edge_row(e: &crate::types::RelatesTo) -> String {
    format!(
        "  [{status:<22}]  pri={pri:.2}  conf={conf:.2}  {from} → {to}  id={id}",
        status = e.inspection_status,
        pri = e.priority_score,
        conf = e.confidence,
        from = e.from_name,
        to = e.to_name,
        id = e.id
    )
}

pub fn fmt_edge_detail(e: &crate::types::RelatesTo) -> String {
    let last = if e.last_inspected.is_empty() {
        "(never)"
    } else {
        &e.last_inspected
    };
    let by = if e.inspected_by.is_empty() {
        "(none)"
    } else {
        &e.inspected_by
    };
    let ev = if e.evidence.is_empty() {
        "(none)"
    } else {
        &e.evidence
    };
    let crit = if e.criterion.is_empty() {
        "(none)"
    } else {
        &e.criterion
    };
    let notes = if e.notes.is_empty() {
        "(none)"
    } else {
        &e.notes
    };
    // An unexplored pair has no materialised edge yet (id is empty) — say so
    // rather than printing a blank field that reads like a bug.
    let id = if e.id.is_empty() {
        "(not yet created — `loom edge explore` records it)"
    } else {
        &e.id
    };
    format!(
        "  id:                {}\n  from:              {} ({})\n  to:                {} ({})\n\
         \n  inspection_status: {}\n  criterion:         {}\n  evidence:          {}\
         \n  confidence:        {:.2}\n  priority:          {:.2}\
         \n  last_inspected:    {}\n  inspected_by:      {}\n  notes:             {}",
        id,
        e.from_name,
        e.from_id,
        e.to_name,
        e.to_id,
        e.inspection_status,
        crit,
        ev,
        e.confidence,
        e.priority_score,
        last,
        by,
        notes,
    )
}

pub fn fmt_rule_row(r: &crate::types::QualityRule) -> String {
    format!(
        "  [{sev:<8}]  {name:<40}  {id}",
        sev = r.severity,
        name = r.name,
        id = r.id
    )
}

pub fn fmt_status(s: &crate::types::StatusReport) -> String {
    let pass_pct = s.validation_pass_rate * 100.0;
    // A wall of environmentally-blocked proofs (live target down) would
    // otherwise make the all-up rate read as failures — show the blocked count
    // and the undiluted runnable rate right next to it.
    let blocked_note = if s.blocked_validations > 0 {
        format!(
            "  ({} blocked — environment not ready; of runnable: {:.1}%)",
            s.blocked_validations,
            s.validation_pass_rate_runnable * 100.0
        )
    } else {
        String::new()
    };
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
         \n  validation pass rate:       {pass:.1}%{blocked_note}",
        intents = s.total_intents,
        codefiles = s.total_codefiles,
        validations = s.total_validations,
        total_edges = s.total_edges,
        uninspected = s.uninspected_edges,
        passing = s.passing_edges,
        failing = s.failing_edges,
        independent = s.independent_edges,
        nrv = s.needs_reverification,
        issues = s.open_issues,
        no_val = s.intents_without_validations,
        pass = pass_pct,
        blocked_note = blocked_note,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::{Coverage360, CoverageAxis, GraphState};

    fn graph_state_fixture(name: &str) -> GraphState {
        GraphState {
            version: crate::db::schema::SCHEMA_VERSION.to_string(),
            graph_id: "test-graph".to_string(),
            graph_name: name.to_string(),
            custody: "owned".to_string(),
            intents: 0,
            relates_to_edges: 0,
            implements_edges: 0,
            total_edges: 0,
            unresolved_edges: 0,
            unexplored_pairs: 0,
            codefiles: 0,
            validations: 0,
            notes: 0,
            last_synced: String::new(),
            vertically_complete: true,
            horizontally_explored: true,
            phase: "seed".to_string(),
            next_action: String::new(),
            next_kind: "directive".to_string(),
            coverage: Coverage360 {
                grounded_files: CoverageAxis {
                    covered: 0,
                    total: 0,
                },
                realized_leaves: CoverageAxis {
                    covered: 0,
                    total: 0,
                },
                explored_pairs: CoverageAxis {
                    covered: 0,
                    total: 0,
                },
                measured_pairs: CoverageAxis {
                    covered: 0,
                    total: 0,
                },
                proven_leaves: CoverageAxis {
                    covered: 0,
                    total: 0,
                },
            },
            note_hygiene: String::new(),
        }
    }

    #[test]
    fn fmt_intent_renders_refs_and_tags() {
        let intent = crate::types::Intent {
            id: "i".to_string(),
            name: "intent".to_string(),
            description: "description".to_string(),
            abstraction_level: "feature".to_string(),
            domain: "test".to_string(),
            layer: String::new(),
            source_refs: vec!["src/a.rs".to_string(), "docs/SPEC.md".to_string()],
            status: "proposed".to_string(),
            aspect: String::new(),
            tags: vec!["enforcement".to_string()],
            visibility: String::new(),
            boundary: "inbound".to_string(),
            lifecycle: "implemented".to_string(),
            created_at: "t0".to_string(),
            updated_at: "t0".to_string(),
        };

        let rendered = fmt_intent(&intent);
        assert!(rendered.contains("src/a.rs, docs/SPEC.md"), "{rendered}");
        assert!(rendered.contains("tags:        enforcement"), "{rendered}");
        // A set boundary renders; an unset one stays off the card.
        assert!(rendered.contains("boundary:    inbound"), "{rendered}");
    }

    #[test]
    fn more_marker_is_an_affordance_not_an_apology() {
        assert_eq!(
            more_marker(10, 10, "loom x"),
            None,
            "nothing elided, no marker"
        );
        assert_eq!(
            more_marker(9, 10, "loom x"),
            None,
            "shown >= total, no marker"
        );
        let m = more_marker(12, 10, "loom note list --edge e1").unwrap();
        assert!(m.contains("+2 more"), "{m}");
        assert!(
            m.contains("loom note list --edge e1"),
            "the marker must carry the runnable fetch: {m}"
        );
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
        let db = crate::db::sqlite::SqliteGraphStore::in_memory().unwrap();
        db.initialize(
            crate::db::schema::SCHEMA_VERSION,
            "t",
            "anchor",
            "owned",
            "t",
        )
        .unwrap();
        let v = with_read_anchor(serde_json::json!({"status": "ok"}), &db, "`loom next`").unwrap();
        assert_eq!(v["status"], "ok", "existing fields preserved");
        assert_eq!(v["next_step"], "`loom next`");
        assert!(
            v["graph_state"].get("phase").is_some(),
            "the pulse travels in json: {v}"
        );
        // Non-object payloads pass through untouched (lists wrap themselves).
        let arr = with_read_anchor(serde_json::json!([1, 2]), &db, "x").unwrap();
        assert!(arr.is_array());
    }

    #[test]
    fn pulse_is_a_surface_not_the_full_state() {
        let gs = graph_state_fixture("pulse");
        let p = pulse_json(&gs);
        // Everything the next decision needs travels…
        for k in [
            "phase",
            "role",
            "vertical",
            "horizontal",
            "intents",
            "codefiles",
            "edges",
            "unresolved",
            "synced",
            "coverage",
        ] {
            assert!(p.get(k).is_some(), "pulse must carry '{k}': {p}");
        }
        assert_eq!(p["graph"], "pulse", "identity is the human-form name");
        assert!(
            p["coverage"].is_string(),
            "coverage is the compact axis vector: {p}"
        );
        // …and none of the deep-view fields `loom status --json` owns (tier-2),
        // nor the compass sentence (the payload's `next_step` is the guidance
        // channel — `next_action` repeated in every nested pulse was noise).
        for k in [
            "version",
            "graph_id",
            "graph_name",
            "custody",
            "notes",
            "next_action",
            "validations",
            "relates_to_edges",
            "implements_edges",
            "last_synced",
        ] {
            assert!(
                p.get(k).is_none(),
                "'{k}' is tier-2 — dig via `loom status --json`: {p}"
            );
        }
        // Never synced reads as prose, not as an empty string.
        assert_eq!(p["synced"], "never");
    }

    #[test]
    fn coverage_line_never_shows_a_vacuous_full() {
        let c = Coverage360 {
            grounded_files: CoverageAxis {
                covered: 5,
                total: 5,
            }, // closed → ✓
            realized_leaves: CoverageAxis {
                covered: 2,
                total: 4,
            }, // partial → fraction
            explored_pairs: CoverageAxis {
                covered: 0,
                total: 0,
            }, // no surface → —
            measured_pairs: CoverageAxis {
                covered: 1,
                total: 3,
            },
            proven_leaves: CoverageAxis {
                covered: 0,
                total: 0,
            },
        };
        let line = coverage_line(&c);
        assert!(
            line.contains("grounded 5/5 ✓"),
            "closed axis gets a check: {line}"
        );
        assert!(
            line.contains("realized 2/4"),
            "partial axis shows the fraction: {line}"
        );
        assert!(
            line.contains("explored —"),
            "an axis with no surface is —, never a vacuous 100%: {line}"
        );
        assert!(
            !line.contains("0/0"),
            "0/0 must never render as a number: {line}"
        );
    }

    #[test]
    fn fmt_pulse_renders_both_completeness_axes_and_hygiene() {
        let mut gs = graph_state_fixture("pulse");
        gs.vertically_complete = false;
        gs.horizontally_explored = false;
        gs.note_hygiene = String::new();
        let p = fmt_pulse(&gs);
        assert!(p.contains("vertical ✗"), "an incomplete spine shows ✗: {p}");
        assert!(
            p.contains("horizontal ○"),
            "an unexplored grid shows ○: {p}"
        );
        assert!(
            p.contains("360°:"),
            "the second line is always the coverage vector: {p}"
        );
        assert!(
            !p.contains("\n  ⓘ"),
            "no hygiene line when note_hygiene is empty: {p}"
        );

        gs.vertically_complete = true;
        gs.horizontally_explored = true;
        gs.note_hygiene = "heavy note log — `loom note prune --transitions`".to_string();
        let p = fmt_pulse(&gs);
        assert!(
            p.contains("vertical ✓") && p.contains("horizontal ✓"),
            "closed axes show ✓: {p}"
        );
        assert!(
            p.contains("ⓘ heavy note log"),
            "the hygiene nudge surfaces only when set: {p}"
        );
    }

    #[test]
    fn human_pulse_and_json_pulse_agree() {
        // The PARITY invariant: whatever the human footer shows, the json pulse
        // an orchestrated --json agent reads must carry identically.
        let gs = graph_state_fixture("parity");
        let human = fmt_pulse(&gs);
        let json = pulse_json(&gs);
        assert!(
            human.contains(&format!("phase={}", gs.phase)),
            "human footer names the phase: {human}"
        );
        assert_eq!(
            json["phase"],
            serde_json::json!(gs.phase),
            "json carries the same phase"
        );
        let cov = coverage_line(&gs.coverage);
        assert!(
            human.contains(&cov),
            "human footer embeds the coverage line: {human}"
        );
        assert_eq!(
            json["coverage"],
            serde_json::json!(cov),
            "json carries the byte-identical coverage line"
        );
        // The role stamp travels in BOTH (robust to whatever $LOOM_AGENT is in
        // the test env — usually unset, i.e. "solo").
        let role = crate::agent::session_role();
        let expected = role.as_deref().unwrap_or("solo");
        assert!(
            human.contains(&fmt_role_stamp(role.as_deref())),
            "human footer carries the role stamp: {human}"
        );
        assert_eq!(
            json["role"],
            serde_json::json!(expected),
            "json carries the same role"
        );
    }

    #[test]
    fn fmt_role_stamp_marks_role_or_solo() {
        assert_eq!(fmt_role_stamp(Some("fixer")), "as fixer");
        assert_eq!(
            fmt_role_stamp(None),
            "solo",
            "no declared role reads as solo"
        );
    }

    #[test]
    fn rel_time_is_relative_and_falls_back_to_raw() {
        let two_h = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        assert_eq!(rel_time(&two_h), "2h ago");
        let bad = "not-a-timestamp";
        assert_eq!(
            rel_time(bad),
            bad,
            "an unparseable stamp echoes back, never panics"
        );
    }

    #[test]
    fn fmt_status_surfaces_blocked_proofs() {
        let mut s = crate::types::StatusReport {
            total_intents: 1,
            total_codefiles: 1,
            total_validations: 1,
            total_edges: 0,
            uninspected_edges: 0,
            passing_edges: 0,
            failing_edges: 0,
            independent_edges: 0,
            needs_reverification: 0,
            intents_without_validations: 0,
            validation_pass_rate: 0.5,
            blocked_validations: 0,
            validation_pass_rate_runnable: 1.0,
            open_issues: 0,
        };
        assert!(
            !fmt_status(&s).contains("blocked"),
            "no blocked note when the count is 0"
        );
        s.blocked_validations = 2;
        let out = fmt_status(&s);
        assert!(
            out.contains("2 blocked"),
            "the blocked count surfaces: {out}"
        );
        assert!(
            out.contains("of runnable: 100.0%"),
            "the undiluted runnable rate sits next to it: {out}"
        );
    }

    #[test]
    fn intent_surface_carries_boundary_to_the_driver() {
        let mut s = crate::types::IntentSurface {
            id: "i".into(),
            name: "n".into(),
            description: "d".into(),
            level: "feature".into(),
            domain: "unknown".into(),
            layer: String::new(),
            status: "proposed".into(),
            lifecycle: "implemented".into(),
            aspect: String::new(),
            boundary: String::new(),
            tags: Vec::new(),
            sources: Vec::new(),
        };
        assert!(
            !fmt_intent_surface(&s).contains("boundary"),
            "an unset boundary stays off the work-item card"
        );
        s.boundary = "outbound".into();
        assert!(
            fmt_intent_surface(&s).contains("boundary:    outbound"),
            "a set boundary surfaces in the work item the driver reads"
        );
    }
}
