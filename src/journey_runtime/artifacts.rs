use crate::journey::{BASELINE_SCHEMA, JOURNEY_COMPILER_VERSION};
use crate::Result;
use anyhow::{anyhow, bail, Context};
use std::path::{Path, PathBuf};

use super::compile::canonical_bytes;
use super::types::{CompiledJourneyProof, JourneyBaseline, RuntimeReport, RuntimeStatus};
use super::values::canonicalize;

pub fn proof_path(root: &Path, journey_id: &str, profile: &str) -> Result<PathBuf> {
    crate::journey::validate_stable_id("journey", journey_id)?;
    crate::journey::validate_stable_id("profile", profile)?;
    Ok(root
        .join(crate::LOOM_DIR)
        .join("compiled")
        .join("journeys")
        .join(journey_id)
        .join(format!("{profile}.proof.json")))
}

pub fn baseline_path(root: &Path, journey_id: &str, profile: &str) -> Result<PathBuf> {
    Ok(proof_path(root, journey_id, profile)?.with_file_name(format!("{profile}.baseline.json")))
}

pub fn write_proof(root: &Path, proof: &CompiledJourneyProof) -> Result<PathBuf> {
    let path = proof_path(root, &proof.journey_id, &proof.profile)?;
    atomic_write(&path, &canonical_bytes(proof)?)?;
    Ok(path)
}

pub fn cache_matches(root: &Path, proof: &CompiledJourneyProof) -> Result<bool> {
    let path = proof_path(root, &proof.journey_id, &proof.profile)?;
    let Ok(actual) = std::fs::read(&path) else {
        return Ok(false);
    };
    Ok(actual == canonical_bytes(proof)?)
}

pub fn write_baseline(root: &Path, report: &RuntimeReport) -> Result<PathBuf> {
    if report.status != RuntimeStatus::Passed {
        bail!("only a passing Journey observation can be frozen");
    }
    let baseline = JourneyBaseline {
        schema: BASELINE_SCHEMA.into(),
        compiler_version: JOURNEY_COMPILER_VERSION.into(),
        journey_id: report.journey_id.clone(),
        journey_hash: report.journey_hash.clone(),
        surface_hash: report.surface_hash.clone(),
        profile: report.profile.clone(),
        report: report.clone(),
    };
    let path = baseline_path(root, &report.journey_id, &report.profile)?;
    let mut bytes = serde_json::to_vec_pretty(&canonicalize(serde_json::to_value(baseline)?))?;
    bytes.push(b'\n');
    atomic_write(&path, &bytes)?;
    Ok(path)
}

pub fn baseline_current(root: &Path, proof: &CompiledJourneyProof) -> Result<Option<bool>> {
    let path = baseline_path(root, &proof.journey_id, &proof.profile)?;
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(None);
    };
    let Ok(baseline) = serde_json::from_slice::<JourneyBaseline>(&bytes) else {
        return Ok(Some(false));
    };
    Ok(Some(
        baseline.schema == BASELINE_SCHEMA
            && baseline.compiler_version == JOURNEY_COMPILER_VERSION
            && baseline.journey_id == proof.journey_id
            && baseline.profile == proof.profile
            && baseline.journey_hash == proof.journey_hash
            && baseline.surface_hash == proof.surface_hash,
    ))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("artifact path '{}' has no parent", path.display()))?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("journey"),
        std::process::id()
    ));
    std::fs::write(&temporary, bytes)
        .with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("installing {}", path.display()))?;
    Ok(())
}
