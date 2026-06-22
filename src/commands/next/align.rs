use super::*;

// ---------------------------------------------------------------------------
// Align mode: the validator's user↔intent drift queue — meaning to re-affirm
// ---------------------------------------------------------------------------

pub(super) fn run_take_align(
    store: &dyn GraphReadRepository,
    take: usize,
    printer: &Printer,
) -> Result<()> {
    let snapshot = store.query_snapshot()?;
    let candidates = store.align_candidates(&snapshot)?;
    let gs = store.graph_state(&snapshot)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "align",
                "message": ALIGN_EMPTY_MESSAGE,
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
            }));
        } else {
            println!("✓ No drift suspected — nothing to batch for alignment.");
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    const TAKE_CAP: usize = 50;
    let n = take.min(TAKE_CAP).min(candidates.len());
    let queue_total = candidates.len();
    let items: Vec<_> = candidates
        .iter()
        .take(n)
        .map(|c| {
            let visibility = if c.intent.visibility.is_empty() {
                "untriaged"
            } else {
                c.intent.visibility.as_str()
            };
            let audience = match visibility {
                "user_visible" => "user-visible capability",
                "internal" => "internal machinery",
                _ => "untriaged: ask whether this is user-visible capability or internal machinery",
            };
            let id = c.intent.id.as_str();
            serde_json::json!({
                "intent": {
                    "id": id,
                    "name": c.intent.name,
                    "level": c.intent.abstraction_level,
                    "visibility": visibility,
                    "description": c.intent.description,
                },
                "last_confirmed": c.last_confirmed,
                "churn_since_confirm": c.churn_since_confirm,
                "degree": c.degree,
                "score": c.score,
                "ask": format!("Does '{}' still match what you expect loom to do here?", c.intent.name),
                "audience_prompt": audience,
                "commands": {
                    "confirm": format!("loom intent confirm {id}"),
                    "confirm_user_visible": format!("loom intent confirm {id} --visibility user_visible"),
                    "confirm_internal": format!("loom intent confirm {id} --visibility internal"),
                    "reword": format!("loom intent update {id} --description \"…\" --reword --reason \"user clarified wording during align\""),
                    "update_meaning": format!("loom intent update {id} --description \"…\" --reason \"user changed expected behavior during align\""),
                    "retire": format!("loom intent retire {id} --reason \"superseded during align\" --replaced-by <successor>"),
                },
            })
        })
        .collect();
    let possible_proven_target_adds = items
        .iter()
        .filter(|item| item["intent"]["visibility"].as_str() == Some("untriaged"))
        .count();
    let guidance = "Use this as ONE human agenda. For each item, align the concept in plain language, not implementation wording. Record exactly one outcome: confirm, confirm --visibility user_visible, confirm --visibility internal, reword, update meaning, retire, or add a newly revealed missing concept. Confirming a leaf as user_visible adds it to the Proven journey-proof target. After recording outcomes, rerun `loom next --mode align --take <N>` until it is empty.";

    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "mode": "align",
            "taken": n,
            "queue_total": queue_total,
            "items": items,
            "guidance": guidance,
            "possible_proven_target_adds": possible_proven_target_adds,
            "dispatch": { "role": "validator", "effort": "mid", "gate": "human" },
            "graph_state": pulse_json(&gs),
        }));
        return Ok(());
    }

    println!("── Align agenda: {n} of {queue_total} human-gated meaning check(s) ────");
    for (idx, item) in items.iter().enumerate() {
        println!();
        println!(
            "  {}. {}  [{} · {}]",
            idx + 1,
            item["intent"]["name"].as_str().unwrap_or(""),
            item["intent"]["visibility"].as_str().unwrap_or(""),
            item["intent"]["id"].as_str().unwrap_or("")
        );
        println!(
            "     {}",
            item["intent"]["description"].as_str().unwrap_or("")
        );
        println!("     ask: {}", item["ask"].as_str().unwrap_or(""));
        println!(
            "     confirm: {}",
            item["commands"]["confirm"].as_str().unwrap_or("")
        );
        if item["intent"]["visibility"].as_str() == Some("untriaged") {
            println!(
                "     user-visible: {}  (adds to Proven journey-proof target if this is a leaf)",
                item["commands"]["confirm_user_visible"]
                    .as_str()
                    .unwrap_or("")
            );
            println!(
                "     internal: {}",
                item["commands"]["confirm_internal"].as_str().unwrap_or("")
            );
        }
    }
    println!();
    println!("  {guidance}");
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}

