/// The one-line counterpart of `build_suggested_action`: the same decision,
/// stripped to a single runnable template (compact mode and machine drivers).
pub(super) fn build_suggested_action_compact(edge: &crate::types::RelatesTo) -> String {
    match edge.inspection_status.as_str() {
        "failing" => format!(
            "fix the code, `loom sync`, then `loom edge fix {} --description \"<what changed>\"`",
            edge.id
        ),
        "needs_reverification" => format!(
            "re-inspect: loom edge explore {from} {to} ground --criterion \"<updated>\" --confidence 0.9  (or: issue / independent)",
            from = edge.from_id, to = edge.to_id,
        ),
        _ => format!(
            "loom edge explore {from} {to} ground --criterion \"<text>\" --confidence 0.9  (or: issue --evidence \"…\" / independent --notes \"…\")",
            from = edge.from_id, to = edge.to_id,
        ),
    }
}

// ---------------------------------------------------------------------------
// Build a one-line (or multi-line) action hint for the LLM
// ---------------------------------------------------------------------------

pub(super) fn build_suggested_action(edge: &crate::types::RelatesTo, _score: &f64) -> String {
    match edge.inspection_status.as_str() {
        "unexplored" => format!(
            "No relationship is tracked yet between intent '{}' and intent '{}'.{} \
             Inspect whether they interact, then record the result (this creates the edge):\n\
             \n  loom edge explore {from} {to} ground --criterion \"<coexistence criterion>\" --confidence 0.9\
             \n  loom edge explore {from} {to} issue  --criterion \"<criterion>\" --evidence \"<problem>\"\
             \n  loom edge explore {from} {to} independent --notes \"<why unrelated>\"",
            edge.from_name, edge.to_name,
            if edge.notes.is_empty() { String::new() } else { format!(" ({})", edge.notes) },
            from = edge.from_id, to = edge.to_id,
        ),
        "uninspected" => format!(
            "Ground this edge — inspect whether intent '{}' and intent '{}' interact:\n\
             \n  loom edge explore {from} {to} ground --criterion \"<coexistence criterion>\" --confidence 0.9\
             \n  loom edge explore {from} {to} issue  --criterion \"<criterion>\" --evidence \"<problem>\"\
             \n  loom edge explore {from} {to} independent --notes \"<why unrelated>\"",
            edge.from_name, edge.to_name,
            from = edge.from_id, to = edge.to_id,
        ),
        "failing" => format!(
            "Fix the violation, then record it — IN THIS ORDER (sync before fix: \
             sync flips passing claims on changed files, so syncing after would \
             stale the green you just earned):\n\
             \n  1. Change the code so the criterion holds (minimal change, root cause).\
             \n  2. loom sync   (flags everything the change touched; this edge stays failing)\
             \n  3. loom edge fix {id} --description \"<what you changed>\"\n\
             \nNot fixing now? Discharge it honestly instead of leaving a lingering red: \
             DEFER as tracked work (`loom hypothesis add` the violation as the claim → \
             `loom hypothesis adopt --spawned`), or if the violation is DELIBERATE, JUSTIFY it \
             (`loom note add --edge {id} --kind decision --text \"<why it's accepted>\"`) — the \
             edge stays failing until the decision reopens it.",
            id = edge.id
        ),
        "needs_reverification" => format!(
            "Re-inspect this edge — a code change invalidated the previous assessment:\n\
             \n  loom edge explore {from} {to} ground --criterion \"<updated criterion>\"\
             \n  loom edge explore {from} {to} issue  --criterion \"<criterion>\" --evidence \"<finding>\"",
            from = edge.from_id, to = edge.to_id,
        ),
        other => format!("Review edge with inspection_status='{}' (id: {})", other, edge.id),
    }
}

// ---------------------------------------------------------------------------
// Role dispatch — name the lane that owns this item + the fields it fills, so an
// orchestrator can hand it to a role-scoped subagent straight from `loom next`.
// ---------------------------------------------------------------------------

