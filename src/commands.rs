//! Command handlers (ring 1 subset).
//!
//! Plane: orchestration. Resolves the target graph, calls the store, renders
//! output. No SQL here — that lives in `crate::store`.

use crate::cli::{
    Cli, CodefileCmd, Command, DebtCmd, FindingCmd, HypothesisCmd, IgnoreCmd, InboxCmd, LayerCmd,
    NoteCmd, QuestionCmd, RuleCmd, SurfaceCmd, TaskCmd, ValidationCmd, VocabCmd,
};
use crate::model::{EdgeKind, InspectionStatus, Node, NodeType, TargetKind, TruthClass};
use crate::store::Store;
use crate::Result;
use crate::{travel, workitem};
use anyhow::{anyhow, bail, Context};
use serde::de::DeserializeOwned;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

mod apply_cmd;
mod bootstrap_cmd;
mod capture_cmd;
mod codefile_cmd;
mod context_cmd;
mod diagnostics_cmd;
mod discover_cmd;
mod domain_cmd;
mod drive_cmd;
mod edge;
mod graph_cmd;
mod hook_cmd;
mod intent;
mod journey;
mod misc_cmd;
mod orient_cmd;
mod proof_cmd;
mod proposal_cmd;
mod pulse;
mod status_cmd;
mod wiki;
pub use crate::grammar::looks_like_symbol;
pub(crate) use status_cmd::require_lane;
// The in-band surface: `crate::mcp` calls exactly these, so an MCP tool and its
// CLI twin can never diverge.
pub(crate) use context_cmd::served_context;
// The honest way to make a proof true: let loom run it. Public so callers other
// than the CLI — absorb, fixtures — take the same path rather than a seam.
pub use proof_cmd::{observe_validation, prove_intent};
pub(crate) use apply_cmd::apply_value;
pub(crate) use proof_cmd::observe_run;
pub(crate) use status_cmd::{next_output, status_value};

