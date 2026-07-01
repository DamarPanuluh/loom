//! Command handlers (ring 1 subset).
//!
//! Plane: orchestration. Resolves the target graph, calls the store, renders
//! output. No SQL here — that lives in `crate::store`.

use crate::cli::{
    Cli, CodefileCmd, Command, FindingCmd, HypothesisCmd, IgnoreCmd, InboxCmd, InterfaceCmd,
    LayerCmd, RuleCmd, SurfaceCmd, TaskCmd, ValidationCmd, VocabCmd,
};
use crate::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use crate::store::Store;
use crate::Result;
use crate::{travel, workitem};
use anyhow::{anyhow, bail};
use std::path::{Path, PathBuf};

mod edge;
mod intent;
mod saga;
pub use intent::looks_like_symbol;

/// Dispatch a parsed CLI invocation.
pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Init {
            path,
            name,
            observed,
        } => {
            let root = path.unwrap_or_else(|| PathBuf::from("."));
            let store = Store::init(&root, name.as_deref(), observed)?;
            let id = store.identity()?;
            println!(
                "initialized graph '{}' ({}) at {}",
                id.name,
                &id.graph_id[..8.min(id.graph_id.len())],
                root.join(crate::LOOM_DIR).display()
            );
            if observed {
                println!(
                    "  observed graph — discovery/quality/validation only; build/fix lanes disabled"
                );
            }
            Ok(())
        }
        Command::Intent { cmd } => intent::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Codefile { cmd } => codefile(cli.graph.as_deref(), cmd, cli.json),
        Command::Export { check } => export(cli.graph.as_deref(), check),
        Command::Import { file } => import(cli.graph.as_deref(), &file),
        Command::Sync => sync_cmd(cli.graph.as_deref()),
        Command::Status => status(cli.graph.as_deref(), cli.json),
        Command::Next { mode, all } => {
            if all {
                next_all(cli.graph.as_deref(), cli.json)
            } else {
                next_cmd(cli.graph.as_deref(), mode.as_deref(), cli.json)
            }
        }
        Command::Edge { cmd } => edge::dispatch(cli.graph.as_deref(), cmd),
        Command::Door { utterance } => door(cli.graph.as_deref(), &utterance),
        Command::Inbox { cmd } => inbox(cli.graph.as_deref(), cmd),
        Command::Task { cmd } => task(cli.graph.as_deref(), cmd),
        Command::Session => session(cli.graph.as_deref()),
        Command::Guide { role } => guide(role.as_deref()),
        Command::Find { query, limit } => find_cmd(cli.graph.as_deref(), &query, limit),
        Command::Detect => detect_cmd(cli.graph.as_deref()),
        Command::Schema => schema_cmd(),
        Command::Rule { cmd } => rule(cli.graph.as_deref(), cmd),
        Command::Validation { cmd } => validation(cli.graph.as_deref(), cmd),
        Command::Validate { intent, all } => validate_cmd(cli.graph.as_deref(), &intent, all),
        Command::Hypothesis { cmd } => hypothesis(cli.graph.as_deref(), cmd),
        Command::Surface { cmd } => surface(cli.graph.as_deref(), cmd),
        Command::Saga { cmd } => saga::dispatch(cli.graph.as_deref(), cmd),
        Command::Vocab { cmd } => vocab(cli.graph.as_deref(), cmd),
        Command::Layer { cmd } => layer(cli.graph.as_deref(), cmd),
        Command::Interface { cmd } => interface(cli.graph.as_deref(), cmd),
        Command::Smells => smells_cmd(cli.graph.as_deref(), cli.json),
        Command::Debt => debt_cmd(cli.graph.as_deref(), cli.json),
        Command::Finding { cmd } => finding(cli.graph.as_deref(), cmd, cli.json),
        Command::Doctor => doctor_cmd(cli.graph.as_deref(), cli.json),
        Command::Coverage => coverage_cmd(cli.graph.as_deref()),
        Command::Ignore { cmd } => ignore_cmd(cli.graph.as_deref(), cmd),
        Command::Whoami => whoami_cmd(cli.graph.as_deref()),
    }
}

/// Resolve the graph root: explicit `--graph`, else nearest ancestor with
/// `.loom/`, else error pointing at `loom init`.
fn resolve_root(graph: Option<&Path>) -> Result<PathBuf> {
    if let Some(g) = graph {
        return Ok(g.to_path_buf());
    }
    let cwd = std::env::current_dir()?;
    Store::find_root(&cwd).ok_or_else(|| {
        anyhow!(
            "no loom graph found from {} — run `loom init`",
            cwd.display()
        )
    })
}

fn open(graph: Option<&Path>) -> Result<Store> {
    let root = resolve_root(graph)?;
    Store::open(&root)
}

/// After a judgment (a verdict / mark), echo loom's recommended next move so the
/// loop never dead-ends at a terminal confirmation — the same self-teaching the
/// work-item `next_step` gives, but emitted at the point of action so a driver
/// is never stranded without re-running `loom next`.
pub(crate) fn print_next_move(store: &Store) -> Result<()> {
    let ladder = crate::maturity::ladder(store)?;
    println!("→ next: {}", ladder.next_command);
    Ok(())
}

fn codefile(graph: Option<&Path>, cmd: CodefileCmd, json: bool) -> Result<()> {
    match cmd {
        CodefileCmd::Add { path } => {
            let root = resolve_root(graph)?;
            let store = Store::open(&root)?;
            // Expand globs against the graph root; register each new file.
            let matched = crate::fsglob::expand(&root, &path)?;
            let targets: Vec<String> = if matched.is_empty() {
                // No glob match: treat as a literal path (may be a not-yet-existing file).
                vec![path.replace('\\', "/")]
            } else {
                matched
            };
            let existing: std::collections::HashSet<String> =
                store.codefiles()?.into_iter().map(|n| n.name).collect();
            let mut added = 0usize;
            for t in &targets {
                if existing.contains(t) {
                    continue;
                }
                store.add_node(NodeType::CodeFile, t, "", "", serde_json::json!({}))?;
                added += 1;
            }
            // Remember the glob so `codefile rescan` can pick up files that
            // appear later (e.g. a new endpoint in a vendored upstream).
            if path.contains('*') || path.contains('?') {
                remember_glob(&store, &path)?;
            }
            println!(
                "registered {added} codefile(s) ({} matched, {} already present)",
                targets.len(),
                targets.len() - added
            );
            Ok(())
        }
        CodefileCmd::Rescan => codefile_rescan(graph),
        CodefileCmd::Remove { key } => {
            let store = open(graph)?;
            let n = store.resolve_node(&key, Some(NodeType::CodeFile))?;
            store.delete_node(&n.id)?;
            println!("removed codefile '{}' (and its groundings)", n.name);
            Ok(())
        }
        CodefileCmd::Show { key } => codefile_show(graph, &key, json),
        CodefileCmd::List { limit } => {
            let store = open(graph)?;
            let files = store.list_nodes(Some(NodeType::CodeFile), limit)?;
            if files.is_empty() {
                println!("no codefiles");
            }
            for n in &files {
                println!("{} [{}]", n.name, &n.id[..8]);
            }
            Ok(())
        }
    }
}

