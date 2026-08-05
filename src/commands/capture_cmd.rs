//! Capture command family — door, inbox, question, note, task.
//!
//! Plane: CLI surface over asserted capture. `door`/`inbox` record raw
//! utterances and offer a routing menu; notes and tasks are asserted records.
//! No derived truth and no verdicts are written from this module.

use super::*;

const DOOR_CAPABILITY: &str = "Your idea does not need to be a complete specification. Loom can help the LLM round it out, fill technical and repository-derivable gaps, and ask you one plain-language product question at a time when your judgment is needed.";

pub(crate) fn door(graph: Option<&Path>, utterance: &str, json: bool) -> Result<()> {
    // A name hit is worth two points and a description hit one. Requiring four
    // points prevents one generic verb in an intent name ("fix", "record",
    // "validate") from displacing the safer new-intent landing while still
    // promoting a match with two terms in its name.
    const STRONG_MATCH_SCORE: usize = 4;

    let store = open(graph)?;
    let item = store.add_node(
        NodeType::InboxItem,
        &truncate(utterance, 60),
        utterance,
        "new",
        serde_json::json!({ "source": "human" }),
    )?;
    let short = crate::model::short(&item.id);
    // Routing context: the closest existing intents, the compass, and a landing
    // menu of prefilled commands. The caller (usually the PM-side model) picks
    // ONE landing, runs it, then marks the capture routed — nothing is decided
    // here, but nothing requires a second lookup either.
    let matches = super::discover_cmd::keyword_hits(&store, utterance, &[NodeType::Intent], 5)?;
    let ladder = crate::maturity::ladder(&store)?;
    let q = crate::workitem::q;
    let mark_routed = format!("loom inbox mark {short} routed --reason '<destination>'");
    let mut menu: Vec<serde_json::Value> = Vec::new();
    let strong_matches: Vec<_> = matches
        .iter()
        .filter(|(score, _, _, _)| *score >= STRONG_MATCH_SCORE)
        .collect();
    let weak_matches: Vec<_> = matches
        .iter()
        .filter(|(score, _, _, _)| *score < STRONG_MATCH_SCORE)
        .collect();
    for (score, _, name, id) in &strong_matches {
        menu.push(serde_json::json!({
            "landing": "existing_intent",
            "confidence": "strong",
            "why": format!("closest existing intent (score {score}) — the utterance may refine, extend, or contradict it"),
            "intent": name,
            "id": id,
            "command": format!("loom intent show {}", q(name)),
        }));
    }
    menu.push(serde_json::json!({
        "landing": "new_intent",
        "why": "the utterance names a behavior no intent covers",
        "command": "loom intent add --name '<one falsifiable behavior>' --description '<what makes it true>' --level feature --visibility user_visible --aspect happy",
        "after": "loom next --mode elaborate grows the forgotten surroundings; the LLM should explain this capability, fill inferable gaps, and ask the user one plain-language product question at a time",
    }));
    menu.push(serde_json::json!({
        "landing": "hypothesis",
        "why": "the utterance is a redesign idea — prove it before it becomes work",
        "command": "loom hypothesis add --name '<idea>' --claim '<what is wrong now>' --proposal '<the change>' --predicted-outcome '<measurable result>' --target '<intent>'",
    }));
    menu.push(serde_json::json!({
        "landing": "spike",
        "why": "the utterance needs investigation before it can land anywhere",
        "command": "loom task add '<question>' --kind investigation --target '<intent>'   # --target lands the outcome as a note on the intent; omit it for a diary-only record",
    }));
    menu.push(serde_json::json!({
        "landing": "external_research",
        "why": "a current external fact is missing; this is not for discovering or replacing user preference",
        "command": "loom task add '<bounded external question>' --kind research --why-external '<why repository knowledge is insufficient or freshness matters>' --preferred-source '<primary authoritative source guidance>' --target '<intent>'",
    }));
    for (score, _, name, id) in &weak_matches {
        menu.push(serde_json::json!({
            "landing": "existing_intent",
            "confidence": "weak",
            "why": format!("weak lexical overlap only (score {score}); prefer new_intent unless this truly refines the existing intent"),
            "intent": name,
            "id": id,
            "command": format!("loom intent show {}", q(name)),
        }));
    }
    menu.push(serde_json::json!({
        "landing": "dismiss",
        "why": "not actionable — record why so it does not resurface",
        "command": format!("loom inbox mark {short} rejected --reason '<why>'"),
    }));
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "captured": node_json(&item),
                "capability": DOOR_CAPABILITY,
                "compass": { "phase": ladder.phase, "next_command": ladder.next_command },
                "landing_menu": menu,
                "next_step": format!("choose ONE landing, run it, then: {mark_routed}"),
            }))?
        );
    } else {
        print_door_text(&item, &menu, &strong_matches, &weak_matches, &mark_routed);
    }
    Ok(())
}