struct AlignContext<'a> {
    groundings: Vec<crate::types::Implements>,
    groundings_total: usize,
    notes: Vec<crate::types::NoteSurface>,
    notes_total: usize,
    last_confirmed: &'a str,
    siblings: Vec<String>,
    independent_of: Vec<String>,
    visibility: &'a str,
    audience_brief: String,
    where_it_sits: String,
    not_this: Vec<String>,
    action: String,
    dispatch: &'static str,
}

fn build_align_context<'a>(
    store: &dyn GraphReadRepository,
    c: &'a AlignCandidate,
) -> Result<AlignContext<'a>> {
    let mut groundings = store.list_implements_for_intent(&c.intent.id)?;
    let groundings_total = cap_section(&mut groundings);
    let (notes, notes_total) = note_surfaces(store.notes_for_target(&c.intent.id)?, "validator");
    let last_confirmed = c.last_confirmed.as_deref().unwrap_or("never");

    let mut parent_chain: Vec<String> = Vec::new();
    let mut immediate_parent_id: Option<String> = None;
    let mut cursor = c.intent.id.clone();
    for _ in 0..6 {
        let Some(h) = store
            .list_hierarchy_for_intent(&cursor)?
            .into_iter()
            .find(|h| h.child_id == cursor)
        else {
            break;
        };
        if immediate_parent_id.is_none() {
            immediate_parent_id = Some(h.parent_id.clone());
        }
        parent_chain.push(h.parent_name.clone());
        cursor = h.parent_id;
    }
    parent_chain.reverse();
    let mut siblings: Vec<String> = Vec::new();
    if let Some(pid) = &immediate_parent_id {
        siblings = store
            .list_hierarchy_for_intent(pid)?
            .into_iter()
            .filter(|h| h.parent_id == *pid && h.child_id != c.intent.id)
            .map(|h| h.child_name)
            .take(5)
            .collect();
    }
    let independent_of: Vec<String> = store
        .edges_for_intent(&c.intent.id)?
        .into_iter()
        .filter(|e| e.inspection_status == "independent")
        .map(|e| {
            if e.from_id == c.intent.id {
                e.to_name
            } else {
                e.from_name
            }
        })
        .take(5)
        .collect();
    let visibility = if c.intent.visibility.is_empty() {
        "untriaged"
    } else {
        c.intent.visibility.as_str()
    };
    let audience_brief = match visibility {
        "internal" => "internal machinery (serves other parts; the user never touches it directly)"
            .to_string(),
        "user_visible" => "a user-visible capability (the user can see or feel it)".to_string(),
        _ => "untriaged — decide from the groundings whether this is user-visible capability or \
              internal machinery, OPEN with that framing, and let the user correct you"
            .to_string(),
    };
    let where_it_sits = if parent_chain.is_empty() {
        c.intent.name.clone()
    } else {
        format!("{} → {}", parent_chain.join(" → "), c.intent.name)
    };
    let mut not_this = Vec::new();
    if !siblings.is_empty() {
        not_this.push(format!(
            "sibling concepts (distinct on purpose): {}",
            siblings.join(" · ")
        ));
    }
    if !independent_of.is_empty() {
        not_this.push(format!(
            "verified independent of: {}",
            independent_of.join(" · ")
        ));
    }
    let not_this_block = if not_this.is_empty() {
        String::new()
    } else {
        format!("\n  what it is NOT: {}", not_this.join("; "))
    };
    let action = format!(
        "Interview move — align the CONCEPT, not the wording. Present, in the user's plain \
         language:\n  \
         1. what the product can DO because this exists — one or two sentences, no file \
         paths, no internal nouns (jargon test: would a non-coder nod?)\n  \
         2. why it matters: its place in the design — {where_it_sits}\n  \
         3. its audience, UP FRONT: {audience_brief}{not_this}\n\
         Vocabulary stays out of the question unless the user asks, gets confused, or uses \
         a term that conflicts with the graph — then reconcile terms, not before.\n\
         \n  meaning on record (graph-speak — source material, never read aloud): {description}\n\
         \nAsk: \"does that match what you expect this product to do here?\" \
         Record exactly ONE outcome:\n  \
         - concept still right → loom intent confirm {id}\n  \
         - user-visible capability → loom intent confirm {id} --visibility user_visible  \
         (adds leaf intents to Proven's boundary-proof target)\n  \
         - words confusing, concept right → loom intent update {id} --description \"…\" \
         --reword --reason \"…\"  (no ripple; the clock still resets)\n  \
         - concept evolved → translate their answer into a falsifiable description: \
         loom intent update {id} --description \"…\" --reason \"…\"  (ripples; audience re-triaged)\n  \
         - internal machinery, stop asking → loom intent confirm {id} --visibility internal  \
         (leaves this queue until the meaning is redefined)\n  \
         - superseded → loom intent retire {id} --reason \"…\" --replaced-by <successor>\n  \
         - missing concept revealed → loom intent add … --lifecycle planned\n\
         \nThe user rules on BEHAVIOR, never on wording.",
        description = c.intent.description.as_str(),
        id = c.intent.id.as_str(),
        not_this = not_this_block,
    );

    let dispatch = "this is validator work — fills the alignment outcome: present the CONCEPT \
         to the USER, then record exactly ONE of `loom intent confirm` (optionally \
         `--visibility internal`) / `update` (`--reword` for wording-only) / `retire` / \
         `add` (update/retire/add are builder-lane: a solo agent records them directly; a \
         role-split validator hands the user's words to a builder). Whoever takes it declares \
         `LOOM_AGENT=llm:validator` (or stay bare `llm` for solo); its queue is \
         `loom next --mode align`. One item per question — after recording the outcome, pull \
         the queue AGAIN: it admits only drift suspects and every outcome resets that intent's \
         clock, so it drains to empty. Iterate until it reports clean; never stop after one \
         answer, never interrogate beyond what it serves.";

    Ok(AlignContext {
        groundings,
        groundings_total,
        notes,
        notes_total,
        last_confirmed,
        siblings,
        independent_of,
        visibility,
        audience_brief,
        where_it_sits,
        not_this,
        action,
        dispatch,
    })
}