/// The globs registered via `codefile add`, stored in meta so `rescan` can
/// re-expand them later.
fn registered_globs(store: &Store) -> Result<Vec<String>> {
    Ok(store
        .get_meta("codefile_globs")?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}

fn remember_glob(store: &Store, pattern: &str) -> Result<()> {
    let mut globs = registered_globs(store)?;
    if !globs.iter().any(|g| g == pattern) {
        globs.push(pattern.to_string());
        store.set_meta("codefile_globs", &serde_json::to_string(&globs)?)?;
    }
    Ok(())
}

/// Re-expand every remembered glob and register files that have appeared since
/// — the "an upstream added a new endpoint" path. New files carry no prior hash,
/// so the following `loom sync` extracts them without falsely rippling anything.
fn codefile_rescan(graph: Option<&Path>) -> Result<()> {
    let root = resolve_root(graph)?;
    let store = Store::open(&root)?;
    let globs = registered_globs(&store)?;
    if globs.is_empty() {
        println!("no globs remembered — register files with `loom codefile add '<glob>'` first");
        return Ok(());
    }
    let existing: std::collections::HashSet<String> =
        store.codefiles()?.into_iter().map(|n| n.name).collect();
    let mut new_files: Vec<String> = Vec::new();
    for g in &globs {
        for t in crate::fsglob::expand(&root, g)? {
            if existing.contains(&t) || new_files.contains(&t) {
                continue;
            }
            store.add_node(NodeType::CodeFile, &t, "", "", serde_json::json!({}))?;
            new_files.push(t);
        }
    }
    println!(
        "rescanned {} glob(s): {} new file(s) registered",
        globs.len(),
        new_files.len()
    );
    for f in new_files.iter().take(10) {
        println!("    + {f}");
    }
    if !new_files.is_empty() {
        println!("  run `loom sync` to extract them");
    }
    Ok(())
}

/// Show a codefile's full graph context: structural facets, the intents that own
/// it (with locator + verdict), and the findings flagging it (with triage
/// state). This is the "what is this file and is it cohesive?" lookup — the
/// judgment input grep cannot give.
fn codefile_show(graph: Option<&Path>, key: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(key, Some(NodeType::CodeFile))?;
    let facet = |k: &str| -> Result<String> {
        Ok(store
            .get_facet(&n.id, TargetKind::Node, k)?
            .unwrap_or_default())
    };
    let (language, role, loc, symbols) = (
        facet("language")?,
        facet("role")?,
        facet("loc")?,
        facet("symbol_count")?,
    );

    // Owning intents: implements edges INTO this file, each with its locator and
    // recorded verdict.
    let mut owners = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Implements), None, Some(&n.id))? {
        let name = store
            .get_node(&e.from_id)?
            .map(|x| x.name)
            .unwrap_or_else(|| e.from_id.clone());
        let locator = store
            .get_facet(&e.id, TargetKind::Edge, "locator")?
            .unwrap_or_default();
        owners.push((name, locator, e.status.as_str().to_string(), e.evidence));
    }
    owners.sort_by(|a, b| a.0.cmp(&b.0));

    // Findings flagging this file, joined with their adjudication state.
    let flagged: std::collections::HashSet<String> = store
        .edges_with(Some(EdgeKind::Flags), None, Some(&n.id))?
        .into_iter()
        .map(|e| e.from_id)
        .collect();
    let findings: Vec<crate::signal::FindingView> = crate::signal::findings_view(&store)?
        .into_iter()
        .filter(|fv| flagged.contains(&fv.node.id))
        .collect();

    if json {
        let out = serde_json::json!({
            "name": n.name,
            "id": n.id,
            "language": language,
            "role": role,
            "loc": loc.parse::<u64>().ok(),
            "symbol_count": symbols.parse::<u64>().ok(),
            "owners": owners.iter().map(|(name, loc, verdict, ev)| serde_json::json!({
                "intent": name, "locator": loc, "verdict": verdict, "evidence": ev,
            })).collect::<Vec<_>>(),
            "findings": findings.iter().map(|fv| serde_json::json!({
                "state": fv.state, "stale": fv.stale, "title": fv.node.name, "reason": fv.reason,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("{} [{}]", n.name, &n.id[..8.min(n.id.len())]);
    let mut facets = Vec::new();
    if !language.is_empty() {
        facets.push(format!("language={language}"));
    }
    if !role.is_empty() {
        facets.push(format!("role={role}"));
    }
    if !loc.is_empty() {
        facets.push(format!("loc={loc}"));
    }
    if !symbols.is_empty() {
        facets.push(format!("symbols={symbols}"));
    }
    if !facets.is_empty() {
        println!("  {}", facets.join("  "));
    }
    println!("  owned by {} intent(s):", owners.len());
    if owners.is_empty() {
        println!("    (unowned — no implements edge; coverage gap or non-behavioral)");
    }
    for (name, locator, verdict, ev) in &owners {
        let at = if locator.is_empty() {
            String::new()
        } else {
            format!(" @ {locator}")
        };
        let ev = if ev.is_empty() {
            String::new()
        } else {
            format!(" — {ev}")
        };
        println!("    ↳ {name}{at} [{verdict}]{ev}");
    }
    if !findings.is_empty() {
        println!("  findings:");
        for fv in &findings {
            let stale = if fv.stale { "·STALE" } else { "" };
            let why = if fv.state == "untriaged" {
                String::new()
            } else {
                format!(" — {}", fv.reason)
            };
            println!("    [{}{}] {}{}", fv.state, stale, fv.node.description, why);
        }
    }
    Ok(())
}

fn export(graph: Option<&Path>, check: bool) -> Result<()> {
    let store = open(graph)?;
    if check {
        if travel::export_is_fresh(&store)? {
            println!("export is fresh");
            Ok(())
        } else {
            bail!(
                "committed {} is stale — run `loom export`",
                crate::GRAPH_EXPORT
            );
        }
    } else {
        let path = travel::export_to_file(&store)?;
        println!("wrote {}", path.display());
        Ok(())
    }
}

fn import(graph: Option<&Path>, file: &Path) -> Result<()> {
    let root = if let Some(g) = graph {
        g.to_path_buf()
    } else {
        std::env::current_dir()?
    };
    // Phase 1: parse the export fully before touching the store.
    let export = travel::read_export(file)?;
    // Initialize a fresh store and restore.
    let mut store = Store::init(&root, None, false)?;
    store.restore(&export.into_snapshot())?;
    let id = store.identity()?;
    println!("imported graph '{}' from {}", id.name, file.display());
    Ok(())
}

fn sync_cmd(graph: Option<&Path>) -> Result<()> {
    let root = resolve_root(graph)?;
    let store = Store::open(&root)?;
    let report = crate::sync::run(&store, &root)?;
    println!(
        "sync: {} scanned, {} changed, {} edges staled, {} validations reset, {} findings",
        report.files_scanned,
        report.files_changed,
        report.edges_staled,
        report.validations_reset,
        report.findings
    );
    if report.contracts_reset > 0 {
        println!(
            "  integration: {} upstream surface(s) changed → {} of the reset contract(s) need re-verification   [loom next --mode validate]",
            report.surfaces_affected, report.contracts_reset
        );
    }
    if report.files_deleted > 0 {
        println!(
            "  {} registered file(s) gone since last sync — dependents re-flagged",
            report.files_deleted
        );
    }
    if !report.missing.is_empty() {
        println!(
            "  {} registered file(s) missing on disk:",
            report.missing.len()
        );
        for m in report.missing.iter().take(10) {
            println!("    {m}");
        }
    }
    Ok(())
}

fn status(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let id = store.identity()?;
    let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?.len();
    let files = store
        .list_nodes(Some(NodeType::CodeFile), usize::MAX)?
        .len();
    let edges = store.list_edges(None, usize::MAX)?.len();
    let ladder = crate::maturity::ladder(&store)?;
    let pulse = workitem::graph_state(&store)?;
    if json {
        let out = serde_json::json!({
            "graph": {
                "name": id.name,
                "graph_id": id.graph_id,
                "schema_version": id.schema_version,
                "observed": id.observed,
            },
            "counts": {
                "intents": intents,
                "codefiles": files,
                "edges": edges,
            },
            "maturity": ladder,
            "graph_state": pulse,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!(
        "graph: {} ({}){}",
        id.name,
        id.graph_id,
        if id.observed {
            "  [observed — monitoring; build/fix lanes off]"
        } else {
            ""
        }
    );
    println!("  intents: {intents}  codefiles: {files}  edges: {edges}");
    println!("  maturity:");
    for r in &ladder.rungs {
        let mark = match r.state {
            crate::maturity::RungState::Met => "✓",
            crate::maturity::RungState::Unmet => "·",
            crate::maturity::RungState::NotApplicable => "—",
        };
        println!("    {mark} {:<12} {}", r.name, r.detail);
    }
    println!(
        "  compass: phase={} → {}",
        ladder.phase, ladder.next_command
    );
    println!(
        "  queues: build={} fix={} analyze={} inbox={}  (advisory: findings={} untriaged={} stale_findings={} needed={})",
        pulse.planned,
        pulse.stale,
        pulse.uninspected,
        pulse.inbox,
        pulse.findings,
        pulse.untriaged,
        pulse.stale_findings,
        pulse.needed
    );
    Ok(())
}

fn next_all(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let ladder = crate::maturity::ladder(&store)?;
    let pulse = workitem::graph_state(&store)?;
    let modes = [
        ("fix", workitem::Mode::Fix),
        ("validate", workitem::Mode::Validate),
        ("build", workitem::Mode::Build),
        ("quality", workitem::Mode::Quality),
        ("prove", workitem::Mode::Prove),
        ("analyze", workitem::Mode::Analyze),
        ("triage", workitem::Mode::Triage),
    ];
    if json {
        let mut queues = serde_json::Map::new();
        for (name, m) in modes {
            let item = workitem::next(&store, Some(m))?;
            queues.insert(name.to_string(), serde_json::to_value(item)?);
        }
        let out = serde_json::json!({
            "compass": { "phase": ladder.phase, "next_command": ladder.next_command },
            "graph_state": pulse,
            "queues": queues,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "closeout — compass phase={} → {}",
            ladder.phase, ladder.next_command
        );
        for (name, m) in modes {
            match workitem::next(&store, Some(m))? {
                Some(w) => println!("  {name:<8} → {}", w.target.name),
                None => println!("  {name:<8} → (empty)"),
            }
        }
        println!(
            "  graph_state: planned={} stale={} uninspected={} findings={} untriaged={} stale_findings={} needed={} inbox={}",
            pulse.planned,
            pulse.stale,
            pulse.uninspected,
            pulse.findings,
            pulse.untriaged,
            pulse.stale_findings,
            pulse.needed,
            pulse.inbox
        );
    }
    Ok(())
}

fn require_lane(store: &Store, owner: crate::registry::OwnerRole) -> Result<()> {
    match store.agent() {
        crate::store::Agent::Solo => Ok(()),
        crate::store::Agent::Lane(r) if r == owner => Ok(()),
        crate::store::Agent::Lane(r) => bail!(
            "lane gate: agent '{}' may not write '{}'-owned facts",
            r.as_str(),
            owner.as_str()
        ),
    }
}

fn next_cmd(graph: Option<&Path>, mode: Option<&str>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let parsed = match mode {
        Some(m) => Some(workitem::Mode::parse(m).ok_or_else(|| anyhow!("unknown mode '{m}'"))?),
        None => None,
    };
    let item = workitem::next(&store, parsed)?;
    let pulse = workitem::graph_state(&store)?;
    if json {
        let out = workitem::NextOutput {
            work_item: item,
            graph_state: pulse,
        };
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        match &item {
            Some(w) => print_work_item(w),
            None => println!(
                "no work in this queue — see `loom status` for the next move, or `loom guide` to pick a lane"
            ),
        }
        println!(
            "  graph_state: planned={} stale={} uninspected={} findings={} untriaged={} stale_findings={} needed={} inbox={}",
            pulse.planned,
            pulse.stale,
            pulse.uninspected,
            pulse.findings,
            pulse.untriaged,
            pulse.stale_findings,
            pulse.needed,
            pulse.inbox
        );
    }
    Ok(())
}

fn print_work_item(item: &workitem::WorkItem) {
    let c = &item.prompt_contract;
    let short = &item.target.id[..8.min(item.target.id.len())];
    println!(
        "[{}] {} (effort {})",
        item.mode, item.target.name, item.effort
    );
    println!("  id: {short}");
    println!("  why: {}", item.reason);
    println!("  role: {}", c.role);
    println!("  mindset: {}", c.mindset);
    println!("  allowed:");
    for a in &c.allowed_actions {
        println!("    - {a}");
    }
    println!("  forbidden:");
    for f in &c.forbidden_actions {
        println!("    - {f}");
    }
    println!("  evidence: {}", c.required_evidence);
    println!("  write-back: {}", c.write_back);
    println!("  stop: {}", c.stop_condition);
    println!("  next_step: {}", item.next_step);
}

fn verdict_status(verdict: &str) -> Result<InspectionStatus> {
    match verdict {
        "ground" => Ok(InspectionStatus::Passing),
        "issue" => Ok(InspectionStatus::Failing),
        "independent" => Ok(InspectionStatus::Independent),
        other => bail!("unknown verdict '{other}' (use ground|issue|independent)"),
    }
}

fn door(graph: Option<&Path>, utterance: &str) -> Result<()> {
    let store = open(graph)?;
    let item = store.add_node(
        NodeType::InboxItem,
        &truncate(utterance, 60),
        utterance,
        "new",
        serde_json::json!({ "source": "human" }),
    )?;
    println!("captured inbox item [{}]", &item.id[..8]);
    println!("  normalize it, then route via loom intent/edge/rule, then `loom inbox mark`");
    Ok(())
}

fn inbox(graph: Option<&Path>, cmd: InboxCmd) -> Result<()> {
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
            if items.is_empty() {
                println!("inbox empty");
            }
            for n in &items {
                println!("{:<10} {} [{}]", n.status, n.name, &n.id[..8]);
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

fn task(graph: Option<&Path>, cmd: TaskCmd) -> Result<()> {
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
            println!("{} [{}]", t.name, t.id);
            println!("  status: {}", t.status);
            println!("  {}", t.body);
            Ok(())
        }
        TaskCmd::List { limit } => {
            let tasks = store.list_nodes(Some(NodeType::TaskRecord), limit)?;
            if tasks.is_empty() {
                println!("no tasks");
            }
            for n in &tasks {
                println!("{:<10} {} [{}]", n.status, n.name, &n.id[..8]);
            }
            Ok(())
        }
    }
}

fn session(graph: Option<&Path>) -> Result<()> {
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
    println!("what do you want from this session? offers:");
    if stale > 0 {
        println!(
            "  - repair {stale} failing/stale claim(s)   [loom next --mode fix]  (recommended)"
        );
    } else if planned > 0 {
        println!(
            "  - build {planned} unrealized intent(s)    [loom next --mode build]  (recommended)"
        );
    } else if uninspected > 0 {
        println!("  - inspect {uninspected} claim(s)           [loom next --mode analyze]  (recommended)");
    } else {
        let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?.len();
        let files = store
            .list_nodes(Some(NodeType::CodeFile), usize::MAX)?
            .len();
        if intents == 0 && files == 0 {
            println!("  - fresh graph — nothing mapped yet. Start here:");
            println!("      loom guide                  the driving loop + roles");
            println!("      loom guide --role monitor   watch an upstream you depend on");
            println!("      loom intent add --name <pillar>   seed what this codebase should do");
        } else {
            println!("  - graph is settled; map more, or just get to work");
        }
    }
    if inbox > 0 {
        println!("  - {inbox} inbox item(s) to triage          [loom inbox list]");
    }
    Ok(())
}

fn guide(role: Option<&str>) -> Result<()> {
    match role {
        None => {
            println!("loom — driving protocol (the loop):");
            println!("  loom sync     recompute the structural plane after code changes");
            println!("  loom next     the asserted residue: one work item + its prompt contract");
            println!("  loom next --mode triage   judge the programmatic flags (findings): justified | needed | blocked");
            println!("  loom status   maturity + the single next move");
            println!("  loom door     capture a raw utterance before routing it");
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
                    "Realize behavior in code; ground to file+symbol. Functions are locators, not intents.",
                    "edit code; loom edge implement; loom intent mark; loom sync",
                    "loom rule verdict passing; loom validation mark passed",
                ),
                "analyzer" => (
                    "Read both sides; hypothesis first; record exactly what the code shows. Also triages findings — record justified/needed/blocked with a reason.",
                    "loom edge explore ground|issue|independent; loom finding verdict <id> justified|needed|blocked --reason '…'",
                    "edit code; verdict from name similarity",
                ),
                "fixer" => (
                    "Repair the root cause; code moving is not behavior changing. Findings judged `needed` are queued work — consult `loom finding list --state needed`.",
                    "edit code; loom sync; re-ground; loom finding list --state needed",
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

/// The integration-monitoring playbook: how to watch an upstream you consume so
/// that when it changes, loom tells you which of your contracts went unproven.
/// Every command below is real and copy-paste ready (placeholders in <…>).
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

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}

/// Distinctive search terms from a natural-language query: lowercased, stripped
/// of surrounding punctuation, with stopwords and sub-3-char tokens removed
/// (they substring-match almost anything — "a" is inside "asserted"). Falls
/// back to any >= 2-char token when a query is all filler. Sorted + deduped for
/// stable scoring.
fn query_terms(query: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "a", "an", "and", "or", "of", "to", "in", "is", "it", "on", "by", "for", "with",
        "that", "this", "its", "be", "as", "at", "are", "was", "how", "what", "where", "why",
        "does", "do", "can",
    ];
    fn norm(s: &str) -> String {
        s.to_lowercase()
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_string()
    }
    let mut q: Vec<String> = query
        .split_whitespace()
        .map(norm)
        .filter(|t| t.len() >= 3 && !STOP.contains(&t.as_str()))
        .collect();
    q.sort();
    q.dedup();
    if q.is_empty() {
        q = query
            .split_whitespace()
            .map(norm)
            .filter(|t| t.len() >= 2)
            .collect();
        q.sort();
        q.dedup();
    }
    q
}

fn find_cmd(graph: Option<&Path>, query: &str, limit: usize) -> Result<()> {
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
    if hits.is_empty() {
        println!("no match for '{query}' — try `loom status` to see coverage, or it may not exist");
    }
    for (s, kind, name, id) in hits.into_iter().take(limit) {
        println!("{:<10} {} [{}] (score {s})", kind, name, &id[..8]);
        // An intent's worth is *where its behavior lives* and *whether that is
        // confirmed* — not merely that a node exists. Surface the grounding so
        // `find` answers "where + proven?", the edge a plain text search lacks.
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
    Ok(())
}

fn detect_cmd(graph: Option<&Path>) -> Result<()> {
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

fn schema_cmd() -> Result<()> {
    use crate::model::*;
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

fn rule(graph: Option<&Path>, cmd: RuleCmd) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        RuleCmd::Seed { pack } => {
            let n = crate::packs::seed(&store, &pack)?;
            println!("seeded pack '{pack}': {n} rule(s)");
            Ok(())
        }
        RuleCmd::Verdict {
            rule,
            intent,
            status,
            criterion,
            evidence,
            confidence,
        } => {
            let r = store.resolve_node(&rule, Some(NodeType::QualityRule))?;
            let i = store.resolve_node(&intent, Some(NodeType::Intent))?;
            let edge = store.ensure_edge(EdgeKind::Governs, &r.id, &i.id)?;
            let st = verdict_status_quality(&status)?;
            store.record_verdict(&edge.id, st, &criterion, &evidence, confidence, "llm")?;
            println!("rule '{}' {} on '{}'", r.name, st, i.name);
            print_next_move(&store)?;
            Ok(())
        }
        RuleCmd::List { limit } => {
            for n in store.list_nodes(Some(NodeType::QualityRule), limit)? {
                let cat = n
                    .body
                    .get("category")
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
                println!("{:<14} {} [{}]", cat, n.name, &n.id[..8]);
            }
            Ok(())
        }
        RuleCmd::Show { key } => {
            let n = store.resolve_node(&key, Some(NodeType::QualityRule))?;
            println!("{} [{}]", n.name, n.id);
            println!("  {}", n.description);
            if let Some(g) = n.body.get("inspection_guide").and_then(|v| v.as_str()) {
                println!("  inspection_guide: {g}");
            }
            if let Some(t) = n.body.get("evidence_template") {
                println!("  evidence_template: {t}");
            }
            Ok(())
        }
        RuleCmd::Add {
            name,
            category,
            description,
        } => {
            let r = store.add_node(
                NodeType::QualityRule,
                &name,
                &description,
                "",
                serde_json::json!({ "category": category }),
            )?;
            println!("added quality rule '{}' [{}]", r.name, &r.id[..8]);
            Ok(())
        }
        RuleCmd::Remove { key } => {
            let r = store.resolve_node(&key, Some(NodeType::QualityRule))?;
            store.delete_node(&r.id)?;
            println!("removed quality rule '{}'", r.name);
            Ok(())
        }
        RuleCmd::Ungovern { rule, intent } => {
            let r = store.resolve_node(&rule, Some(NodeType::QualityRule))?;
            let i = store.resolve_node(&intent, Some(NodeType::Intent))?;
            match store
                .edges_with(Some(EdgeKind::Governs), Some(&r.id), Some(&i.id))?
                .into_iter()
                .next()
            {
                Some(e) => {
                    store.delete_edge(&e.id)?;
                    println!("'{}' no longer governs '{}'", r.name, i.name);
                }
                None => bail!("'{}' does not govern '{}'", r.name, i.name),
            }
            Ok(())
        }
    }
}

fn verdict_status_quality(s: &str) -> Result<InspectionStatus> {
    match s {
        "passing" => Ok(InspectionStatus::Passing),
        "failing" => Ok(InspectionStatus::Failing),
        "independent" => Ok(InspectionStatus::Independent),
        other => bail!("unknown status '{other}' (use passing|failing|independent)"),
    }
}

fn validation(graph: Option<&Path>, cmd: ValidationCmd) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        ValidationCmd::Add {
            name,
            r#type,
            command,
            intent,
        } => {
            let i = store.resolve_node(&intent, Some(NodeType::Intent))?;
            let val = store.add_node(
                NodeType::Validation,
                &name,
                "",
                "not_run",
                serde_json::json!({ "type": r#type, "command": command }),
            )?;
            store.ensure_edge(EdgeKind::Validates, &val.id, &i.id)?;
            println!("added validation '{}' → '{}'", val.name, i.name);
            Ok(())
        }
        ValidationCmd::Mark {
            key,
            result,
            evidence,
            reason,
        } => {
            let val = store.resolve_node(&key, Some(NodeType::Validation))?;
            mark_validation(&store, &val.id, &result, &evidence, &reason)?;
            println!("validation '{}' → {result}", val.name);
            print_next_move(&store)?;
            Ok(())
        }
        ValidationCmd::Show { key } => {
            let val = store.resolve_node(&key, Some(NodeType::Validation))?;
            println!("{} [{}]", val.name, val.id);
            println!("  status: {}", val.status);
            println!("  {}", val.body);
            for e in store.edges_with(Some(EdgeKind::Validates), Some(&val.id), None)? {
                let i = store
                    .get_node(&e.to_id)?
                    .map(|n| n.name)
                    .unwrap_or_else(|| e.to_id.clone());
                println!("  validates: {i}");
            }
            Ok(())
        }
        ValidationCmd::Update {
            key,
            r#type,
            command,
        } => {
            let val = store.resolve_node(&key, Some(NodeType::Validation))?;
            let mut body = val.body.clone();
            if let Some(t) = &r#type {
                body["type"] = serde_json::json!(t);
            }
            if let Some(c) = &command {
                body["command"] = serde_json::json!(c);
            }
            store.set_node_body(&val.id, &body)?;
            println!("updated validation '{}'", val.name);
            Ok(())
        }
        ValidationCmd::Unlink { validation, intent } => {
            let v = store.resolve_node(&validation, Some(NodeType::Validation))?;
            let i = store.resolve_node(&intent, Some(NodeType::Intent))?;
            match store
                .edges_with(Some(EdgeKind::Validates), Some(&v.id), Some(&i.id))?
                .into_iter()
                .next()
            {
                Some(e) => {
                    store.delete_edge(&e.id)?;
                    println!("unlinked '{}' from '{}'", v.name, i.name);
                }
                None => bail!("'{}' does not validate '{}'", v.name, i.name),
            }
            Ok(())
        }
        ValidationCmd::Delete { key } => {
            let val = store.resolve_node(&key, Some(NodeType::Validation))?;
            store.delete_node(&val.id)?;
            println!("deleted validation '{}'", val.name);
            Ok(())
        }
        ValidationCmd::List { limit } => {
            for n in store.list_nodes(Some(NodeType::Validation), limit)? {
                println!("{:<10} {} [{}]", n.status, n.name, &n.id[..8]);
            }
            Ok(())
        }
    }
}

fn mark_validation(
    store: &Store,
    val_id: &str,
    result: &str,
    evidence: &str,
    reason: &str,
) -> Result<()> {
    let (node_status, edge_status, ev) = match result {
        "passed" => ("passed", InspectionStatus::Passing, evidence),
        "failed" => ("failed", InspectionStatus::Failing, evidence),
        "blocked" => ("blocked", InspectionStatus::Blocked, reason),
        other => bail!("unknown result '{other}' (use passed|failed|blocked)"),
    };
    // Record the edge verdicts FIRST: record_verdict enforces INV-6 (a
    // passing/failing verdict needs non-empty evidence) and will bail on, e.g.,
    // an empty `--evidence`. Setting the node status only after they all succeed
    // keeps the mark atomic — a rejected verdict never leaves the validation
    // showing `passed` while the command exits non-zero.
    for e in store.edges_with(Some(EdgeKind::Validates), Some(val_id), None)? {
        store.record_verdict(&e.id, edge_status, "proof", ev, 1.0, "llm")?;
    }
    store.set_node_status(val_id, node_status)?;
    Ok(())
}

fn validate_cmd(graph: Option<&Path>, intent: &str, all: bool) -> Result<()> {
    let store = open(graph)?;
    // collect validations to run
    let vals: Vec<_> = if all {
        store
            .list_nodes(Some(NodeType::Validation), usize::MAX)?
            .into_iter()
            .filter(|v| v.status == "not_run")
            .collect()
    } else {
        let i = store.resolve_node(intent, Some(NodeType::Intent))?;
        let mut out = Vec::new();
        for e in store.edges_with(Some(EdgeKind::Validates), None, Some(&i.id))? {
            if let Some(v) = store.get_node(&e.from_id)? {
                out.push(v);
            }
        }
        out
    };
    if vals.is_empty() {
        println!("no validations to run");
        return Ok(());
    }
    let root = store.root().to_path_buf();
    for v in &vals {
        let command = v.body.get("command").and_then(|c| c.as_str()).unwrap_or("");
        if command.is_empty() {
            println!(
                "skip '{}' (manual_check — use loom validation mark)",
                v.name
            );
            continue;
        }
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&root)
            .output();
        match out {
            Ok(o) if o.status.success() => {
                mark_validation(&store, &v.id, "passed", &format!("`{command}` exit 0"), "")?;
                println!("PASS {}", v.name);
            }
            Ok(o) => {
                let code = o.status.code().unwrap_or(-1);
                mark_validation(
                    &store,
                    &v.id,
                    "failed",
                    &format!("`{command}` exit {code}"),
                    "",
                )?;
                println!("FAIL {} (exit {code})", v.name);
            }
            Err(e) => {
                mark_validation(&store, &v.id, "blocked", "", &format!("could not run: {e}"))?;
                println!("BLOCKED {} ({e})", v.name);
            }
        }
    }
    Ok(())
}

fn hypothesis(graph: Option<&Path>, cmd: HypothesisCmd) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        HypothesisCmd::Add {
            name,
            claim,
            proposal,
            predicted_outcome,
            target,
        } => {
            let t = store.resolve_node(&target, Some(NodeType::Intent))?;
            let h = store.add_node(
                NodeType::Hypothesis,
                &name,
                &claim,
                "proposed",
                serde_json::json!({ "proposal": proposal, "predicted_outcome": predicted_outcome }),
            )?;
            store.ensure_edge(EdgeKind::Targets, &h.id, &t.id)?;
            println!("hypothesis '{}' targets '{}'", h.name, t.name);
            Ok(())
        }
        HypothesisCmd::Prove {
            key,
            verdict,
            evidence,
        } => {
            let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
            let status = match verdict.as_str() {
                "supported" => "supported",
                "refuted" => "refuted",
                other => bail!("unknown verdict '{other}' (use supported|refuted)"),
            };
            store.set_node_status(&h.id, status)?;
            store.add_note(&h.id, "decision", &format!("{status}: {evidence}"))?;
            println!("hypothesis '{}' {status}", h.name);
            Ok(())
        }
        HypothesisCmd::Adopt { key, spawned } => {
            let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
            if h.status != "supported" {
                bail!(
                    "only a supported hypothesis can be adopted (current: {})",
                    h.status
                );
            }
            store.set_node_status(&h.id, "adopted")?;
            let name = spawned.unwrap_or_else(|| format!("{} (adopted)", h.name));
            let intent = store.add_node(
                NodeType::Intent,
                &name,
                &h.description,
                "planned",
                serde_json::json!({}),
            )?;
            store.add_note(
                &h.id,
                "decision",
                &format!("adopted → spawned intent {}", intent.id),
            )?;
            println!("adopted '{}' → planned intent '{}'", h.name, intent.name);
            Ok(())
        }
        HypothesisCmd::Reject { key, reason } => {
            let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
            store.set_node_status(&h.id, "rejected")?;
            store.add_note(&h.id, "decision", &format!("rejected: {reason}"))?;
            println!("rejected '{}'", h.name);
            Ok(())
        }
        HypothesisCmd::Show { key } => {
            let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
            println!("{} [{}]", h.name, h.id);
            println!("  status: {}", h.status);
            if !h.description.is_empty() {
                println!("  claim: {}", h.description);
            }
            println!("  {}", h.body);
            for e in store.edges_with(Some(EdgeKind::Targets), Some(&h.id), None)? {
                let t = store
                    .get_node(&e.to_id)?
                    .map(|n| n.name)
                    .unwrap_or_else(|| e.to_id.clone());
                println!("  targets: {t}");
            }
            Ok(())
        }
        HypothesisCmd::List { limit } => {
            for n in store.list_nodes(Some(NodeType::Hypothesis), limit)? {
                println!("{:<10} {} [{}]", n.status, n.name, &n.id[..8]);
            }
            Ok(())
        }
    }
}

fn surface(graph: Option<&Path>, cmd: SurfaceCmd) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        SurfaceCmd::Add {
            name,
            kind,
            identity,
            codefile,
        } => {
            let s = store.add_node(
                NodeType::InterfaceSurface,
                &name,
                "",
                "",
                serde_json::json!({ "kind": kind, "identity": identity }),
            )?;
            if let Some(cf) = codefile {
                let c = store.resolve_node(&cf, Some(NodeType::CodeFile))?;
                store.add_edge(EdgeKind::Exposes, &s.id, &c.id, TruthClass::Asserted)?;
            }
            println!("declared surface '{}' [{}]", s.name, &s.id[..8]);
            Ok(())
        }
        SurfaceCmd::Show { key } => {
            let n = store.resolve_node(&key, Some(NodeType::InterfaceSurface))?;
            println!("{} [{}]", n.name, n.id);
            println!("  {}", n.body);
            Ok(())
        }
        SurfaceCmd::Update {
            key,
            kind,
            identity,
            codefile,
        } => {
            let s = store.resolve_node(&key, Some(NodeType::InterfaceSurface))?;
            let mut body = s.body.clone();
            if let Some(k) = &kind {
                body["kind"] = serde_json::json!(k);
            }
            if let Some(id) = &identity {
                body["identity"] = serde_json::json!(id);
            }
            store.set_node_body(&s.id, &body)?;
            if let Some(cf) = codefile {
                let c = store.resolve_node(&cf, Some(NodeType::CodeFile))?;
                // re-bind: drop the old exposes edge(s) from this surface, add the new one.
                for e in store.edges_with(Some(EdgeKind::Exposes), Some(&s.id), None)? {
                    store.delete_edge(&e.id)?;
                }
                store.add_edge(EdgeKind::Exposes, &s.id, &c.id, TruthClass::Asserted)?;
            }
            println!("updated surface '{}'", s.name);
            Ok(())
        }
        SurfaceCmd::Delete { key } => {
            let n = store.resolve_node(&key, Some(NodeType::InterfaceSurface))?;
            store.delete_node(&n.id)?;
            println!("deleted surface '{}'", n.name);
            Ok(())
        }
        SurfaceCmd::List { limit } => {
            for n in store.list_nodes(Some(NodeType::InterfaceSurface), limit)? {
                println!("{} [{}]", n.name, &n.id[..8]);
            }
            Ok(())
        }
    }
}

fn vocab(graph: Option<&Path>, cmd: VocabCmd) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        VocabCmd::Add { term, why } => {
            store.add_vocab_term(&term, &why)?;
            println!("registered vocab term '{term}'");
            Ok(())
        }
        VocabCmd::Remove { term } => {
            store.remove_vocab_term(&term)?;
            println!("removed vocab term '{term}' (and untagged any nodes carrying it)");
            Ok(())
        }
        VocabCmd::List => {
            for (term, why) in store.list_vocab()? {
                println!("{term}  — {why}");
            }
            Ok(())
        }
    }
}

fn layer(graph: Option<&Path>, cmd: LayerCmd) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        LayerCmd::Order { layers } => {
            if layers.is_empty() {
                bail!("provide the layer order, top first");
            }
            store.set_meta("layer_order", &serde_json::to_string(&layers)?)?;
            println!("layer order: {}", layers.join(" > "));
            Ok(())
        }
        LayerCmd::List => {
            match store.get_meta("layer_order")? {
                Some(v) => {
                    let layers: Vec<String> = serde_json::from_str(&v).unwrap_or_default();
                    println!("{}", layers.join(" > "));
                }
                None => println!("no layer order declared"),
            }
            Ok(())
        }
        LayerCmd::Clear => {
            store.set_meta("layer_order", "[]")?;
            println!("layer order cleared");
            Ok(())
        }
    }
}

fn interface(graph: Option<&Path>, cmd: InterfaceCmd) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        InterfaceCmd::Gaps => {
            let surfaces = store.list_nodes(Some(NodeType::InterfaceSurface), usize::MAX)?;
            let mut gaps = 0usize;
            for s in &surfaces {
                let exposes = store.edges_with(Some(EdgeKind::Exposes), Some(&s.id), None)?;
                let calls = store.edges_with(Some(EdgeKind::Calls), None, Some(&s.id))?;
                if exposes.is_empty() {
                    println!("surface '{}' exposes no codefile", s.name);
                    gaps += 1;
                }
                if calls.is_empty() {
                    println!("surface '{}' is never called by a validation/saga", s.name);
                    gaps += 1;
                }
            }
            println!(
                "{gaps} interface gap(s) across {} surface(s)",
                surfaces.len()
            );
            Ok(())
        }
    }
}

