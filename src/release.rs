//! Detached, rehearsal-only release verification.
//!
//! The public interface is deliberately one operation: produce a structured
//! attestation for one named phase. All copying, trust-boundary checks, direct
//! argv execution, and cleanup stay behind it. There is no release/install or
//! Git mutation operation in this module.

use crate::journey::SurfaceManifest;
use crate::model::{Node, NodeType};
use crate::Result;
use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub const OUTER_JOURNEY_ID_ENV: &str = "LOOM_RESERVED_OUTER_JOURNEY_ID";
pub const OUTER_JOURNEY_PROFILE_ENV: &str = "LOOM_RESERVED_OUTER_JOURNEY_PROFILE";
pub const OUTER_JOURNEY_RUN_ID_ENV: &str = "LOOM_RESERVED_OUTER_JOURNEY_RUN_ID";
pub const OUTER_JOURNEY_HASH_ENV: &str = "LOOM_RESERVED_OUTER_JOURNEY_HASH";
pub const OUTER_SURFACE_HASH_ENV: &str = "LOOM_RESERVED_OUTER_SURFACE_HASH";
pub const OUTER_COMPILER_VERSION_ENV: &str = "LOOM_RESERVED_OUTER_COMPILER_VERSION";
pub const OUTER_PROOF_HASH_ENV: &str = "LOOM_RESERVED_OUTER_PROOF_HASH";
pub const OUTER_CONTEXT_CAPSULE_ENV: &str = "LOOM_RESERVED_OUTER_CONTEXT_CAPSULE";
/// One-shot token issued by `release authorize-derivations`. The Journey
/// runtime consumes it while constructing the outer context capsule; it is
/// never forwarded to a detached candidate.
pub const DERIVATION_AUTHORITY_TOKEN_ENV: &str = "LOOM_RELEASE_DERIVATION_AUTHORITY";
#[doc(hidden)]
pub const DERIVATION_AUTHORITY_STORE_ENV: &str = "LOOM_RELEASE_DERIVATION_AUTHORITY_STORE";
pub const RELEASE_CARGO_HOME_ENV: &str = "LOOM_RELEASE_CARGO_HOME";

pub const RELEASE_REHEARSAL_SCHEMA: &str = "loom.release-rehearsal/v1";
const RELEASE_JOURNEY_ID: &str = "release-workflow";
const RELEASE_PROFILE: &str = "proof";
const SURFACE_MANIFEST_ROOT: &str = "journeys/surfaces";
const RELEASE_INVENTORY_PATH: &str = "release/inventory.json";
const SOURCE_EXCLUDES: [&str; 3] = [".git", ".loom", "target"];
// loom.graph.json is excluded because the gate's own export step rewrites it
// with candidate-local journal ids (<millis>-<pid>-<seq>), fresh validation
// node ids, and wall-clock timestamps — legitimate per-candidate identity that
// can never be byte-stable across independent rehearsals. The semantic
// attestation therefore compares the materialized source tree and the
// reauthorized manifest identities; each candidate's graph health is already
// proven separately by its own doctor/audit/coverage/drift and dogfood gates.
const RESULT_EXCLUDES: [&str; 5] = [
    ".git",
    ".loom",
    ".release-sandbox",
    "loom.graph.json",
    "target",
];
const INVENTORY_RESERVED_COMPONENTS: [&str; 8] = [
    ".claude",
    ".git",
    ".loom",
    ".qoder",
    ".reasonix",
    ".release-sandbox",
    "review-manifests",
    "target",
];
const INVENTORY_SECRET_PATTERNS: [&str; 12] = [
    ".env",
    ".env.*",
    "*.key",
    "*.pem",
    ".netrc",
    ".npmrc",
    ".pypirc",
    "credentials",
    "credentials.json",
    "id_ed25519",
    "id_rsa",
    "secrets.json",
];
const RELEASE_DIAGNOSTIC_BYTES: usize = 64 * 1024;
const DERIVATION_AUTHORITY_SCHEMA: &str = "loom.release-derivation-authority/v1";
const BOUND_DERIVATION_AUTHORITY_SCHEMA: &str = "loom.release-bound-derivation-authority/v1";
const DERIVATION_AUTHORITY_TOKEN_PREFIX: &str = "rda1_";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleasePhase {
    IsolatedDogfood,
    FreshFixpoint,
    GatedPreparation,
}