pub(super) fn run_align(store: &dyn GraphReadRepository, printer: &Printer) -> Result<()> {
    let snapshot = store.query_snapshot()?;
    let candidates = store.align_candidates(&snapshot)?;
    let gs = store.graph_state(&snapshot)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "align",
                "message": ALIGN_EMPTY_MESSAGE,
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
            }));
        } else {
            println!("✓ {ALIGN_EMPTY_MESSAGE}");
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    let c = &candidates[0];
    let ctx = build_align_context(store, c)?;

    if printer.json {
        render_align_json(c, candidates.len(), &ctx, &gs, printer);
        return Ok(());
    }

    render_align_human(c, candidates.len(), &ctx, &gs);
    Ok(())
}

fn render_align_json(
    c: &AlignCandidate,
    queue_depth: usize,
    ctx: &AlignContext<'_>,
    gs: &GraphState,
    printer: &Printer,
) {
    let mut obj = serde_json::Map::new();
    obj.insert("mode".to_string(), serde_json::json!("align"));
    obj.insert(
        "intent".to_string(),
        serde_json::json!(IntentSurface::from(&c.intent)),
    );
    obj.insert(
        "last_confirmed".to_string(),
        serde_json::json!(c.last_confirmed),
    );
    obj.insert(
        "churn_since_confirm".to_string(),
        serde_json::json!(c.churn_since_confirm),
    );
    obj.insert("degree".to_string(), serde_json::json!(c.degree));
    obj.insert("score".to_string(), serde_json::json!(c.score));
    obj.insert("queue_depth".to_string(), serde_json::json!(queue_depth));
    obj.insert("visibility".to_string(), serde_json::json!(ctx.visibility));
    obj.insert(
        "where_it_sits".to_string(),
        serde_json::json!(ctx.where_it_sits),
    );
    obj.insert(
        "not_to_confuse_with".to_string(),
        serde_json::json!({
        "siblings": ctx.siblings,
        "verified_independent": ctx.independent_of,
        }),
    );
    obj.insert(
        "groundings".to_string(),
        serde_json::json!(ctx
            .groundings
            .iter()
            .map(GroundingSurface::from)
            .collect::<Vec<_>>()),
    );
    obj.insert(
        "groundings_total".to_string(),
        serde_json::json!(ctx.groundings_total),
    );
    obj.insert("notes".to_string(), serde_json::json!(ctx.notes));
    obj.insert(
        "notes_total".to_string(),
        serde_json::json!(ctx.notes_total),
    );
    obj.insert(
        "suggested_action".to_string(),
        serde_json::json!(ctx.action),
    );
    obj.insert("graph_state".to_string(), pulse_json(gs));
    obj.insert("owner_role".to_string(), serde_json::json!("validator"));
    obj.insert("effort".to_string(), serde_json::json!("mid"));
    obj.insert("dispatch".to_string(), serde_json::json!(ctx.dispatch));
    printer.print_json(&serde_json::Value::Object(obj));
}

