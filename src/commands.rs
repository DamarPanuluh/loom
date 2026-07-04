//! Command handlers (ring 1 subset).
//!
//! Plane: orchestration. Resolves the target graph, calls the store, renders
//! output. No SQL here — that lives in `crate::store`.

use crate::cli::{
    Cli, CodefileCmd, Command, FindingCmd, HypothesisCmd, IgnoreCmd, InboxCmd, LayerCmd, NoteCmd,
    RuleCmd, SurfaceCmd, TaskCmd, ValidationCmd, VocabCmd,
};
use crate::model::{EdgeKind, InspectionStatus, Node, NodeType, TargetKind, TruthClass};
use crate::store::Store;
use crate::Result;
use crate::{travel, workitem};
use anyhow::{anyhow, bail};
use std::path::{Path, PathBuf};

mod apply_cmd;
mod codefile_cmd;
mod diagnostics_cmd;
mod domain_cmd;
mod edge;
mod intent;
mod journey;
mod misc_cmd;
mod proof_cmd;
mod proposal_cmd;
mod pulse;
mod status_cmd;
pub use intent::looks_like_symbol;
pub(crate) use status_cmd::require_lane;

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
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "initialized": true,
                        "name": id.name,
                        "graph_id": id.graph_id,
                        "path": root.join(crate::LOOM_DIR),
                        "observed": observed,
                    }))?
                );
            } else {
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
            }
            Ok(())
        }
        Command::Intent { cmd } => intent::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Codefile { cmd } => codefile_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Export { check } => status_cmd::export(cli.graph.as_deref(), check, cli.json),
        Command::Import { file } => status_cmd::import(cli.graph.as_deref(), &file, cli.json),
        Command::Apply { file } => apply_cmd::apply(cli.graph.as_deref(), &file, cli.json),
        Command::Sync => status_cmd::sync_cmd(cli.graph.as_deref(), cli.json),
        Command::Status => status_cmd::status(cli.graph.as_deref(), cli.json),
        Command::Next { mode, all } => {
            if all {
                status_cmd::next_all(cli.graph.as_deref(), cli.json)
            } else {
                status_cmd::next_cmd(
                    cli.graph.as_deref(),
                    mode.map(crate::cli::ModeArg::as_str),
                    cli.json,
                )
            }
        }
        Command::Edge { cmd } => edge::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Door { utterance } => misc_cmd::door(cli.graph.as_deref(), &utterance, cli.json),
        Command::Inbox { cmd } => misc_cmd::inbox(cli.graph.as_deref(), cmd, cli.json),
        Command::Task { cmd } => misc_cmd::task(cli.graph.as_deref(), cmd, cli.json),
        Command::Note { cmd } => misc_cmd::note(cli.graph.as_deref(), cmd, cli.json),
        Command::Session => misc_cmd::session(cli.graph.as_deref(), cli.json),
        Command::Guide { role } => misc_cmd::guide(role.map(crate::cli::RoleArg::as_str), cli.json),
        Command::Find { query, limit } => {
            misc_cmd::find_cmd(cli.graph.as_deref(), &query, limit, cli.json)
        }
        Command::Detect => misc_cmd::detect_cmd(cli.graph.as_deref(), cli.json),
        Command::Schema => misc_cmd::schema_cmd(cli.json),
        Command::Rule { cmd } => proof_cmd::rule(cli.graph.as_deref(), cmd, cli.json),
        Command::Validation { cmd } => proof_cmd::validation(cli.graph.as_deref(), cmd, cli.json),
        Command::Hypothesis { cmd } => domain_cmd::hypothesis(cli.graph.as_deref(), cmd, cli.json),
        Command::Surface { cmd } => domain_cmd::surface(cli.graph.as_deref(), cmd, cli.json),
        Command::Vocab { cmd } => domain_cmd::vocab(cli.graph.as_deref(), cmd, cli.json),
        Command::Layer { cmd } => layer(cli.graph.as_deref(), cmd, cli.json),
        Command::Smells => diagnostics_cmd::smells_cmd(cli.graph.as_deref(), cli.json),
        Command::Debt => diagnostics_cmd::debt_cmd(cli.graph.as_deref(), cli.json),
        Command::Finding { cmd } => diagnostics_cmd::finding(cli.graph.as_deref(), cmd, cli.json),
        Command::Doctor => diagnostics_cmd::doctor_cmd(cli.graph.as_deref(), cli.json),
        Command::Coverage => diagnostics_cmd::coverage_cmd(cli.graph.as_deref(), cli.json),
        Command::Ignore { cmd } => diagnostics_cmd::ignore_cmd(cli.graph.as_deref(), cmd, cli.json),
        Command::Whoami => diagnostics_cmd::whoami_cmd(cli.graph.as_deref(), cli.json),
        Command::Proposal { cmd } => proposal_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Journey { cmd } => journey::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Scan { cmd } => diagnostics_cmd::scan_cmd(cli.graph.as_deref(), cmd, cli.json),
        Command::Calibrate { write } => {
            diagnostics_cmd::calibrate_cmd(cli.graph.as_deref(), write, cli.json)
        }
        Command::Completeness { key } => {
            diagnostics_cmd::completeness_cmd(cli.graph.as_deref(), key.as_deref(), cli.json)
        }
    }
}