fn print_door_text(
    item: &Node,
    menu: &[serde_json::Value],
    strong_matches: &[&(usize, String, String, String)],
    weak_matches: &[&(usize, String, String, String)],
    mark_routed: &str,
) {
    println!("captured inbox item [{}]", crate::model::short(&item.id));
    println!("  {DOOR_CAPABILITY}");
    for (label, matches) in [
        ("closest intents", strong_matches),
        ("weak intent matches", weak_matches),
    ] {
        if matches.is_empty() {
            continue;
        }
        println!("  {label}:");
        for (score, _, name, id) in matches {
            let qualifier = if label == "closest intents" {
                format!("strong score {score}")
            } else {
                format!("weak score {score}; prefer new_intent unless this truly refines it")
            };
            println!("    - {} [{}] ({qualifier})", name, crate::model::short(id));
        }
    }
    println!("  landings:");
    for entry in menu {
        println!(
            "    - {}: {}",
            entry["landing"].as_str().unwrap_or(""),
            entry["command"].as_str().unwrap_or("")
        );
        if let Some(after) = entry.get("after").and_then(|v| v.as_str()) {
            println!("      then: {after}");
        }
    }
    println!("  then: {mark_routed}");
}

pub(crate) fn inbox(graph: Option<&Path>, cmd: InboxCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        InboxCmd::Add { text, source, link } => {
            match source.as_str() {
                "human" | "external" | "support" | "import" => {}
                "code_audit" | "wiki" | "validation" | "llm" => {
                    bail!("evidence-backed observations belong in loom finding add")
                }
                "question" => bail!("product questions belong in loom question add"),
                other => {
                    bail!("unknown inbox source '{other}' (use human|external|support|import)")
                }
            }
            let mut body = serde_json::json!({ "source": source });
            if let Some(l) = &link {
                body["link"] = serde_json::Value::String(l.clone());
            }
            let item = store.add_node(
                NodeType::InboxItem,
                &truncate(&text, 60),
                &text,
                "new",
                body,
            )?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "inbox_item": node_json(&item) }),
                "return to the current work item; triage routes it later",
                format!("inbox item [{}]", crate::model::short(&item.id)),
            )
        }
        InboxCmd::List {
            limit,
            offset,
            status,
        } => {
            // Status filter runs before paging, so page over the filtered set
            // (fetch all, filter, then skip/take) rather than the raw store
            // window — otherwise offset would count filtered-out rows.
            let filtered: Vec<_> = store
                .list_nodes(Some(NodeType::InboxItem), usize::MAX)?
                .into_iter()
                .filter(|n| status.as_deref().is_none_or(|s| n.status == s))
                .collect();
            let total = filtered.len();
            let items: Vec<_> = filtered.into_iter().skip(offset).take(limit).collect();
            if json {
                let rows: Vec<_> = items.iter().map(inbox_json).collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&super::pagination_envelope(
                        &rows, offset, limit, total
                    ))?
                );
            } else {
                if items.is_empty() && offset == 0 {
                    println!("inbox empty");
                }
                for n in &items {
                    println!(
                        "{:<10} {} [{}]",
                        n.status,
                        n.name,
                        crate::model::short(&n.id)
                    );
                }
                if let Some(footer) = super::page_footer(items.len(), offset, total) {
                    println!("{footer}");
                }
            }
            Ok(())
        }
        InboxCmd::Show { key } => {
            let n = store.resolve_node(&key, Some(NodeType::InboxItem))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&inbox_json(&n))?);
            } else {
                println!(
                    "{:<10} {} [{}]",
                    n.status,
                    n.name,
                    crate::model::short(&n.id)
                );
                println!("{}", n.description);
            }
            Ok(())
        }
        InboxCmd::Mark {
            key,
            status,
            reason,
        } => {
            const DISPOSITIONS: &[&str] = &["routed", "rejected", "duplicate", "deferred"];
            if !DISPOSITIONS.contains(&status.as_str()) {
                bail!(
                    "unknown disposition '{status}' (use {})",
                    DISPOSITIONS.join("|")
                );
            }
            let n = store.resolve_node(&key, Some(NodeType::InboxItem))?;
            store.update_node(&n.id, None, None, Some(&status))?;
            if let Some(r) = &reason {
                store.add_note(&n.id, "decision", &format!("{status}: {r}"))?;
            }
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "id": n.id, "status": status, "reason": reason }),
                "loom status",
                format!("inbox item '{}' → {status}", crate::model::short(&n.id)),
            )
        }
        InboxCmd::Remove { key } => {
            let n = store.resolve_node(&key, Some(NodeType::InboxItem))?;
            store.delete_node(&n.id)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "removed": n.id }),
                "loom status",
                format!("removed inbox item [{}]", crate::model::short(&n.id)),
            )
        }
    }
}

