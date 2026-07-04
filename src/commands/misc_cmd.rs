use super::*;

pub(crate) fn door(graph: Option<&Path>, utterance: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    let item = store.add_node(
        NodeType::InboxItem,
        &truncate(utterance, 60),
        utterance,
        "new",
        serde_json::json!({ "source": "human" }),
    )?;
    let short = &item.id[..8.min(item.id.len())];
    // Routing context: the closest existing intents, the compass, and a landing
    // menu of prefilled commands. The caller (usually the PM-side model) picks
    // ONE landing, runs it, then marks the capture routed — nothing is decided
    // here, but nothing requires a second lookup either.
    let matches = keyword_hits(&store, utterance, &[NodeType::Intent], 5)?;
    let ladder = crate::maturity::ladder(&store)?;
    let q = crate::workitem::q;
    let mark_routed = format!("loom inbox mark {short} routed --reason '<destination>'");
    let mut menu: Vec<serde_json::Value> = Vec::new();
    let strong_matches: Vec<_> = matches
        .iter()
        .filter(|(score, _, _, _)| *score >= 2)
        .collect();
    let weak_matches: Vec<_> = matches
        .iter()
        .filter(|(score, _, _, _)| *score < 2)
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
        "after": "loom next --mode elaborate grows the forgotten surroundings (failure scenarios, prerequisites, questions)",
    }));
    menu.push(serde_json::json!({
        "landing": "hypothesis",
        "why": "the utterance is a redesign idea — prove it before it becomes work",
        "command": "loom hypothesis add --name '<idea>' --claim '<what is wrong now>' --proposal '<the change>' --predicted-outcome '<measurable result>' --target '<intent>'",
    }));
    menu.push(serde_json::json!({
        "landing": "spike",
        "why": "the utterance needs investigation before it can land anywhere",
        "command": "loom task add '<question>' --kind investigation",
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
                "compass": { "phase": ladder.phase, "next_command": ladder.next_command },
                "landing_menu": menu,
                "next_step": format!("choose ONE landing, run it, then: {mark_routed}"),
            }))?
        );
    } else {
        println!("captured inbox item [{short}]");
        if !strong_matches.is_empty() {
            println!("  closest intents:");
            for (score, _, name, id) in &strong_matches {
                println!(
                    "    - {} [{}] (strong score {score})",
                    name,
                    &id[..8.min(id.len())]
                );
            }
        }
        if !weak_matches.is_empty() {
            println!("  weak intent matches:");
            for (score, _, name, id) in &weak_matches {
                println!(
                    "    - {} [{}] (weak score {score}; prefer new_intent unless this truly refines it)",
                    name,
                    &id[..8.min(id.len())]
                );
            }
        }
        println!("  landings:");
        for m in &menu {
            println!(
                "    - {}: {}",
                m["landing"].as_str().unwrap_or(""),
                m["command"].as_str().unwrap_or("")
            );
        }
        println!("  then: {mark_routed}");
    }
    Ok(())
}
pub(crate) fn inbox(graph: Option<&Path>, cmd: InboxCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        InboxCmd::Add { text, source, link } => {
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
                format!("inbox item [{}]", &item.id[..8]),
            )
        }
        InboxCmd::List { limit, status } => {
            let items: Vec<_> = store
                .list_nodes(Some(NodeType::InboxItem), limit)?
                .into_iter()
                .filter(|n| status.as_deref().is_none_or(|s| n.status == s))
                .collect();
            if json {
                let rows: Vec<_> = items.iter().map(inbox_json).collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                if items.is_empty() {
                    println!("inbox empty");
                }
                for n in &items {
                    println!("{:<10} {} [{}]", n.status, n.name, &n.id[..8]);
                }
            }
            Ok(())
        }
        InboxCmd::Show { key } => {
            let n = store.resolve_node(&key, Some(NodeType::InboxItem))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&inbox_json(&n))?);
            } else {
                println!("{:<10} {} [{}]", n.status, n.name, &n.id[..8]);
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
                format!("inbox item '{}' → {status}", &n.id[..8]),
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
                format!("removed inbox item [{}]", &n.id[..8]),
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
                format!("noted {kind} on '{target_name}' [{}]", &note.id[..8]),
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
                    &note.id[..8]
                ),
            )
        }
        NoteCmd::List { target, limit } => {
            let target_id = target
                .map(|t| resolve_note_target(&store, &t))
                .transpose()?
                .map(|(id, _)| id);
            let notes: Vec<_> = store
                .list_nodes(Some(NodeType::Note), usize::MAX)?
                .into_iter()
                .filter(|n| {
                    target_id
                        .as_deref()
                        .is_none_or(|t| n.body.get("target_id").and_then(|v| v.as_str()) == Some(t))
                })
                .take(limit)
                .collect();
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
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                if notes.is_empty() {
                    println!("no notes");
                }
                for n in &notes {
                    println!("{:<9} {} [{}]", n.status, n.description, &n.id[..8]);
                }
            }
            Ok(())
        }
    }
}