impl ReleasePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IsolatedDogfood => "isolated_dogfood",
            Self::FreshFixpoint => "fresh_fixpoint",
            Self::GatedPreparation => "gated_preparation",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseStatus {
    Passed,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OuterJourneyAttestation {
    pub journey_id: String,
    pub profile: String,
    pub run_id: String,
    pub journey_hash: String,
    pub surface_hash: String,
    pub compiler_version: String,
    pub proof_hash: String,
    pub excluded_from_nested_execution: bool,
    pub exclusion_reason: String,
    pub context_binding_limit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OuterJourneyContextCapsule {
    pub schema: String,
    pub journey_id: String,
    pub profile: String,
    pub run_id: String,
    pub journey_hash: String,
    pub surface_hash: String,
    pub compiler_version: String,
    pub proof_hash: String,
    pub derivation_authority: BoundDerivationAuthority,
}

/// One exact reviewed projection transported as data plus its canonical
/// identity. Imported graph facts are intentionally absent: the candidate
/// must establish fresh local authority from these bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizedDerivation {
    pub journey_id: String,
    pub journey_hash: String,
    pub proposal_id: String,
    pub manifest_hash: String,
    pub manifest: crate::journey::DerivationManifest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingDerivationAuthority {
    schema: String,
    batch_hash: String,
    authority: String,
    executor: String,
    human_decision: crate::ratification::HumanDecision,
    gate_token_digest: String,
    gate_binding: crate::journey_gate::GateBinding,
    derivations: Vec<AuthorizedDerivation>,
}

/// Host-facing result of the typed authorization command. The opaque token is
/// consumed once by the outer Journey runtime, never by release rehearsal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivationAuthorizationGrant {
    pub schema: String,
    pub status: String,
    pub token: String,
    pub batch_hash: String,
    pub authority: String,
    pub executor: String,
    pub derivations: Vec<DerivationAuthoritySubject>,
    pub continuation_environment: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivationAuthoritySubject {
    pub journey_id: String,
    pub journey_hash: String,
    pub proposal_id: String,
    pub manifest_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundDerivationAuthority {
    pub schema: String,
    pub batch_hash: String,
    pub authority: String,
    pub executor: String,
    pub human_decision: crate::ratification::HumanDecision,
    pub gate_token_digest: String,
    pub gate_binding: crate::journey_gate::GateBinding,
    pub outer_journey_id: String,
    pub outer_profile: String,
    pub outer_run_id: String,
    pub outer_journey_hash: String,
    pub outer_surface_hash: String,
    pub outer_compiler_version: String,
    pub outer_proof_hash: String,
    pub derivations: Vec<AuthorizedDerivation>,
    pub candidate_permits: Vec<DerivationCandidatePermit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivationCandidatePermit {
    pub phase: ReleasePhase,
    pub ordinal: usize,
    pub permit_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceAttestation {
    pub detached: bool,
    pub source_excludes: Vec<String>,
    pub initially_empty: bool,
    pub nonempty_probe: String,
    pub preinitialized_probe: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceInventoryAttestation {
    pub schema: String,
    pub path: String,
    pub manifest_hash: String,
    pub inventory_hash: String,
    pub provenance: String,
    pub git_verification: String,
    pub git_influenced_plan: bool,
    /// Total manifest entries, including explicit `absent` tombstones.
    pub entry_count: usize,
    /// Regular/executable files actually materialized into the candidate.
    pub file_count: usize,
    /// Explicit tracked deletions bound into the manifest but never copied.
    pub tombstone_count: usize,
    pub materialized_matches: bool,
    pub missing: usize,
    pub unexpected: usize,
    pub secret: usize,
    pub symlink: usize,
    pub non_regular: usize,
    pub reserved: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSnapshotReport {
    pub schema: String,
    pub status: String,
    pub candidate_hash: String,
    pub source_inventory: SourceInventoryAttestation,
    pub workspace: WorkspaceAttestation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestAttestation {
    pub journey_id: String,
    pub surface_id: String,
    pub path: String,
    pub manifest_hash: String,
    pub codefile: String,
    pub locator: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphAttestation {
    pub schema_version: u32,
    pub legacy_imported: bool,
    pub legacy_migrated: bool,
    pub imported_surfaces_quarantined: usize,
    pub manifests_reauthorized: Vec<ManifestAttestation>,
    pub authority_fail_closed: bool,
    pub authority_fabricated: bool,
    pub outer_profile: OuterJourneyAttestation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyCacheAttestation {
    pub strategy: String,
    pub cargo_home: String,
    pub provenance: String,
    pub before_hash: String,
    pub after_hash: String,
    pub unchanged: bool,
    pub offline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseEvent {
    pub id: String,
    pub outcome: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgvLedgerEntry {
    pub source: String,
    pub executable: String,
    pub argv: Vec<String>,
    pub policy: String,
    pub attempted: bool,
    pub outcome: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandObservation {
    pub success: bool,
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Testable direct-argv execution seam. The production adapter clears the
/// environment and installs a candidate-confined HOME/CARGO_HOME/Git config;
/// tests use a recording adapter so successful and failing gate evidence can
/// be exercised without running a release toolchain.
pub trait ReleaseExecutor {
    fn execute(
        &mut self,
        cwd: &Path,
        executable: &Path,
        argv: &[String],
        environment: &BTreeMap<String, String>,
    ) -> Result<CommandObservation>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct FixpointAttestation {
    pub performed: bool,
    pub candidate_hash_equal: bool,
    pub result_hash_equal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct EffectAttestation {
    pub live_source_changed: bool,
    pub live_graph_changed: bool,
    pub live_target_changed: bool,
    pub live_git_changed: bool,
    pub live_git_head_changed: bool,
    pub live_git_index_changed: bool,
    pub live_git_remotes_changed: bool,
    pub installed_binary_changed: bool,
    pub release_paths_changed: Vec<String>,
    pub argv_attempt_scope: String,
    pub top_level_install_argv_attempted: bool,
    pub top_level_commit_argv_attempted: bool,
    pub top_level_push_argv_attempted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePolicyAttestation {
    pub push_requires_explicit_human_decision: bool,
    pub bitwise_reproducibility_claimed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRehearsalReport {
    pub schema: String,
    pub phase: ReleasePhase,
    pub status: ReleaseStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_hash: Option<String>,
    pub workspace: WorkspaceAttestation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_inventory: Option<SourceInventoryAttestation>,
    pub graph: GraphAttestation,
    pub fixpoint: FixpointAttestation,
    pub timeline: Vec<ReleaseEvent>,
    pub execution_ledger: Vec<ArgvLedgerEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_cache: Option<DependencyCacheAttestation>,
    pub effects: EffectAttestation,
    pub policy: ReleasePolicyAttestation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ReleaseRehearsalReport {
    fn blocked(phase: ReleasePhase, outer: OuterJourneyAttestation, detail: String) -> Self {
        Self {
            schema: RELEASE_REHEARSAL_SCHEMA.into(),
            phase,
            status: ReleaseStatus::Blocked,
            candidate_hash: None,
            result_hash: None,
            workspace: WorkspaceAttestation {
                detached: false,
                source_excludes: SOURCE_EXCLUDES.iter().map(ToString::to_string).collect(),
                initially_empty: false,
                nonempty_probe: "not_run".into(),
                preinitialized_probe: "not_run".into(),
            },
            source_inventory: None,
            graph: GraphAttestation {
                schema_version: crate::SCHEMA_VERSION,
                legacy_imported: false,
                legacy_migrated: false,
                imported_surfaces_quarantined: 0,
                manifests_reauthorized: Vec::new(),
                authority_fail_closed: true,
                authority_fabricated: false,
                outer_profile: outer,
            },
            fixpoint: FixpointAttestation::default(),
            timeline: Vec::new(),
            execution_ledger: Vec::new(),
            dependency_cache: None,
            effects: EffectAttestation::default(),
            policy: ReleasePolicyAttestation {
                push_requires_explicit_human_decision: true,
                bitwise_reproducibility_claimed: false,
            },
            detail: Some(detail),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveState {
    source: String,
    graph: String,
    target: String,
    git: String,
    git_head: String,
    git_index: String,
    git_remotes: String,
    installed_binary: String,
}

#[derive(Debug)]
struct GateResult {
    candidate_hash: String,
    result_hash: String,
    imported_surfaces_quarantined: usize,
    manifests_reauthorized: Vec<ManifestAttestation>,
    source_inventory: SourceInventoryAttestation,
}

struct GateRuntime<'a> {
    outer: &'a OuterJourneyAttestation,
    derivation_authority: &'a BoundDerivationAuthority,
    executor: &'a mut dyn ReleaseExecutor,
    dependency_cache: &'a DependencyCacheGuard,
}

/// Run one release-rehearsal phase and return a single structured attestation.
/// Expected policy/readiness failures are represented as `blocked`; filesystem
/// errors still return `Err` because no trustworthy attestation can be formed.
pub fn rehearse(root: &Path, phase: ReleasePhase) -> Result<ReleaseRehearsalReport> {
    let mut executor = SystemReleaseExecutor;
    rehearse_with_executor(root, phase, &mut executor)
}

/// Materialize the exact source-controlled release inventory into an empty
/// destination without reading or creating Loom graph state.
pub fn snapshot(root: &Path, destination: &Path) -> Result<SourceSnapshotReport> {
    let root = root.canonicalize()?;
    let destination_metadata = fs::symlink_metadata(destination)?;
    if destination_metadata.file_type().is_symlink() || !destination_metadata.is_dir() {
        bail!("release snapshot destination must be a regular directory");
    }
    let destination = destination.canonicalize().with_context(|| {
        format!(
            "canonicalizing snapshot destination {}",
            destination.display()
        )
    })?;
    if destination.starts_with(&root) {
        bail!("release snapshot destination must be outside the source repository");
    }
    if fs::read_dir(&destination)?.next().is_some() {
        bail!("release snapshot destination must be empty");
    }
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("release snapshot destination has no parent"))?;
    let destination_identity = crate::artifact::fingerprint(&destination.to_string_lossy());
    let stage = parent.join(format!(
        ".loom-release-snapshot-stage-{}-{destination_identity}",
        std::process::id(),
    ));
    let backup = parent.join(format!(
        ".loom-release-snapshot-empty-{}-{destination_identity}",
        std::process::id(),
    ));
    if stage.exists() || backup.exists() {
        bail!("release snapshot staging path already exists");
    }
    fs::create_dir(&stage)?;
    let mut ledger = Vec::new();
    let source_inventory = match copy_candidate(&root, &stage, &mut ledger) {
        Ok(inventory) => inventory,
        Err(error) => {
            let _ = fs::remove_dir_all(&stage);
            return Err(error);
        }
    };
    fs::rename(&destination, &backup)?;
    if let Err(error) = fs::rename(&stage, &destination) {
        let _ = fs::rename(&backup, &destination);
        let _ = fs::remove_dir_all(&stage);
        return Err(error.into());
    }
    fs::remove_dir(&backup)?;
    Ok(SourceSnapshotReport {
        schema: "loom.release-snapshot/v1".into(),
        status: "passed".into(),
        candidate_hash: source_inventory.inventory_hash.clone(),
        source_inventory,
        workspace: passed_workspace(false),
    })
}

/// Return the canonical manifest identity used by snapshot and rehearsal
/// attestations without constructing or copying a candidate.
pub fn source_inventory_manifest_hash(root: &Path) -> Result<String> {
    let (_, manifest_hash) = load_source_inventory(root)?;
    Ok(manifest_hash)
}

#[doc(hidden)]
pub fn rehearse_with_executor(
    root: &Path,
    phase: ReleasePhase,
    executor: &mut dyn ReleaseExecutor,
) -> Result<ReleaseRehearsalReport> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing release candidate {}", root.display()))?;
    let before = LiveState::capture(&root)?;
    let (outer, derivation_authority) = match required_outer_context(&root) {
        Ok(context) => context,
        Err(error) => {
            let mut report = ReleaseRehearsalReport::blocked(
                phase,
                missing_outer_attestation(),
                format!("{error:#}"),
            );
            report.effects = before.compare(&LiveState::capture(&root)?, &[]);
            return Ok(report);
        }
    };

    let dependency_cache = match DependencyCacheGuard::open(&root) {
        Ok(cache) => cache,
        Err(error) => {
            let mut report = ReleaseRehearsalReport::blocked(phase, outer, format!("{error:#}"));
            report.effects = before.compare(&LiveState::capture(&root)?, &[]);
            return Ok(report);
        }
    };
    let mut ledger = Vec::new();
    let mut source_inventory = None;
    let outcome = {
        let mut runtime = GateRuntime {
            outer: &outer,
            derivation_authority: &derivation_authority,
            executor,
            dependency_cache: &dependency_cache,
        };
        rehearse_inner(
            &root,
            phase,
            &mut runtime,
            &mut ledger,
            &mut source_inventory,
        )
    };
    let cache_attestation = dependency_cache.attest()?;
    let outcome = if cache_attestation.unchanged {
        outcome
    } else {
        Err(anyhow!(
            "release rehearsal changed the explicit dependency cache"
        ))
    };
    let after = LiveState::capture(&root)?;
    let effects = before.compare(&after, &ledger);
    let mut report = match outcome {
        Ok(report) => report,
        Err(error) => ReleaseRehearsalReport::blocked(phase, outer, format!("{error:#}")),
    };
    report.execution_ledger = ledger;
    if report.source_inventory.is_none() {
        report.source_inventory = source_inventory;
    }
    report.dependency_cache = Some(cache_attestation);
    report.effects = effects;
    if caller_state_changed(&report.effects) {
        report.status = ReleaseStatus::Blocked;
        report.detail = Some("release rehearsal changed caller state".into());
    }
    Ok(report)
}

fn caller_state_changed(effects: &EffectAttestation) -> bool {
    effects.live_source_changed
        || effects.live_graph_changed
        || effects.live_target_changed
        || effects.live_git_changed
        || effects.live_git_head_changed
        || effects.live_git_index_changed
        || effects.live_git_remotes_changed
        || effects.installed_binary_changed
}

fn rehearse_inner(
    root: &Path,
    phase: ReleasePhase,
    runtime: &mut GateRuntime<'_>,
    ledger: &mut Vec<ArgvLedgerEntry>,
    source_inventory: &mut Option<SourceInventoryAttestation>,
) -> Result<ReleaseRehearsalReport> {
    let (gate, timeline, workspace, fixpoint) = match phase {
        ReleasePhase::IsolatedDogfood => {
            let permit = claim_candidate_permit(runtime.derivation_authority, phase, 0)?;
            let gate = run_isolated_gate(root, &permit, runtime, ledger, source_inventory)?;
            (
                gate,
                vec![event("isolated_dogfood", "passed")],
                passed_workspace(false),
                FixpointAttestation::default(),
            )
        }
        ReleasePhase::FreshFixpoint => {
            let first_permit = claim_candidate_permit(runtime.derivation_authority, phase, 0)?;
            let second_permit = claim_candidate_permit(runtime.derivation_authority, phase, 1)?;
            let probes = probe_empty_workspace_policy(root, ledger)?;
            let first = run_isolated_gate(root, &first_permit, runtime, ledger, source_inventory)?;
            let second =
                run_isolated_gate(root, &second_permit, runtime, ledger, source_inventory)?;
            if first.candidate_hash != second.candidate_hash
                || first.result_hash != second.result_hash
            {
                bail!("independent release rehearsals produced different semantic attestations");
            }
            (
                second,
                vec![event("fresh_fixpoint", "passed")],
                probes,
                FixpointAttestation {
                    performed: true,
                    candidate_hash_equal: true,
                    result_hash_equal: true,
                },
            )
        }
        ReleasePhase::GatedPreparation => {
            let first_permit = claim_candidate_permit(runtime.derivation_authority, phase, 0)?;
            let second_permit = claim_candidate_permit(runtime.derivation_authority, phase, 1)?;
            let probes = probe_empty_workspace_policy(root, ledger)?;
            let first = run_isolated_gate(root, &first_permit, runtime, ledger, source_inventory)?;
            let second =
                run_isolated_gate(root, &second_permit, runtime, ledger, source_inventory)?;
            if first.candidate_hash != second.candidate_hash
                || first.result_hash != second.result_hash
            {
                bail!("release preparation gates disagree on semantic readiness");
            }
            (
                second,
                vec![
                    event("isolated_dogfood", "passed"),
                    event("fresh_fixpoint", "passed"),
                    event("mutation", "skipped_rehearsal"),
                ],
                probes,
                FixpointAttestation {
                    performed: true,
                    candidate_hash_equal: true,
                    result_hash_equal: true,
                },
            )
        }
    };

    Ok(ReleaseRehearsalReport {
        schema: RELEASE_REHEARSAL_SCHEMA.into(),
        phase,
        status: ReleaseStatus::Passed,
        candidate_hash: Some(gate.candidate_hash),
        result_hash: Some(gate.result_hash),
        workspace,
        source_inventory: Some(gate.source_inventory),
        graph: GraphAttestation {
            schema_version: crate::SCHEMA_VERSION,
            legacy_imported: false,
            legacy_migrated: false,
            imported_surfaces_quarantined: gate.imported_surfaces_quarantined,
            manifests_reauthorized: gate.manifests_reauthorized,
            authority_fail_closed: true,
            authority_fabricated: false,
            outer_profile: runtime.outer.clone(),
        },
        fixpoint,
        timeline,
        execution_ledger: Vec::new(),
        dependency_cache: None,
        effects: EffectAttestation::default(),
        policy: ReleasePolicyAttestation {
            push_requires_explicit_human_decision: true,
            bitwise_reproducibility_claimed: false,
        },
        detail: None,
    })
}

fn event(id: &str, outcome: &str) -> ReleaseEvent {
    ReleaseEvent {
        id: id.into(),
        outcome: outcome.into(),
    }
}

fn missing_outer_attestation() -> OuterJourneyAttestation {
    OuterJourneyAttestation {
        journey_id: String::new(),
        profile: String::new(),
        run_id: String::new(),
        journey_hash: String::new(),
        surface_hash: String::new(),
        compiler_version: String::new(),
        proof_hash: String::new(),
        excluded_from_nested_execution: false,
        exclusion_reason: "reserved outer Journey context was unavailable".into(),
        context_binding_limit:
            "same-user filesystem/process isolation is not a cryptographic authority boundary"
                .into(),
    }
}

/// Seal the exact current derivation projection set after the host has
/// obtained a substantive human answer. This is read-only with respect to the
/// Loom graph: the only write is an opaque one-shot capsule under the runtime
/// temporary authority store.
pub fn authorize_derivations(
    root: &Path,
    manifest_dir: &Path,
    human_decision: String,
    executor: &str,
) -> Result<DerivationAuthorizationGrant> {
    let root = root.canonicalize()?;
    let manifest_dir = manifest_dir.canonicalize().with_context(|| {
        format!(
            "canonicalizing derivation manifest directory {}",
            manifest_dir.display()
        )
    })?;
    let metadata = fs::symlink_metadata(&manifest_dir)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("derivation manifest directory must be a regular directory");
    }
    if executor.trim().is_empty() || crate::model::is_placeholder(executor) {
        bail!("derivation authority executor must be substantive");
    }
    let human_decision = crate::ratification::HumanDecision::mediated(human_decision)?;
    let store = crate::store::Store::open_read(&root)?;
    let registered: BTreeMap<String, Node> = store
        .list_nodes(Some(NodeType::Journey), usize::MAX)?
        .into_iter()
        .map(|node| (node.name.clone(), node))
        .collect();
    if registered.is_empty() {
        bail!("derivation authorization requires registered Journeys");
    }
    let specs: BTreeMap<String, crate::journey::JourneySpec> = journey_artifacts(&root)?
        .into_iter()
        .map(|path| {
            let spec = crate::journey::parse(&path)?;
            Ok((spec.id.clone(), spec))
        })
        .collect::<Result<_>>()?;
    let proposals = store.list_nodes(Some(NodeType::Proposal), usize::MAX)?;
    let mut paths = regular_files_with_suffix(&manifest_dir, ".json")?;
    paths.sort();
    let mut derivations = Vec::new();
    let mut seen = BTreeSet::new();
    for path in paths {
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)
            .with_context(|| format!("parsing reviewed manifest {}", path.display()))?;
        if value.get("schema").and_then(serde_json::Value::as_str)
            != Some(crate::journey::DERIVATION_SCHEMA)
        {
            continue;
        }
        let manifest: crate::journey::DerivationManifest = serde_json::from_value(value)
            .with_context(|| format!("parsing {} as a derivation manifest", path.display()))?;
        if !seen.insert(manifest.journey_id.clone()) {
            bail!(
                "reviewed derivation batch repeats Journey '{}'",
                manifest.journey_id
            );
        }
        let journey = registered.get(&manifest.journey_id).ok_or_else(|| {
            anyhow!(
                "reviewed derivation targets unregistered Journey '{}'",
                manifest.journey_id
            )
        })?;
        let spec = specs.get(&manifest.journey_id).ok_or_else(|| {
            anyhow!(
                "registered Journey '{}' has no authored source",
                manifest.journey_id
            )
        })?;
        let journey_hash = spec.semantic_hash()?;
        manifest.validate_for(spec, &journey_hash)?;
        if journey
            .body
            .get("semantic_hash")
            .and_then(serde_json::Value::as_str)
            != Some(journey_hash.as_str())
        {
            bail!(
                "registered Journey '{}' is stale for its authored source",
                manifest.journey_id
            );
        }
        let canonical = canonical_json(serde_json::to_value(&manifest)?);
        let canonical_text = serde_json::to_string(&canonical)?;
        let manifest_hash = crate::artifact::fingerprint(&canonical_text);
        let matching: Vec<&Node> = proposals
            .iter()
            .filter(|proposal| {
                proposal.status == "adopted"
                    && proposal
                        .body
                        .get("source")
                        .and_then(serde_json::Value::as_str)
                        == Some("journey_derivation")
                    && proposal
                        .body
                        .get("journey_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(manifest.journey_id.as_str())
                    && proposal
                        .body
                        .get("journey_hash")
                        .and_then(serde_json::Value::as_str)
                        == Some(journey_hash.as_str())
                    && proposal
                        .body
                        .get("proposal_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(manifest.proposal_id.as_str())
                    && proposal
                        .body
                        .get("manifest_hash")
                        .and_then(serde_json::Value::as_str)
                        == Some(manifest_hash.as_str())
                    && proposal.body.get("raw").and_then(serde_json::Value::as_str)
                        == Some(canonical_text.as_str())
            })
            .collect();
        if matching.len() != 1 {
            bail!(
                "reviewed derivation for Journey '{}' has {} exact current adopted Proposals (expected one)",
                manifest.journey_id,
                matching.len()
            );
        }
        derivations.push(AuthorizedDerivation {
            journey_id: manifest.journey_id.clone(),
            journey_hash,
            proposal_id: manifest.proposal_id.clone(),
            manifest_hash,
            manifest,
        });
    }
    derivations.sort_by(|left, right| left.journey_id.cmp(&right.journey_id));
    let registered_ids: BTreeSet<&str> = registered.keys().map(String::as_str).collect();
    let authorized_ids: BTreeSet<&str> = derivations
        .iter()
        .map(|subject| subject.journey_id.as_str())
        .collect();
    if registered_ids != authorized_ids {
        let missing = registered_ids
            .difference(&authorized_ids)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        let unexpected = authorized_ids
            .difference(&registered_ids)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "reviewed derivation batch must cover every registered Journey exactly once (missing: [{missing}], unexpected: [{unexpected}])"
        );
    }
    let subject_hash = derivation_subject_hash(&derivations)?;
    let release_journey = registered.get(RELEASE_JOURNEY_ID).ok_or_else(|| {
        anyhow!("derivation authorization requires registered '{RELEASE_JOURNEY_ID}'")
    })?;
    let release_surface_hash = crate::journey::surface_projection_hash(&store, release_journey)?
        .ok_or_else(|| anyhow!("release-workflow has no current accepted surface"))?;
    let prompt = crate::journey_gate::HumanPrompt::new(
        format!(
            "Approve the exact {}-manifest derivation batch bound by SHA-256 {subject_hash}?",
            derivations.len()
        ),
        "Approve only if the listed Journey, semantic, proposal, and canonical manifest hashes match the reviewed batch.",
        vec![
            crate::journey_gate::HumanOption::new(
                "approve",
                "Approve exact batch",
                "Authorize only these exact current manifests for one outer release run.",
                false,
            ),
            crate::journey_gate::HumanOption::new(
                "revise",
                "Request revisions",
                "Do not authorize; revise one or more derivations.",
                true,
            ),
            crate::journey_gate::HumanOption::new(
                "defer",
                "Defer",
                "Do not authorize or mutate release state.",
                false,
            ),
        ],
    )?;
    let binding = crate::journey_gate::GateBinding {
        journey_id: RELEASE_JOURNEY_ID.into(),
        profile: RELEASE_PROFILE.into(),
        journey_hash: release_journey
            .body
            .get("semantic_hash")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("release-workflow lacks a semantic hash"))?
            .into(),
        surface_hash: release_surface_hash,
        step_id: "authorize-derivations".into(),
        step_index: 0,
        subject: crate::journey_gate::GateSubject {
            kind: "release_derivation_batch".into(),
            id: format!("batch-{subject_hash}"),
            hash: subject_hash,
        },
        prompt_hash: prompt.digest()?,
    };
    let authority_root = authority_store_root(&root)?;
    let gate_store = crate::journey_gate::CapsuleStore::new(authority_root.join("human-gate"))?;
    let issued = gate_store.issue(binding.clone(), prompt)?;
    let claimed = gate_store.claim(
        &issued.pending.resume_token,
        &binding,
        crate::journey_gate::ResumeAnswer {
            choice_id: "approve".into(),
            human_decision: match &human_decision {
                crate::ratification::HumanDecision::Mediated { response } => response.clone(),
                crate::ratification::HumanDecision::Direct { .. } => {
                    bail!("release derivation authorization must be host-mediated")
                }
            },
            free_form: None,
        },
        executor,
    )?;
    let receipt = claimed.receipt;
    if receipt.authority != "human"
        || receipt.executor != executor
        || receipt.choice_id != "approve"
        || receipt.binding != binding
        || receipt.human_decision != human_decision
    {
        bail!("host-mediated derivation authority receipt is inconsistent");
    }
    let batch_hash = derivation_batch_hash(&derivations, &human_decision)?;
    let pending = PendingDerivationAuthority {
        schema: DERIVATION_AUTHORITY_SCHEMA.into(),
        batch_hash: batch_hash.clone(),
        authority: "human".into(),
        executor: executor.to_string(),
        human_decision,
        gate_token_digest: receipt.token_digest,
        gate_binding: receipt.binding,
        derivations: derivations.clone(),
    };
    let token = issue_derivation_authority(&root, &pending)?;
    Ok(DerivationAuthorizationGrant {
        schema: DERIVATION_AUTHORITY_SCHEMA.into(),
        status: "authorized_pending_outer_runtime".into(),
        token,
        batch_hash,
        authority: "human".into(),
        executor: executor.to_string(),
        derivations: derivations
            .into_iter()
            .map(|subject| DerivationAuthoritySubject {
                journey_id: subject.journey_id,
                journey_hash: subject.journey_hash,
                proposal_id: subject.proposal_id,
                manifest_hash: subject.manifest_hash,
            })
            .collect(),
        continuation_environment: DERIVATION_AUTHORITY_TOKEN_ENV.into(),
    })
}

fn derivation_batch_hash(
    derivations: &[AuthorizedDerivation],
    decision: &crate::ratification::HumanDecision,
) -> Result<String> {
    let value = canonical_json(serde_json::json!({
        "authority": "human",
        "human_decision": decision,
        "derivations": derivations,
    }));
    Ok(crate::journey_gate::sha256_digest(&serde_json::to_vec(
        &value,
    )?))
}

fn derivation_subject_hash(derivations: &[AuthorizedDerivation]) -> Result<String> {
    let value = canonical_json(serde_json::json!({"derivations": derivations}));
    Ok(crate::journey_gate::sha256_digest(&serde_json::to_vec(
        &value,
    )?))
}

fn authority_store_root(live_root: &Path) -> Result<PathBuf> {
    let configured = std::env::var_os(DERIVATION_AUTHORITY_STORE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("loom-release-derivation-authorities-v1"));
    fs::create_dir_all(configured.join("pending"))?;
    fs::create_dir_all(configured.join("claimed"))?;
    fs::create_dir_all(configured.join("batches"))?;
    let root = configured.canonicalize()?;
    let temp = std::env::temp_dir().canonicalize()?;
    if !root.starts_with(&temp) || root.starts_with(live_root.canonicalize()?) {
        bail!("release derivation authority store must be under the runtime temp root and outside the live graph");
    }
    Ok(root)
}

fn issue_derivation_authority(root: &Path, pending: &PendingDerivationAuthority) -> Result<String> {
    let store = authority_store_root(root)?;
    let marker = store.join("batches").join(&pending.batch_hash);
    let mut marker_options = fs::OpenOptions::new();
    marker_options.write(true).create_new(true);
    if marker_options.open(&marker).is_err() {
        bail!("this exact derivation authority batch has already been issued");
    }
    let result = (|| {
        let random: String = rusqlite::Connection::open_in_memory()?.query_row(
            "SELECT lower(hex(randomblob(32)))",
            [],
            |row| row.get(0),
        )?;
        let token = format!("{DERIVATION_AUTHORITY_TOKEN_PREFIX}{random}");
        validate_derivation_authority_token(&token)?;
        let path = store.join("pending").join(&token);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        serde_json::to_writer(&mut file, pending)?;
        use std::io::Write;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(token)
    })();
    if result.is_err() {
        let _ = fs::remove_file(marker);
    }
    result
}

fn validate_derivation_authority_token(token: &str) -> Result<()> {
    let random = token
        .strip_prefix(DERIVATION_AUTHORITY_TOKEN_PREFIX)
        .ok_or_else(|| anyhow!("invalid release derivation authority token"))?;
    if random.len() != 64
        || !random
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("invalid release derivation authority token");
    }
    Ok(())
}

pub fn write_outer_context_capsule(
    live_root: &Path,
    directory: &Path,
    spec: &crate::journey::JourneySpec,
    proof: &crate::journey_runtime::CompiledJourneyProof,
    run_id: &str,
) -> Result<(PathBuf, OuterJourneyContextCapsule)> {
    let proof_bytes = crate::journey_runtime::canonical_bytes(proof)?;
    let proof_text = std::str::from_utf8(&proof_bytes).context("compiled proof is not UTF-8")?;
    let journey_hash = spec.semantic_hash()?;
    let proof_hash = crate::artifact::fingerprint(proof_text);
    let derivation_authority = claim_derivation_authority(
        live_root,
        &spec.id,
        &proof.profile,
        run_id,
        &journey_hash,
        &proof.surface_hash,
        &proof.compiler_version,
        &proof_hash,
    )?;
    let capsule = OuterJourneyContextCapsule {
        schema: "loom.release-outer-context/v1".into(),
        journey_id: spec.id.clone(),
        profile: proof.profile.clone(),
        run_id: run_id.into(),
        journey_hash,
        surface_hash: proof.surface_hash.clone(),
        compiler_version: proof.compiler_version.clone(),
        proof_hash,
        derivation_authority,
    };
    let path = directory.join("release-outer-context.json");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("creating release context capsule {}", path.display()))?;
    serde_json::to_writer(&mut file, &capsule)?;
    use std::io::Write;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok((path, capsule))
}

#[allow(clippy::too_many_arguments)]
fn claim_derivation_authority(
    live_root: &Path,
    outer_journey_id: &str,
    outer_profile: &str,
    outer_run_id: &str,
    outer_journey_hash: &str,
    outer_surface_hash: &str,
    outer_compiler_version: &str,
    outer_proof_hash: &str,
) -> Result<BoundDerivationAuthority> {
    let token = std::env::var(DERIVATION_AUTHORITY_TOKEN_ENV)
        .context("release-workflow requires a one-shot derivation authority token")?;
    validate_derivation_authority_token(&token)?;
    let store = authority_store_root(live_root)?;
    let pending_path = store.join("pending").join(&token);
    let claimed_path = store.join("claimed").join(&token);
    let metadata = match fs::symlink_metadata(&pending_path) {
        Ok(metadata) => metadata,
        Err(_error) if claimed_path.exists() => {
            bail!("release derivation authority token has already been consumed")
        }
        Err(error) => return Err(error).context("unknown release derivation authority token"),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("release derivation authority capsule must be a regular non-symlink file");
    }
    let pending: PendingDerivationAuthority = serde_json::from_slice(&fs::read(&pending_path)?)
        .context("release derivation authority capsule is malformed")?;
    if pending.schema != DERIVATION_AUTHORITY_SCHEMA
        || pending.authority != "human"
        || pending.derivations.is_empty()
        || derivation_batch_hash(&pending.derivations, &pending.human_decision)?
            != pending.batch_hash
        || pending.gate_binding.subject.hash != derivation_subject_hash(&pending.derivations)?
        || pending.gate_binding.journey_id != RELEASE_JOURNEY_ID
        || pending.gate_binding.profile != RELEASE_PROFILE
    {
        bail!("release derivation authority capsule failed its exact binding");
    }
    fs::rename(&pending_path, &claimed_path)
        .map_err(anyhow::Error::from)
        .context("atomically claiming release derivation authority")?;
    let candidate_permits =
        expected_candidate_permits(&pending.batch_hash, outer_run_id, outer_proof_hash);
    Ok(BoundDerivationAuthority {
        schema: BOUND_DERIVATION_AUTHORITY_SCHEMA.into(),
        batch_hash: pending.batch_hash,
        authority: pending.authority,
        executor: pending.executor,
        human_decision: pending.human_decision,
        gate_token_digest: pending.gate_token_digest,
        gate_binding: pending.gate_binding,
        outer_journey_id: outer_journey_id.into(),
        outer_profile: outer_profile.into(),
        outer_run_id: outer_run_id.into(),
        outer_journey_hash: outer_journey_hash.into(),
        outer_surface_hash: outer_surface_hash.into(),
        outer_compiler_version: outer_compiler_version.into(),
        outer_proof_hash: outer_proof_hash.into(),
        derivations: pending.derivations,
        candidate_permits,
    })
}

fn expected_candidate_permits(
    batch_hash: &str,
    outer_run_id: &str,
    outer_proof_hash: &str,
) -> Vec<DerivationCandidatePermit> {
    let mut permits = Vec::new();
    for (phase, count) in [
        (ReleasePhase::IsolatedDogfood, 1usize),
        (ReleasePhase::FreshFixpoint, 2usize),
        (ReleasePhase::GatedPreparation, 2usize),
    ] {
        for ordinal in 0..count {
            let bytes = format!(
                "{batch_hash}\0{outer_run_id}\0{outer_proof_hash}\0{}\0{ordinal}",
                phase.as_str()
            );
            permits.push(DerivationCandidatePermit {
                phase,
                ordinal,
                permit_hash: crate::journey_gate::sha256_digest(bytes.as_bytes()),
            });
        }
    }
    permits
}

fn required_outer_context(
    root: &Path,
) -> Result<(OuterJourneyAttestation, BoundDerivationAuthority)> {
    let journey_id = std::env::var(OUTER_JOURNEY_ID_ENV)
        .context("release rehearsal requires reserved outer Journey id context")?;
    let profile = std::env::var(OUTER_JOURNEY_PROFILE_ENV)
        .context("release rehearsal requires reserved outer Journey profile context")?;
    let run_id = std::env::var(OUTER_JOURNEY_RUN_ID_ENV)
        .context("release rehearsal requires reserved outer Journey run-id context")?;
    let journey_hash = std::env::var(OUTER_JOURNEY_HASH_ENV)
        .context("release rehearsal requires reserved Journey semantic hash context")?;
    let surface_hash = std::env::var(OUTER_SURFACE_HASH_ENV)
        .context("release rehearsal requires reserved surface hash context")?;
    let compiler_version = std::env::var(OUTER_COMPILER_VERSION_ENV)
        .context("release rehearsal requires reserved compiler version context")?;
    let proof_hash = std::env::var(OUTER_PROOF_HASH_ENV)
        .context("release rehearsal requires reserved compiled proof hash context")?;
    let capsule_path = PathBuf::from(
        std::env::var(OUTER_CONTEXT_CAPSULE_ENV)
            .context("release rehearsal requires a runtime-owned context capsule")?,
    );
    if journey_id != RELEASE_JOURNEY_ID || profile != RELEASE_PROFILE {
        bail!(
            "release rehearsal requires exact outer Journey '{RELEASE_JOURNEY_ID}' profile '{RELEASE_PROFILE}'"
        );
    }
    let expected_prefix = format!("{RELEASE_JOURNEY_ID}.{RELEASE_PROFILE}.");
    if !run_id.starts_with(&expected_prefix) || run_id.len() == expected_prefix.len() {
        bail!("release rehearsal received malformed reserved outer Journey run-id context");
    }
    let metadata = fs::symlink_metadata(&capsule_path)
        .with_context(|| format!("reading release context capsule {}", capsule_path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("release context capsule must be a regular non-symlink file");
    }
    let canonical_capsule = capsule_path.canonicalize()?;
    let canonical_root = root.canonicalize()?;
    let temp_root = std::env::temp_dir().canonicalize()?;
    if canonical_capsule.starts_with(&canonical_root) || !canonical_capsule.starts_with(&temp_root)
    {
        bail!("release context capsule is not in a detached runtime root");
    }
    let capsule: OuterJourneyContextCapsule =
        serde_json::from_slice(&fs::read(&canonical_capsule)?)
            .context("release context capsule is malformed")?;
    if capsule.schema != "loom.release-outer-context/v1"
        || capsule.journey_id != journey_id
        || capsule.profile != profile
        || capsule.run_id != run_id
        || capsule.journey_hash != journey_hash
        || capsule.surface_hash != surface_hash
        || capsule.compiler_version != compiler_version
        || capsule.proof_hash != proof_hash
    {
        bail!("release context capsule does not match reserved runtime context");
    }
    validate_bound_derivation_authority(root, &capsule)?;
    if compiler_version != crate::journey::JOURNEY_COMPILER_VERSION {
        bail!("release context capsule uses a stale Journey compiler version");
    }
    let store = crate::store::Store::open_read(root)?;
    let journey = store.resolve_node(RELEASE_JOURNEY_ID, Some(NodeType::Journey))?;
    if journey
        .body
        .get("semantic_hash")
        .and_then(serde_json::Value::as_str)
        != Some(journey_hash.as_str())
    {
        bail!("release context Journey semantic hash is stale for the current graph");
    }
    let current_surface = crate::journey::surface_projection_hash(&store, &journey)?
        .ok_or_else(|| anyhow!("release-workflow has no current accepted surface"))?;
    if current_surface != surface_hash {
        bail!("release context surface hash is stale for the current graph");
    }
    let outer = OuterJourneyAttestation {
        journey_id,
        profile,
        run_id,
        journey_hash,
        surface_hash,
        compiler_version,
        proof_hash,
        excluded_from_nested_execution: true,
        exclusion_reason:
            "the exact outer release-workflow/proof run executes this profile; only its nested duplicate is suppressed"
                .into(),
        context_binding_limit:
            "same-user filesystem/process isolation is not a cryptographic authority boundary"
                .into(),
    };
    Ok((outer, capsule.derivation_authority))
}

fn validate_bound_derivation_authority(
    root: &Path,
    capsule: &OuterJourneyContextCapsule,
) -> Result<()> {
    let authority = &capsule.derivation_authority;
    if authority.schema != BOUND_DERIVATION_AUTHORITY_SCHEMA
        || authority.authority != "human"
        || authority.outer_journey_id != capsule.journey_id
        || authority.outer_profile != capsule.profile
        || authority.outer_run_id != capsule.run_id
        || authority.outer_journey_hash != capsule.journey_hash
        || authority.outer_surface_hash != capsule.surface_hash
        || authority.outer_compiler_version != capsule.compiler_version
        || authority.outer_proof_hash != capsule.proof_hash
        || authority.derivations.is_empty()
        || authority.gate_token_digest.len() != 64
        || !authority
            .gate_token_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || authority.gate_binding.journey_id != capsule.journey_id
        || authority.gate_binding.profile != capsule.profile
        || authority.gate_binding.journey_hash != capsule.journey_hash
        || authority.gate_binding.surface_hash != capsule.surface_hash
        || authority.gate_binding.step_id != "authorize-derivations"
        || authority.gate_binding.subject.kind != "release_derivation_batch"
        || authority.gate_binding.subject.hash != derivation_subject_hash(&authority.derivations)?
        || derivation_batch_hash(&authority.derivations, &authority.human_decision)?
            != authority.batch_hash
        || authority.candidate_permits
            != expected_candidate_permits(
                &authority.batch_hash,
                &authority.outer_run_id,
                &authority.outer_proof_hash,
            )
    {
        bail!("release context derivation authority is missing, stale, or malformed");
    }
    let specs: BTreeMap<String, crate::journey::JourneySpec> = journey_artifacts(root)?
        .into_iter()
        .map(|path| {
            let spec = crate::journey::parse(&path)?;
            Ok((spec.id.clone(), spec))
        })
        .collect::<Result<_>>()?;
    let mut seen = BTreeSet::new();
    for subject in &authority.derivations {
        if !seen.insert(subject.journey_id.as_str())
            || subject.manifest.journey_id != subject.journey_id
            || subject.manifest.journey_hash != subject.journey_hash
            || subject.manifest.proposal_id != subject.proposal_id
        {
            bail!("release derivation authority repeats or mismatches a subject");
        }
        let spec = specs.get(&subject.journey_id).ok_or_else(|| {
            anyhow!(
                "release derivation authority targets missing Journey '{}'",
                subject.journey_id
            )
        })?;
        subject
            .manifest
            .validate_for(spec, &spec.semantic_hash()?)?;
        let canonical = canonical_json(serde_json::to_value(&subject.manifest)?);
        let observed = crate::artifact::fingerprint(&serde_json::to_string(&canonical)?);
        if observed != subject.manifest_hash {
            bail!(
                "release derivation authority manifest for '{}' changed after approval",
                subject.journey_id
            );
        }
    }
    Ok(())
}

fn claim_candidate_permit(
    authority: &BoundDerivationAuthority,
    phase: ReleasePhase,
    ordinal: usize,
) -> Result<DerivationCandidatePermit> {
    let permit = authority
        .candidate_permits
        .iter()
        .find(|permit| permit.phase == phase && permit.ordinal == ordinal)
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "release derivation authority has no permit for {} candidate {ordinal}",
                phase.as_str()
            )
        })?;
    let capsule = PathBuf::from(
        std::env::var(OUTER_CONTEXT_CAPSULE_ENV)
            .context("release candidate permit requires the outer context capsule")?,
    )
    .canonicalize()?;
    let parent = capsule
        .parent()
        .ok_or_else(|| anyhow!("release context capsule has no runtime parent"))?;
    let directory = parent.join("release-derivation-candidate-permits");
    fs::create_dir_all(&directory)?;
    let directory = directory.canonicalize()?;
    if !directory.starts_with(parent) {
        bail!("release derivation candidate permit store escaped the runtime capsule root");
    }
    let path = directory.join(&permit.permit_hash);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options.open(&path).map_err(|error| {
        if path.exists() {
            anyhow!(
                "release derivation candidate permit for {} ordinal {ordinal} has already been consumed",
                phase.as_str()
            )
        } else {
            anyhow::Error::from(error)
        }
    })?;
    use std::io::Write;
    file.write_all(permit.permit_hash.as_bytes())?;
    file.sync_all()?;
    Ok(permit)
}

fn passed_workspace(probed: bool) -> WorkspaceAttestation {
    WorkspaceAttestation {
        detached: true,
        source_excludes: SOURCE_EXCLUDES.iter().map(ToString::to_string).collect(),
        initially_empty: true,
        nonempty_probe: if probed { "rejected" } else { "not_run" }.into(),
        preinitialized_probe: if probed { "rejected" } else { "not_run" }.into(),
    }
}

fn probe_empty_workspace_policy(
    root: &Path,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<WorkspaceAttestation> {
    let nonempty = DetachedCandidate::allocate(root, "nonempty-probe")?;
    fs::write(nonempty.path().join("sentinel"), b"occupied")?;
    let nonempty_result = copy_candidate(root, nonempty.path(), ledger);
    let nonempty_rejected = nonempty_result.as_ref().is_err_and(|error| {
        validate_workspace_probe_failure("nonempty", &error.to_string()).is_ok()
    });
    ledger.push(ArgvLedgerEntry {
        source: "empty_workspace_probe:nonempty".into(),
        executable: "internal-copy-plan".into(),
        argv: vec![nonempty.path().to_string_lossy().into_owned()],
        policy: "destination_must_be_empty".into(),
        attempted: true,
        outcome: if nonempty_rejected {
            "rejected".into()
        } else {
            "unexpectedly_accepted".into()
        },
    });
    if !nonempty_rejected {
        bail!("nonempty workspace rejection probe did not observe the exact policy failure");
    }
    let initialized = DetachedCandidate::allocate(root, "initialized-probe")?;
    fs::create_dir_all(initialized.path().join(".loom"))?;
    let initialized_result = copy_candidate(root, initialized.path(), ledger);
    let initialized_rejected = initialized_result.as_ref().is_err_and(|error| {
        validate_workspace_probe_failure("preinitialized", &error.to_string()).is_ok()
    });
    ledger.push(ArgvLedgerEntry {
        source: "empty_workspace_probe:preinitialized".into(),
        executable: "internal-copy-plan".into(),
        argv: vec![initialized.path().to_string_lossy().into_owned()],
        policy: "destination_must_not_be_initialized".into(),
        attempted: true,
        outcome: if initialized_rejected {
            "rejected".into()
        } else {
            "unexpectedly_accepted".into()
        },
    });
    if !initialized_rejected {
        bail!("preinitialized workspace rejection probe did not observe the exact policy failure");
    }
    Ok(passed_workspace(true))
}

#[doc(hidden)]
pub fn validate_workspace_probe_failure(probe: &str, detail: &str) -> Result<()> {
    let required = match probe {
        "nonempty" => "must be empty",
        "preinitialized" => "preinitialized .loom",
        _ => bail!("unknown release workspace probe '{probe}'"),
    };
    if !detail.contains(required) {
        bail!("workspace probe '{probe}' observed an unrelated failure instead of '{required}'");
    }
    Ok(())
}

fn run_isolated_gate(
    root: &Path,
    candidate_permit: &DerivationCandidatePermit,
    runtime: &mut GateRuntime<'_>,
    ledger: &mut Vec<ArgvLedgerEntry>,
    observed_inventory: &mut Option<SourceInventoryAttestation>,
) -> Result<GateResult> {
    let (candidate, source_inventory) = DetachedCandidate::copy(root, "candidate", ledger)?;
    *observed_inventory = Some(source_inventory.clone());
    let candidate_hash = source_inventory.inventory_hash.clone();
    let sandbox = ProcessSandbox::create(candidate.path(), runtime.dependency_cache.path())?;
    let export = inspect_candidate_export(candidate.path())?;
    let imported_surfaces = export
        .as_ref()
        .map(imported_executable_surfaces)
        .transpose()?
        .unwrap_or_default();
    let manifests = candidate_manifest_attestations(
        candidate.path(),
        &imported_surfaces,
        runtime.outer,
        ledger,
    )?;

    run_code_gates(candidate.path(), runtime.executor, &sandbox, ledger)?;
    let binary = candidate.path().join("target/debug/loom");
    run_loom(
        candidate.path(),
        &binary,
        &["init", ".", "--name", "loom-release-rehearsal", "--json"],
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    if export.is_some() {
        run_loom(
            candidate.path(),
            &binary,
            &["import", "loom.graph.json", "--json"],
            runtime.executor,
            &sandbox,
            ledger,
        )?;
    }
    for manifest in &manifests {
        run_loom(
            candidate.path(),
            &binary,
            &[
                "journey",
                "surface-accept",
                &manifest.journey_id,
                "--manifest",
                &manifest.path,
                "--json",
            ],
            runtime.executor,
            &sandbox,
            ledger,
        )?;
    }
    seed_candidate_graph(
        candidate.path(),
        &binary,
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    replay_derivation_authority(
        candidate.path(),
        &binary,
        runtime.derivation_authority,
        candidate_permit,
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    run_candidate_journeys(
        candidate.path(),
        &binary,
        runtime.outer,
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    run_loom(
        candidate.path(),
        &binary,
        &["doctor", "--json"],
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    let coverage = run_loom(
        candidate.path(),
        &binary,
        &["coverage", "--json"],
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    require_clean_coverage(&coverage.stdout)?;
    run_loom(
        candidate.path(),
        &binary,
        &["export", "--json"],
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    run_loom(
        candidate.path(),
        &binary,
        &["export", "--check", "--json"],
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    let drift = run_loom(
        candidate.path(),
        &binary,
        &["journey", "drift", "--json"],
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    require_clean_drift_excusing_gates(&drift.stdout, &human_gated_journeys(candidate.path())?)?;

    let result_hash = semantic_result_hash(candidate.path(), &manifests, runtime.outer)?;
    Ok(GateResult {
        candidate_hash,
        result_hash,
        imported_surfaces_quarantined: imported_surfaces.len(),
        manifests_reauthorized: manifests,
        source_inventory,
    })
}

fn inspect_candidate_export(root: &Path) -> Result<Option<crate::travel::Export>> {
    let path = root.join(crate::GRAPH_EXPORT);
    if !path.exists() {
        return Ok(None);
    }
    crate::travel::read_export(&path).map(Some).with_context(|| {
        "release rehearsal refuses legacy or malformed exports; only an exact v12 export may enter the detached verifier"
    })
}

fn imported_executable_surfaces(export: &crate::travel::Export) -> Result<Vec<Node>> {
    let mut snapshot = export.clone().into_snapshot();
    crate::travel::quarantine_imported_execution(&mut snapshot)?;
    let mut surfaces: Vec<Node> = snapshot
        .nodes
        .into_iter()
        .filter(|node| node.node_type == NodeType::InterfaceSurface && node.status == "quarantined")
        .collect();
    surfaces.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(surfaces)
}

fn candidate_manifest_attestations(
    root: &Path,
    imported_surfaces: &[Node],
    outer: &OuterJourneyAttestation,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<Vec<ManifestAttestation>> {
    if imported_surfaces.is_empty() {
        return Ok(Vec::new());
    }
    let manifest_root = root.join(SURFACE_MANIFEST_ROOT);
    if !manifest_root.is_dir() {
        bail!(
            "imported executable surfaces remain quarantined: candidate-owned canonical manifests are missing at '{SURFACE_MANIFEST_ROOT}'"
        );
    }
    let mut paths = regular_files_with_suffix(&manifest_root, ".surface.json")?;
    paths.sort();
    let imported: BTreeMap<&str, &Node> = imported_surfaces
        .iter()
        .map(|surface| (surface.name.as_str(), surface))
        .collect();
    let mut matched = BTreeSet::new();
    let mut attestations = Vec::new();
    for path in paths {
        let manifest = SurfaceManifest::parse_json(&path)?;
        let Some(surface) = imported.get(manifest.surface.id.as_str()) else {
            bail!(
                "candidate manifest '{}' does not reauthorize an imported quarantined surface",
                path.display()
            );
        };
        if surface.body != manifest.surface.node_body()? {
            bail!(
                "candidate manifest '{}' does not exactly match imported surface '{}'",
                path.display(),
                surface.name
            );
        }
        if !matched.insert(surface.id.clone()) {
            bail!("candidate repeats manifest for surface '{}'", surface.name);
        }
        let spec_path = journey_artifacts(root)?
            .into_iter()
            .find_map(|candidate| {
                crate::journey::parse(&candidate)
                    .ok()
                    .filter(|spec| spec.id == manifest.journey_id)
                    .map(|spec| (candidate, spec))
            })
            .ok_or_else(|| {
                anyhow!(
                    "candidate manifest '{}' has no authored Journey source for '{}'",
                    path.display(),
                    manifest.journey_id
                )
            })?;
        let (_, spec) = spec_path;
        let semantic_hash = spec.semantic_hash()?;
        manifest.validate_for(&spec, &semantic_hash)?;
        inspect_candidate_manifest_operations(&spec, &manifest, outer, ledger)?;
        let codefile_path = root.join(&manifest.surface.codefile);
        let metadata = fs::symlink_metadata(&codefile_path).with_context(|| {
            format!(
                "candidate manifest '{}' exposes missing CodeFile '{}'",
                path.display(),
                manifest.surface.codefile
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!(
                "candidate manifest '{}' exposes non-regular CodeFile '{}'",
                path.display(),
                manifest.surface.codefile
            );
        }
        let relative = path
            .strip_prefix(root)
            .context("candidate manifest escaped detached root")?;
        let canonical = canonical_json(serde_json::to_value(&manifest)?);
        attestations.push(ManifestAttestation {
            journey_id: manifest.journey_id,
            surface_id: manifest.surface.id,
            path: relative.to_string_lossy().into_owned(),
            manifest_hash: crate::artifact::fingerprint(&serde_json::to_string(&canonical)?),
            codefile: manifest.surface.codefile,
            locator: manifest.surface.locator,
        });
    }
    let missing: Vec<&str> = imported_surfaces
        .iter()
        .filter(|surface| !matched.contains(&surface.id))
        .map(|surface| surface.name.as_str())
        .collect();
    if !missing.is_empty() {
        bail!(
            "imported executable surfaces remain quarantined without candidate manifests: {}",
            missing.join(", ")
        );
    }
    attestations.sort_by(|left, right| left.surface_id.cmp(&right.surface_id));
    Ok(attestations)
}

#[doc(hidden)]
pub fn inspect_candidate_manifest_operations(
    spec: &crate::journey::JourneySpec,
    manifest: &SurfaceManifest,
    outer: &OuterJourneyAttestation,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<()> {
    let plan = match crate::candidate_surface_policy::inspect_manifest(
        spec,
        manifest,
        crate::candidate_surface_policy::PolicyMode::DetachedReleaseInspection {
            outer_journey_id: &outer.journey_id,
            outer_surface_id: "loom-release-rehearsal",
        },
    ) {
        Ok(plan) => plan,
        Err(error) => {
            let operation = manifest.surface.operations.first();
            ledger.push(ArgvLedgerEntry {
                source: format!("candidate_manifest:{}:policy", manifest.surface.id),
                executable: operation
                    .and_then(|operation| operation.argv.first())
                    .cloned()
                    .unwrap_or_default(),
                argv: operation
                    .and_then(|operation| operation.argv.get(1..))
                    .unwrap_or_default()
                    .to_vec(),
                policy: "candidate-surface/v1".into(),
                attempted: false,
                outcome: "rejected_candidate_surface_policy".into(),
            });
            return Err(error);
        }
    };
    let operations: BTreeMap<&str, &crate::journey::CliOperation> = manifest
        .surface
        .operations
        .iter()
        .map(|operation| (operation.id.as_str(), operation))
        .collect();
    for inspection in plan.inspections() {
        let operation = operations
            .get(inspection.operation_id.as_str())
            .expect("policy inspected one declared operation");
        ledger.push(ArgvLedgerEntry {
            source: format!(
                "candidate_manifest:{}:{}",
                manifest.surface.id, inspection.operation_id
            ),
            executable: operation.argv.first().cloned().unwrap_or_default(),
            argv: operation.argv.get(1..).unwrap_or_default().to_vec(),
            policy: if inspection.outcome == "suppressed_exact_outer" {
                "outer_profile_compile_only".into()
            } else {
                plan.policy_version().into()
            },
            attempted: false,
            outcome: inspection.outcome.clone(),
        });
        for nested in &inspection.nested {
            ledger.push(ArgvLedgerEntry {
                source: format!(
                    "candidate_manifest:{}:{}/{}",
                    manifest.surface.id, inspection.operation_id, nested.source
                ),
                executable: String::new(),
                argv: Vec::new(),
                policy: plan.policy_version().into(),
                attempted: false,
                outcome: nested.outcome.clone(),
            });
        }
    }
    Ok(())
}

fn run_code_gates(
    root: &Path,
    executor: &mut dyn ReleaseExecutor,
    sandbox: &ProcessSandbox,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<()> {
    for args in [
        vec!["fmt", "--all", "--", "--check"],
        vec![
            "clippy",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
        vec!["test", "--all-targets", "--quiet"],
        vec!["build", "--quiet"],
    ] {
        let args: Vec<String> = args.into_iter().map(ToString::to_string).collect();
        execute_checked(
            executor,
            sandbox,
            root,
            Path::new("cargo"),
            &args,
            "code_gate",
            ledger,
        )
        .with_context(|| format!("release code gate `cargo {}` failed", args.join(" ")))?;
    }
    Ok(())
}

fn seed_candidate_graph(
    root: &Path,
    binary: &Path,
    executor: &mut dyn ReleaseExecutor,
    sandbox: &ProcessSandbox,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<()> {
    const TEST_IGNORE_REASON: &str = "Tests are Validation/proof artifacts, not implementation ownership; literal test paths may be re-registered when an Exercises edge needs source-drift tracking.";
    let ignores = run_loom(
        root,
        binary,
        &["ignore", "list", "--json"],
        executor,
        sandbox,
        ledger,
    )?;
    let ignore_rows: serde_json::Value = serde_json::from_slice(&ignores.stdout)
        .context("release candidate ignore list is not JSON")?;
    let matching: Vec<&serde_json::Value> = ignore_rows
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["glob"] == "tests/**")
        .collect();
    match matching.as_slice() {
        [] => {
            run_loom(
                root,
                binary,
                &[
                    "ignore",
                    "add",
                    "tests/**",
                    "--reason",
                    TEST_IGNORE_REASON,
                    "--json",
                ],
                executor,
                sandbox,
                ledger,
            )?;
        }
        [row] if row["reason"] == TEST_IGNORE_REASON => {}
        _ => bail!("release candidate has conflicting tests/** coverage policy"),
    }
    run_loom(
        root,
        binary,
        &["codefile", "add", "src/**/*.rs", "--json"],
        executor,
        sandbox,
        ledger,
    )?;
    run_loom(
        root,
        binary,
        &["codefile", "add", "scripts/*.sh", "--json"],
        executor,
        sandbox,
        ledger,
    )?;
    run_loom(root, binary, &["sync", "--json"], executor, sandbox, ledger)?;
    for path in journey_artifacts(root)? {
        let relative = path
            .strip_prefix(root)
            .context("Journey artifact escaped detached candidate")?;
        run_loom(
            root,
            binary,
            &["journey", "add", &relative.to_string_lossy(), "--json"],
            executor,
            sandbox,
            ledger,
        )?;
    }
    Ok(())
}

fn replay_derivation_authority(
    root: &Path,
    binary: &Path,
    authority: &BoundDerivationAuthority,
    permit: &DerivationCandidatePermit,
    executor: &mut dyn ReleaseExecutor,
    sandbox: &ProcessSandbox,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<()> {
    if !authority.candidate_permits.contains(permit) {
        bail!("detached candidate received an unbound derivation authority permit");
    }
    let manifest_root = root
        .join(".release-sandbox")
        .join("derivation-authority")
        .join(&permit.permit_hash);
    fs::create_dir_all(&manifest_root)?;
    let manifest_root = manifest_root.canonicalize()?;
    if !manifest_root.starts_with(root.join(".release-sandbox").canonicalize()?) {
        bail!("candidate derivation manifests escaped the detached sandbox");
    }
    let decision = match &authority.human_decision {
        crate::ratification::HumanDecision::Mediated { response } => response.as_str(),
        crate::ratification::HumanDecision::Direct { .. } => {
            bail!("release derivation replay requires host-mediated human authority")
        }
    };
    let specs: BTreeMap<String, crate::journey::JourneySpec> = journey_artifacts(root)?
        .into_iter()
        .map(|path| {
            let spec = crate::journey::parse(&path)?;
            Ok((spec.id.clone(), spec))
        })
        .collect::<Result<_>>()?;
    let mut builder_sandbox = sandbox.clone();
    builder_sandbox
        .environment
        .insert(crate::identity::AGENT_ENV.into(), "llm:builder".into());
    builder_sandbox.environment.insert(
        crate::identity::PROFILE_ENV.into(),
        "release-rehearsal".into(),
    );
    for subject in &authority.derivations {
        let spec = specs.get(&subject.journey_id).ok_or_else(|| {
            anyhow!(
                "approved derivation Journey '{}' is missing from the candidate",
                subject.journey_id
            )
        })?;
        let semantic_hash = spec.semantic_hash()?;
        subject.manifest.validate_for(spec, &semantic_hash)?;
        if semantic_hash != subject.journey_hash
            || subject.manifest.proposal_id != subject.proposal_id
        {
            bail!(
                "approved derivation for '{}' is stale in the candidate",
                subject.journey_id
            );
        }
        let canonical = canonical_json(serde_json::to_value(&subject.manifest)?);
        let canonical_text = serde_json::to_string(&canonical)?;
        if crate::artifact::fingerprint(&canonical_text) != subject.manifest_hash {
            bail!(
                "approved derivation manifest for '{}' changed before replay",
                subject.journey_id
            );
        }
        let path = manifest_root.join(format!("{}.json", subject.journey_id));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = options.open(&path)?;
        use std::io::Write;
        file.write_all(canonical_text.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        let relative = path
            .strip_prefix(root)
            .context("candidate derivation manifest escaped detached root")?
            .to_string_lossy()
            .into_owned();
        run_loom(
            root,
            binary,
            &[
                "journey",
                "derive-accept",
                &subject.journey_id,
                "--manifest",
                &relative,
                "--human-decision",
                decision,
                "--json",
            ],
            executor,
            &builder_sandbox,
            ledger,
        )?;
    }
    // The import boundary deliberately voids ratification authority, and the
    // derive-accepts above re-establish it only for derivation subjects. The
    // remaining domain intents would read needs_reconfirmation forever inside
    // the candidate, so the audit-clean discrimination journeys could never
    // measure the state they claim. Re-ratify them under the same sealed
    // human decision that authorized this rehearsal; `intent ratify --all`
    // seals its own batch envelope before writing.
    run_loom(
        root,
        binary,
        &[
            "intent",
            "ratify",
            "--all",
            "--evidence",
            "release rehearsal: re-establish domain-intent authority voided by candidate import, under the sealed rehearsal decision",
            "--human-decision",
            decision,
            "--json",
        ],
        executor,
        &builder_sandbox,
        ledger,
    )?;
    Ok(())
}

fn run_candidate_journeys(
    root: &Path,
    binary: &Path,
    outer: &OuterJourneyAttestation,
    executor: &mut dyn ReleaseExecutor,
    sandbox: &ProcessSandbox,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<()> {
    let mut excluded = 0usize;
    for path in journey_artifacts(root)? {
        let spec = crate::journey::parse(&path)?;
        for profile in spec.profiles.keys() {
            if spec.id == outer.journey_id && profile == &outer.profile {
                excluded += 1;
                let observed = run_loom(
                    root,
                    binary,
                    &[
                        "journey",
                        "compile",
                        &spec.id,
                        "--profile",
                        profile,
                        "--json",
                    ],
                    executor,
                    sandbox,
                    ledger,
                )?;
                require_outer_compile_report(&observed.stdout, outer)?;
                continue;
            }
            let observed = run_loom(
                root,
                binary,
                &["journey", "run", &spec.id, "--profile", profile, "--json"],
                executor,
                sandbox,
                ledger,
            )?;
            if let Some(pending) = pending_human_gate(&observed.stdout) {
                require_declared_human_gate(root, &spec.id, profile, &pending)?;
                continue;
            }
            require_passed_journey_report_with_sandbox(
                &observed.stdout,
                &spec.id,
                profile,
                spec.steps.len(),
                Some(sandbox),
            )?;
        }
    }
    if excluded != 1 {
        bail!(
            "nested verifier must exclude exactly one outer release-workflow/proof profile (found {excluded})"
        );
    }
    Ok(())
}

fn require_outer_compile_report(bytes: &[u8], outer: &OuterJourneyAttestation) -> Result<()> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).context("outer Journey compile output is not JSON")?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("outer Journey compile output is not an object"))?;
    let exact = object.get("compiled").and_then(serde_json::Value::as_bool) == Some(true)
        && object.get("journey_id").and_then(serde_json::Value::as_str)
            == Some(outer.journey_id.as_str())
        && object.get("profile").and_then(serde_json::Value::as_str)
            == Some(outer.profile.as_str())
        && object
            .get("journey_hash")
            .and_then(serde_json::Value::as_str)
            == Some(outer.journey_hash.as_str())
        && object
            .get("surface_hash")
            .and_then(serde_json::Value::as_str)
            == Some(outer.surface_hash.as_str())
        && object
            .get("compiler_version")
            .and_then(serde_json::Value::as_str)
            == Some(outer.compiler_version.as_str());
    if !exact {
        bail!("outer release-workflow compile attestation is incomplete or stale");
    }
    Ok(())
}

#[doc(hidden)]
pub fn require_passed_journey_report(
    bytes: &[u8],
    journey_id: &str,
    profile: &str,
    expected_steps: usize,
) -> Result<()> {
    require_passed_journey_report_with_sandbox(bytes, journey_id, profile, expected_steps, None)
}

/// A headless candidate cannot answer a host-mediated gate: missing human
/// authority is a pause, not a failure. A suspended run is acceptable in the
/// isolated dogfood only when the journey's canonical candidate manifest
/// genuinely declares that gate.
fn pending_human_gate(bytes: &[u8]) -> Option<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    (value.get("schema").and_then(serde_json::Value::as_str)
        == Some(crate::journey_gate::PENDING_HUMAN_SCHEMA)
        && value.get("status").and_then(serde_json::Value::as_str) == Some("pending_human"))
    .then_some(value)
}

fn require_declared_human_gate(
    root: &Path,
    journey_id: &str,
    profile: &str,
    pending: &serde_json::Value,
) -> Result<()> {
    let bound_journey = pending
        .pointer("/binding/journey_id")
        .and_then(serde_json::Value::as_str);
    let bound_profile = pending
        .pointer("/binding/profile")
        .and_then(serde_json::Value::as_str);
    if bound_journey != Some(journey_id) || bound_profile != Some(profile) {
        bail!(
            "pending Journey gate binding '{}':'{}' does not match the dogfood run '{journey_id}':'{profile}'",
            bound_journey.unwrap_or("?"),
            bound_profile.unwrap_or("?")
        );
    }
    let manifest_path = root
        .join(SURFACE_MANIFEST_ROOT)
        .join(format!("{journey_id}.surface.json"));
    let manifest = crate::journey::SurfaceManifest::parse_json(&manifest_path)?;
    if !manifest
        .bindings
        .iter()
        .any(|binding| matches!(binding, crate::journey::SurfaceBinding::HumanDecision(_)))
    {
        bail!(
            "Journey '{journey_id}' suspended at a human gate its canonical manifest never declares"
        );
    }
    Ok(())
}

fn require_passed_journey_report_with_sandbox(
    bytes: &[u8],
    journey_id: &str,
    profile: &str,
    expected_steps: usize,
    sandbox: Option<&ProcessSandbox>,
) -> Result<()> {
    let report: crate::journey_runtime::RuntimeReport = serde_json::from_slice(bytes)
        .context("Journey run output is not one complete runtime report")?;
    if report.journey_id != journey_id
        || report.profile != profile
        || report.status != crate::journey_runtime::RuntimeStatus::Passed
        || report.assertions_failed != 0
        || report.steps.len() != expected_steps
        || report
            .steps
            .iter()
            .any(|step| step.exit_code != 0 || step.assertions_failed != 0)
    {
        let failing_steps: Vec<_> = report
            .steps
            .iter()
            .filter(|step| step.exit_code != 0 || step.assertions_failed != 0)
            .take(8)
            .map(|step| {
                serde_json::json!({
                    "step_id": bounded_diagnostic_text(&step.step_id, 256),
                    "operation_id": bounded_diagnostic_text(&step.operation_id, 256),
                    "exit_code": step.exit_code,
                    "assertions_passed": step.assertions_passed,
                    "assertions_failed": step.assertions_failed,
                })
            })
            .collect();
        let detail = report.detail.as_deref().map(|detail| match sandbox {
            Some(sandbox) => release_diagnostic_stream(detail.as_bytes(), sandbox),
            None => bounded_diagnostic_text(detail, RELEASE_DIAGNOSTIC_BYTES),
        });
        let diagnostic = serde_json::json!({
            "expected": {
                "journey_id": journey_id,
                "profile": profile,
                "status": "passed",
                "assertions_failed": 0,
                "step_count": expected_steps,
            },
            "observed": {
                "journey_id": bounded_diagnostic_text(&report.journey_id, 256),
                "profile": bounded_diagnostic_text(&report.profile, 256),
                "status": report.status.as_str(),
                "assertions_passed": report.assertions_passed,
                "assertions_failed": report.assertions_failed,
                "step_count": report.steps.len(),
                "detail": detail,
                "failing_steps": failing_steps,
                "failing_steps_omitted": report.steps.iter()
                    .filter(|step| step.exit_code != 0 || step.assertions_failed != 0)
                    .count().saturating_sub(8),
            }
        });
        bail!(
            "Journey '{journey_id}:{profile}' did not return one complete passing report: {}",
            bounded_diagnostic_text(
                &serde_json::to_string(&diagnostic)?,
                RELEASE_DIAGNOSTIC_BYTES
            )
        );
    }
    Ok(())
}

#[doc(hidden)]
pub fn require_clean_coverage(bytes: &[u8]) -> Result<()> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).context("release candidate coverage output is not JSON")?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("release candidate coverage output is not an object"))?;
    let planned = object
        .get("intents")
        .and_then(|value| value.get("planned_or_needs_change"))
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("coverage is missing intents.planned_or_needs_change"))?;
    let ungrounded = object
        .get("grounding")
        .and_then(|value| value.get("ungrounded"))
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("coverage is missing grounding.ungrounded"))?;
    let codefiles = object
        .get("codefiles")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow!("coverage is missing codefiles"))?;
    let registered = codefiles
        .get("registered")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("coverage is missing codefiles.registered"))?;
    let owned = codefiles
        .get("owned")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("coverage is missing codefiles.owned"))?;
    let _observed = codefiles
        .get("observed")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("coverage is missing codefiles.observed"))?;
    let unowned = codefiles
        .get("unowned")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("coverage is missing codefiles.unowned"))?;
    if planned != 0
        || ungrounded != 0
        || unowned != 0
        || registered != owned.saturating_add(unowned)
    {
        bail!(
            "release candidate coverage is blocking: planned={planned}, ungrounded={ungrounded}, unowned={unowned}"
        );
    }
    Ok(())
}

#[doc(hidden)]
pub fn require_clean_drift(bytes: &[u8]) -> Result<()> {
    require_clean_drift_excusing_gates(bytes, &BTreeSet::new())
}

/// A journey suspended at a declared host-mediated gate never settles its
/// compiled projection inside a headless candidate — that pause is authority
/// hygiene, not drift. Every other stale projection still blocks, and the
/// reported stale count must be exactly the excused pauses.
#[doc(hidden)]
pub fn require_clean_drift_excusing_gates(
    bytes: &[u8],
    human_gated: &BTreeSet<String>,
) -> Result<()> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .context("release candidate Journey drift output is not JSON")?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("release candidate Journey drift output is not an object"))?;
    let rows = object
        .get("journeys")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow!("release candidate Journey drift journeys is not an array"))?;
    let stale = object
        .get("stale")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("release candidate Journey drift is missing stale count"))?;
    let mut excused = 0u64;
    for (index, row) in rows.iter().enumerate() {
        let current = row
            .as_object()
            .and_then(|object| object.get("current"))
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| anyhow!("Journey drift row {index} is missing boolean current"))?;
        if current {
            continue;
        }
        let journey_id = row
            .as_object()
            .and_then(|object| object.get("journey_id"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("Journey drift row {index} is missing journey_id"))?;
        if human_gated.contains(journey_id) {
            excused += 1;
            continue;
        }
        bail!("release candidate retains stale Journey projections");
    }
    if stale != excused {
        bail!(
            "release candidate reports {stale} stale Journey projection(s) beyond {excused} declared human-gate pause(s)"
        );
    }
    Ok(())
}

/// Journey ids whose canonical candidate manifests declare a host-mediated
/// human-decision binding.
fn human_gated_journeys(root: &Path) -> Result<BTreeSet<String>> {
    let mut gated = BTreeSet::new();
    let manifest_root = root.join(SURFACE_MANIFEST_ROOT);
    if !manifest_root.is_dir() {
        return Ok(gated);
    }
    for entry in fs::read_dir(&manifest_root)? {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let Ok(manifest) = SurfaceManifest::parse_json(&path) else {
            continue;
        };
        if manifest
            .bindings
            .iter()
            .any(|binding| matches!(binding, crate::journey::SurfaceBinding::HumanDecision(_)))
        {
            gated.insert(manifest.journey_id.clone());
        }
    }
    Ok(gated)
}

fn semantic_result_hash(
    root: &Path,
    manifests: &[ManifestAttestation],
    outer: &OuterJourneyAttestation,
) -> Result<String> {
    let value = serde_json::json!({
        "candidate_hash": hash_tree(root, &RESULT_EXCLUDES)?,
        "manifests": manifests,
        "outer_journey": outer.journey_id,
        "outer_profile": outer.profile,
        "schema_version": crate::SCHEMA_VERSION,
    });
    Ok(crate::artifact::fingerprint(&serde_json::to_string(
        &value,
    )?))
}

fn journey_artifacts(root: &Path) -> Result<Vec<PathBuf>> {
    let journey_root = root.join("journeys");
    let mut paths = Vec::new();
    collect_regular_files(&journey_root, &mut paths, &|path| {
        matches!(
            path.extension().and_then(OsStr::to_str),
            Some("yaml" | "yml" | "json")
        ) && !path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.ends_with(".surface.json"))
    })?;
    paths.sort();
    Ok(paths)
}

fn regular_files_with_suffix(root: &Path, suffix: &str) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    collect_regular_files(root, &mut paths, &|path| {
        path.file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.ends_with(suffix))
    })?;
    Ok(paths)
}

fn collect_regular_files(
    root: &Path,
    out: &mut Vec<PathBuf>,
    keep: &impl Fn(&Path) -> bool,
) -> Result<()> {
    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() {
        bail!(
            "release verifier refuses symlinked path '{}'",
            root.display()
        );
    }
    let mut entries: Vec<_> = fs::read_dir(root)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            bail!(
                "release verifier refuses symlinked path '{}'",
                path.display()
            );
        }
        if metadata.is_dir() {
            collect_regular_files(&path, out, keep)?;
        } else if metadata.is_file() && keep(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn run_loom(
    root: &Path,
    binary: &Path,
    args: &[&str],
    executor: &mut dyn ReleaseExecutor,
    sandbox: &ProcessSandbox,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<CommandObservation> {
    let mut full = vec!["--graph".into(), ".".into()];
    full.extend(args.iter().map(|arg| (*arg).to_string()));
    execute_checked(
        executor,
        sandbox,
        root,
        binary,
        &full,
        "candidate_loom",
        ledger,
    )
}

struct SystemReleaseExecutor;

impl ReleaseExecutor for SystemReleaseExecutor {
    fn execute(
        &mut self,
        cwd: &Path,
        executable: &Path,
        argv: &[String],
        environment: &BTreeMap<String, String>,
    ) -> Result<CommandObservation> {
        let output = Command::new(executable)
            .args(argv)
            .current_dir(cwd)
            .env_clear()
            .envs(environment)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| format!("starting {}", executable.display()))?;
        Ok(CommandObservation {
            success: output.status.success(),
            exit_code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

struct DependencyCacheGuard {
    cargo_home: PathBuf,
    provenance: String,
    before_hash: String,
}

impl DependencyCacheGuard {
    fn open(candidate_root: &Path) -> Result<Self> {
        let (configured, provenance) = if let Some(path) = std::env::var_os(RELEASE_CARGO_HOME_ENV)
        {
            (PathBuf::from(path), RELEASE_CARGO_HOME_ENV.to_string())
        } else if let Some(path) = std::env::var_os("CARGO_HOME") {
            (PathBuf::from(path), "CARGO_HOME".into())
        } else if let Some(home) = std::env::var_os("HOME") {
            (PathBuf::from(home).join(".cargo"), "HOME/.cargo".into())
        } else {
            bail!("release rehearsal has no explicit existing Cargo dependency cache");
        };
        let cargo_home = configured.canonicalize().with_context(|| {
            format!(
                "canonicalizing release dependency cache {}",
                configured.display()
            )
        })?;
        if cargo_home.starts_with(candidate_root.canonicalize()?) {
            bail!("release dependency cache must be outside the candidate repository");
        }
        for relative in ["registry/cache", "registry/index", "registry/src"] {
            let path = cargo_home.join(relative);
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("release dependency cache is missing '{relative}'"))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                bail!("release dependency cache '{relative}' must be a real directory");
            }
        }
        let before_hash = dependency_cache_hash(&cargo_home)?;
        Ok(Self {
            cargo_home,
            provenance,
            before_hash,
        })
    }

    fn path(&self) -> &Path {
        &self.cargo_home
    }

    fn attest(&self) -> Result<DependencyCacheAttestation> {
        let after_hash = dependency_cache_hash(&self.cargo_home)?;
        Ok(DependencyCacheAttestation {
            strategy: "existing_cargo_home_read_only_verified".into(),
            cargo_home: self.cargo_home.to_string_lossy().into_owned(),
            provenance: self.provenance.clone(),
            before_hash: self.before_hash.clone(),
            unchanged: self.before_hash == after_hash,
            after_hash,
            offline: true,
        })
    }
}

fn dependency_cache_hash(cargo_home: &Path) -> Result<String> {
    let mut projection = BTreeMap::new();
    for relative in ["registry/cache", "registry/index", "registry/src", "git"] {
        let path = cargo_home.join(relative);
        projection.insert(
            relative,
            if path.exists() {
                hash_tree(&path, &[".package-cache", ".package-cache-mut"])?
            } else {
                "absent".into()
            },
        );
    }
    Ok(fingerprint_bytes(&serde_json::to_vec(&projection)?))
}

/// Bounded production-adapter smoke: check this checkout's host library target
/// against its lockfile in offline mode with the same isolated process
/// environment used by a rehearsal. Build output stays in the detached temp;
/// it does not run a Journey or write a release artifact.
#[doc(hidden)]
pub fn dependency_cache_smoke(root: &Path) -> Result<DependencyCacheAttestation> {
    let root = root.canonicalize()?;
    let dependency_cache = DependencyCacheGuard::open(&root)?;
    let temp = DetachedCandidate::allocate(&root, "dependency-cache-smoke")?;
    let sandbox = ProcessSandbox::create(temp.path(), dependency_cache.path())?;
    let mut executor = SystemReleaseExecutor;
    let mut ledger = Vec::new();
    let argv = ["check", "--locked", "--offline", "--lib"]
        .into_iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    execute_checked(
        &mut executor,
        &sandbox,
        &root,
        Path::new("cargo"),
        &argv,
        "dependency_cache_smoke",
        &mut ledger,
    )?;
    let attestation = dependency_cache.attest()?;
    if !attestation.unchanged {
        bail!("cargo metadata smoke changed the explicit dependency cache");
    }
    Ok(attestation)
}

#[derive(Clone)]
struct ProcessSandbox {
    environment: BTreeMap<String, String>,
    _external_temp: Arc<ExternalProcessTemp>,
}

impl ProcessSandbox {
    fn create(candidate: &Path, cargo_home: &Path) -> Result<Self> {
        let root = candidate.join(".release-sandbox");
        let home = root.join("home");
        let external_temp = Arc::new(ExternalProcessTemp::allocate(candidate)?);
        let temp = external_temp.path();
        fs::create_dir_all(&home)?;
        let git_config = root.join("gitconfig");
        fs::write(
            &git_config,
            b"[credential]\n\thelper =\n[commit]\n\tgpgsign = false\n",
        )?;
        let mut environment = BTreeMap::new();
        for key in [
            "PATH",
            "RUSTUP_HOME",
            "SYSTEMROOT",
            "WINDIR",
            "PATHEXT",
            "COMSPEC",
        ] {
            if let Ok(value) = std::env::var(key) {
                environment.insert(key.into(), value);
            }
        }
        environment.insert("HOME".into(), home.to_string_lossy().into_owned());
        environment.insert(
            "CARGO_HOME".into(),
            cargo_home.to_string_lossy().into_owned(),
        );
        environment.insert("TMPDIR".into(), temp.to_string_lossy().into_owned());
        environment.insert("TEMP".into(), temp.to_string_lossy().into_owned());
        environment.insert("TMP".into(), temp.to_string_lossy().into_owned());
        environment.insert("CARGO_NET_OFFLINE".into(), "true".into());
        environment.insert(
            "CARGO_TARGET_DIR".into(),
            candidate.join("target").to_string_lossy().into_owned(),
        );
        environment.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
        environment.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
        environment.insert(
            "GIT_CONFIG_GLOBAL".into(),
            git_config.to_string_lossy().into_owned(),
        );
        environment.insert("GIT_CONFIG_SYSTEM".into(), "/dev/null".into());
        Ok(Self {
            environment,
            _external_temp: external_temp,
        })
    }
}

struct ExternalProcessTemp(PathBuf);

impl ExternalProcessTemp {
    fn allocate(candidate: &Path) -> Result<Self> {
        let candidate = candidate.canonicalize()?;
        let parent = candidate
            .parent()
            .ok_or_else(|| anyhow!("detached release candidate has no parent"))?
            .canonicalize()?;
        if parent.starts_with(&candidate) {
            bail!("release process temp parent must be outside the candidate");
        }
        for sequence in 0..1000_u32 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default();
            let path = parent.join(format!(
                "loom-release-process-temp-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        bail!("could not allocate detached release process temp root")
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ExternalProcessTemp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn execute_checked(
    executor: &mut dyn ReleaseExecutor,
    sandbox: &ProcessSandbox,
    cwd: &Path,
    executable: &Path,
    argv: &[String],
    source: &str,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<CommandObservation> {
    let policy = inspect_process_argv(executable, argv)?;
    let index = ledger.len();
    ledger.push(ArgvLedgerEntry {
        source: source.into(),
        executable: executable.to_string_lossy().into_owned(),
        argv: argv.to_vec(),
        policy: policy.into(),
        attempted: true,
        outcome: "started".into(),
    });
    let observed = executor.execute(cwd, executable, argv, &sandbox.environment)?;
    ledger[index].outcome = if observed.success {
        "passed".into()
    } else {
        format!("failed_exit_{}", observed.exit_code)
    };
    if !observed.success {
        let stdout = release_diagnostic_stream(&observed.stdout, sandbox);
        let stderr = release_diagnostic_stream(&observed.stderr, sandbox);
        bail!(
            "{} {} exited {}\nstdout:\n{}\nstderr:\n{}",
            executable.display(),
            argv.join(" "),
            observed.exit_code,
            stdout,
            stderr,
        );
    }
    Ok(observed)
}

fn release_diagnostic_stream(bytes: &[u8], sandbox: &ProcessSandbox) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    for name in [
        "PATH",
        "HOME",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "TMPDIR",
        "TEMP",
        "TMP",
        "CARGO_TARGET_DIR",
        "GIT_CONFIG_GLOBAL",
    ] {
        if let Some(secret) = sandbox
            .environment
            .get(name)
            .filter(|value| value.len() >= 4)
        {
            text = text.replace(secret, "[REDACTED]");
        }
    }
    bounded_diagnostic_text(&text, RELEASE_DIAGNOSTIC_BYTES)
}

fn bounded_diagnostic_text(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.trim().to_string();
    }
    let half = limit / 2;
    let mut head = half;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - half;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!(
        "{}\n...[diagnostic output omitted]...\n{}",
        text[..head].trim_end(),
        text[tail..].trim_start()
    )
}

fn inspect_process_argv(executable: &Path, argv: &[String]) -> Result<&'static str> {
    let name = executable
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(
        name.as_str(),
        "sh" | "bash" | "zsh" | "fish" | "env" | "python" | "python3" | "perl" | "ruby" | "node"
    ) {
        bail!("release verifier refuses shell/interpreter executable '{name}'");
    }
    if argv.iter().any(|part| part.contains('\0')) {
        bail!("release verifier refuses argv containing NUL");
    }
    if name == "cargo" {
        let subcommand = argv.first().map(String::as_str).unwrap_or("");
        if matches!(
            subcommand,
            "install" | "publish" | "package" | "login" | "owner"
        ) {
            bail!("release rehearsal refuses cargo mutation '{subcommand}'");
        }
        if !matches!(subcommand, "fmt" | "clippy" | "test" | "build" | "check") {
            bail!("release rehearsal refuses unapproved cargo command '{subcommand}'");
        }
        return Ok("candidate_code_gate");
    }
    if name == "git" {
        let subcommand = argv.first().map(String::as_str).unwrap_or("");
        if !matches!(subcommand, "-C" | "ls-files") {
            bail!("release rehearsal refuses Git command '{subcommand}'");
        }
        return Ok("candidate_file_plan");
    }
    if name == "loom" || name.ends_with("loom.exe") || name == "target/debug/loom" {
        if argv
            .windows(2)
            .any(|parts| parts == ["release", "rehearse"])
        {
            bail!("release verifier refuses nested release rehearsal execution");
        }
        return Ok("candidate_loom_gate");
    }
    if executable.to_string_lossy().ends_with("target/debug/loom") {
        return Ok("candidate_loom_gate");
    }
    bail!(
        "release verifier refuses unapproved executable '{}'",
        executable.display()
    )
}

struct DetachedCandidate(PathBuf);

impl DetachedCandidate {
    fn allocate(live_root: &Path, label: &str) -> Result<Self> {
        let parent = std::env::temp_dir().canonicalize()?;
        let live_root = live_root.canonicalize()?;
        if parent == live_root || parent.starts_with(&live_root) {
            bail!("release rehearsal temp root must be outside the caller repository");
        }
        for sequence in 0..1000_u32 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default();
            let path = parent.join(format!(
                "loom-release-{label}-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        bail!("could not allocate detached release candidate")
    }

    fn copy(
        live_root: &Path,
        label: &str,
        ledger: &mut Vec<ArgvLedgerEntry>,
    ) -> Result<(Self, SourceInventoryAttestation)> {
        let candidate = Self::allocate(live_root, label)?;
        let inventory = copy_candidate(live_root, candidate.path(), ledger)?;
        Ok((candidate, inventory))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for DetachedCandidate {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_candidate(
    source: &Path,
    destination: &Path,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<SourceInventoryAttestation> {
    if destination.join(crate::LOOM_DIR).exists() {
        bail!(
            "detached release destination '{}' contains preinitialized .loom state",
            destination.display()
        );
    }
    if fs::read_dir(destination)?.next().is_some() {
        bail!(
            "detached release destination '{}' must be empty",
            destination.display()
        );
    }
    let (plan, attestation) = candidate_file_plan(source, ledger)?;
    for relative in plan {
        let from = source.join(&relative);
        let to = destination.join(&relative);
        let (mut source_file, metadata) = open_inventory_file(&from)?;
        fs::create_dir_all(
            to.parent()
                .ok_or_else(|| anyhow!("candidate file has no parent"))?,
        )?;
        let mode = normalized_mode(&metadata);
        let mut target = fs::File::create(&to)?;
        std::io::copy(&mut source_file, &mut target)?;
        set_inventory_permissions(&to, mode)?;
    }
    let copied_manifest: SourceInventoryManifest =
        serde_json::from_slice(&fs::read(destination.join(RELEASE_INVENTORY_PATH))?)?;
    validate_inventory_manifest(&copied_manifest)?;
    expand_inventory(destination, &copied_manifest)?;
    let copied_hash = inventory_content_hash(destination, &copied_manifest.files)?;
    if copied_hash != attestation.inventory_hash {
        bail!("materialized release snapshot does not match its inventory hash");
    }
    Ok(attestation)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceInventoryManifest {
    schema: String,
    files: Vec<SourceInventoryEntry>,
    reserved_components: Vec<String>,
    secret_name_patterns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceInventoryEntry {
    path: String,
    mode: String,
}

fn candidate_file_plan(
    root: &Path,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<(Vec<PathBuf>, SourceInventoryAttestation)> {
    let (manifest, manifest_hash) = load_source_inventory(root)?;
    let paths = expand_inventory(root, &manifest)?;
    validate_reserved_roots(root)?;
    let inventory_hash = inventory_content_hash(root, &manifest.files)?;

    let git_inventory = git_candidate_paths(root, ledger)?;
    let provenance = match git_inventory {
        Some(git_paths) => {
            let planned: BTreeMap<PathBuf, String> = manifest
                .files
                .iter()
                .map(|entry| (PathBuf::from(&entry.path), entry.mode.clone()))
                .collect();
            if planned != git_paths {
                let missing: Vec<String> = git_paths
                    .keys()
                    .filter(|path| !planned.contains_key(*path))
                    .take(5)
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect();
                let extra: Vec<String> = planned
                    .keys()
                    .filter(|path| !git_paths.contains_key(*path))
                    .take(5)
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect();
                bail!(
                    "release source inventory is stale against tracked/nonignored Git files (missing: [{}], extra: [{}])",
                    missing.join(", "),
                    extra.join(", ")
                );
            }
            verify_snapshot_inventory(root, &manifest)?;
            "source_controlled_manifest_git_verified"
        }
        None => {
            verify_snapshot_inventory(root, &manifest)?;
            ledger.push(ArgvLedgerEntry {
                source: "candidate_file_plan".into(),
                executable: "internal-source-inventory".into(),
                argv: vec![RELEASE_INVENTORY_PATH.into()],
                policy: "source_controlled_manifest_non_git".into(),
                attempted: true,
                outcome: "passed".into(),
            });
            "source_controlled_manifest_non_git"
        }
    };
    let attestation = SourceInventoryAttestation {
        schema: "loom.release-source-inventory-attestation/v1".into(),
        path: RELEASE_INVENTORY_PATH.into(),
        manifest_hash,
        inventory_hash,
        provenance: provenance.into(),
        git_verification: if provenance.ends_with("git_verified") {
            "verified".into()
        } else {
            "not_applicable".into()
        },
        git_influenced_plan: false,
        entry_count: manifest.files.len(),
        file_count: paths.len(),
        tombstone_count: manifest
            .files
            .iter()
            .filter(|entry| entry.mode == "absent")
            .count(),
        materialized_matches: true,
        missing: 0,
        unexpected: 0,
        secret: 0,
        symlink: 0,
        non_regular: 0,
        reserved: 0,
    };
    Ok((paths, attestation))
}

fn load_source_inventory(root: &Path) -> Result<(SourceInventoryManifest, String)> {
    let manifest_path = root.join(RELEASE_INVENTORY_PATH);
    let metadata = fs::symlink_metadata(&manifest_path).with_context(|| {
        format!("release source inventory is missing at '{RELEASE_INVENTORY_PATH}'")
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("release source inventory must be a regular non-symlink file");
    }
    let manifest: SourceInventoryManifest = serde_json::from_slice(&fs::read(&manifest_path)?)
        .context("release source inventory is malformed")?;
    validate_inventory_manifest(&manifest)?;
    let canonical = canonical_json(serde_json::to_value(&manifest)?);
    let manifest_hash = crate::artifact::fingerprint(&serde_json::to_string(&canonical)?);
    Ok((manifest, manifest_hash))
}

fn validate_reserved_roots(root: &Path) -> Result<()> {
    for name in INVENTORY_RESERVED_COMPONENTS {
        let path = root.join(name);
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            bail!("release snapshot refuses symlinked reserved root '{name}'");
        }
        if name != ".git" && !metadata.is_dir() {
            bail!("release snapshot reserved root '{name}' must be a directory");
        }
    }
    Ok(())
}

fn validate_inventory_manifest(manifest: &SourceInventoryManifest) -> Result<()> {
    if manifest.schema != "loom.release-inventory/v2" {
        bail!(
            "release source inventory has unsupported schema '{}'",
            manifest.schema
        );
    }
    let expected_reserved: Vec<String> = INVENTORY_RESERVED_COMPONENTS
        .iter()
        .map(ToString::to_string)
        .collect();
    let expected_secrets: Vec<String> = INVENTORY_SECRET_PATTERNS
        .iter()
        .map(ToString::to_string)
        .collect();
    if manifest.reserved_components != expected_reserved
        || manifest.secret_name_patterns != expected_secrets
    {
        bail!("release source inventory weakens the canonical reserved/secret policy");
    }
    if manifest.files.is_empty() {
        bail!("release source inventory must declare files");
    }
    let mut prior: Option<&str> = None;
    let mut has_self = false;
    for entry in &manifest.files {
        validate_candidate_path(Path::new(&entry.path))?;
        if !matches!(entry.mode.as_str(), "regular" | "executable" | "absent") {
            bail!(
                "release source inventory path '{}' has invalid mode",
                entry.path
            );
        }
        if prior.is_some_and(|previous| previous >= entry.path.as_str()) {
            bail!("release source inventory files must be unique and sorted");
        }
        has_self |= entry.path == RELEASE_INVENTORY_PATH;
        prior = Some(&entry.path);
    }
    if !has_self {
        bail!("release source inventory must include its own manifest entry");
    }
    Ok(())
}

fn expand_inventory(root: &Path, manifest: &SourceInventoryManifest) -> Result<Vec<PathBuf>> {
    let mut paths = BTreeSet::new();
    for entry in &manifest.files {
        let relative = Path::new(&entry.path);
        if entry.mode == "absent" {
            validate_candidate_path(relative)?;
            match fs::symlink_metadata(root.join(relative)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
                Ok(_) => bail!(
                    "release inventory tombstone '{}' is present",
                    relative.display()
                ),
            }
        } else {
            insert_inventory_file(root, relative, &entry.mode, &mut paths)?;
        }
    }
    if paths.is_empty() {
        bail!("release source inventory expands to no files");
    }
    Ok(paths.into_iter().collect())
}

fn insert_inventory_file(
    root: &Path,
    relative: &Path,
    expected_mode: &str,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    validate_candidate_path(relative)?;
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path)
        .with_context(|| format!("release inventory file '{}' is missing", relative.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "release inventory file '{}' must be regular",
            relative.display()
        );
    }
    if normalized_mode(&metadata) != expected_mode {
        bail!(
            "release inventory file '{}' mode is stale",
            relative.display()
        );
    }
    paths.insert(relative.to_path_buf());
    Ok(())
}

fn normalized_mode(metadata: &fs::Metadata) -> &'static str {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 != 0 {
            return "executable";
        }
    }
    "regular"
}

fn open_inventory_file(path: &Path) -> Result<(fs::File, fs::Metadata)> {
    let before = fs::symlink_metadata(path)?;
    if before.file_type().is_symlink() || !before.is_file() {
        bail!(
            "release inventory refuses non-regular source '{}'",
            path.display()
        );
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("opening no-follow inventory source '{}'", path.display()))?;
    let opened = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != opened.dev() || before.ino() != opened.ino() {
            bail!(
                "release inventory source changed while opening '{}'",
                path.display()
            );
        }
    }
    Ok((file, opened))
}

fn set_inventory_permissions(path: &Path, mode: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if mode == "executable" { 0o755 } else { 0o644 }),
        )?;
    }
    Ok(())
}

fn inventory_content_hash(root: &Path, entries: &[SourceInventoryEntry]) -> Result<String> {
    let mut bytes = Vec::new();
    for entry in entries {
        let mut content = Vec::new();
        if entry.mode != "absent" {
            let (mut file, _) = open_inventory_file(&root.join(&entry.path))?;
            file.read_to_end(&mut content)?;
        }
        bytes.extend_from_slice(entry.path.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(entry.mode.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(content.len().to_string().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&content);
        bytes.push(0xff);
    }
    Ok(fingerprint_bytes(&bytes))
}

fn git_candidate_paths(
    root: &Path,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<Option<BTreeMap<PathBuf, String>>> {
    if !root.join(".git").exists() {
        return Ok(None);
    }
    let sandbox = DetachedCandidate::allocate(root, "git-inventory-sandbox")?;
    let argv = vec![
        "-C".into(),
        root.to_string_lossy().into_owned(),
        "ls-files".into(),
        "-z".into(),
        "--cached".into(),
        "--others".into(),
        "--exclude-standard".into(),
    ];
    let mut command = Command::new("git");
    command.args(&argv).env_clear();
    for key in ["PATH", "SYSTEMROOT", "WINDIR", "PATHEXT", "COMSPEC"] {
        if let Ok(value) = std::env::var(key) {
            command.env(key, value);
        }
    }
    command
        .env("HOME", sandbox.path())
        .env("XDG_CONFIG_HOME", sandbox.path())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output()
        .context("building Git-aware release candidate file plan")?;
    ledger.push(ArgvLedgerEntry {
        source: "candidate_file_plan".into(),
        executable: "git".into(),
        argv,
        policy: "read_only_git_inventory".into(),
        attempted: true,
        outcome: if output.status.success() {
            "passed".into()
        } else {
            format!("failed_exit_{}", output.status.code().unwrap_or(-1))
        },
    });
    if !output.status.success() {
        bail!(
            "release candidate requires a readable Git worktree inventory: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let flags_argv = vec![
        "-C".into(),
        root.to_string_lossy().into_owned(),
        "ls-files".into(),
        "-z".into(),
        "-v".into(),
        "--cached".into(),
    ];
    let mut flags_command = Command::new("git");
    flags_command.args(&flags_argv).env_clear();
    for key in ["PATH", "SYSTEMROOT", "WINDIR", "PATHEXT", "COMSPEC"] {
        if let Ok(value) = std::env::var(key) {
            flags_command.env(key, value);
        }
    }
    let flags_output = flags_command
        .env("HOME", sandbox.path())
        .env("XDG_CONFIG_HOME", sandbox.path())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("checking Git index inventory flags")?;
    ledger.push(ArgvLedgerEntry {
        source: "candidate_file_plan:index_flags".into(),
        executable: "git".into(),
        argv: flags_argv,
        policy: "read_only_git_inventory".into(),
        attempted: true,
        outcome: if flags_output.status.success() {
            "passed".into()
        } else {
            format!("failed_exit_{}", flags_output.status.code().unwrap_or(-1))
        },
    });
    if !flags_output.status.success() {
        bail!("release candidate cannot verify Git index flags");
    }
    for raw in flags_output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
    {
        if raw.len() < 3 || raw[0] != b'H' || raw[1] != b' ' {
            let path =
                std::str::from_utf8(raw.get(2..).unwrap_or_default()).unwrap_or("<non-utf8>");
            bail!(
                "release candidate refuses sparse, assume-unchanged, or exceptional Git index path '{}'",
                path
            );
        }
    }
    let mut candidate_paths = BTreeSet::new();
    for raw in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
    {
        let text =
            std::str::from_utf8(raw).context("Git file inventory contains a non-UTF-8 path")?;
        let relative = PathBuf::from(text);
        validate_candidate_path(&relative)?;
        candidate_paths.insert(relative);
    }

    // The index no longer names staged deletions or the old side of staged
    // renames. Union the committed tree so those paths must appear as explicit
    // `absent` tombstones instead of disappearing from the candidate identity.
    let head_argv = vec![
        "-C".into(),
        root.to_string_lossy().into_owned(),
        "ls-tree".into(),
        "-r".into(),
        "-z".into(),
        "--name-only".into(),
        "HEAD".into(),
    ];
    let mut head_command = Command::new("git");
    head_command.args(&head_argv).env_clear();
    for key in ["PATH", "SYSTEMROOT", "WINDIR", "PATHEXT", "COMSPEC"] {
        if let Ok(value) = std::env::var(key) {
            head_command.env(key, value);
        }
    }
    let head_output = head_command
        .env("HOME", sandbox.path())
        .env("XDG_CONFIG_HOME", sandbox.path())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("reading committed release candidate paths")?;
    ledger.push(ArgvLedgerEntry {
        source: "candidate_file_plan:committed_tree".into(),
        executable: "git".into(),
        argv: head_argv,
        policy: "read_only_git_inventory".into(),
        attempted: true,
        outcome: if head_output.status.success() {
            "passed".into()
        } else {
            "unborn_head".into()
        },
    });
    if head_output.status.success() {
        for raw in head_output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|raw| !raw.is_empty())
        {
            let text = std::str::from_utf8(raw)
                .context("committed Git inventory contains a non-UTF-8 path")?;
            let relative = PathBuf::from(text);
            validate_candidate_path(&relative)?;
            candidate_paths.insert(relative);
        }
    }

    let mut paths = BTreeMap::new();
    for relative in candidate_paths {
        let text = relative.to_string_lossy();
        let absolute = root.join(&relative);
        match fs::symlink_metadata(&absolute) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    bail!("release candidate refuses non-regular path '{}'", text);
                }
                paths.insert(relative, normalized_mode(&metadata).into());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                paths.insert(relative, "absent".into());
            }
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting candidate path '{}'", text));
            }
        }
    }
    if paths.is_empty() {
        bail!("release candidate Git file plan is empty");
    }
    Ok(Some(paths))
}

fn verify_snapshot_inventory(root: &Path, manifest: &SourceInventoryManifest) -> Result<()> {
    let planned: BTreeMap<PathBuf, String> = manifest
        .files
        .iter()
        .filter(|entry| entry.mode != "absent")
        .map(|entry| (PathBuf::from(&entry.path), entry.mode.clone()))
        .collect();
    let observed = scan_snapshot_files(root)?;
    if planned != observed {
        let missing: Vec<String> = observed
            .keys()
            .filter(|path| !planned.contains_key(*path))
            .take(5)
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let extra: Vec<String> = planned
            .keys()
            .filter(|path| !observed.contains_key(*path))
            .take(5)
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        bail!(
            "release source tree differs from its explicit inventory (undeclared: [{}], missing: [{}])",
            missing.join(", "),
            extra.join(", ")
        );
    }
    Ok(())
}

fn scan_snapshot_files(root: &Path) -> Result<BTreeMap<PathBuf, String>> {
    fn walk(
        root: &Path,
        directory: &Path,
        depth: usize,
        out: &mut BTreeMap<PathBuf, String>,
    ) -> Result<()> {
        let mut entries: Vec<_> = fs::read_dir(directory)?.collect::<std::io::Result<_>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow!("release snapshot contains non-UTF-8 path"))?;
            let metadata = fs::symlink_metadata(&path)?;
            if INVENTORY_RESERVED_COMPONENTS.contains(&name.as_str()) {
                if metadata.file_type().is_symlink() || !metadata.is_dir() || depth > 0 {
                    bail!(
                        "release snapshot contains unsafe reserved path '{}'",
                        relative.display()
                    );
                }
                continue;
            }
            if metadata.file_type().is_symlink() {
                bail!("release snapshot refuses symlink '{}'", relative.display());
            }
            if metadata.is_dir() {
                walk(root, &path, depth + 1, out)?;
            } else if metadata.is_file() {
                validate_candidate_path(relative)?;
                out.insert(relative.to_path_buf(), normalized_mode(&metadata).into());
            } else {
                bail!(
                    "release snapshot refuses non-regular path '{}'",
                    relative.display()
                );
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    walk(root, root, 0, &mut files)?;
    Ok(files)
}

fn validate_candidate_path(path: &Path) -> Result<()> {
    use std::path::Component;

    if path.as_os_str().is_empty() || path.is_absolute() {
        bail!("release candidate path must be a confined relative path");
    }
    for component in path.components() {
        let Component::Normal(component) = component else {
            bail!(
                "release candidate path '{}' is not confined",
                path.display()
            );
        };
        let component = component
            .to_str()
            .ok_or_else(|| anyhow!("release candidate path is not UTF-8"))?;
        if INVENTORY_RESERVED_COMPONENTS.contains(&component) {
            bail!(
                "release candidate path '{}' enters reserved component '{}'",
                path.display(),
                component
            );
        }
    }
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let secret = name == ".env"
        || name.starts_with(".env.")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || matches!(
            name.as_str(),
            "id_rsa"
                | "id_ed25519"
                | "credentials"
                | "credentials.json"
                | "secrets"
                | "secrets.json"
                | ".npmrc"
                | ".pypirc"
                | ".netrc"
        );
    if secret {
        bail!(
            "release candidate refuses secret-bearing path '{}'",
            path.display()
        );
    }
    Ok(())
}

impl LiveState {
    fn capture(root: &Path) -> Result<Self> {
        let git_dir = git_directory(root)?;
        Ok(Self {
            source: hash_tree(root, &SOURCE_EXCLUDES)?,
            // The cooperative holder record is ephemeral coordination, not
            // graph truth. Read-only Store access refreshes it by design; the
            // rehearsal compares every durable graph byte around it.
            graph: hash_optional_path_excluding(&root.join(crate::LOOM_DIR), &["lock"])?,
            target: hash_optional_path(&root.join("target"))?,
            git: hash_optional_path(&root.join(".git"))?,
            git_head: git_head_hash(git_dir.as_deref())?,
            git_index: git_dir
                .as_deref()
                .map(|directory| hash_optional_path(&directory.join("index")))
                .transpose()?
                .unwrap_or_else(|| "absent".into()),
            git_remotes: git_dir
                .as_deref()
                .map(|directory| hash_optional_path(&directory.join("config")))
                .transpose()?
                .unwrap_or_else(|| "absent".into()),
            installed_binary: installed_loom_hash()?,
        })
    }

    fn compare(&self, after: &Self, ledger: &[ArgvLedgerEntry]) -> EffectAttestation {
        let live_source_changed = self.source != after.source;
        let live_graph_changed = self.graph != after.graph;
        let live_target_changed = self.target != after.target;
        let live_git_changed = self.git != after.git;
        let live_git_head_changed = self.git_head != after.git_head;
        let live_git_index_changed = self.git_index != after.git_index;
        let live_git_remotes_changed = self.git_remotes != after.git_remotes;
        let installed_binary_changed = self.installed_binary != after.installed_binary;
        let mut release_paths_changed = Vec::new();
        for (changed, label) in [
            (live_source_changed, "caller_source"),
            (live_graph_changed, "caller_graph"),
            (live_target_changed, "caller_target"),
            (live_git_changed, "caller_git"),
            (live_git_head_changed, "caller_git_head"),
            (live_git_index_changed, "caller_git_index"),
            (live_git_remotes_changed, "caller_git_remotes"),
            (installed_binary_changed, "installed_loom"),
        ] {
            if changed {
                release_paths_changed.push(label.into());
            }
        }
        let attempted = |executable: &str, command: &str| {
            ledger.iter().any(|entry| {
                entry.attempted
                    && Path::new(&entry.executable)
                        .file_name()
                        .and_then(OsStr::to_str)
                        .is_some_and(|name| name.eq_ignore_ascii_case(executable))
                    && entry.argv.first().is_some_and(|arg| arg == command)
            })
        };
        EffectAttestation {
            live_source_changed,
            live_graph_changed,
            live_target_changed,
            live_git_changed,
            live_git_head_changed,
            live_git_index_changed,
            live_git_remotes_changed,
            installed_binary_changed,
            release_paths_changed,
            argv_attempt_scope:
                "direct top-level argv ledger only; descendant process containment is not claimed"
                    .into(),
            top_level_install_argv_attempted: attempted("cargo", "install"),
            top_level_commit_argv_attempted: attempted("git", "commit"),
            top_level_push_argv_attempted: attempted("git", "push"),
        }
    }
}

fn git_directory(root: &Path) -> Result<Option<PathBuf>> {
    let dot_git = root.join(".git");
    if !dot_git.exists() {
        return Ok(None);
    }
    let metadata = fs::symlink_metadata(&dot_git)?;
    if metadata.file_type().is_symlink() {
        bail!("release rehearsal refuses a symlinked caller Git directory");
    }
    if metadata.is_dir() {
        return Ok(Some(dot_git));
    }
    let pointer = fs::read_to_string(&dot_git)?;
    let relative = pointer
        .trim()
        .strip_prefix("gitdir:")
        .map(str::trim)
        .ok_or_else(|| anyhow!("caller .git file has no gitdir pointer"))?;
    let path = PathBuf::from(relative);
    let resolved = if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
    .canonicalize()?;
    Ok(Some(resolved))
}

fn git_head_hash(git_dir: Option<&Path>) -> Result<String> {
    let Some(git_dir) = git_dir else {
        return Ok("absent".into());
    };
    let head_path = git_dir.join("HEAD");
    if !head_path.exists() {
        return Ok("absent".into());
    }
    let head = fs::read(&head_path)?;
    let mut projection = BTreeMap::new();
    projection.insert("HEAD", fingerprint_bytes(&head));
    if let Ok(text) = std::str::from_utf8(&head) {
        if let Some(reference) = text.trim().strip_prefix("ref: ") {
            projection.insert("referent", hash_optional_path(&git_dir.join(reference))?);
            projection.insert(
                "packed_refs",
                hash_optional_path(&git_dir.join("packed-refs"))?,
            );
        }
    }
    Ok(fingerprint_bytes(&serde_json::to_vec(&projection)?))
}

fn installed_loom_hash() -> Result<String> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")));
    match cargo_home {
        Some(home) => hash_optional_path(&home.join("bin/loom")),
        None => Ok("unavailable".into()),
    }
}

fn hash_optional_path(path: &Path) -> Result<String> {
    hash_optional_path_excluding(path, &[])
}

fn hash_optional_path_excluding(path: &Path, top_level_excludes: &[&str]) -> Result<String> {
    if !path.exists() {
        return Ok("absent".into());
    }
    hash_tree(path, top_level_excludes)
}

fn hash_tree(root: &Path, top_level_excludes: &[&str]) -> Result<String> {
    let mut bytes = Vec::new();
    hash_tree_into(root, root, top_level_excludes, &mut bytes)?;
    Ok(fingerprint_bytes(&bytes))
}

fn hash_tree_into(
    root: &Path,
    path: &Path,
    top_level_excludes: &[&str],
    out: &mut Vec<u8>,
) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let relative = path.strip_prefix(root).unwrap_or(path);
    if metadata.file_type().is_symlink() {
        out.extend_from_slice(b"link\0");
        out.extend_from_slice(relative.to_string_lossy().as_bytes());
        out.push(0);
        out.extend_from_slice(fs::read_link(path)?.to_string_lossy().as_bytes());
        return Ok(());
    }
    if metadata.is_file() {
        out.extend_from_slice(b"file\0");
        out.extend_from_slice(relative.to_string_lossy().as_bytes());
        out.push(0);
        out.extend_from_slice(&fs::read(path)?);
        return Ok(());
    }
    if !metadata.is_dir() {
        out.extend_from_slice(b"special\0");
        out.extend_from_slice(relative.to_string_lossy().as_bytes());
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(path)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if path == root
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| top_level_excludes.contains(&name))
        {
            continue;
        }
        hash_tree_into(root, &entry.path(), top_level_excludes, out)?;
    }
    Ok(())
}

fn fingerprint_bytes(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn canonical_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(object) => {
            let sorted: BTreeMap<String, serde_json::Value> = object
                .into_iter()
                .map(|(key, value)| (key, canonical_json(value)))
                .collect();
            serde_json::to_value(sorted).expect("canonical JSON map serializes")
        }
        scalar => scalar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_live_root(label: &str) -> DetachedCandidate {
        let parent = std::env::temp_dir().canonicalize().unwrap();
        for sequence in 0..1000_u32 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default();
            let path = parent.join(format!(
                "loom-release-live-{label}-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return DetachedCandidate(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("creating isolated release fixture: {error}"),
            }
        }
        panic!("could not allocate isolated release fixture")
    }

    fn write_test_inventory(root: &Path, declared: &[(&str, &str)]) {
        let mut files = declared
            .iter()
            .map(|(path, mode)| SourceInventoryEntry {
                path: (*path).into(),
                mode: (*mode).into(),
            })
            .collect::<Vec<_>>();
        files.push(SourceInventoryEntry {
            path: RELEASE_INVENTORY_PATH.into(),
            mode: "regular".into(),
        });
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let manifest = SourceInventoryManifest {
            schema: "loom.release-inventory/v2".into(),
            files,
            reserved_components: INVENTORY_RESERVED_COMPONENTS
                .iter()
                .map(ToString::to_string)
                .collect(),
            secret_name_patterns: INVENTORY_SECRET_PATTERNS
                .iter()
                .map(ToString::to_string)
                .collect(),
        };
        fs::create_dir_all(root.join("release")).unwrap();
        fs::write(
            root.join(RELEASE_INVENTORY_PATH),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn copy_refuses_nonempty_and_excludes_reserved_roots() {
        let root = isolated_live_root("source");
        fs::write(root.path().join("Cargo.toml"), b"[package]\nname='x'\n").unwrap();
        fs::write(root.path().join(".gitignore"), b".loom/\ntarget/\n").unwrap();
        write_test_inventory(
            root.path(),
            &[(".gitignore", "regular"), ("Cargo.toml", "regular")],
        );
        fs::create_dir(root.path().join(".git")).unwrap();
        fs::write(root.path().join(".git/config"), b"secret").unwrap();
        fs::create_dir(root.path().join(".loom")).unwrap();
        fs::write(root.path().join(".loom/graph.sqlite"), b"graph").unwrap();
        fs::create_dir(root.path().join("target")).unwrap();
        fs::write(root.path().join("target/output"), b"build").unwrap();
        assert!(Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success());

        let destination = DetachedCandidate::allocate(root.path(), "destination").unwrap();
        let mut ledger = Vec::new();
        copy_candidate(root.path(), destination.path(), &mut ledger).unwrap();
        assert!(destination.path().join("Cargo.toml").is_file());
        for excluded in SOURCE_EXCLUDES {
            assert!(!destination.path().join(excluded).exists());
        }

        fs::write(destination.path().join("occupied"), b"x").unwrap();
        assert!(copy_candidate(root.path(), destination.path(), &mut ledger).is_err());
        assert!(ledger
            .iter()
            .any(|entry| entry.source == "candidate_file_plan" && entry.attempted));
    }

    #[test]
    fn imported_surface_requires_an_independent_candidate_manifest() {
        let root = isolated_live_root("manifest-gap");
        let surface = Node {
            id: "surface-id".into(),
            node_type: NodeType::InterfaceSurface,
            name: "candidate-cli".into(),
            description: String::new(),
            status: "quarantined".into(),
            truth_class: crate::model::TruthClass::Asserted,
            body: serde_json::json!({"operations": [{"argv": ["candidate"]}]}),
            created_at: String::new(),
            updated_at: String::new(),
        };
        let mut ledger = Vec::new();
        let error = candidate_manifest_attestations(
            root.path(),
            &[surface],
            &missing_outer_attestation(),
            &mut ledger,
        )
        .unwrap_err();
        assert!(error.to_string().contains("remain quarantined"));
    }

    #[test]
    fn candidate_plan_refuses_secret_and_nested_reserved_paths() {
        for relative in ["nested/.env.production", "nested/target/payload"] {
            let root = isolated_live_root("secret-source");
            fs::create_dir_all(root.path().join(relative).parent().unwrap()).unwrap();
            fs::write(root.path().join(relative), b"secret").unwrap();
            write_test_inventory(root.path(), &[]);
            assert!(Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(root.path())
                .status()
                .unwrap()
                .success());
            let destination =
                DetachedCandidate::allocate(root.path(), "secret-destination").unwrap();
            let error = copy_candidate(root.path(), destination.path(), &mut Vec::new())
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("secret-bearing") || error.contains("reserved component"),
                "{error}"
            );
        }
    }

    #[test]
    fn linked_worktree_head_change_blocks_even_when_git_pointer_bytes_do_not_change() {
        let root = isolated_live_root("linked-worktree");
        let git = DetachedCandidate::allocate(root.path(), "linked-gitdir").unwrap();
        fs::create_dir_all(git.path().join("refs/heads")).unwrap();
        fs::write(git.path().join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        fs::write(git.path().join("refs/heads/main"), b"first\n").unwrap();
        fs::write(git.path().join("index"), b"index").unwrap();
        fs::write(git.path().join("config"), b"[remote \"origin\"]\n").unwrap();
        fs::write(
            root.path().join(".git"),
            format!("gitdir: {}\n", git.path().display()),
        )
        .unwrap();

        let before = LiveState::capture(root.path()).unwrap();
        fs::write(git.path().join("refs/heads/main"), b"second\n").unwrap();
        let after = LiveState::capture(root.path()).unwrap();
        let effects = before.compare(&after, &[]);
        assert!(!effects.live_git_changed, "the .git pointer file is stable");
        assert!(effects.live_git_head_changed);
        assert!(!effects.live_git_index_changed);
        assert!(!effects.live_git_remotes_changed);
        assert!(caller_state_changed(&effects));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_git_pointer_is_rejected_before_state_comparison() {
        use std::os::unix::fs::symlink;

        let root = isolated_live_root("symlink-worktree");
        let git = DetachedCandidate::allocate(root.path(), "symlink-gitdir").unwrap();
        symlink(git.path(), root.path().join(".git")).unwrap();
        assert!(git_directory(root.path())
            .unwrap_err()
            .to_string()
            .contains("symlinked caller Git directory"));
    }
}