/// The JSON projection of one inbox item, shared by list/show.
fn inbox_json(n: &crate::model::Node) -> serde_json::Value {
    serde_json::json!({
        "id": n.id,
        "status": n.status,
        "title": n.name,
        "text": n.description,
        "source": n.body.get("source").and_then(|v| v.as_str()),
        "link": n.body.get("link").and_then(|v| v.as_str()),
        "body": n.body,
        "created_at": n.created_at,
        "updated_at": n.updated_at,
    })
}

pub(crate) fn question(graph: Option<&Path>, cmd: QuestionCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        QuestionCmd::Add { text, intent } => {
            if crate::model::is_placeholder(&text) {
                bail!("question add requires substantive text");
            }
            let intent = store.resolve_node(&intent, Some(NodeType::Intent))?;
            require_lane(&store, crate::registry::OwnerRole::Builder)?;
            let question = store.add_node(
                NodeType::Question,
                &truncate(&text, 60),
                &text,
                "open",
                serde_json::json!({ "intent": intent.id }),
            )?;
            store.add_edge(
                EdgeKind::Questions,
                &question.id,
                &intent.id,
                TruthClass::Asserted,
            )?;
            let next_step = format!(
                "ask the human, then loom question answer {} --answer '…'",
                crate::model::short(&question.id)
            );
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "question": node_json(&question),
                    "intent": node_json(&intent),
                }),
                &next_step,
                format!(
                    "question [{}] opened for '{}'",
                    crate::model::short(&question.id),
                    intent.name
                ),
            )
        }
        QuestionCmd::List {
            limit,
            offset,
            status,
        } => {
            let filtered: Vec<_> = store
                .list_nodes(Some(NodeType::Question), usize::MAX)?
                .into_iter()
                .filter(|n| status.as_deref().is_none_or(|s| n.status == s))
                .collect();
            let total = filtered.len();
            let items: Vec<_> = filtered.into_iter().skip(offset).take(limit).collect();
            if json {
                let rows: Result<Vec<_>> = items.iter().map(|n| question_json(&store, n)).collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&super::pagination_envelope(
                        &rows?, offset, limit, total
                    ))?
                );
            } else {
                if items.is_empty() && offset == 0 {
                    println!("questions empty");
                }
                for n in &items {
                    println!(
                        "{:<10} {} [{}]",
                        n.status,
                        n.name,
                        crate::model::short(&n.id)
                    );
                }
                if let Some(footer) = super::page_footer(items.len(), offset, total) {
                    println!("{footer}");
                }
            }
            Ok(())
        }
        QuestionCmd::Show { key } => {
            let n = store.resolve_node(&key, Some(NodeType::Question))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&question_json(&store, &n)?)?
                );
            } else {
                println!(
                    "{:<10} {} [{}]",
                    n.status,
                    n.name,
                    crate::model::short(&n.id)
                );
                println!("{}", n.description);
            }
            Ok(())
        }
        QuestionCmd::Answer { key, answer } => {
            if crate::model::is_placeholder(&answer) {
                bail!("question answer requires a substantive answer");
            }
            let n = store.resolve_node(&key, Some(NodeType::Question))?;
            // loom-stability-exempt: answers a question
            store.set_node_status(&n.id, "answered")?;
            store.add_note(&n.id, "decision", &format!("answered: {answer}"))?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "id": n.id, "status": "answered", "answer": answer }),
                "loom status",
                format!("question [{}] answered", crate::model::short(&n.id)),
            )
        }
        QuestionCmd::Close {
            key,
            status,
            reason,
        } => {
            const STATUSES: &[&str] = &["withdrawn", "duplicate", "deferred"];
            if !STATUSES.contains(&status.as_str()) {
                bail!(
                    "unknown question close status '{status}' (use withdrawn|duplicate|deferred)"
                );
            }
            if crate::model::is_placeholder(&reason) {
                bail!("question close requires a substantive reason");
            }
            let n = store.resolve_node(&key, Some(NodeType::Question))?;
            // loom-stability-exempt: resolves an inbox question
            store.set_node_status(&n.id, &status)?;
            store.add_note(&n.id, "decision", &format!("{status}: {reason}"))?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "id": n.id, "status": status, "reason": reason }),
                "loom status",
                format!("question [{}] → {status}", crate::model::short(&n.id)),
            )
        }
        QuestionCmd::Remove { key } => {
            let n = store.resolve_node(&key, Some(NodeType::Question))?;
            store.delete_node(&n.id)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "removed": n.id }),
                "loom status",
                format!("removed question [{}]", crate::model::short(&n.id)),
            )
        }
    }
}