fn smells_cmd(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let smells = crate::signal::smells(&store)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&smells)?);
    } else if smells.is_empty() {
        println!("no open smells");
    } else {
        for s in &smells {
            println!("[{}] {}", s.kind, s.message);
            println!("    remedy: {}", s.remedy);
        }
        println!("{} open finding(s)", smells.len());
    }
    Ok(())
}

fn debt_cmd(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let debt = crate::signal::debt(&store)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&debt)?);
    } else if debt.is_empty() {
        println!("no debt signals");
    } else {
        for d in &debt {
            println!("[{}] {} (impact {})", d.kind, d.message, d.impact);
            println!("    confirm: {}", d.confirm);
        }
        println!("{} ranked signal(s) — advisory, never required", debt.len());
    }
    Ok(())
}

fn finding(graph: Option<&Path>, cmd: FindingCmd, json: bool) -> Result<()> {
    match cmd {
        FindingCmd::List { kind, state } => finding_list(graph, kind, state, json),
        FindingCmd::Verdict {
            id,
            verdict,
            reason,
        } => finding_verdict(graph, &id, &verdict, &reason),
    }
}

fn finding_list(
    graph: Option<&Path>,
    kind: Option<String>,
    state: Option<String>,
    json: bool,
) -> Result<()> {
    if let Some(s) = &state {
        validate_finding_filter_state(s)?;
    }
    let store = open(graph)?;
    let untriaged = crate::signal::untriaged_findings(&store)?.len();
    let stale_findings = crate::signal::stale_findings(&store)?.len();
    let mut findings = crate::signal::findings_view(&store)?;
    if let Some(k) = &kind {
        findings.retain(|fv| &fv.node.status == k);
    }
    if let Some(s) = &state {
        if s == "stale" {
            findings.retain(|fv| fv.stale);
        } else {
            findings.retain(|fv| &fv.state == s);
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else {
        if findings.is_empty() {
            println!("no findings");
        } else {
            for fv in &findings {
                let stale = if fv.stale { "·STALE" } else { "" };
                let id = &fv.node.id[..8.min(fv.node.id.len())];
                println!("[{}{}] {} {}", fv.state, stale, id, fv.node.name);
                if !fv.node.description.is_empty() {
                    println!("  ↳ {}", fv.node.description);
                }
                if fv.state != "untriaged" {
                    println!("  ↳ {}: {}", fv.state, fv.reason);
                }
            }
        }
        match &state {
            Some(s) => println!("{} finding(s) in state '{}'", findings.len(), s),
            None => println!(
                "{} finding(s); {} untriaged, {} stale — judge with `loom finding verdict <id> …`",
                findings.len(),
                untriaged,
                stale_findings
            ),
        }
    }
    Ok(())
}

fn finding_verdict(graph: Option<&Path>, id: &str, verdict: &str, reason: &str) -> Result<()> {
    validate_finding_verdict(verdict)?;
    if reason.trim().is_empty() {
        bail!("finding verdict requires --reason");
    }
    let store = open(graph)?;
    let finding = store.resolve_finding(id)?;
    store.record_finding_verdict(&finding.id, verdict, reason)?;
    println!("{verdict} '{}'", finding.name);
    print_next_move(&store)?;
    Ok(())
}

fn validate_finding_filter_state(state: &str) -> Result<()> {
    match state {
        "untriaged" | "stale" | "justified" | "needed" | "blocked" => Ok(()),
        other => {
            bail!("unknown finding state '{other}' (use untriaged|stale|justified|needed|blocked)")
        }
    }
}

fn validate_finding_verdict(verdict: &str) -> Result<()> {
    match verdict {
        "justified" | "needed" | "blocked" => Ok(()),
        other => bail!("unknown verdict '{other}' (use justified|needed|blocked)"),
    }
}

fn doctor_cmd(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let issues = crate::signal::doctor(&store)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&issues)?);
    } else {
        for i in &issues {
            println!("[{}] {}", i.kind, i.message);
        }
    }
    if issues.is_empty() {
        if !json {
            println!("doctor: clean");
        }
        Ok(())
    } else {
        bail!("doctor found {} integrity issue(s)", issues.len())
    }
}

