//! CLI adapter for detached, rehearsal-only release verification.

use crate::cli::{ReleaseCmd, ReleasePhaseArg};
use crate::release::{ReleasePhase, ReleaseStatus};
use crate::Result;
use anyhow::bail;
use std::path::{Path, PathBuf};

pub(crate) fn dispatch(graph: Option<&Path>, cmd: ReleaseCmd, json: bool) -> Result<()> {
    if !json {
        bail!("`loom release` requires --json");
    }
    let root = graph.map(PathBuf::from).unwrap_or(std::env::current_dir()?);
    match cmd {
        ReleaseCmd::AuthorizeDerivations {
            manifest_dir,
            human_decision,
        } => {
            let executor = crate::identity::ExecutionIdentity::resolve_env()?.actor();
            let authorization = crate::release::authorize_derivations(
                &root,
                &manifest_dir,
                human_decision,
                &executor,
            )?;
            println!("{}", serde_json::to_string_pretty(&authorization)?);
            Ok(())
        }
        ReleaseCmd::Rehearse { phase } => rehearse_cmd(&root, phase),
        ReleaseCmd::Snapshot { destination } => {
            let report = crate::release::snapshot(&root, &destination)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
    }
}

/// Render the single structured attestation consumed by the Journey surface.
pub(crate) fn rehearse_cmd(root: &Path, phase: ReleasePhaseArg) -> Result<()> {
    let phase = match phase {
        ReleasePhaseArg::IsolatedDogfood => ReleasePhase::IsolatedDogfood,
        ReleasePhaseArg::FreshFixpoint => ReleasePhase::FreshFixpoint,
        ReleasePhaseArg::GatedPreparation => ReleasePhase::GatedPreparation,
    };
    let report = crate::release::rehearse(root, phase)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report.status == ReleaseStatus::Blocked {
        return Err(super::JsonStdoutComplete::fail(format!(
            "release rehearsal blocked: {}",
            report.detail.as_deref().unwrap_or("gate did not pass")
        )));
    }
    Ok(())
}