fn question_json(store: &crate::store::Store, n: &crate::model::Node) -> Result<serde_json::Value> {
    let edge = store
        .edges_with(Some(EdgeKind::Questions), Some(&n.id), None)?
        .into_iter()
        .next();
    let intent = match edge {
        Some(e) => store.get_node(&e.to_id)?,
        None => None,
    };
    Ok(serde_json::json!({
        "id": n.id,
        "status": n.status,
        "title": n.name,
        "text": n.description,
        "body": n.body,
        "created_at": n.created_at,
        "updated_at": n.updated_at,
        "intent": intent.as_ref().map(node_json),
    }))
}
/// Resolve a note target: any node (name/id/fragment) or, failing that, any
/// edge (id/prefix) — adjudications attach to claims, and claims live on
/// edges too. Nodes win on collision; the precedence is deliberate.
fn resolve_note_target(store: &crate::store::Store, key: &str) -> Result<(String, String)> {
    let node_err = match store.resolve_node(key, None) {
        Ok(n) => return Ok((n.id, n.name)),
        Err(e) => e,
    };
    // An ambiguous node fragment carries candidates — surface it; never fall
    // through to an edge when the key ALMOST named a node.
    if !node_err.to_string().starts_with("no node matches") {
        return Err(node_err);
    }
    match store.resolve_edge(key) {
        Ok(edge) => {
            let endpoint = |id: &str| {
                store
                    .get_node(id)
                    .ok()
                    .flatten()
                    .map(|n| n.name)
                    .unwrap_or_else(|| id.chars().take(8).collect())
            };
            let name = format!(
                "{} —{}→ {}",
                endpoint(&edge.from_id),
                edge.kind,
                endpoint(&edge.to_id)
            );
            Ok((edge.id, name))
        }
        // Edge ambiguity carries the match count — keep it too.
        Err(edge_err) if edge_err.to_string().starts_with("ambiguous") => Err(edge_err),
        Err(_) => bail!("no node or edge matches '{key}'"),
    }
}