pub(crate) fn task(graph: Option<&Path>, cmd: TaskCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        TaskCmd::Add { title, kind } => {
            let t = store.add_node(
                NodeType::TaskRecord,
                &title,
                "",
                "proposed",
                serde_json::json!({ "kind": kind }),
            )?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "task": node_json(&t) }),
                "loom status",
                format!("task [{}] {}", &t.id[..8], t.name),
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
            let t = store.update_node(&t.id, None, Some(&result), Some("completed"))?;
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
                format!("removed accidental task '{}' [{}]", t.name, &t.id[..8]),
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
        TaskCmd::List { limit } => {
            let tasks = store.list_nodes(Some(NodeType::TaskRecord), limit)?;
            if json {
                let rows: Vec<_> = tasks.iter().map(node_json).collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                if tasks.is_empty() {
                    println!("no tasks");
                }
                for n in &tasks {
                    println!("{:<10} {} [{}]", n.status, n.name, &n.id[..8]);
                }
            }
            Ok(())
        }
    }
}
pub(crate) fn session(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    // One source of truth for the counts: the same pulse every work item and
    // mutating command emits. Session only adds the offer framing on top.
    let pulse = crate::workitem::graph_state(&store)?;
    let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?.len();
    let codefiles = store
        .list_nodes(Some(NodeType::CodeFile), usize::MAX)?
        .len();
    let open_axes: usize = crate::completeness::all_scorecards(&store)?
        .iter()
        .filter(|c| c.visibility.as_deref() == Some("user_visible"))
        .map(|c| c.open)
        .sum();
    let ladder = crate::maturity::ladder(&store)?;
    if json {
        let rungs: Vec<_> = ladder
            .rungs
            .iter()
            .map(|r| {
                serde_json::json!({
                    "name": r.name,
                    "state": r.state,
                    "detail": r.detail,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "graph_state": pulse,
                "intents": intents,
                "codefiles": codefiles,
                "open_completeness_axes": open_axes,
                "phase": ladder.phase,
                "recommended": ladder.next_command,
                "capture_entry": "loom door \"<utterance>\" — capture-first entry for a new topic/story/change",
                "rungs": rungs,
            }))?
        );
        return Ok(());
    }
    println!("what do you want from this session? offers:");
    println!(
        "  - recommended: {}              (phase: {})",
        ladder.next_command, ladder.phase
    );
    let queues = crate::workitem::queue_counts(&store)?;
    if queues.fix > 0 {
        println!(
            "  - repair {} failing claim(s)       [loom next --mode fix]",
            queues.fix
        );
    } else if queues.build > 0 {
        println!(
            "  - build {} unrealized intent(s)    [loom next --mode build]",
            queues.build
        );
    } else if queues.coverage > 0 {
        println!(
            "  - own {} uncovered codefile(s)     [loom next --mode coverage]",
            queues.coverage
        );
    } else if queues.validate > 0 {
        println!(
            "  - prove {} open proof claim(s)     [loom next --mode validate]",
            queues.validate
        );
    } else if queues.quality > 0 {
        println!(
            "  - measure {} quality claim(s)      [loom next --mode quality]",
            queues.quality
        );
    } else if queues.analyze > 0 {
        println!(
            "  - inspect {} claim(s)              [loom next --mode analyze]",
            queues.analyze
        );
    } else if intents == 0 && codefiles == 0 {
        println!("  - fresh graph — nothing mapped yet. Start here:");
        println!("      loom guide                  the driving loop + roles");
        println!("      loom guide --role monitor   watch an upstream you depend on");
        println!("      loom intent add --name <pillar>   seed what this codebase should do");
    } else if queues.prove + queues.triage + queues.review + queues.elaborate == 0 {
        println!("  - graph is settled; map more, or just get to work");
    }
    if pulse.open_questions > 0 {
        println!(
            "  - {} question(s) waiting for YOUR answer  [loom inbox list --status new]",
            pulse.open_questions
        );
    }
    if pulse.inbox > pulse.open_questions {
        println!(
            "  - {} inbox item(s) to triage          [loom inbox list --status new]",
            pulse.inbox - pulse.open_questions
        );
    }
    if pulse.low_confidence > 0 {
        println!(
            "  - re-inspect {} low-confidence verdict(s) [loom next --mode review]",
            pulse.low_confidence
        );
    }
    if open_axes > 0 {
        println!(
            "  - grow {open_axes} open completeness axis(es) around user-visible ideas [loom next --mode elaborate]"
        );
    }
    println!(
        "  - got a topic/story/change in mind?  loom door \"<utterance>\"   (capture + landing menu)"
    );
    Ok(())
}
fn truth_axis_matrix() -> Vec<serde_json::Value> {
    crate::truth::TRUTH_AXES
        .iter()
        .map(|axis| {
            let g = axis.gap();
            serde_json::json!({
                "axis": g.axis.as_str(),
                "missing_form": g.missing_form,
                "correct_when": g.correct_when,
                "authoritative_write": g.authoritative_write,
                "forbidden_write": g.forbidden_write,
                "after_write": g.after_write,
            })
        })
        .collect()
}
fn operator_loops() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "mode": "seeding",
            "purpose": "turn ambiguous product/code understanding into durable graph artifacts",
            "caller": "user or orchestrator chooses this when using a stronger model or human operator",
            "prefer": [
                "loom door <utterance>",
                "loom next --mode coverage",
                "loom next --mode build",
                "loom next --mode elaborate",
                "loom journey coverage discover",
                "loom journey prompt <intent>",
                "loom rule seed <pack>"
            ],
            "creates": [
                "intents",
                "scenario families",
                "prerequisite edges",
                "interface boundaries",
                "validations",
                "journey coverage",
                "journey invariant points",
                "product questions",
                "reasoned non-question waivers"
            ],
            "forbidden": [
                "answering product questions for the human",
                "marking proofs passed without observed runs",
                "using prose summaries instead of graph artifacts"
            ],
        }),
        serde_json::json!({
            "mode": "draining",
            "purpose": "close already-routed graph gaps one packet at a time",
            "caller": "user or orchestrator chooses this when using a cheaper/bounded model or automation",
            "prefer": [
                "loom next",
                "loom next --mode fix",
                "loom next --mode validate",
                "loom next --mode quality",
                "loom next --mode analyze",
                "loom next --mode review",
                "loom validation run <intent>",
                "loom journey run <spec>",
                "loom export --check"
            ],
            "closes": [
                "failing/stale implementation claims",
                "unrun validations",
                "stale journey proofs",
                "unmeasured quality rules",
                "uninspected relationships",
                "low-confidence review items",
                "export freshness"
            ],
            "forbidden": [
                "inventing broad product structure",
                "expanding beyond the packet",
                "silently waiving missing meaning"
            ],
        }),
    ]
}