/// Resolve the graph root: explicit `--graph`, else nearest ancestor with
/// `.loom/`, else error pointing at `loom init`.
pub(crate) fn resolve_root(graph: Option<&Path>) -> Result<PathBuf> {
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

pub(crate) fn open(graph: Option<&Path>) -> Result<Store> {
    let root = resolve_root(graph)?;
    Store::open(&root)
}

/// Open the target graph read-only (shared lock, `query_only`). Read commands
/// use this so several agents can query one graph concurrently and never block
/// each other; only a writer holding the boundary makes them wait.
pub(crate) fn open_read(graph: Option<&Path>) -> Result<Store> {
    let root = resolve_root(graph)?;
    Store::open_read(&root)
}

pub(crate) fn node_json(n: &Node) -> serde_json::Value {
    serde_json::json!({
        "id": n.id,
        "type": n.node_type.as_str(),
        "name": n.name,
        "description": n.description,
        "status": n.status,
        "truth_class": n.truth_class.as_str(),
        "body": n.body,
        "created_at": n.created_at,
        "updated_at": n.updated_at,
    })
}

/// Registered CodeFiles with no owning `implements` edge and not matched by a
/// coverage-exclusion glob (`loom ignore`). This is the single definition of
/// the coverage gap: the diagnostic, the `realized` maturity gate, and the
/// `coverage` work queue all read it, so they can never disagree. Sorted by
/// name for a stable next-item.
pub(crate) fn unowned_codefiles(store: &Store) -> Result<Vec<Node>> {
    let ignore = crate::fsglob::matcher(store.ignore_globs()?)?;
    let mut unowned = Vec::new();
    for cf in store.codefiles()? {
        if ignore.is_match(&cf.name) {
            continue; // deliberately outside the tracked surface
        }
        if store.realizing_implementers(&cf.id)?.is_empty() {
            unowned.push(cf);
        }
    }
    unowned.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(unowned)
}

/// `(registered, owned, unowned_names)` after coverage exclusions. Ignored
/// files are dropped from every bucket, so `registered == owned + unowned`.
pub(crate) fn code_ownership_summary(store: &Store) -> Result<(usize, usize, Vec<String>)> {
    let ignore = crate::fsglob::matcher(store.ignore_globs()?)?;
    let mut owned = 0usize;
    let mut unowned = Vec::new();
    for cf in store.codefiles()? {
        if ignore.is_match(&cf.name) {
            continue;
        }
        if store.realizing_implementers(&cf.id)?.is_empty() {
            unowned.push(cf.name);
        } else {
            owned += 1;
        }
    }
    unowned.sort();
    Ok((owned + unowned.len(), owned, unowned))
}

pub(crate) fn verdict_status(verdict: &str) -> Result<InspectionStatus> {
    match verdict {
        "ground" => Ok(InspectionStatus::Passing),
        "issue" => Ok(InspectionStatus::Failing),
        "independent" => Ok(InspectionStatus::Independent),
        other => bail!("unknown verdict '{other}' (use ground|issue|independent)"),
    }
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
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
pub(crate) fn query_terms(query: &str) -> Vec<String> {
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

fn layer(graph: Option<&Path>, cmd: LayerCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        LayerCmd::Order { layers } => {
            if layers.is_empty() {
                bail!("provide the layer order, top first");
            }
            store.set_meta("layer_order", &serde_json::to_string(&layers)?)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "layer_order": layers }),
                "loom sync",
                format!("layer order: {}", layers.join(" > ")),
            )
        }
        LayerCmd::List => {
            let state = domain_cmd::layer_detector_state(&store)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&state)?);
            } else if let Some(order) = state.get("order").and_then(|v| v.as_array()) {
                if order.is_empty() {
                    println!("no layer order declared");
                } else {
                    let labels: Vec<&str> = order.iter().filter_map(|v| v.as_str()).collect();
                    println!("{}", labels.join(" > "));
                }
            }
            Ok(())
        }
        LayerCmd::Clear => {
            store.set_meta("layer_order", "[]")?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "layer_order": [] }),
                "loom status",
                "layer order cleared".to_string(),
            )
        }
    }
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