fn coverage_cmd(graph: Option<&Path>) -> Result<()> {
    let store = open(graph)?;
    let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?;
    let active: Vec<_> = intents
        .iter()
        .filter(|n| n.status != "deprecated")
        .collect();
    let implemented: Vec<_> = active
        .iter()
        .filter(|n| n.status == "implemented")
        .collect();
    let parents: std::collections::HashSet<String> = store
        .list_edges(Some(EdgeKind::Hierarchy), usize::MAX)?
        .into_iter()
        .map(|e| e.from_id)
        .collect();
    let mut ungrounded = Vec::new();
    for n in &implemented {
        if parents.contains(&n.id) {
            continue; // roll-up parent — realized via children
        }
        if store
            .edges_with(Some(EdgeKind::Implements), Some(&n.id), None)?
            .is_empty()
        {
            ungrounded.push(n.name.clone());
        }
    }
    let codefiles = store.codefiles()?;
    let mut owned = 0usize;
    let mut unowned = Vec::new();
    for cf in &codefiles {
        if store
            .edges_with(Some(EdgeKind::Implements), None, Some(&cf.id))?
            .is_empty()
        {
            unowned.push(cf.name.clone());
        } else {
            owned += 1;
        }
    }
    println!("coverage (vertical spine):");
    println!(
        "  intents: {} active ({} implemented, {} planned/needs_change)",
        active.len(),
        implemented.len(),
        active.len() - implemented.len()
    );
    println!(
        "  grounding: {} implemented, {} ungrounded",
        implemented.len() - ungrounded.len(),
        ungrounded.len()
    );
    for u in ungrounded.iter().take(10) {
        println!("    ungrounded: {u}");
    }
    println!(
        "  codefiles: {} registered, {owned} owned, {} unowned",
        codefiles.len(),
        unowned.len()
    );
    for u in unowned.iter().take(10) {
        println!("    unowned: {u}");
    }
    Ok(())
}