fn print_operator_loops() {
    println!("Operator modes — caller chooses the mode/model; evidence still proves truth:");
    println!("  seeding   use a stronger model/human to turn ambiguous understanding into graph artifacts");
    println!("            create intents, scenarios, prerequisites, validations, journey coverage, invariant points, questions, and reasoned waivers");
    println!(
        "            do not answer product questions or mark proofs passed without observed runs"
    );
    println!(
        "  draining  use a bounded/cheaper model to close already-routed gaps one packet at a time"
    );
    println!("            run validations/journeys, inspect stated claims, record evidence, confidence, or blocked prerequisites");
    println!("            do not invent broad product structure or expand beyond the packet");
    println!("  invariant mode routes work; role controls writes; evidence determines truth.");
}

pub(crate) fn guide(role: Option<&str>, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "role": role,
                "commands": ["loom sync", "loom next --all", "loom status", "loom coverage", "loom doctor", "loom export --check", "loom door"],
                "intake": {
                    "raw_thought": "loom door \"<utterance>\" — capture first, route later (inbox mark closes it)",
                    "structured_plan": "loom proposal add --title '…' (--file <path> | --text '…') — decompose into adoptable items",
                    "falsifiable_design_claim": "loom hypothesis add --name '…' --claim '…' --target <intent> — prove supported|refuted before it becomes work",
                    "timeboxed_activity": "loom task add '<title>' --kind spike — close with a result; promote durable outcomes to graph facts"
                },
                "roles": ["builder", "analyzer", "fixer", "validator", "quality", "monitor"],
                "rung_gates": ["seeded", "realized", "proven", "hardened", "excellent", "exported"],
                "closeout": ["loom coverage", "loom doctor", "loom next --all", "loom export", "loom export --check"],
                "operator_loops": operator_loops(),
                "truth_axes": truth_axis_matrix(),
            }))?
        );
        return Ok(());
    }
    match role {
        None => {
            println!("loom — driving protocol (the loop):");
            println!("  loom sync       recompute the structural plane after code changes");
            println!("  loom next --all show every lane queue + compass");
            println!("  loom next       serve one work item + its prompt contract");
            println!("  loom status     rung ladder + the single next move");
            println!("  loom door       capture a raw utterance before routing it");
            println!("Capture routing — pick the entrance by input shape:");
            println!("  raw thought / finding / question   loom door \"<utterance>\"        capture first; route via inbox mark");
            println!("  structured plan / RFC              loom proposal add               decompose into adoptable items");
            println!("  falsifiable design claim           loom hypothesis add             prove supported|refuted, then adopt");
            println!("  timeboxed activity                 loom task add                   close with a result; promote outcomes to graph facts");
            println!(
                "Closeout gates: loom coverage; loom doctor; loom next --all; loom export --check."
            );
            print_operator_loops();
            println!();
            println!("Truth forms — fill the one that is stale/missing (loom next names it):");
            for axis in crate::truth::TRUTH_AXES {
                let g = axis.gap();
                println!("  {:<15} {}", g.axis.as_str(), g.missing_form);
                println!("      correct when: {}", g.correct_when);
                println!("      make true:    {}", g.authoritative_write);
                println!("      then:         {}", g.after_write);
            }
            println!("Roles: builder | analyzer | fixer | validator | quality (see `loom guide --role`).");
            println!("Integration monitoring (watch an upstream you depend on): loom guide --role monitor");
            Ok(())
        }
        Some("monitor") => {
            guide_monitor();
            Ok(())
        }
        Some(r) => {
            let (mindset, allowed, forbidden, axis) = match r {
                "builder" => (
                    "Use Loom first to understand why, likely files/entities, and prior evidence; then inspect relevant code before editing. Functions are locators, not intents.",
                    "loom status; loom next --all; loom intent show <intent>; loom codefile list; loom codefile show <file>; edit code; loom edge implement; loom intent update <intent> --lifecycle implemented --reason '…'; loom sync",
                    "loom rule verdict passing; loom validation verdict passed",
                    crate::truth::TruthAxis::Implementation,
                ),
                "analyzer" => (
                    "Read both sides; hypothesis first; record exactly what the code shows. Also triages findings — record justified/needed/blocked with a reason. Serves the review queue too: re-inspect low-confidence verdicts independently before reading the recorded evidence.",
                    "loom edge explore <a> <b> ground|issue|independent; loom edge verdict <edge_id> ground|issue|independent (non-relates claims); loom finding verdict <id> justified|needed|blocked --reason '…'",
                    "edit code; verdict from name similarity; inheriting a prior verdict's confidence",
                    crate::truth::TruthAxis::Verdict,
                ),
                "fixer" => (
                    "Use Loom first to understand the stale/failing criterion, linked entities, likely files, and prior evidence; then inspect relevant code before repairing the root cause. Findings judged `needed` are queued work — consult `loom finding list --state needed`. After the fix, `loom sync` re-opens the claim and its owning lane re-measures — do not record the verdict yourself.",
                    "loom status; loom next --all; loom edge show <edge_id>; loom intent show <linked intent>; loom codefile show <file>; edit code; loom sync; re-ground; loom finding list --state needed",
                    "suppress the symptom; record the passing verdict from the fixer hat",
                    crate::truth::TruthAxis::Implementation,
                ),
                "validator" => (
                    "Run or honestly mark proofs; never edit code to make a proof pass.",
                    "run validation; loom validation run <intent>; loom validation verdict <validation> passed|failed|blocked --evidence '…'",
                    "edit code; mark passed without observed proof",
                    crate::truth::TruthAxis::Proof,
                ),
                "quality" => (
                    "Measure a rule against an intent at the highest honest altitude. Follow the rule's inspection_guide and evidence_template from the work packet; do not invent your own protocol.",
                    "loom rule verdict <rule> <intent> passing|failing|independent --criterion '…' --evidence '…' --confidence <n>",
                    "edit code; mark passing without inspecting; mark independent without evidence",
                    crate::truth::TruthAxis::Verdict,
                ),
                other => bail!("unknown role '{other}'"),
            };
            println!("role: {r}");
            println!("  mindset:   {mindset}");
            println!(
                "  axis:      {} — correct when {}",
                axis.as_str(),
                axis.gap().correct_when
            );
            println!("  allowed:   {allowed}");
            println!("  forbidden: {forbidden}");
            println!("  honesty:   confidence below {} (the default policy cutoff) routes the verdict to review — uncertainty is honest, a confident guess corrupts the graph", crate::policy::DEFAULT_REVIEW_CONFIDENCE_FLOOR);
            println!("  set: export LOOM_AGENT=llm:{r}");
            Ok(())
        }
    }
}
fn guide_monitor() {
    println!("loom — integration monitoring (watch an upstream you depend on):");
    println!(
        "  Goal: when an upstream you consume changes, loom resets the contracts that exercise it,"
    );
    println!("  so `loom sync` tells you exactly what needs re-checking. This is your own graph.");
    println!(
        "  Pass intents/validations/surfaces by NAME (the quoted string) or by the short [id]."
    );
    println!();
    println!("  1. Get the upstream's files onto disk under vendor/<name>/ . If it is a git repo,");
    println!("     a submodule keeps it pinned; otherwise just copy/vendor the files in:");
    println!(
        "       git submodule add <upstream-url> vendor/<name>     # or vendor the files by hand"
    );
    println!("  2. Register the upstream files you depend on:");
    println!("       loom codefile add 'vendor/<name>/**/*.rs'");
    println!("  3. Name what YOUR code needs from the upstream as an intent (this CREATES it):");
    println!("       loom intent add --name \"<what your service relies on>\"");
    println!("  4. Declare each integration point you consume as a surface, bound to its file:");
    println!(
        "       loom surface add --name <Point> --kind sdk_method --codefile vendor/<name>/<file>"
    );
    println!("       (kinds: http | cli | ui_route | message_topic | sdk_method | internal_module | storage)");
    println!("  5. Put the point under contract — a validation that exercises the surface,");
    println!("     linked to the intent from step 3:");
    println!("       loom validation add --name \"<what you rely on>\" --type contract --intent \"<intent from step 3>\"");
    println!("       loom edge call \"<validation name>\" \"<surface name>\"");
    println!("  6. Baseline: sync, then record that the contract holds right now:");
    println!("       loom sync");
    println!("       loom validation verdict \"<validation name>\" passed --evidence \"<how you verified it>\"");
    println!("  7. Later, after the upstream moves (re-pull, rescan for new files, then sync):");
    println!(
        "       git submodule update --remote vendor/<name>     # or update the vendored files"
    );
    println!("       loom codefile rescan     # register any endpoints the upstream just added");
    println!("       loom sync     # → 'integration: N upstream surface(s) changed → M contract(s) need re-verification'");
    println!(
        "       loom next --mode validate     # re-verify each contract against the new upstream"
    );
    println!();
    println!("  Check every integration point is under contract:  loom surface gaps");
}
/// Keyword scoring shared by `loom find` and the door's landing menu: score
/// nodes of the given kinds against the query terms, best first, capped at
/// `limit`. Returns `(score, kind, name, id)` rows.
fn keyword_hits(
    store: &Store,
    query: &str,
    kinds: &[NodeType],
    limit: usize,
) -> Result<Vec<(usize, String, String, String)>> {
    let q = query_terms(query);
    let score = |hay: &str| -> usize {
        let h = hay.to_lowercase();
        q.iter().filter(|t| h.contains(t.as_str())).count()
    };
    let mut hits: Vec<(usize, String, String, String)> = Vec::new();
    for nt in kinds {
        for n in store.list_nodes(Some(*nt), usize::MAX)? {
            if n.status == "deprecated" {
                continue;
            }
            let s = score(&n.name) * 2 + score(&n.description);
            if s > 0 {
                hits.push((s, nt.as_str().to_string(), n.name.clone(), n.id.clone()));
            }
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(&b.2)));
    hits.truncate(limit);
    Ok(hits)
}