/// Attach and list durable notes on any node or edge — the adjudication trail.
pub(crate) fn note(graph: Option<&Path>, cmd: NoteCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        NoteCmd::Add { target, kind, text } => {
            const KINDS: &[&str] = &["decision", "context", "warning"];
            if !KINDS.contains(&kind.as_str()) {
                bail!("unknown note kind '{kind}' (use {})", KINDS.join("|"));
            }
            if text.trim().is_empty() {
                bail!("a note needs substantive --text");
            }
            let (target_id, target_name) = resolve_note_target(&store, &target)?;
            let note = store.add_note(&target_id, &kind, &text)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "note": node_json(&note),
                    "target": { "id": target_id, "name": target_name },
                }),
                "loom status",
                format!(
                    "noted {kind} on '{target_name}' [{}]",
                    crate::model::short(&note.id)
                ),
            )
        }
        NoteCmd::Remove { id } => {
            let note = store.resolve_node(&id, Some(NodeType::Note))?;
            store.delete_node(&note.id)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "removed": true,
                    "note": node_json(&note),
                }),
                "loom status",
                format!(
                    "removed accidental note '{}' [{}]",
                    note.description,
                    crate::model::short(&note.id)
                ),
            )
        }
        NoteCmd::List {
            target,
            limit,
            offset,
        } => {
            let all: Vec<_> = match target {
                Some(t) => {
                    let (id, _) = resolve_note_target(&store, &t)?;
                    store.notes_for(&id)?
                }
                None => store.list_nodes(Some(NodeType::Note), usize::MAX)?,
            };
            let total = all.len();
            let notes: Vec<_> = all.into_iter().skip(offset).take(limit).collect();
            if json {
                let rows: Vec<_> = notes
                    .iter()
                    .map(|n| {
                        serde_json::json!({
                            "id": n.id,
                            "kind": n.status,
                            "text": n.description,
                            "target_id": n.body.get("target_id").and_then(|v| v.as_str()),
                            "created_at": n.created_at,
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&super::pagination_envelope(
                        &rows, offset, limit, total
                    ))?
                );
            } else {
                if notes.is_empty() && offset == 0 {
                    println!("no notes");
                }
                for n in &notes {
                    println!(
                        "{:<9} {} [{}]",
                        n.status,
                        n.description,
                        crate::model::short(&n.id)
                    );
                }
                if let Some(footer) = super::page_footer(notes.len(), offset, total) {
                    println!("{footer}");
                }
            }
            Ok(())
        }
    }
}

/// A finished task's outcome lands as a note on its target intent: packets
/// surface notes, so the result reaches future work on that intent unprompted.
/// A task without a target (or whose target was since removed) stays a plain
/// diary entry — that is its documented contract.
fn task_outcome_note(
    store: &crate::store::Store,
    task: &crate::model::Node,
    outcome: &str,
    text: &str,
) -> Result<()> {
    let Some(target_id) = task.body.get("target_id").and_then(|v| v.as_str()) else {
        return Ok(());
    };
    if store.get_node(target_id)?.is_none() {
        return Ok(());
    }
    let kind = task
        .body
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("task");
    if crate::research::is_governed(task) {
        store.add_node(NodeType::Note, &format!("Research reference: {}", task.name), "Governed research advisory; resolve the immutable task dynamically.", "context", serde_json::json!({"kind":"research_reference", "target_id":target_id, "research_task_id":task.id}))?;
        return Ok(());
    }
    store.add_note(
        target_id,
        "context",
        &format!(
            "{kind} '{}' [{}] {outcome}: {text}",
            task.name,
            crate::model::short(&task.id),
        ),
    )?;
    Ok(())
}