fn ignore_cmd(graph: Option<&Path>, cmd: IgnoreCmd) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        IgnoreCmd::Add { glob, reason } => {
            let mut list: Vec<serde_json::Value> = store
                .get_meta("ignores")?
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default();
            list.push(serde_json::json!({ "glob": glob, "reason": reason }));
            store.set_meta("ignores", &serde_json::to_string(&list)?)?;
            println!("ignoring '{glob}' ({reason})");
            Ok(())
        }
        IgnoreCmd::Remove { glob } => {
            let mut list: Vec<serde_json::Value> = store
                .get_meta("ignores")?
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default();
            let before = list.len();
            list.retain(|r| r.get("glob").and_then(|g| g.as_str()) != Some(glob.as_str()));
            if list.len() == before {
                bail!("no ignore rule for glob '{glob}'");
            }
            store.set_meta("ignores", &serde_json::to_string(&list)?)?;
            println!("removed ignore rule '{glob}'");
            Ok(())
        }
        IgnoreCmd::List => {
            let list: Vec<serde_json::Value> = store
                .get_meta("ignores")?
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default();
            if list.is_empty() {
                println!("no ignore rules");
            }
            for r in &list {
                println!(
                    "{}  — {}",
                    r.get("glob").and_then(|g| g.as_str()).unwrap_or(""),
                    r.get("reason").and_then(|g| g.as_str()).unwrap_or("")
                );
            }
            Ok(())
        }
    }
}