pub(crate) fn find_cmd(graph: Option<&Path>, query: &str, limit: usize, json: bool) -> Result<()> {
    let store = open(graph)?;
    let limited = keyword_hits(
        &store,
        query,
        &[NodeType::Intent, NodeType::CodeFile, NodeType::QualityRule],
        limit,
    )?;
    if json {
        let mut rows = Vec::new();
        for (s, kind, name, id) in &limited {
            let mut groundings = Vec::new();
            if kind == "intent" {
                for e in store.edges_with(Some(EdgeKind::Implements), Some(id), None)? {
                    if store.edge_superseded(&e.id)? {
                        continue;
                    }
                    let path = store
                        .get_node(&e.to_id)?
                        .map(|n| n.name)
                        .unwrap_or_else(|| e.to_id.clone());
                    let locator = store
                        .get_facet(&e.id, TargetKind::Edge, "locator")?
                        .unwrap_or_default();
                    groundings.push(serde_json::json!({
                        "edge_id": e.id,
                        "path": path,
                        "locator": locator,
                        "role": store.grounding_role(&e.id)?.as_str(),
                        "status": e.status.as_str(),
                        "evidence": e.evidence,
                    }));
                }
            }
            rows.push(serde_json::json!({
                "score": s,
                "kind": kind,
                "name": name,
                "id": id,
                "groundings": groundings,
            }));
        }
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        if limited.is_empty() {
            println!(
                "no match for '{query}' — try `loom status` to see coverage, or it may not exist"
            );
        }
        for (s, kind, name, id) in limited {
            println!("{:<10} {} [{}] (score {s})", kind, name, &id[..8]);
            if kind == "intent" {
                let grounds = store.edges_with(Some(EdgeKind::Implements), Some(&id), None)?;
                if store.realizing_groundings(&id)?.is_empty() {
                    println!("             ↳ (no realizing grounding yet)");
                }
                for e in grounds {
                    if store.edge_superseded(&e.id)? {
                        continue;
                    }
                    let role = store.grounding_role(&e.id)?;
                    let path = store
                        .get_node(&e.to_id)?
                        .map(|n| n.name)
                        .unwrap_or_else(|| e.to_id.clone());
                    let loc = store
                        .get_facet(&e.id, TargetKind::Edge, "locator")?
                        .unwrap_or_default();
                    let at = if loc.is_empty() {
                        String::new()
                    } else {
                        format!(" @ {loc}")
                    };
                    let ev = if e.evidence.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", e.evidence)
                    };
                    println!(
                        "             ↳ [{role}] {path}{at} [{}]{ev}",
                        e.status.as_str()
                    );
                }
            }
        }
    }
    Ok(())
}
pub(crate) fn detect_cmd(graph: Option<&Path>, json: bool) -> Result<()> {
    let root = resolve_root(graph).or_else(|_| std::env::current_dir())?;
    let mut langs: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    let mut markers: Vec<&str> = Vec::new();
    for (marker, label) in [
        ("Cargo.toml", "rust"),
        ("package.json", "node"),
        ("go.mod", "go"),
        ("pyproject.toml", "python"),
        ("Dockerfile", "docker"),
    ] {
        if root.join(marker).exists() {
            markers.push(label);
        }
    }
    count_exts(&root, &mut langs, 0);
    // Recommend only packs that actually exist (crate::packs::PACKS), from
    // honest signals: a recommendation the seeder rejects is a dead end.
    let mut recommended: Vec<&str> = vec!["iso5055"];
    if markers.contains(&"docker") {
        recommended.push("docker");
    }
    if markers.contains(&"node") {
        recommended.push("web-ui");
        recommended.push("service");
    }
    if markers.contains(&"rust") || markers.contains(&"go") {
        recommended.push("concurrency");
    }
    if root.join("migrations").is_dir() || langs.contains_key("sql") {
        recommended.push("data");
    }
    debug_assert!(
        recommended.iter().all(|p| crate::packs::PACKS.contains(p)),
        "detect recommended a pack that cannot be seeded"
    );
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "languages": langs,
                "project_markers": markers,
                "recommended_quality_packs": recommended,
                "available_packs": crate::packs::PACKS,
            }))?
        );
        return Ok(());
    }
    println!("detected languages:");
    for (ext, n) in &langs {
        println!("  {ext}: {n} file(s)");
    }
    println!(
        "project markers: {}",
        if markers.is_empty() {
            "none".into()
        } else {
            markers.join(", ")
        }
    );
    println!("recommended quality packs: {}", recommended.join(", "));
    println!(
        "  seed with: loom rule seed <pack>   (available: {})",
        crate::packs::PACKS.join(", ")
    );
    Ok(())
}
fn count_exts(
    dir: &Path,
    langs: &mut std::collections::BTreeMap<&'static str, usize>,
    depth: usize,
) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            count_exts(&p, langs, depth + 1);
        } else if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
            let label = match ext {
                "rs" => "rust",
                "py" => "python",
                "go" => "go",
                "ts" | "tsx" => "typescript",
                "js" | "jsx" => "javascript",
                "sql" => "sql",
                _ => continue,
            };
            *langs.entry(label).or_insert(0) += 1;
        }
    }
}
pub(crate) fn schema_cmd(json: bool) -> Result<()> {
    use crate::model::*;
    if json {
        let edge_kinds: Vec<_> = crate::registry::REGISTRY
            .iter()
            .map(|s| {
                serde_json::json!({
                    "kind": s.kind.as_str(),
                    "from": s.from.as_str(),
                    "to": s.to.as_str(),
                    "truth_classes": s.truth_classes.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
                    "owner": s.owner.as_str(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "node_types": NodeType::ALL.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
                "edge_kinds": edge_kinds,
                "inspection_statuses": InspectionStatus::ALL.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                "intent_lifecycle": IntentLifecycle::ALL.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
                "truth_classes": TruthClass::ALL.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
                "finding_verdicts": ["justified", "needed", "blocked"],
            }))?
        );
        return Ok(());
    }
    println!("node types:");
    for t in NodeType::ALL {
        println!("  {}", t.as_str());
    }
    println!("edge kinds (from registry):");
    for s in crate::registry::REGISTRY {
        let tcs: Vec<&str> = s.truth_classes.iter().map(|t| t.as_str()).collect();
        println!(
            "  {:<12} {} → {}  [{}] owner={}",
            s.kind.as_str(),
            s.from.as_str(),
            s.to.as_str(),
            tcs.join("|"),
            s.owner.as_str()
        );
    }
    println!("inspection statuses:");
    for s in InspectionStatus::ALL {
        print!(" {}", s.as_str());
    }
    println!();
    println!(
        "intent lifecycle: {}",
        IntentLifecycle::ALL
            .iter()
            .map(|l| l.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!(
        "truth classes (stored edges): {}",
        TruthClass::ALL
            .iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!("finding verdicts:");
    println!("  justified | needed | blocked");
    println!("  stored as asserted adjudication facets on stable derived Finding ids");
    println!("  verdicts go stale when the flagged codefile content hash changes");
    Ok(())
}