/// The fields the owning role fills for a work item, keyed by role.
fn role_fills(role: &str) -> &'static str {
    match role {
        "analyzer" => "criterion, evidence, confidence, inspection_status (the verdict)",
        "builder" => {
            "write code → `loom codefile add` → `loom edge implement` (locator) → mark implemented"
        }
        "fixer" => "the minimal change → `loom edge fix` / mark implemented",
        "validator" => "run the proof (or `loom validation mark`) → `loom intent confirm`",
        "quality" => "the GOVERNS verdict — criterion, evidence, confidence (`loom rule verdict`)",
        _ => "its owned fields (see `loom schema`)",
    }
}

/// One-line dispatch hint: which role owns this item, how to run it as that role,
/// and what it fills. Used in both `--json` (as `owner_role`/`dispatch`) and human.
pub(super) fn dispatch_line(role: &str) -> String {
    let lane = crate::gate::mode_for_role(role).unwrap_or("");
    dispatch_line_for_lane(role, lane)
}

/// Same dispatch hint, but with the concrete queue that served the item.
/// Most roles have one canonical lane, but `loom next --mode fix` deliberately
/// mixes fixer work (failing edges) with analyzer work (stale edge reinspection).
/// In that case the ROLE is analyzer, while the correct NEXT QUEUE is still
/// `fix`; telling a goldfish agent to jump to the analyzer's discovery queue is
/// instruction drift.
pub(super) fn dispatch_line_for_lane(role: &str, lane: &str) -> String {
    format!(
        "this is {role} work — fills {fills}. ADOPT the lane's discipline JIT: `loom guide --role {role}` \
         (the binary serves the full loom-{role} skill — no install). Declares `LOOM_AGENT=llm:{role}` \
         (or stay bare `llm` for solo); its queue is `loom next --mode {lane}`. Same contract whether \
         that's you now, a later pass, or a parallel agent.",
        fills = role_fills(role),
    )
}

pub(super) fn relates_dispatch(
    mode: &str,
    edge: &crate::types::RelatesTo,
    score: f64,
) -> (&'static str, &'static str) {
    let role = match (mode, edge.inspection_status.as_str()) {
        ("fix", "failing") => "fixer",
        _ => "analyzer",
    };
    (role, relates_effort(edge, score))
}

/// Order the effort tiers so a bulk take can report the highest it contains.
pub(super) fn effort_rank(effort: &str) -> u8 {
    match effort {
        "high" => 2,
        "mid" => 1,
        _ => 0,
    }
}

fn relates_effort(edge: &crate::types::RelatesTo, score: f64) -> &'static str {
    let centrality =
        edge.discovery_centrality.a_degree.max(0) + edge.discovery_centrality.b_degree.max(0);
    let signal_count = edge.discovery_signals.len();
    let structural_weight = if centrality > 0 {
        centrality as f64 + (signal_count as f64 * 3.0)
    } else {
        score
    };

    if structural_weight >= 20.0 || signal_count >= 3 {
        "high"
    } else if structural_weight >= 8.0 || signal_count > 0 {
        "mid"
    } else {
        "low"
    }
}

/// Inject `owner_role` + `effort` + `dispatch` into a work-item JSON object.
/// `effort` (low | mid | high) names how much capability THIS item's work
/// needs — a statement about the work, computed from structure; the harness
/// maps it to whatever models exist. Never a model name.
pub(super) fn add_dispatch(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    role: &str,
    effort: &str,
) {
    add_dispatch_for_lane(
        obj,
        role,
        effort,
        crate::gate::mode_for_role(role).unwrap_or(""),
    );
}

/// Inject dispatch fields with a mode override for mixed queues.
pub(super) fn add_dispatch_for_lane(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    role: &str,
    effort: &str,
    lane: &str,
) {
    obj.insert("owner_role".to_string(), serde_json::json!(role));
    obj.insert("effort".to_string(), serde_json::json!(effort));
    obj.insert(
        "dispatch".to_string(),
        serde_json::json!(dispatch_line_for_lane(role, lane)),
    );
}