fn render_align_human(
    c: &AlignCandidate,
    queue_depth: usize,
    ctx: &AlignContext<'_>,
    gs: &GraphState,
) {
    println!(
        "── Next Align Item  [score={:.2}]  ({} drift suspect(s) queued) ─────",
        c.score, queue_depth
    );
    println!();
    println!("── Intent ──────────────────────────────────────────────────────────");
    println!("  {}  ({})", c.intent.name, c.intent.id);
    println!("  level: {}", c.intent.abstraction_level);
    println!("  description: {}", c.intent.description);
    println!(
        "  lifecycle: {}  status: {}",
        c.intent.lifecycle, c.intent.status
    );
    println!("  audience: {}", ctx.audience_brief);
    println!("  sits under: {}", ctx.where_it_sits);
    if !ctx.not_this.is_empty() {
        println!("  not this: {}", ctx.not_this.join("; "));
    }
    println!("  last_confirmed: {}", ctx.last_confirmed);
    println!(
        "  churn since confirm: {} staled-claim flip(s)",
        c.churn_since_confirm
    );
    println!();
    if !ctx.groundings.is_empty() {
        println!("── Groundings ──────────────────────────────────────────────────────");
        for im in &ctx.groundings {
            let loc = if im.locator.is_empty() {
                String::new()
            } else {
                format!("  @ {}", im.locator)
            };
            println!("  {}{}", im.codefile_path, loc);
        }
        if let Some(m) = more_marker(
            ctx.groundings_total,
            ctx.groundings.len(),
            &format!("loom intent show {}", c.intent.id),
        ) {
            println!("  {m}");
        }
        println!();
    }
    if !ctx.notes.is_empty() {
        if ctx.notes_total > ctx.notes.len() {
            println!(
                "── Notes ({}, showing {}) ─────────────────────────────────────────",
                ctx.notes_total,
                ctx.notes.len()
            );
        } else {
            println!(
                "── Notes ({}) ──────────────────────────────────────────────────────",
                ctx.notes.len()
            );
        }
        for n in &ctx.notes {
            if n.times > 1 {
                println!("  [{}] {}  ({}, ×{})", n.kind, n.text, n.author, n.times);
            } else {
                println!("  [{}] {}  ({})", n.kind, n.text, n.author);
            }
        }
        if let Some(m) = more_marker(
            ctx.notes_total,
            ctx.notes.len(),
            &note_list_intent_command(&c.intent.id),
        ) {
            println!("  {m}");
        }
        println!();
    }
    println!("── Suggested Action ────────────────────────────────────────────────");
    println!("{}", ctx.action);
    println!();
    println!("  Dispatch — {}  [effort: mid]", ctx.dispatch);
    println!("  {}", fmt_pulse(gs));
}