/// Dispatch a parsed CLI invocation.
pub fn run(cli: Cli) -> Result<()> {
    // Bare `loom` (no subcommand) lands a confused human on the orientation.
    let Some(command) = cli.command else {
        return misc_cmd::welcome(cli.graph.as_deref(), cli.json);
    };
    match command {
        Command::Welcome => misc_cmd::welcome(cli.graph.as_deref(), cli.json),
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
        Command::Import {
            file,
            repair_orphans,
        } => status_cmd::import(cli.graph.as_deref(), &file, repair_orphans, cli.json),
        Command::Apply { file } => apply_cmd::apply(cli.graph.as_deref(), &file, cli.json),
        Command::Sync { quiet } => status_cmd::sync_cmd(cli.graph.as_deref(), cli.json, quiet),
        Command::Status => status_cmd::status(cli.graph.as_deref(), cli.json),
        Command::Mode { mode } => status_cmd::mode_cmd(
            cli.graph.as_deref(),
            mode.map(crate::cli::GraphModeArg::is_observed),
            cli.json,
        ),
        Command::Next { mode, all } => match (mode, all) {
            // `--mode <m> --all`: the full roster of that one queue (depth view).
            (Some(m), true) => status_cmd::queue_list(cli.graph.as_deref(), m.as_str(), cli.json),
            // `--all` alone: the closeout — top item of every queue.
            (_, true) => status_cmd::next_all(cli.graph.as_deref(), cli.json),
            // Default: the single next work item (full packet).
            (m, false) => status_cmd::next_cmd(
                cli.graph.as_deref(),
                m.map(crate::cli::ModeArg::as_str),
                cli.json,
            ),
        },
        Command::Edge { cmd } => edge::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Door { utterance } => misc_cmd::door(cli.graph.as_deref(), &utterance, cli.json),
        Command::Inbox { cmd } => misc_cmd::inbox(cli.graph.as_deref(), cmd, cli.json),
        Command::Question { cmd } => misc_cmd::question(cli.graph.as_deref(), cmd, cli.json),
        Command::Task { cmd } => misc_cmd::task(cli.graph.as_deref(), cmd, cli.json),
        Command::Note { cmd } => misc_cmd::note(cli.graph.as_deref(), cmd, cli.json),
        Command::Session => misc_cmd::session(cli.graph.as_deref(), cli.json),
        Command::Guide { role } => misc_cmd::guide(role.map(crate::cli::RoleArg::as_str), cli.json),
        Command::Find {
            query,
            limit,
            exact,
            tag,
            where_facets,
        } => misc_cmd::find_cmd(
            cli.graph.as_deref(),
            &query,
            limit,
            exact,
            tag.as_deref(),
            &where_facets,
            cli.json,
        ),
        Command::Explain { intent } => {
            misc_cmd::explain_cmd(cli.graph.as_deref(), &intent, cli.json)
        }
        Command::Context { target } => {
            context_cmd::context_cmd(cli.graph.as_deref(), &target, cli.json)
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
        Command::Debt { cmd } => diagnostics_cmd::debt(cli.graph.as_deref(), cmd, cli.json),
        Command::Finding { cmd } => diagnostics_cmd::finding(cli.graph.as_deref(), cmd, cli.json),
        Command::Doctor => diagnostics_cmd::doctor_cmd(cli.graph.as_deref(), cli.json),
        Command::Coverage => diagnostics_cmd::coverage_cmd(cli.graph.as_deref(), cli.json),
        Command::Ignore { cmd } => diagnostics_cmd::ignore_cmd(cli.graph.as_deref(), cmd, cli.json),
        Command::Whoami => diagnostics_cmd::whoami_cmd(cli.graph.as_deref(), cli.json),
        Command::Proposal { cmd } => proposal_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Journey { cmd } => journey::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Drive { cmd } => drive_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Hook { cmd } => hook_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Impact { target, depth } => {
            diagnostics_cmd::impact_cmd(cli.graph.as_deref(), &target, depth, cli.json)
        }
        Command::Observe {
            target,
            timeout,
            command,
        } => proof_cmd::observe_cmd(
            cli.graph.as_deref(),
            target.as_deref(),
            timeout,
            &command,
            cli.json,
        ),
        Command::Absorb { confirm } => {
            diagnostics_cmd::absorb_cmd(cli.graph.as_deref(), confirm, cli.json)
        }
        Command::Audit => diagnostics_cmd::audit_cmd(cli.graph.as_deref(), cli.json),
        Command::Deepen { limit } => {
            diagnostics_cmd::deepen_cmd(cli.graph.as_deref(), limit, cli.json)
        }
        Command::Mcp { cmd } => match cmd {
            crate::cli::McpCmd::Serve => crate::mcp::serve(cli.graph.as_deref()),
        },
        Command::Wiki { cmd } => wiki::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Scan { cmd } => diagnostics_cmd::scan_cmd(cli.graph.as_deref(), cmd, cli.json),
        Command::Calibrate { write } => {
            diagnostics_cmd::calibrate_cmd(cli.graph.as_deref(), write, cli.json)
        }
        Command::Threshold { cmd } => {
            diagnostics_cmd::threshold_cmd(cli.graph.as_deref(), cmd, cli.json)
        }
        Command::Policy { cmd } => diagnostics_cmd::policy_cmd(cli.graph.as_deref(), cmd, cli.json),
        Command::Completeness { key } => {
            diagnostics_cmd::completeness_cmd(cli.graph.as_deref(), key.as_deref(), cli.json)
        }
        Command::Graph { cmd } => graph_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Bootstrap { cmd } => bootstrap_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json),
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

/// Is a person operating this CLI right now? Silent — no prompt, no failure.
///
/// Three conditions, and all three are needed. A tty alone is not enough (an
/// agent can be given one); a `Solo` agent alone is not enough (an unset
/// `LOOM_AGENT` in automation reads as Solo, which is the failure documented in
/// finding 62b197cc); and `LOOM_NON_INTERACTIVE` lets a wrapper say plainly
/// that nobody is watching.
///
/// This is what makes `loom intent add` at a terminal an utterance rather than
/// a form to fill in: a person typing the behavior IS the ratification, so
/// there is nothing more to ask them. An agent gets no such path.
pub(crate) fn human_present() -> bool {
    // A test seam, and only in a debug build. Integration tests spawn the CLI
    // as a separate process, so a thread-local cannot reach it and an
    // environment variable is the only mechanism that crosses the boundary.
    // Compiled out of release entirely: there is no shipped path by which an
    // agent can declare itself a person.
    #[cfg(debug_assertions)]
    if let Some(v) = std::env::var_os("LOOM_PRESENCE_PROBE") {
        return v == "human";
    }
    io::stdin().is_terminal()
        && matches!(crate::store::Agent::from_env(), Ok(crate::store::Agent::Solo))
        && std::env::var_os("LOOM_NON_INTERACTIVE").is_none()
}

/// Demand a typed confirmation before a ratifying write. Retained for the
/// explicit `loom intent ratify`, where the human is deliberately being asked.
pub(crate) fn require_challenge(subject: &str) -> Result<&'static str> {
    if !io::stdin().is_terminal() {
        bail!(
            "INV-8 / finding 62b197cc: non-interactive ratification is indistinguishable from an LLM"
        );
    }
    print!("Human presence required. Type '{subject}' to confirm: ");
    io::stdout().flush()?;
    let mut typed = String::new();
    io::stdin().read_line(&mut typed)?;
    if typed.trim() != subject {
        bail!("confirmation did not match '{subject}'; ratification was not written");
    }
    Ok("tty+challenge")
}

/// Open the target graph read-only (shared lock, `query_only`). Read commands
/// use this so several agents can query one graph concurrently and never block
/// each other; only a writer holding the boundary makes them wait.
pub(crate) fn open_read(graph: Option<&Path>) -> Result<Store> {
    let root = resolve_root(graph)?;
    Store::open_read(&root)
}

/// Read typed JSON configuration from meta. Absence means the type's default;
/// malformed persisted state is corruption and must be surfaced to the caller.
pub(crate) fn read_json_meta<T>(store: &Store, key: &str) -> Result<T>
where
    T: DeserializeOwned + Default,
{
    let Some(raw) = store.get_meta(key)? else {
        return Ok(T::default());
    };
    serde_json::from_str(&raw).with_context(|| format!("parsing meta '{key}'"))
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

/// Whether a CodeFile is registered as observed: monitored upstream code that
/// stays in the sync/surface/contract plane but carries no ownership, coverage,
/// or build obligations. The per-file counterpart of the graph-level observed
/// mode. Asserted at registration (`codefile add --observed`), never touched by
/// sync — derivers write facets, not the body.
pub(crate) fn codefile_observed(n: &Node) -> bool {
    n.body
        .get("observed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Registered CodeFiles with no owning `implements` edge, not matched by a
/// coverage-exclusion glob (`loom ignore`), and not registered as observed.
/// This is the single definition of the coverage gap: the diagnostic, the
/// `realized` maturity gate, and the `coverage` work queue all read it, so they
/// can never disagree. Sorted by name for a stable next-item.
pub fn unowned_names(store: &Store) -> Result<Vec<String>> {
    Ok(unowned_codefiles(store)?
        .into_iter()
        .map(|n| n.name)
        .collect())
}

pub(crate) fn unowned_codefiles(store: &Store) -> Result<Vec<Node>> {
    let ignore = crate::fsglob::matcher(store.ignore_globs()?)?;
    let mut unowned = Vec::new();
    for cf in store.codefiles()? {
        if ignore.is_match(&cf.name) {
            continue; // deliberately outside the tracked surface
        }
        if codefile_observed(&cf) {
            continue; // monitored upstream — no ownership obligation
        }
        // A TEST file is never realized by a behavior — it verifies one, and
        // demanding a realizing owner for it would mean `tests/` could only be
        // registered by permanently reddening coverage. That is exactly why
        // 22.8k lines of this repo's evidence backbone stayed outside the graph
        // while coverage reported 67/67 owned.
        if crate::extract::Role::detect(&cf.name) == crate::extract::Role::Test {
            let mut verified = false;
            for e in store.edges_with(Some(crate::model::EdgeKind::Implements), None, Some(&cf.id))?
            {
                if !store.edge_superseded(&e.id)?
                    && store.grounding_role(&e.id)? == crate::model::GroundingRole::Verifies
                {
                    verified = true;
                    break;
                }
            }
            if !verified {
                unowned.push(cf);
            }
            continue;
        }
        if store.realizing_implementers(&cf.id)?.is_empty() {
            unowned.push(cf);
        }
    }
    unowned.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(unowned)
}

/// `(registered, owned, unowned_names, observed)` after coverage exclusions.
/// Ignored files are dropped from every bucket and observed files count only in
/// the `observed` bucket, so `registered == owned + unowned`.
pub(crate) fn code_ownership_summary(store: &Store) -> Result<(usize, usize, Vec<String>, usize)> {
    let ignore = crate::fsglob::matcher(store.ignore_globs()?)?;
    let mut owned = 0usize;
    let mut observed = 0usize;
    let mut unowned = Vec::new();
    for cf in store.codefiles()? {
        if ignore.is_match(&cf.name) {
            continue;
        }
        if codefile_observed(&cf) {
            observed += 1;
            continue;
        }
        if store.realizing_implementers(&cf.id)?.is_empty() {
            unowned.push(cf.name);
        } else {
            owned += 1;
        }
    }
    unowned.sort();
    Ok((owned + unowned.len(), owned, unowned, observed))
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

/// Text-mode page footer for a `list` command. Given how many rows were
/// `shown` starting at `offset` out of `total`, returns the hint to fetch the
/// next page — or, when `offset` overshoots the end, says so. Returns `None`
/// when the current page already reaches the end. JSON output stays a bare
/// array (no footer) so machine parsers are unaffected; the offset alone lets a
/// caller walk every page. The absent "more exist" signal is what hid rows
/// past the first page during recovery.
pub(crate) fn page_footer(shown: usize, offset: usize, total: usize) -> Option<String> {
    let end = offset + shown;
    if shown == 0 {
        if offset > 0 && total > 0 {
            return Some(format!("(offset {offset} is past the end — {total} total)"));
        }
        return None;
    }
    if end < total {
        return Some(format!(
            "… showing {}–{end} of {total}; --offset {end} for the next page",
            offset + 1
        ));
    }
    None
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
