//! Candidate-owned Journey surface execution policy.
//!
//! A source manifest is data, not authority. This module turns that data into
//! an inspected plan, derives the capability of every operation (including
//! nested Validation and MCP payloads), and is the only place that can mint an
//! [`AuthorizedInvocation`] for the direct Journey executor.

use crate::cli::{
    Cli, CodefileCmd, Command, DebtCmd, EdgeCmd, FindingCmd, IgnoreCmd, InboxCmd, IntentCmd,
    JourneyCmd, McpCmd, ReleaseCmd, ValidationCmd,
};
use crate::journey::{
    argv_token_source, CliOperation, OperationArgument, SetupGraph, SurfaceBinding,
    SurfaceManifest, SurfaceSetup,
};
use crate::Result;
use anyhow::{anyhow, bail, Context};
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

const POLICY_VERSION: &str = "candidate-surface/v1";
const MAX_NESTING: usize = 3;
const MAX_ARGV_TOKENS: usize = 512;
const MAX_ARGV_BYTES: usize = 256 * 1024;
const DETACHED_OUTER_ENVIRONMENT: &[&str] = &["CARGO_HOME", "RUSTUP_HOME"];

const RESERVED_ENVIRONMENT: &[&str] = &[
    crate::identity::AGENT_ENV,
    crate::identity::PROFILE_ENV,
    "LOOM_NON_INTERACTIVE",
    "LOOM_PRESENCE_PROBE",
    crate::release::OUTER_JOURNEY_ID_ENV,
    crate::release::OUTER_JOURNEY_PROFILE_ENV,
    crate::release::OUTER_JOURNEY_RUN_ID_ENV,
    crate::release::OUTER_JOURNEY_HASH_ENV,
    crate::release::OUTER_SURFACE_HASH_ENV,
    crate::release::OUTER_COMPILER_VERSION_ENV,
    crate::release::OUTER_PROOF_HASH_ENV,
    crate::release::OUTER_CONTEXT_CAPSULE_ENV,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DerivedCapability {
    Read,
    ConfinedMutation,
    DetachedProcess,
    SuppressedOuter,
}

impl DerivedCapability {
    fn requires_isolation(self) -> bool {
        matches!(self, Self::ConfinedMutation)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActualConfinement {
    LiveReadOnly,
    FreshIsolated,
    LocalSnapshot,
}

impl ActualConfinement {
    fn is_isolated(self) -> bool {
        !matches!(self, Self::LiveReadOnly)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PolicyMode<'a> {
    Runtime,
    DetachedReleaseInspection {
        outer_journey_id: &'a str,
        outer_surface_id: &'a str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NestedInspection {
    pub source: String,
    pub capability: DerivedCapability,
    pub outcome: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationInspection {
    pub operation_id: String,
    pub capability: DerivedCapability,
    pub outcome: String,
    pub nested: Vec<NestedInspection>,
}

#[derive(Debug, Clone)]
struct PlannedOperation {
    declared: CliOperation,
    capability: DerivedCapability,
}

#[derive(Debug, Clone)]
pub struct SurfacePlan {
    policy_version: &'static str,
    operations: BTreeMap<String, PlannedOperation>,
    inspections: Vec<OperationInspection>,
    requires_isolation: bool,
}

impl SurfacePlan {
    pub fn policy_version(&self) -> &'static str {
        self.policy_version
    }

    pub fn inspections(&self) -> &[OperationInspection] {
        &self.inspections
    }

    pub fn requires_isolation(&self) -> bool {
        self.requires_isolation
    }

    pub fn authorize(
        &self,
        operation_id: &str,
        resolved_argv: Vec<String>,
        confinement: ActualConfinement,
    ) -> Result<AuthorizedInvocation> {
        let operation = self
            .operations
            .get(operation_id)
            .ok_or_else(|| anyhow!("surface policy has no operation '{operation_id}'"))?;
        ensure_resolved_shape(&operation.declared, &resolved_argv)?;
        if operation.declared.argv.first().map(String::as_str) != Some("loom") {
            if !confinement.is_isolated() {
                bail!("external operation '{operation_id}' requires a confined Journey workspace");
            }
            return Ok(AuthorizedInvocation {
                argv: resolved_argv,
                inject_graph: false,
            });
        }
        let parsed = parse_cli(&resolved_argv)?;
        let capability = classify_cli(
            &parsed,
            &resolved_argv,
            NestedPurpose::TopLevel,
            0,
            &mut Vec::new(),
        )?;
        if capability != operation.capability {
            bail!(
                "operation '{operation_id}' resolved to capability {:?}, not inspected capability {:?}",
                capability,
                operation.capability
            );
        }
        if capability == DerivedCapability::SuppressedOuter {
            bail!("suppressed outer operation '{operation_id}' cannot be executed");
        }
        if capability.requires_isolation() && !confinement.is_isolated() {
            bail!("operation '{operation_id}' requires a confined Journey workspace");
        }
        Ok(AuthorizedInvocation {
            argv: resolved_argv,
            inject_graph: true,
        })
    }
}

/// Direct-process authorization. Its argv stays private so callers cannot
/// inspect one command and then spawn another.
#[derive(Debug)]
pub struct AuthorizedInvocation {
    argv: Vec<String>,
    inject_graph: bool,
}

impl AuthorizedInvocation {
    pub(crate) fn injects_graph(&self) -> bool {
        self.inject_graph
    }

    pub(crate) fn into_graph_argv(mut self, graph_root: &Path) -> Vec<String> {
        if self.inject_graph {
            self.argv.insert(1, graph_root.display().to_string());
            self.argv.insert(1, "--graph".into());
        }
        self.argv
    }
}

pub fn inspect_manifest(
    spec: &crate::journey::JourneySpec,
    manifest: &SurfaceManifest,
    mode: PolicyMode<'_>,
) -> Result<SurfacePlan> {
    validate_codefile_path(&manifest.surface.codefile)?;
    inspect_surface(
        spec,
        &manifest.journey_id,
        &manifest.surface.id,
        &manifest.surface.operations,
        manifest.setup.as_ref(),
        &manifest.bindings,
        mode,
    )
}

pub fn inspect_surface(
    spec: &crate::journey::JourneySpec,
    journey_id: &str,
    surface_id: &str,
    operations: &[CliOperation],
    setup: Option<&SurfaceSetup>,
    bindings: &[SurfaceBinding],
    mode: PolicyMode<'_>,
) -> Result<SurfacePlan> {
    if journey_id != spec.id {
        bail!("candidate policy journey id does not match authored source");
    }
    validate_detached_profile_environment(spec, mode)?;
    validate_membership(spec, operations, setup, bindings)?;
    let ordered = execution_order(spec, operations, setup, bindings)?;
    let run_names = validation_run_names(&ordered)?;
    let mut registrations = BTreeMap::new();
    let mut inspections = Vec::with_capacity(ordered.len());
    let mut planned = BTreeMap::new();
    let mut requires_isolation = setup.is_some();

    for operation in ordered {
        validate_operation_envelope(
            operation,
            matches!(mode, PolicyMode::DetachedReleaseInspection { .. }),
        )?;
        let static_argv = static_argv(operation)?;
        if operation.argv.first().map(String::as_str) != Some("loom") {
            requires_isolation = true;
            inspections.push(OperationInspection {
                operation_id: operation.id.clone(),
                capability: DerivedCapability::ConfinedMutation,
                outcome: "runtime_external_process_confined".into(),
                nested: Vec::new(),
            });
            planned.insert(
                operation.id.clone(),
                PlannedOperation {
                    declared: operation.clone(),
                    capability: DerivedCapability::ConfinedMutation,
                },
            );
            continue;
        }
        let parsed = parse_cli(&static_argv)
            .with_context(|| format!("candidate surface operation '{}'", operation.id))?;
        let exact_outer = is_exact_outer(journey_id, surface_id, &parsed, mode);
        validate_detached_operation_environment(operation, exact_outer, mode)?;
        let mut nested = Vec::new();
        let mut capability = classify_cli(
            &parsed,
            &static_argv,
            NestedPurpose::TopLevel,
            0,
            &mut nested,
        )?;

        if let Some((name, command)) = validation_registration(&parsed) {
            if registrations
                .insert(name.clone(), command.clone())
                .is_some()
            {
                bail!("surface registers Validation '{name}' more than once");
            }
            if run_names.contains(&name) {
                let nested_capability = classify_validation_payload(&command, 1)?;
                nested.push(NestedInspection {
                    source: format!("validation:{name}:command"),
                    capability: nested_capability,
                    outcome: "approved_for_exact_manifest_run".into(),
                });
                if nested_capability.requires_isolation() {
                    capability = DerivedCapability::ConfinedMutation;
                }
            } else {
                nested.push(NestedInspection {
                    source: format!("validation:{name}:command"),
                    capability: DerivedCapability::ConfinedMutation,
                    outcome: "stored_only_never_run".into(),
                });
            }
        }
        if let Some(name) = validation_run(&parsed) {
            let command = registrations.get(&name).ok_or_else(|| {
                anyhow!("Validation run '{name}' has no earlier exact manifest registration")
            })?;
            let nested_capability = classify_validation_payload(command, 1)?;
            nested.push(NestedInspection {
                source: format!("validation:{name}:run"),
                capability: nested_capability,
                outcome: "linked_to_exact_registration".into(),
            });
            capability = DerivedCapability::ConfinedMutation;
        }

        if exact_outer {
            capability = DerivedCapability::SuppressedOuter;
        } else if matches!(parsed.command, Some(Command::Release { .. }))
            && matches!(mode, PolicyMode::DetachedReleaseInspection { .. })
        {
            bail!("candidate surface declares release recursion outside the exact outer profile");
        }

        // `read_only` is an assertion we verify, never a capability grant.
        if operation.read_only && capability == DerivedCapability::ConfinedMutation {
            bail!(
                "operation '{}' is marked read_only but derives a graph/process mutation",
                operation.id
            );
        }
        requires_isolation |= capability.requires_isolation() || !operation.read_only;
        inspections.push(OperationInspection {
            operation_id: operation.id.clone(),
            capability,
            outcome: if capability == DerivedCapability::SuppressedOuter {
                "suppressed_exact_outer".into()
            } else {
                "inspected_before_reauthorization".into()
            },
            nested,
        });
        planned.insert(
            operation.id.clone(),
            PlannedOperation {
                declared: operation.clone(),
                capability,
            },
        );
    }

    Ok(SurfacePlan {
        policy_version: POLICY_VERSION,
        operations: planned,
        inspections,
        requires_isolation,
    })
}

/// Rebuild the policy plan from the compiler-owned executable projection.
/// Runtime calls this on every execution, so a cached proof never bypasses a
/// policy upgrade.
pub fn inspect_compiled_operations(
    journey_id: &str,
    operations: &[CliOperation],
    local_snapshot: bool,
) -> Result<SurfacePlan> {
    let mut registrations = BTreeMap::new();
    let ordered = operations.iter().collect::<Vec<_>>();
    let run_names = validation_run_names(&ordered)?;
    let mut inspections = Vec::with_capacity(operations.len());
    let mut planned = BTreeMap::new();
    let mut requires_isolation = local_snapshot;
    for operation in operations {
        validate_operation_envelope(operation, false)?;
        let static_argv = static_argv(operation)?;
        if operation.argv.first().map(String::as_str) != Some("loom") {
            requires_isolation = true;
            inspections.push(OperationInspection {
                operation_id: operation.id.clone(),
                capability: DerivedCapability::ConfinedMutation,
                outcome: "runtime_external_process_confined".into(),
                nested: Vec::new(),
            });
            planned.insert(
                operation.id.clone(),
                PlannedOperation {
                    declared: operation.clone(),
                    capability: DerivedCapability::ConfinedMutation,
                },
            );
            continue;
        }
        let parsed = parse_cli(&static_argv)
            .with_context(|| format!("compiled operation '{}'", operation.id))?;
        let mut nested = Vec::new();
        let mut capability = classify_cli(
            &parsed,
            &static_argv,
            NestedPurpose::TopLevel,
            0,
            &mut nested,
        )?;
        if let Some((name, command)) = validation_registration(&parsed) {
            if registrations
                .insert(name.clone(), command.clone())
                .is_some()
            {
                bail!("compiled surface registers Validation '{name}' more than once");
            }
            if run_names.contains(&name) {
                let nested_capability = classify_validation_payload(&command, 1)?;
                nested.push(NestedInspection {
                    source: format!("validation:{name}:command"),
                    capability: nested_capability,
                    outcome: "approved_for_exact_manifest_run".into(),
                });
                if nested_capability.requires_isolation() {
                    capability = DerivedCapability::ConfinedMutation;
                }
            }
        }
        if let Some(name) = validation_run(&parsed) {
            let command = registrations.get(&name).ok_or_else(|| {
                anyhow!("Validation run '{name}' has no earlier compiled registration")
            })?;
            let nested_capability = classify_validation_payload(command, 1)?;
            nested.push(NestedInspection {
                source: format!("validation:{name}:run"),
                capability: nested_capability,
                outcome: "linked_to_exact_registration".into(),
            });
            capability = DerivedCapability::ConfinedMutation;
        }
        if operation.read_only && capability == DerivedCapability::ConfinedMutation {
            bail!(
                "compiled operation '{}' is marked read_only but derives a mutation",
                operation.id
            );
        }
        requires_isolation |= capability.requires_isolation() || !operation.read_only;
        inspections.push(OperationInspection {
            operation_id: operation.id.clone(),
            capability,
            outcome: "runtime_policy_rebuilt".into(),
            nested,
        });
        planned.insert(
            operation.id.clone(),
            PlannedOperation {
                declared: operation.clone(),
                capability,
            },
        );
    }
    let _ = journey_id;
    Ok(SurfacePlan {
        policy_version: POLICY_VERSION,
        operations: planned,
        inspections,
        requires_isolation,
    })
}

fn validate_membership(
    spec: &crate::journey::JourneySpec,
    operations: &[CliOperation],
    setup: Option<&SurfaceSetup>,
    bindings: &[SurfaceBinding],
) -> Result<()> {
    let operation_ids: BTreeSet<&str> = operations.iter().map(|op| op.id.as_str()).collect();
    if operation_ids.len() != operations.len() {
        bail!("candidate surface repeats an operation id");
    }
    let mut used = BTreeSet::new();
    if let Some(setup) = setup {
        if setup.graph != SetupGraph::LocalSnapshot {
            bail!("candidate surface setup must use local_snapshot");
        }
        for id in &setup.operations {
            if !operation_ids.contains(id.as_str()) || !used.insert(id.as_str()) {
                bail!("candidate surface has invalid setup operation '{id}'");
            }
        }
    }
    let steps: BTreeSet<&str> = spec.steps.iter().map(|step| step.id.as_str()).collect();
    let mut bound_steps = BTreeSet::new();
    for binding in bindings {
        if !bound_steps.insert(binding.step_id()) || !steps.contains(binding.step_id()) {
            bail!(
                "candidate surface has an invalid binding for '{}'",
                binding.step_id()
            );
        }
        if let Some(id) = binding.operation_id() {
            if !operation_ids.contains(id) || !used.insert(id) {
                bail!("candidate surface has invalid bound operation '{id}'");
            }
        }
    }
    if bound_steps.len() != spec.steps.len() {
        bail!("candidate surface does not bind every authored step");
    }
    if used.len() != operations.len() {
        let unused = operation_ids
            .difference(&used)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        bail!("candidate surface contains unreachable operation(s): {unused}");
    }
    Ok(())
}

fn execution_order<'a>(
    spec: &crate::journey::JourneySpec,
    operations: &'a [CliOperation],
    setup: Option<&SurfaceSetup>,
    bindings: &[SurfaceBinding],
) -> Result<Vec<&'a CliOperation>> {
    let by_id: BTreeMap<&str, &CliOperation> =
        operations.iter().map(|op| (op.id.as_str(), op)).collect();
    let mut ordered = Vec::with_capacity(operations.len());
    if let Some(setup) = setup {
        for id in &setup.operations {
            ordered.push(
                *by_id
                    .get(id.as_str())
                    .ok_or_else(|| anyhow!("missing setup op"))?,
            );
        }
    }
    let by_step: BTreeMap<&str, &SurfaceBinding> = bindings
        .iter()
        .map(|binding| (binding.step_id(), binding))
        .collect();
    for step in &spec.steps {
        let binding = by_step
            .get(step.id.as_str())
            .ok_or_else(|| anyhow!("missing step binding"))?;
        if let Some(id) = binding.operation_id() {
            ordered.push(*by_id.get(id).ok_or_else(|| anyhow!("missing bound op"))?);
        }
    }
    Ok(ordered)
}

fn validate_operation_envelope(operation: &CliOperation, require_bare_loom: bool) -> Result<()> {
    if require_bare_loom && operation.argv.first().map(String::as_str) != Some("loom") {
        bail!(
            "operation '{}' must use exact bare argv0 'loom'",
            operation.id
        );
    }
    let bytes = operation.argv.iter().map(String::len).sum::<usize>();
    if operation.argv.len() > MAX_ARGV_TOKENS || bytes > MAX_ARGV_BYTES {
        bail!(
            "operation '{}' argv exceeds candidate policy limits",
            operation.id
        );
    }
    if operation
        .argv
        .iter()
        .any(|part| matches!(part.as_str(), "--graph" | "--root"))
    {
        bail!(
            "operation '{}' attempts to override runtime confinement",
            operation.id
        );
    }
    for name in &operation.environment {
        if RESERVED_ENVIRONMENT.contains(&name.as_str()) {
            bail!(
                "operation '{}' declares reserved environment name '{name}'",
                operation.id
            );
        }
    }
    for (index, token) in operation.argv.iter().enumerate() {
        if argv_token_source(token)?.is_some()
            && (require_bare_loom || operation.argv.first().map(String::as_str) == Some("loom"))
            && !template_allowed(operation, index)
        {
            bail!(
                "operation '{}' templates a control argv token",
                operation.id
            );
        }
    }
    for argument in &operation.arguments {
        if argument.flag.as_deref() != Some("--command") {
            bail!(
                "operation '{}' has a dynamic argument outside the exact --command data seam",
                operation.id
            );
        }
    }
    Ok(())
}

fn template_allowed(operation: &CliOperation, index: usize) -> bool {
    let without_json = operation
        .argv
        .iter()
        .enumerate()
        .filter(|(_, token)| token.as_str() != "--json")
        .collect::<Vec<_>>();
    let Some(position) = without_json
        .iter()
        .position(|(original, _)| *original == index)
    else {
        return false;
    };
    let tokens = without_json
        .iter()
        .map(|(_, token)| token.as_str())
        .collect::<Vec<_>>();
    matches!(
        (tokens.get(1).copied(), tokens.get(2).copied(), position),
        (Some("door"), _, 2) | (Some("inbox"), Some("show"), 3) | (Some("inbox"), Some("mark"), 3)
    )
}

fn static_argv(operation: &CliOperation) -> Result<Vec<String>> {
    let mut argv = operation.argv.clone();
    for argument in &operation.arguments {
        let flag = argument
            .flag
            .as_ref()
            .ok_or_else(|| anyhow!("dynamic argument requires a fixed flag"))?;
        argv.push(flag.clone());
        argv.push(static_argument_value(argument));
    }
    Ok(argv)
}

fn static_argument_value(argument: &OperationArgument) -> String {
    format!("__loom_runtime_value_{}__", argument.id)
}

fn ensure_resolved_shape(operation: &CliOperation, resolved: &[String]) -> Result<()> {
    let dynamic_base: BTreeSet<usize> = operation
        .argv
        .iter()
        .enumerate()
        .filter_map(|(index, token)| argv_token_source(token).ok().flatten().map(|_| index))
        .collect();
    let expected_len = operation.argv.len() + operation.arguments.len() * 2;
    if resolved.len() != expected_len {
        bail!("operation '{}' resolved argv length changed", operation.id);
    }
    for (index, expected) in operation.argv.iter().enumerate() {
        if !dynamic_base.contains(&index) && resolved[index] != *expected {
            bail!(
                "operation '{}' changed fixed argv token {index}",
                operation.id
            );
        }
        if resolved[index].contains('\0') {
            bail!("operation '{}' resolved a NUL byte", operation.id);
        }
    }
    let mut cursor = operation.argv.len();
    for argument in &operation.arguments {
        let flag = argument.flag.as_ref().expect("validated fixed flag");
        if resolved.get(cursor) != Some(flag) {
            bail!("operation '{}' changed dynamic argument flag", operation.id);
        }
        if resolved
            .get(cursor + 1)
            .is_none_or(|value| value.contains('\0'))
        {
            bail!(
                "operation '{}' has invalid dynamic argument value",
                operation.id
            );
        }
        cursor += 2;
    }
    Ok(())
}

fn parse_cli(argv: &[String]) -> Result<Cli> {
    let cli = Cli::try_parse_from(argv).map_err(|error| anyhow!(error.to_string()))?;
    if cli.graph.is_some() {
        bail!("candidate operation must not choose its graph");
    }
    if !cli.json {
        bail!("candidate operation must explicitly request JSON");
    }
    Ok(cli)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NestedPurpose {
    TopLevel,
    Validation,
    Observe,
}

fn classify_cli(
    cli: &Cli,
    argv: &[String],
    purpose: NestedPurpose,
    depth: usize,
    nested: &mut Vec<NestedInspection>,
) -> Result<DerivedCapability> {
    if depth > MAX_NESTING {
        bail!("candidate operation nesting exceeds policy limit");
    }
    let command = cli
        .command
        .as_ref()
        .ok_or_else(|| anyhow!("candidate operation has no command"))?;
    validate_command_paths(command)?;
    let capability = match command {
        Command::Status
        | Command::Find { .. }
        | Command::Context { .. }
        | Command::Impact { .. }
        | Command::Limits
        | Command::Smells
        | Command::Doctor
        | Command::Coverage
        | Command::Whoami
        | Command::Checkpoint {
            cmd: crate::cli::CheckpointCmd::Recommend { .. },
        } => DerivedCapability::Read,
        Command::Next { .. } => DerivedCapability::Read,
        Command::Absorb { .. } => DerivedCapability::ConfinedMutation,
        Command::Audit { cmd: None, .. } => DerivedCapability::Read,
        Command::Codefile {
            cmd: CodefileCmd::Show { .. } | CodefileCmd::Anchor { .. },
        } => DerivedCapability::Read,
        Command::Intent {
            cmd: IntentCmd::Show { .. } | IntentCmd::List { .. } | IntentCmd::Dependents { .. },
        } => DerivedCapability::Read,
        Command::Journey {
            cmd:
                JourneyCmd::Show { .. }
                | JourneyCmd::Lint { .. }
                | JourneyCmd::List { .. }
                | JourneyCmd::Map
                | JourneyCmd::Derive { .. }
                | JourneyCmd::Surface { .. }
                | JourneyCmd::Drift { .. },
        } => DerivedCapability::Read,
        Command::Journey {
            cmd: JourneyCmd::RehearseCold { .. },
        } => DerivedCapability::DetachedProcess,
        Command::Validation {
            cmd: ValidationCmd::Show { .. } | ValidationCmd::List { .. },
        } => DerivedCapability::Read,
        Command::Inbox {
            cmd: InboxCmd::Show { .. } | InboxCmd::List { .. },
        } => DerivedCapability::Read,
        Command::Release {
            cmd: ReleaseCmd::Rehearse { .. },
        } => DerivedCapability::DetachedProcess,
        Command::Sync { .. }
        | Command::Door { .. }
        | Command::Finding {
            cmd: FindingCmd::Add { .. },
        }
        | Command::Ignore {
            cmd: IgnoreCmd::Add { .. } | IgnoreCmd::Remove { .. },
        }
        | Command::Debt {
            cmd: Some(DebtCmd::Promote { .. }),
        }
        | Command::Codefile {
            cmd: CodefileCmd::Add { .. } | CodefileCmd::Remove { .. },
        }
        | Command::Edge {
            cmd: EdgeCmd::Implement { .. } | EdgeCmd::SetLocator { .. },
        }
        | Command::Inbox {
            cmd: InboxCmd::Mark { .. },
        }
        | Command::Intent {
            cmd: IntentCmd::Add { .. } | IntentCmd::Update { .. } | IntentCmd::Impact { .. },
        }
        | Command::Validation {
            cmd:
                ValidationCmd::Add { .. } | ValidationCmd::Run { .. } | ValidationCmd::Remove { .. },
        } => DerivedCapability::ConfinedMutation,
        Command::Mode { mode } if mode.is_some() => DerivedCapability::ConfinedMutation,
        Command::Mode { mode: None } => DerivedCapability::Read,
        Command::Mcp {
            cmd: McpCmd::Transcript { requests_json },
        } => classify_mcp(requests_json, depth + 1, nested)?,
        Command::Intent {
            cmd:
                IntentCmd::Ratify {
                    human_decision: None,
                    ..
                },
        } if purpose == NestedPurpose::Observe => DerivedCapability::ConfinedMutation,
        _ => bail!(
            "candidate operation is outside the exhaustive current capability policy: {argv:?}"
        ),
    };
    if purpose == NestedPurpose::Validation
        && (matches!(
            command,
            Command::Validation { .. } | Command::Mcp { .. } | Command::Release { .. }
        ) || matches!(
            command,
            Command::Journey {
                cmd: JourneyCmd::RehearseCold { .. }
            }
        ))
    {
        bail!("Validation payload cannot open another execution carrier");
    }
    if purpose == NestedPurpose::Observe
        && (matches!(
            command,
            Command::Observe { .. } | Command::Mcp { .. } | Command::Release { .. }
        ) || matches!(
            command,
            Command::Journey {
                cmd: JourneyCmd::RehearseCold { .. }
            }
        ))
    {
        bail!("MCP observation payload cannot recurse into another execution carrier");
    }
    Ok(capability)
}

fn validate_command_paths(command: &Command) -> Result<()> {
    match command {
        Command::Codefile {
            cmd: CodefileCmd::Add { path, .. } | CodefileCmd::Anchor { path, .. },
        } => validate_codefile_path(path),
        Command::Codefile {
            cmd: CodefileCmd::Show { key } | CodefileCmd::Remove { key, .. },
        } if key.contains('/') || key.starts_with('.') => validate_codefile_path(key),
        Command::Edge {
            cmd: EdgeCmd::Implement { codefile, .. },
        } => validate_codefile_path(codefile),
        Command::Finding {
            cmd: FindingCmd::Add {
                file: Some(file), ..
            },
        } => validate_codefile_path(file),
        _ => Ok(()),
    }
}

fn classify_mcp(
    requests_json: &str,
    depth: usize,
    nested: &mut Vec<NestedInspection>,
) -> Result<DerivedCapability> {
    let requests = crate::mcp::inspect_transcript_requests(requests_json)?;
    let mut capability = DerivedCapability::Read;
    for request in requests {
        use crate::mcp::{InspectedMcpRequestKind, McpTranscriptEffect};
        match request.kind {
            InspectedMcpRequestKind::Initialize
            | InspectedMcpRequestKind::ToolsList
            | InspectedMcpRequestKind::Ping => {}
            InspectedMcpRequestKind::UnknownTool { name } => nested.push(NestedInspection {
                source: format!("mcp:{}:unknown:{name}", request.index),
                capability: DerivedCapability::Read,
                outcome: "expected_protocol_rejection".into(),
            }),
            InspectedMcpRequestKind::ToolCall {
                tool,
                effect,
                arguments,
                nested_argv,
            } => match effect {
                McpTranscriptEffect::Read => nested.push(NestedInspection {
                    source: format!("mcp:{}:{tool}", request.index),
                    capability: DerivedCapability::Read,
                    outcome: "strict_read".into(),
                }),
                McpTranscriptEffect::ObserveArgv => {
                    let argv =
                        nested_argv.ok_or_else(|| anyhow!("observe request omitted argv"))?;
                    if argv.first().map(String::as_str) != Some("loom") {
                        bail!("MCP observe payload must use exact bare argv0 'loom'");
                    }
                    let parsed = parse_cli(&argv)?;
                    let nested_capability =
                        classify_cli(&parsed, &argv, NestedPurpose::Observe, depth + 1, nested)?;
                    nested.push(NestedInspection {
                        source: format!("mcp:{}:{tool}:argv", request.index),
                        capability: nested_capability,
                        outcome: "strict_nested_argv".into(),
                    });
                    capability = DerivedCapability::ConfinedMutation;
                }
                McpTranscriptEffect::ApplyFragment => {
                    inspect_apply_fragment(
                        arguments
                            .get("fragment")
                            .ok_or_else(|| anyhow!("MCP apply omitted fragment"))?,
                    )?;
                    nested.push(NestedInspection {
                        source: format!("mcp:{}:{tool}:fragment", request.index),
                        capability: DerivedCapability::ConfinedMutation,
                        outcome: "strict_confined_apply_fragment".into(),
                    });
                    capability = DerivedCapability::ConfinedMutation;
                }
            },
        }
    }
    Ok(capability)
}

fn inspect_apply_fragment(fragment: &Value) -> Result<()> {
    let object = fragment
        .as_object()
        .ok_or_else(|| anyhow!("candidate MCP apply fragment must be an object"))?;
    if object.keys().any(|key| key != "adjudications") {
        bail!("candidate MCP apply may contain only confined adjudications");
    }
    let adjudications = object
        .get("adjudications")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("candidate MCP apply requires adjudications array"))?;
    if adjudications.is_empty() || adjudications.len() > 128 {
        bail!("candidate MCP apply adjudication count is outside policy limits");
    }
    for adjudication in adjudications {
        let fields = adjudication
            .as_object()
            .ok_or_else(|| anyhow!("candidate adjudication must be an object"))?;
        let keys: BTreeSet<&str> = fields.keys().map(String::as_str).collect();
        let allowed = BTreeSet::from(["finding", "verdict", "reason", "evidence"]);
        if !keys.is_subset(&allowed)
            || !["finding", "verdict", "reason"].iter().all(|key| {
                fields
                    .get(*key)
                    .and_then(Value::as_str)
                    .is_some_and(|v| !v.is_empty())
            })
        {
            bail!("candidate adjudication has unknown or missing fields");
        }
    }
    Ok(())
}

fn validation_run_names(operations: &[&CliOperation]) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for operation in operations {
        if operation.argv.first().map(String::as_str) != Some("loom") {
            continue;
        }
        let parsed = parse_cli(&static_argv(operation)?)?;
        if let Some(name) = validation_run(&parsed) {
            if name.is_empty() {
                bail!("candidate Validation run must name one exact registration");
            }
            names.insert(name);
        }
    }
    Ok(names)
}

fn validation_registration(cli: &Cli) -> Option<(String, String)> {
    match cli.command.as_ref()? {
        Command::Validation {
            cmd: ValidationCmd::Add { name, command, .. },
        } => Some((name.clone(), command.clone())),
        _ => None,
    }
}

fn validation_run(cli: &Cli) -> Option<String> {
    match cli.command.as_ref()? {
        Command::Validation {
            cmd: ValidationCmd::Run { key, all: false },
        } => Some(key.clone()),
        _ => None,
    }
}

fn classify_validation_payload(command: &str, depth: usize) -> Result<DerivedCapability> {
    let argv = crate::subprocess::strict_simple_tokens(command)
        .ok_or_else(|| anyhow!("Validation command payload is not strict simple argv"))?;
    if argv.first().map(String::as_str) != Some("loom") {
        bail!("executed Validation payload must use exact bare argv0 'loom'");
    }
    let cli = parse_cli(&argv)?;
    classify_cli(
        &cli,
        &argv,
        NestedPurpose::Validation,
        depth,
        &mut Vec::new(),
    )
}

fn validate_detached_profile_environment(
    spec: &crate::journey::JourneySpec,
    mode: PolicyMode<'_>,
) -> Result<()> {
    if !matches!(mode, PolicyMode::DetachedReleaseInspection { .. }) {
        return Ok(());
    }
    for (profile_id, profile) in &spec.profiles {
        if let Some(name) = profile.workspace.env.keys().next() {
            bail!(
                "detached candidate profile '{profile_id}' declares workspace environment name '{name}'"
            );
        }
    }
    Ok(())
}

fn validate_detached_operation_environment(
    operation: &CliOperation,
    exact_outer: bool,
    mode: PolicyMode<'_>,
) -> Result<()> {
    if !matches!(mode, PolicyMode::DetachedReleaseInspection { .. })
        || operation.environment.is_empty()
    {
        return Ok(());
    }
    if exact_outer
        && operation
            .environment
            .iter()
            .map(String::as_str)
            .eq(DETACHED_OUTER_ENVIRONMENT.iter().copied())
    {
        return Ok(());
    }
    let name = operation.environment.first().expect("nonempty checked");
    bail!(
        "detached candidate operation '{}' declares environment name '{name}' outside the exact outer release allowance",
        operation.id
    )
}

fn is_exact_outer(journey_id: &str, surface_id: &str, cli: &Cli, mode: PolicyMode<'_>) -> bool {
    let PolicyMode::DetachedReleaseInspection {
        outer_journey_id,
        outer_surface_id,
    } = mode
    else {
        return false;
    };
    journey_id == outer_journey_id
        && surface_id == outer_surface_id
        && matches!(
            cli.command,
            Some(Command::Release {
                cmd: ReleaseCmd::Rehearse { .. }
            })
        )
}

fn validate_codefile_path(raw: &str) -> Result<()> {
    let path = Path::new(raw);
    if raw.is_empty()
        || raw.trim() != raw
        || raw.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("candidate surface CodeFile path is not repository-confined");
    }
    let first = path
        .components()
        .next()
        .and_then(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        });
    if first.is_some_and(|name| matches!(name, ".git" | ".loom" | "target")) {
        bail!("candidate surface CodeFile path enters a reserved root");
    }
    Ok(())
}

pub fn reserved_runtime_environment(name: &str) -> bool {
    RESERVED_ENVIRONMENT.contains(&name)
}
