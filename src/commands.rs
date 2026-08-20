//! Command handlers for the Journey-root public surface.
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
use serde::Serialize;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

mod apply_cmd;
mod audit_cmd;
mod bootstrap_cmd;
mod capture_cmd;
mod checkpoint_cmd;
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
mod judgment_cmd;
mod limits_cmd;
mod misc_cmd;
mod orient_cmd;
mod pattern_cmd;
mod proof_cmd;
mod proposal_cmd;
mod pulse;
mod release_cmd;
mod role_cmd;
mod status_cmd;
mod wiki;
pub(crate) use crate::coverage::codefile_observed;
pub use crate::coverage::{code_ownership_summary, unowned_names};
pub use crate::grammar::looks_like_symbol;
pub(crate) use status_cmd::require_lane;
// The in-band surface: `crate::mcp` calls exactly these, so an MCP tool and its
// CLI twin can never diverge.
pub(crate) use context_cmd::served_context;
pub(crate) use diagnostics_cmd::impact_report;
// The honest way to make a proof true: let loom run it. Public so callers other
// than the CLI — absorb, fixtures — take the same path rather than a seam.
pub(crate) use apply_cmd::apply_value;
pub(crate) use proof_cmd::observe_run;
pub use proof_cmd::{observe_validation, prove_intent};
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
            let root = path
                .or_else(|| cli.graph.clone())
                .unwrap_or_else(|| PathBuf::from("."));
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
                    crate::model::short(&id.graph_id),
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
        Command::Pattern { cmd } => pattern_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Codefile { cmd } => codefile_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Checkpoint { cmd } => {
            checkpoint_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json)
        }
        Command::Release { cmd } => release_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Export { check } => status_cmd::export(cli.graph.as_deref(), check, cli.json),
        Command::Import {
            file,
            repair_orphans,
        } => status_cmd::import(cli.graph.as_deref(), &file, repair_orphans, cli.json),
        Command::Apply { file } => apply_cmd::apply(cli.graph.as_deref(), &file, cli.json),
        Command::Sync { quiet, rebuild } => {
            status_cmd::sync_cmd(cli.graph.as_deref(), cli.json, quiet, rebuild)
        }
        Command::Status => status_cmd::status(cli.graph.as_deref(), cli.json),
        Command::Mode { mode } => status_cmd::mode_cmd(
            cli.graph.as_deref(),
            mode.map(crate::cli::GraphModeArg::is_observed),
            cli.json,
        ),
        Command::Next { mode, all, full } => match (mode, all) {
            // `--mode <m> --all`: the full roster of that one queue (depth view).
            (Some(m), true) => {
                if full {
                    bail!(
                        "--full applies to `loom next --all --json` (unscoped closeout), not \
                         `--mode <m> --all` (lightweight roster; work the top with \
                         `loom next --mode {}`)",
                        m.as_str()
                    );
                }
                status_cmd::queue_list(cli.graph.as_deref(), m.as_str(), cli.json)
            }
            // `--all` alone: the closeout — top item of every queue.
            (_, true) => {
                if full && !cli.json {
                    bail!(
                        "--full requires --json (`loom next --all --full --json`); without \
                         --json the closeout is a text roster and must not mint packets"
                    );
                }
                status_cmd::next_all(cli.graph.as_deref(), cli.json, full)
            }
            // Default: the single next work item (full packet).
            (m, false) => {
                if full {
                    bail!("--full applies to `loom next --all --json` (singular next is already a full packet)");
                }
                status_cmd::next_cmd(
                    cli.graph.as_deref(),
                    m.map(crate::cli::ModeArg::as_str),
                    cli.json,
                )
            }
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
        Command::Limits => limits_cmd::limits_cmd(cli.json),
        Command::Smells => diagnostics_cmd::smells_cmd(cli.graph.as_deref(), cli.json),
        Command::Debt { cmd } => diagnostics_cmd::debt(cli.graph.as_deref(), cmd, cli.json),
        Command::Finding { cmd } => diagnostics_cmd::finding(cli.graph.as_deref(), cmd, cli.json),
        Command::Doctor => diagnostics_cmd::doctor_cmd(cli.graph.as_deref(), cli.json),
        Command::Coverage => diagnostics_cmd::coverage_cmd(cli.graph.as_deref(), cli.json),
        Command::Ignore { cmd } => diagnostics_cmd::ignore_cmd(cli.graph.as_deref(), cmd, cli.json),
        Command::Whoami => diagnostics_cmd::whoami_cmd(cli.graph.as_deref(), cli.json),
        Command::Role { cmd } => role_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Proposal { cmd } => proposal_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Judgment { cmd } => judgment_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Journey { cmd } => journey::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Drive { cmd } => drive_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Hook { cmd } => hook_cmd::dispatch(cli.graph.as_deref(), cmd, cli.json),
        Command::Impact { target, depth } => {
            diagnostics_cmd::impact_cmd(cli.graph.as_deref(), &target, depth, cli.json)
        }
        Command::Decide {
            chose,
            instead_of,
            because,
            evidence,
            about,
        } => capture_cmd::decide_cmd(
            cli.graph.as_deref(),
            &chose,
            &instead_of,
            &because,
            &evidence,
            about.as_deref(),
            cli.json,
        ),
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
        Command::Audit { cmd, efficacy } => match cmd {
            Some(sub) => audit_cmd::dispatch(cli.graph.as_deref(), sub, cli.json),
            None => diagnostics_cmd::audit_cmd(cli.graph.as_deref(), efficacy, cli.json),
        },
        Command::Deepen { limit } => {
            diagnostics_cmd::deepen_cmd(cli.graph.as_deref(), limit, cli.json)
        }
        Command::Mcp { cmd } => match cmd {
            crate::cli::McpCmd::Serve => crate::mcp::serve_stdio(cli.graph.as_deref()),
            crate::cli::McpCmd::Transcript { requests_json } => {
                if !cli.json {
                    bail!("`loom mcp transcript` requires --json");
                }
                let report = crate::mcp::transcript(cli.graph.as_deref(), &requests_json)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
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

/// A `--json` failure that already knows its stdout document (for example
/// `journey run` naming compile/open/settle). `main` prints this object
/// instead of the generic `{status, detail}` envelope so stdout stays one
/// JSON value.
#[derive(Debug)]
pub(crate) struct JsonErrorEnvelope {
    value: serde_json::Value,
    context: String,
}

impl JsonErrorEnvelope {
    pub(crate) fn new(value: serde_json::Value, context: impl Into<String>) -> Self {
        Self {
            value,
            context: context.into(),
        }
    }

    /// Attach this envelope as the process error so `main` can downcast it.
    /// `anyhow::Error::context` keeps only the Display text and is not
    /// downcastable to this type.
    pub(crate) fn into_error(self) -> anyhow::Error {
        anyhow::Error::new(self)
    }
}

impl std::fmt::Display for JsonErrorEnvelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.context)
    }
}

