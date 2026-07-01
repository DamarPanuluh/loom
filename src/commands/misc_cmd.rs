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
    if json {
        println!("{}", serde_json::to_string_pretty(&node_json(&item))?);
    } else {
        println!("captured inbox item [{}]", &item.id[..8]);
        println!("  normalize it, then route via loom intent/edge/rule, then `loom inbox mark`");
    }
    Ok(())
}
pub(crate) fn inbox(graph: Option<&Path>, cmd: InboxCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        InboxCmd::Add { text, source } => {
            let item = store.add_node(
                NodeType::InboxItem,
                &truncate(&text, 60),
                &text,
                "new",
                serde_json::json!({ "source": source }),
            )?;
            println!("inbox item [{}]", &item.id[..8]);
            Ok(())
        }
        InboxCmd::List { limit } => {
            let items = store.list_nodes(Some(NodeType::InboxItem), limit)?;
            if json {
                let rows: Vec<_> = items
                    .iter()
                    .map(|n| {
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
                    })
                    .collect();
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
        InboxCmd::Mark {
            key,
            status,
            reason,
        } => {
            let n = store.resolve_node(&key, Some(NodeType::InboxItem))?;
            store.update_node(&n.id, None, None, Some(&status))?;
            if let Some(r) = reason {
                store.add_note(&n.id, "decision", &format!("{status}: {r}"))?;
            }
            println!("inbox item '{}' → {status}", &n.id[..8]);
            Ok(())
        }
        InboxCmd::Remove { key } => {
            let n = store.resolve_node(&key, Some(NodeType::InboxItem))?;
            store.delete_node(&n.id)?;
            println!("removed inbox item [{}]", &n.id[..8]);
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
            println!("task [{}] {}", &t.id[..8], t.name);
            Ok(())
        }
        TaskCmd::Start { key } => {
            let t = store.resolve_node(&key, Some(NodeType::TaskRecord))?;
            store.update_node(&t.id, None, None, Some("active"))?;
            println!("task '{}' active", t.name);
            Ok(())
        }
        TaskCmd::Close { key, result } => {
            let t = store.resolve_node(&key, Some(NodeType::TaskRecord))?;
            store.update_node(&t.id, None, Some(&result), Some("completed"))?;
            println!("task '{}' completed", t.name);
            Ok(())
        }
        TaskCmd::Abandon { key, reason } => {
            let t = store.resolve_node(&key, Some(NodeType::TaskRecord))?;
            store.update_node(&t.id, None, Some(&reason), Some("abandoned"))?;
            println!("task '{}' abandoned", t.name);
            Ok(())
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
    let planned = store
        .nodes_by_status(NodeType::Intent, &["planned", "needs_change"])?
        .len();
    let stale = store
        .edges_by_status(
            TruthClass::Asserted,
            &[
                InspectionStatus::NeedsReverification,
                InspectionStatus::Failing,
            ],
        )?
        .len();
    let uninspected = store
        .edges_by_status(TruthClass::Asserted, &[InspectionStatus::Uninspected])?
        .len();
    let inbox = store
        .list_nodes(Some(NodeType::InboxItem), usize::MAX)?
        .len();
    let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?.len();
    let codefiles = store
        .list_nodes(Some(NodeType::CodeFile), usize::MAX)?
        .len();
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
                "planned": planned,
                "stale": stale,
                "uninspected": uninspected,
                "inbox": inbox,
                "intents": intents,
                "codefiles": codefiles,
                "phase": ladder.phase,
                "recommended": ladder.next_command,
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
    if stale > 0 {
        println!("  - repair {stale} failing/stale claim(s)   [loom next --mode fix]");
    } else if planned > 0 {
        println!("  - build {planned} unrealized intent(s)    [loom next --mode build]");
    } else if uninspected > 0 {
        println!("  - inspect {uninspected} claim(s)           [loom next --mode analyze]");
    } else if intents == 0 && codefiles == 0 {
        println!("  - fresh graph — nothing mapped yet. Start here:");
        println!("      loom guide                  the driving loop + roles");
        println!("      loom guide --role monitor   watch an upstream you depend on");
        println!("      loom intent add --name <pillar>   seed what this codebase should do");
    } else {
        println!("  - graph is settled; map more, or just get to work");
    }
    if inbox > 0 {
        println!("  - {inbox} inbox item(s) to triage          [loom inbox list]");
    }
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
                "authoritative_write": g.authoritative_write,
                "forbidden_write": g.forbidden_write,
                "after_write": g.after_write,
            })
        })
        .collect()
}
pub(crate) fn guide(role: Option<&str>, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "role": role,
                "commands": ["loom sync", "loom next --all", "loom status", "loom coverage", "loom doctor", "loom export --check", "loom door"],
                "roles": ["builder", "analyzer", "fixer", "validator", "quality", "monitor"],
                "rung_gates": ["seeded", "realized", "proven", "hardened", "excellent", "exported"],
                "closeout": ["loom coverage", "loom doctor", "loom next --all", "loom export", "loom export --check"],
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
            println!(
                "Closeout gates: loom coverage; loom doctor; loom next --all; loom export --check."
            );
            println!("Truth forms — fill the one that is stale/missing (loom next names it):");
            for axis in crate::truth::TRUTH_AXES {
                let g = axis.gap();
                println!("  {:<15} {}", g.axis.as_str(), g.missing_form);
                println!("      make true: {}", g.authoritative_write);
                println!("      then:      {}", g.after_write);
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
            let (mindset, allowed, forbidden) = match r {
                "builder" => (
                    "Use Loom first to understand why, likely files/entities, and prior evidence; then inspect relevant code before editing. Functions are locators, not intents.",
                    "loom status; loom next --all; loom intent show <intent>; loom codefile list; loom codefile show <file>; edit code; loom edge implement; loom intent mark; loom sync",
                    "loom rule verdict passing; loom validation mark passed",
                ),
                "analyzer" => (
                    "Read both sides; hypothesis first; record exactly what the code shows. Also triages findings — record justified/needed/blocked with a reason.",
                    "loom edge explore ground|issue|independent; loom finding verdict <id> justified|needed|blocked --reason '…'",
                    "edit code; verdict from name similarity",
                ),
                "fixer" => (
                    "Use Loom first to understand the stale/failing criterion, linked entities, likely files, and prior evidence; then inspect relevant code before repairing the root cause. Findings judged `needed` are queued work — consult `loom finding list --state needed`.",
                    "loom status; loom next --all; loom edge show <edge_id>; loom intent show <linked intent>; loom codefile show <file>; edit code; loom sync; re-ground; loom finding list --state needed",
                    "suppress the symptom; mark passing without re-verification",
                ),
                "validator" => (
                    "Run or honestly mark proofs; never edit code to make a proof pass.",
                    "run validation; loom validation mark passed|failed|blocked",
                    "edit code; mark passed without observed proof",
                ),
                "quality" => (
                    "Measure a rule against an intent at the highest honest altitude.",
                    "loom rule verdict passing|failing|independent",
                    "edit code; mark passing without inspecting",
                ),
                other => bail!("unknown role '{other}'"),
            };
            println!("role: {r}");
            println!("  mindset:   {mindset}");
            println!("  allowed:   {allowed}");
            println!("  forbidden: {forbidden}");
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
    println!("       loom validation mark \"<validation name>\" --result passed --evidence \"<how you verified it>\"");
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
    println!("  Check every integration point is under contract:  loom interface gaps");
}
pub(crate) fn find_cmd(graph: Option<&Path>, query: &str, limit: usize, json: bool) -> Result<()> {
    let store = open(graph)?;
    let q = query_terms(query);
    let score = |hay: &str| -> usize {
        let h = hay.to_lowercase();
        q.iter().filter(|t| h.contains(t.as_str())).count()
    };
    let mut hits: Vec<(usize, String, String, String)> = Vec::new();
    for nt in [NodeType::Intent, NodeType::CodeFile, NodeType::QualityRule] {
        for n in store.list_nodes(Some(nt), usize::MAX)? {
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
    let limited: Vec<_> = hits.into_iter().take(limit).collect();
    if json {
        let mut rows = Vec::new();
        for (s, kind, name, id) in &limited {
            let mut groundings = Vec::new();
            if kind == "intent" {
                for e in store.edges_with(Some(EdgeKind::Implements), Some(id), None)? {
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
                if grounds.is_empty() {
                    println!("             ↳ (ungrounded — no implements edge yet)");
                }
                for e in grounds {
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
                    println!("             ↳ {path}{at} [{}]{ev}", e.status.as_str());
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
    if json {
        let mut recommended_packs = vec!["iso5055"];
        if markers.contains(&"docker") {
            recommended_packs.push("docker");
        }
        if markers.contains(&"node") {
            recommended_packs.push("web-ui");
            recommended_packs.push("service");
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "languages": langs,
                "project_markers": markers,
                "recommended_quality_packs": recommended_packs,
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
    println!("recommended quality packs: iso5055 (baseline)");
    if markers.contains(&"docker") {
        println!("  + docker");
    }
    if markers.contains(&"node") {
        println!("  + web-ui / service (inspect the app)");
    }
    println!("(quality packs land in ring 5: `loom rule seed <pack>`)");
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
