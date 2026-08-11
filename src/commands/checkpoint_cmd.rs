//! CLI Adapter for read-only semantic checkpoint recommendations.

use super::resolve_root;
use crate::checkpoint::{CheckpointReport, CheckpointStatus};
use crate::cli::CheckpointCmd;
use crate::store::Store;
use crate::Result;
use anyhow::bail;
use std::path::Path;

pub(crate) fn dispatch(graph: Option<&Path>, cmd: CheckpointCmd, json: bool) -> Result<()> {
    let root = resolve_root(graph)?;
    // Enforce the feature's truth boundary at the connection itself: this
    // command cannot migrate, journal, or mutate graph state.
    let store = Store::open_read(&root)?;
    match cmd {
        CheckpointCmd::Recommend { intents } => {
            let report = crate::checkpoint::recommend(&store, &intents)?;
            render(&report, json)?;
            if report.status == CheckpointStatus::Blocked {
                bail!("semantic checkpoint recommendation is blocked");
            }
            Ok(())
        }
    }
}

fn render(report: &CheckpointReport, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    match report.status {
        CheckpointStatus::Ready => {
            println!("semantic checkpoint: ready");
            println!("  scope: {}", report.scope.rationale);
            for path in &report.included_paths {
                println!(
                    "  include: {} [{}] — {}",
                    path.path, path.git_status, path.reason
                );
            }
            for path in &report.excluded_paths {
                println!(
                    "  exclude: {} [{}] — {}",
                    path.path, path.git_status, path.reason
                );
            }
            if let Some(message) = &report.suggested_message {
                println!("  suggested message: {message}");
            }
            println!(
                "  acting LLM may commit or defer; stage only the paths above, never `git add -A`, and leave the commit local"
            );
            println!(
                "  push requires a new explicit human decision bound to repository, remote, branch, and commit"
            );
        }
        CheckpointStatus::Blocked => {
            println!("semantic checkpoint: blocked");
            for blocker in &report.blockers {
                println!("  [{}] {}", blocker.kind, blocker.message);
            }
            for path in &report.excluded_paths {
                println!(
                    "  exclude: {} [{}] — {}",
                    path.path, path.git_status, path.reason
                );
            }
        }
    }
    Ok(())
}