fn whoami_cmd(graph: Option<&Path>) -> Result<()> {
    let store = open(graph)?;
    match store.agent() {
        crate::store::Agent::Solo => {
            println!("agent: solo (LOOM_AGENT unset/llm) — drives every lane; lane gate OFF");
        }
        crate::store::Agent::Lane(r) => {
            println!(
                "agent: {} — lane gate ON (may only write {}-owned facts)",
                r.as_str(),
                r.as_str()
            );
        }
    }
    if store.identity()?.observed {
        println!(
            "graph: observed — maps code you do not own; discovery/quality/validation only (build/fix disabled)"
        );
    } else {
        println!("graph: owned — you may build and fix here");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::query_terms;

    #[test]
    fn query_terms_drop_stopwords_and_short_tokens() {
        // filler and sub-3-char tokens are removed; distinctive terms remain
        let t = query_terms("how does loom decide what to work on");
        assert!(t.contains(&"loom".to_string()));
        assert!(t.contains(&"decide".to_string()));
        assert!(t.contains(&"work".to_string()));
        assert!(!t.iter().any(|w| w == "how" || w == "to" || w == "on"));
        // punctuation stripped, results deduped + sorted
        assert_eq!(query_terms("file, file; FILE!"), vec!["file".to_string()]);
        // an all-filler query falls back to >= 2-char tokens, not nothing
        assert!(!query_terms("is it on").is_empty());
    }
}