pub(crate) fn task(graph: Option<&Path>, cmd: TaskCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        TaskCmd::Add {
            title,
            kind,
            target,
            why_external,
            preferred_sources,
        } => {
            let target = target
                .map(|t| store.resolve_node(&t, Some(NodeType::Intent)))
                .transpose()?;
            if kind == "research" && why_external.is_none() {
                bail!("kind=research requires --why-external");
            }
            if kind != "research" && (why_external.is_some() || !preferred_sources.is_empty()) {
                bail!("--why-external and --preferred-source are only valid for kind=research");
            }
            let mut body = if kind == "research" {
                // Invariant: kind=research requires --why-external — bailed above
                // when why_external.is_none(), and this arm is kind=="research" only.
                serde_json::json!({
                    "kind": "research",
                    "research_schema": 1,
                    "why_external": why_external.expect(
                        "kind=research requires --why-external (bailed above)"
                    ),
                    "preferred_sources": preferred_sources,
                    "sources": [],
                })
            } else {
                serde_json::json!({ "kind": kind })
            };
            if let Some(t) = &target {
                body["target_id"] = serde_json::json!(t.id);
            }
            let t = store.add_node(NodeType::TaskRecord, &title, "", "proposed", body)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "task": node_json(&t),
                    "target": target.as_ref().map(|n| serde_json::json!({ "id": n.id, "name": n.name })),
                }),
                "loom status",
                format!("task [{}] {}", crate::model::short(&t.id), t.name),
            )
        }
        TaskCmd::SourceAdd {
            task,
            url,
            title,
            publisher,
            source_kind,
            quote,
            published_at,
            fresh_until,
        } => {
            let t = store.resolve_node(&task, Some(NodeType::TaskRecord))?;
            if !crate::research::is_open_research(&t) {
                bail!("source-add requires a proposed or active research task");
            }
            store.require_research_owner()?;
            let mut body = crate::research::ResearchBody::parse(&t.body)?;
            let retrieved_at = store.current_timestamp()?;
            let quote_fingerprint = crate::research::quote_fingerprint(&quote);
            let source: crate::research::ResearchSource =
                serde_json::from_value(serde_json::json!({
                    "url": url, "title": title, "publisher": publisher,
                    "source_kind": source_kind, "retrieved_at": retrieved_at,
                    "quote": quote, "quote_fingerprint": quote_fingerprint,
                    "published_at": published_at, "fresh_until": fresh_until,
                }))?;
            source.validate()?;
            let duplicate = body
                .sources
                .iter()
                .any(|s| s.url == source.url && s.quote_fingerprint == source.quote_fingerprint);
            if !duplicate {
                body.sources.push(source.clone());
                store.set_node_body(&t.id, &serde_json::to_value(&body)?)?;
            }
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "task": t.id, "source": source, "duplicate": duplicate }),
                "loom task show",
                if duplicate {
                    "source already recorded"
                } else {
                    "research source recorded"
                },
            )
        }
        TaskCmd::Start { key } => {
            let t = store.resolve_node(&key, Some(NodeType::TaskRecord))?;
            let t = store.update_node(&t.id, None, None, Some("active"))?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "task": node_json(&t), "status": "active" }),
                "loom status",
                format!("task '{}' active", t.name),
            )
        }
        TaskCmd::Close { key, result } => {
            let t = store.resolve_node(&key, Some(NodeType::TaskRecord))?;
            if crate::research::is_governed(&t) {
                store.require_research_owner()?;
                let mut body = crate::research::ResearchBody::parse(&t.body)?;
                body.stamp_conclusion_freshness(chrono::Utc::now())?;
                let r = result.trim().to_ascii_lowercase();
                if result.trim().len() < 12
                    || matches!(
                        r.as_str(),
                        "todo" | "tbd" | "unknown" | "placeholder" | "n/a"
                    )
                    || result.contains('<')
                    || result.contains('>')
                {
                    bail!("research result must be a substantive advisory conclusion");
                }
                store.set_node_body(&t.id, &serde_json::to_value(body)?)?;
            }
            let t = store.update_node(&t.id, None, Some(&result), Some("completed"))?;
            task_outcome_note(&store, &t, "completed", &result)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "task": node_json(&t), "status": "completed", "result": result }),
                "loom status",
                format!("task '{}' completed", t.name),
            )
        }
        TaskCmd::Abandon { key, reason } => {
            let t = store.resolve_node(&key, Some(NodeType::TaskRecord))?;
            let t = store.update_node(&t.id, None, Some(&reason), Some("abandoned"))?;
            task_outcome_note(&store, &t, "abandoned", &reason)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "task": node_json(&t), "status": "abandoned", "reason": reason }),
                "loom status",
                format!("task '{}' abandoned", t.name),
            )
        }
        TaskCmd::Remove { key } => {
            let t = store.resolve_node(&key, Some(NodeType::TaskRecord))?;
            store.delete_node(&t.id)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "removed": true,
                    "task": node_json(&t),
                }),
                "loom status",
                format!(
                    "removed accidental task '{}' [{}]",
                    t.name,
                    crate::model::short(&t.id)
                ),
            )
        }
        TaskCmd::Show { key } => {
            let t = store.resolve_node(&key, Some(NodeType::TaskRecord))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&node_json(&t))?);
            } else {
                println!("{} [{}]", t.name, t.id);
                println!("  status: {}", t.status);
                println!("  {}", t.body);
            }
            Ok(())
        }
        TaskCmd::List { limit, offset } => {
            let tasks = store.list_nodes_page(Some(NodeType::TaskRecord), limit, offset)?;
            let total = store.count_nodes(Some(NodeType::TaskRecord))?;
            if json {
                let rows: Vec<_> = tasks.iter().map(node_json).collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&super::pagination_envelope(
                        &rows, offset, limit, total
                    ))?
                );
            } else {
                if tasks.is_empty() && offset == 0 {
                    println!("no tasks");
                }
                for n in &tasks {
                    println!(
                        "{:<10} {} [{}]",
                        n.status,
                        n.name,
                        crate::model::short(&n.id)
                    );
                }
                if let Some(footer) = super::page_footer(tasks.len(), offset, total) {
                    println!("{footer}");
                }
            }
            Ok(())
        }
    }
}

