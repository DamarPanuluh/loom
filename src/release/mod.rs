//! Detached, rehearsal-only release verification.
//!
//! The public interface is deliberately one operation: produce a structured
//! attestation for one named phase. All copying, trust-boundary checks, direct
//! argv execution, and cleanup stay behind it. There is no release/install or
//! Git mutation operation in this module.

use crate::journey::SurfaceManifest;
use crate::model::{Node, NodeType};
use crate::store::Store;
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

pub const RELEASE_REHEARSAL_SCHEMA: &str = "loom.release-rehearsal/v1";
const COLD_JOURNEY_REHEARSAL_SCHEMA: &str = "loom.journey-cold-rehearsal/v1";
const RELEASE_JOURNEY_ID: &str = "release-workflow";
const RELEASE_PROFILE: &str = "proof";
const SURFACE_MANIFEST_ROOT: &str = "journeys/surfaces";
const RELEASE_INVENTORY_PATH: &str = "release/inventory.json";
const RELEASE_CODE_GATES: &[&[&str]] = &[
    &["cargo", "fmt", "--all", "--", "--check"],
    &[
        "cargo",
        "clippy",
        "--all-targets",
        "--all-features",
        "--",
        "-D",
        "warnings",
    ],
    &[
        "cargo",
        "test",
        "--all-targets",
        "--quiet",
        "--",
        "--test-threads=1",
    ],
    &["cargo", "build", "--quiet"],
];
const RELEASE_CACHE_ROOT_ENVIRONMENT: &[&str] = &["CARGO_HOME", "RUSTUP_HOME"];
const SOURCE_EXCLUDES: [&str; 3] = [".git", ".loom", "target"];
// Release-owned authority and process scratch lives here after the caller
// baseline is captured. It is neither caller source nor result identity.
const CALLER_SOURCE_EXCLUDES: [&str; 4] = [".git", ".loom", ".release-sandbox", "target"];
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
const INVENTORY_RESERVED_COMPONENTS: [&str; 10] = [
    ".claude",
    ".commandcode",
    ".git",
    ".loom",
    ".nodeterm",
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
    /// Literal, runnable handoff into the typed outer Journey runtime.
    pub next_command: String,
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
pub struct CacheRootAttestation {
    pub environment: String,
    pub path: String,
    pub before_hash: String,
    pub after_hash: String,
    pub unchanged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyCacheAttestation {
    pub strategy: String,
    pub roots: Vec<CacheRootAttestation>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct JourneyResultSummary {
    journey_id: String,
    profile: String,
    journey_hash: String,
    surface_hash: String,
    verdict: String,
}

struct GateRuntime<'a> {
    outer: &'a OuterJourneyAttestation,
    derivation_authority: &'a BoundDerivationAuthority,
    executor: &'a mut dyn ReleaseExecutor,
    dependency_cache: &'a DependencyCacheGuard,
    code_gates: &'a [Vec<String>],
    inventory_manifest_hash: &'a str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ColdJourneyRehearsalReport {
    schema: String,
    status: crate::journey_runtime::RuntimeStatus,
    journey_id: String,
    profile: String,
    candidate_hash: String,
    source_inventory: SourceInventoryAttestation,
    dependency_cache: DependencyCacheAttestation,
    runtime: crate::journey_runtime::RuntimeReport,
    settled: bool,
    caller_changed: bool,
}

include!("section_01.rs");
include!("section_02.rs");
include!("section_03.rs");
include!("section_04.rs");
include!("section_05.rs");
include!("section_06.rs");
include!("section_07.rs");
include!("section_08.rs");