impl std::error::Error for JsonErrorEnvelope {}

/// A `--json` command already wrote its stdout document and is exiting
/// non-zero as a status signal (doctor issues, journey lint blocking).
/// `main` must not append a second JSON value.
#[derive(Debug)]
pub(crate) struct JsonStdoutComplete {
    context: String,
}

impl JsonStdoutComplete {
    pub(crate) fn fail(context: impl Into<String>) -> anyhow::Error {
        anyhow::Error::new(Self {
            context: context.into(),
        })
    }
}

impl std::fmt::Display for JsonStdoutComplete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.context)
    }
}

impl std::error::Error for JsonStdoutComplete {}

pub(crate) fn json_stdout_already_complete(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<JsonStdoutComplete>().is_some())
}

/// Stdout document for a `--json` process failure: a command-specific envelope
/// when one was attached, otherwise `{status: "error", detail}`.
pub fn json_error_envelope(error: &anyhow::Error) -> serde_json::Value {
    for cause in error.chain() {
        if let Some(envelope) = cause.downcast_ref::<JsonErrorEnvelope>() {
            return envelope.value.clone();
        }
    }
    serde_json::json!({
        "status": "error",
        "detail": format!("{error:#}"),
    })
}

/// Write exactly one JSON error envelope to stdout. `main` still prints the
/// human `error:` line to stderr and chooses the exit code. Commands that
/// already printed their `--json` document skip this so stdout stays one value.
pub fn write_json_error_envelope(error: &anyhow::Error) {
    if json_stdout_already_complete(error) {
        return;
    }
    match serde_json::to_string_pretty(&json_error_envelope(error)) {
        Ok(rendered) => println!("{rendered}"),
        Err(_) => println!(r#"{{"status":"error","detail":"failed to render error envelope"}}"#),
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
    let store = Store::open(&root)?;
    stamp_writer_version(&store);
    Ok(store)
}

/// Crate + schema of the last write-open. Crate-only used to miss same-version
/// schema forks; the schema stamp is what makes those visible. Compared at
/// every read open: an older crate warns, a higher writer schema is a hard
/// refuse in the store before this runs.
fn stamp_writer_version(store: &Store) {
    // Best-effort operational breadcrumb: never fail a real command over it.
    let _ = store.set_meta(crate::WRITER_VERSION_KEY, crate::CRATE_VERSION);
    let _ = store.set_meta(crate::WRITER_SCHEMA_KEY, &crate::SCHEMA_VERSION.to_string());
}

fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.split('.').map(|part| part.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

fn warn_on_writer_drift(store: &Store) {
    let current = crate::CRATE_VERSION;
    if let Ok(Some(stamped)) = store.get_meta(crate::WRITER_VERSION_KEY) {
        if let (Some(mine), Some(writer)) = (parse_version(current), parse_version(&stamped)) {
            if mine < writer {
                eprintln!(
                    "warning: this loom binary is v{current}, but the graph was last written by \
                     loom v{stamped} — reads may misinterpret newer state; upgrade or rebuild \
                     the binary on PATH"
                );
            }
        }
    }
    if let Ok(Some(raw)) = store.get_meta(crate::WRITER_SCHEMA_KEY) {
        if let Ok(writer_schema) = raw.parse::<u32>() {
            if writer_schema > crate::SCHEMA_VERSION {
                eprintln!(
                    "warning: this loom understands schema v{}, but the graph was last written at \
                     schema v{writer_schema} — a same-crate fork is indistinguishable by \
                     crate version alone; use the build that matches the graph",
                    crate::SCHEMA_VERSION
                );
            }
        }
    }
}

/// Open the store for a single-fact write (a verdict or adjudication),
/// absorbing brief graph-lock contention with a bounded, jittered retry.
///
/// The store's exclusive lock stays fail-fast (`lock_wait_ms`) — that contract
/// protects long writers from queueing invisibly. But a verdict is one row and
/// a journal line: when parallel sub-drivers collide on it, every driver was
/// hand-rolling the same wait-and-retry loop the docs prescribe, and a missed
/// retry misread infrastructure contention as a failed write. The retry lives
/// here, at the door those commands share, so the playbook is the default.
pub(crate) fn open_fact_write(graph: Option<&Path>) -> Result<Store> {
    const ATTEMPTS: u32 = 4;
    let root = resolve_root(graph)?;
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        match Store::open(&root) {
            Ok(store) => return Ok(store),
            Err(error)
                if error
                    .to_string()
                    .contains(crate::store::LOCK_CONTENTION_MARKER) =>
            {
                // Deterministic jitter from pid+attempt: spreads colliding
                // retries without pulling in a rng dependency.
                let jitter = u64::from((std::process::id().wrapping_add(attempt * 7919)) % 500);
                std::thread::sleep(std::time::Duration::from_millis(300 + jitter));
                last = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last.expect("retry loop exits early unless a contention error was seen"))
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
///
/// ACCEPTED RESIDUAL (documented decision, 2026-08-06): TTY and environment
/// are caller-controlled, so a determined agent driving loom as a subprocess
/// can allocate a PTY, set LOOM_AGENT=solo, and omit LOOM_NON_INTERACTIVE.
/// Cryptographic host attestation was evaluated and declined for loom's
/// single-user local trust model; this is the strongest gate that
/// architecture supports.
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
        && matches!(
            crate::store::Agent::from_env(),
            Ok(crate::store::Agent::Solo)
        )
        && std::env::var_os("LOOM_NON_INTERACTIVE").is_none()
}

/// Demand a typed confirmation before a ratifying write. Retained for the
/// explicit `loom intent ratify`, where the human is deliberately being asked.
pub(crate) fn require_challenge(subject: &str) -> Result<&'static str> {
    if !io::stdin().is_terminal() {
        bail!(
            "INV-8 / finding 62b197cc: direct non-interactive ratification is indistinguishable from an LLM; obtain the human's answer through the host and pass it as --human-decision"
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

/// Resolve the two legitimate ways a human decision reaches a ratification
/// write. With a host answer, the current process is only the recorder; the
/// explicit, substantive `--human-decision` is the host conversation's
/// attestation and the journal keeps the executing lane auditable. With no
/// mediated answer, retain the direct typed challenge.
///
/// Loom deliberately has no cryptographic host-conversation attestation in
/// its single-user local trust model. Requiring process-local TTY presence here
/// would make the documented host-mediated path impossible in release builds,
/// while adding no boundary a compromised local host could not already cross.
pub(crate) fn ratification_decision(
    subject: &str,
    response: Option<String>,
) -> Result<crate::ratification::HumanDecision> {
    match response {
        Some(response) => mediated_decision(response),
        None => crate::ratification::HumanDecision::direct(require_challenge(subject)?),
    }
}

/// The mediated branch of a human decision, shared by every write path that
/// accepts `--human-decision`. The explicit answer is the authority-bearing
/// host record; `HumanDecision::mediated` refuses silence and placeholders,
/// and downstream journaling separates human authority from the executor.
pub(crate) fn mediated_decision(response: String) -> Result<crate::ratification::HumanDecision> {
    crate::ratification::HumanDecision::mediated(response)
}

/// Open the target graph read-only (shared lock, `query_only`). Read commands
/// use this so several agents can query one graph concurrently and never block
/// each other; only a writer holding the boundary makes them wait.
pub(crate) fn open_read(graph: Option<&Path>) -> Result<Store> {
    let root = resolve_root(graph)?;
    let store = Store::open_read(&root)?;
    warn_on_writer_drift(&store);
    Ok(store)
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

/// Uniform JSON envelope for every paginated list command.
pub(crate) fn pagination_envelope<T: Serialize>(
    items: &[T],
    offset: usize,
    limit: usize,
    total: usize,
) -> serde_json::Value {
    let returned = items.len();
    let end = offset.saturating_add(returned);
    let has_more = end < total;
    serde_json::json!({
        "items": items,
        "pagination": {
            "offset": offset,
            "limit": limit,
            "returned": returned,
            "total": total,
            "has_more": has_more,
            "next_offset": has_more.then_some(end),
        }
    })
}

/// Text-mode page footer for a `list` command. Given how many rows were
/// `shown` starting at `offset` out of `total`, returns the hint to fetch the
/// next page — or, when `offset` overshoots the end, says so. Returns `None`
/// when the current page already reaches the end.
pub(crate) fn page_footer(shown: usize, offset: usize, total: usize) -> Option<String> {
    let end = offset.saturating_add(shown);
    if shown == 0 {
        if offset > 0 && total > 0 {
            return Some(format!("(offset {offset} is past the end — {total} total)"));
        }
        return None;
    }
    if end < total {
        return Some(format!(
            "… showing {}–{end} of {total}. More items exist; rerun this list command with --offset {end} to see the next page.",
            offset.saturating_add(1)
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
    use super::{
        json_error_envelope, json_stdout_already_complete, page_footer, pagination_envelope,
        query_terms, JsonErrorEnvelope, JsonStdoutComplete,
    };
    use anyhow::anyhow;

    #[test]
    fn pagination_envelope_reports_first_and_final_pages() {
        let first = pagination_envelope(&[0; 50], 0, 50, 117);
        assert_eq!(first["pagination"]["offset"], 0);
        assert_eq!(first["pagination"]["limit"], 50);
        assert_eq!(first["pagination"]["returned"], 50);
        assert_eq!(first["pagination"]["total"], 117);
        assert_eq!(first["pagination"]["has_more"], true);
        assert_eq!(first["pagination"]["next_offset"], 50);

        let final_page = pagination_envelope(&[0; 17], 100, 50, 117);
        assert_eq!(final_page["pagination"]["returned"], 17);
        assert_eq!(final_page["pagination"]["has_more"], false);
        assert!(final_page["pagination"]["next_offset"].is_null());
    }

    #[test]
    fn page_footer_explicitly_tells_agents_how_to_continue() {
        assert_eq!(
            page_footer(50, 0, 117).as_deref(),
            Some(
                "… showing 1–50 of 117. More items exist; rerun this list command with --offset 50 to see the next page."
            )
        );
        assert_eq!(page_footer(17, 100, 117), None);
    }

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

    #[test]
    fn json_error_envelope_keeps_a_command_specific_document() {
        let error = JsonErrorEnvelope::new(
            serde_json::json!({
                "status": "error",
                "stage": "compile",
                "detail": "missing journey",
            }),
            "journey run failed during compile: missing journey",
        )
        .into_error();
        let envelope = json_error_envelope(&error);
        assert_eq!(envelope["status"], "error");
        assert_eq!(envelope["stage"], "compile");
        assert_eq!(envelope["detail"], "missing journey");
        assert_eq!(
            format!("{error:#}"),
            "journey run failed during compile: missing journey"
        );
    }

    #[test]
    fn json_error_envelope_defaults_to_status_and_detail() {
        let error = anyhow!("no loom graph found — run `loom init`");
        let envelope = json_error_envelope(&error);
        assert_eq!(envelope["status"], "error");
        assert_eq!(
            envelope["detail"].as_str(),
            Some("no loom graph found — run `loom init`")
        );
        assert!(envelope.get("stage").is_none());
    }

    #[test]
    fn json_stdout_complete_skips_a_second_envelope() {
        let error = JsonStdoutComplete::fail("doctor found 1 integrity issue(s)");
        assert!(json_stdout_already_complete(&error));
        assert_eq!(format!("{error:#}"), "doctor found 1 integrity issue(s)");
    }
}