/// Record a decision as a reversal.
///
/// What makes this worth storing is the REJECTED alternative. "We use SQLite"
/// is a description anyone can read off the Cargo.toml; "we use SQLite instead
/// of a file-per-node store, because the ripple queries need joins" is the
/// thing that stops the next agent re-litigating it — and the thing that tells
/// them when it stops being true.
///
/// Costs the writing agent nothing: it is a byproduct of work already done, and
/// it points at a real diff rather than at an intention.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decide_cmd(
    graph: Option<&Path>,
    chose: &str,
    instead_of: &str,
    because: &str,
    evidence: &str,
    about: Option<&str>,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    for (field, value) in [
        ("<chose>", chose),
        ("--instead-of", instead_of),
        ("--because", because),
    ] {
        if crate::model::is_placeholder(value) {
            anyhow::bail!("{field} must be substantive — a decision nobody can weigh is a label");
        }
    }
    // The target: an intent or a registered file, so the decision surfaces to
    // whoever next touches that ground. Unattached decisions are allowed but
    // reach nobody, so loom says so rather than pretending otherwise.
    let target = match about {
        // An intent first, then a registered file — the two grounds an agent
        // actually stands on when it opens something.
        Some(key) => Some(
            store
                .resolve_node(key, Some(crate::model::NodeType::Intent))
                .or_else(|_| store.resolve_node(key, Some(crate::model::NodeType::CodeFile)))?,
        ),
        None => None,
    };
    let note = store.add_node(
        crate::model::NodeType::Note,
        "note:decision",
        &format!("{chose} — instead of {instead_of} — because {because}"),
        "decision",
        serde_json::json!({
            "target_id": target.as_ref().map(|n| n.id.clone()).unwrap_or_default(),
            "kind": "decision",
            "chose": chose,
            "instead_of": instead_of,
            "because": because,
            "evidence": evidence,
        }),
    )?;
    crate::journal::append(
        store.root(),
        "decision",
        target.as_ref().map(|n| n.id.as_str()).unwrap_or(""),
        serde_json::json!({
            "chose": chose,
            "instead_of": instead_of,
            "because": because,
            "evidence": evidence,
        }),
    )?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "decision": note.id,
            "chose": chose,
            "instead_of": instead_of,
            "because": because,
            "about": target.as_ref().map(|n| n.name.clone()),
        }),
        "loom status",
        match &target {
            Some(t) => format!("recorded: chose {chose} instead of {instead_of} (on '{}')", t.name),
            None => format!(
                "recorded: chose {chose} instead of {instead_of} —                  unattached, so nobody will be shown it; re-run with --about <behavior|file>"
            ),
        },
    )?;
    Ok(())
}
