use crate::journey::{
    CliOperation, HumanDecisionSource, JourneyInput, JourneySpec, OperationArgument,
    OperationBinding, OutputAssertion, RuntimeSource, SetupGraph, SurfaceBinding,
    SurfaceFileAction, SurfaceSetup, ValueType, COMPILED_PROOF_SCHEMA, JOURNEY_COMPILER_VERSION,
};
use crate::Result;
use anyhow::{anyhow, bail, Context};
use std::collections::{BTreeMap, BTreeSet};

use super::types::{
    CompiledHumanDecision, CompiledJourneyProof, CompiledProfileShape, CompiledSetup,
    CompiledSetupOperation, CompiledStep,
};
use super::values::{canonicalize, profile_for};

pub fn compile(
    spec: &JourneySpec,
    surface_hash: &str,
    profile: &str,
    operations: Vec<CliOperation>,
    bindings: &[OperationBinding],
) -> Result<CompiledJourneyProof> {
    compile_with_setup(spec, surface_hash, profile, operations, None, bindings)
}

pub fn compile_with_setup(
    spec: &JourneySpec,
    surface_hash: &str,
    profile: &str,
    operations: Vec<CliOperation>,
    setup: Option<&SurfaceSetup>,
    bindings: &[OperationBinding],
) -> Result<CompiledJourneyProof> {
    let bindings: Vec<SurfaceBinding> = bindings.iter().cloned().map(Into::into).collect();
    compile_surface(spec, surface_hash, profile, operations, setup, &bindings)
}

/// Compile the exact accepted manifest binding union. Direct callers that
/// only need CLI operations keep using [`compile_with_setup`]; the command
/// adapter uses this entry point so intrinsic human gates survive compilation.
pub fn compile_surface(
    spec: &JourneySpec,
    surface_hash: &str,
    profile: &str,
    operations: Vec<CliOperation>,
    setup: Option<&SurfaceSetup>,
    bindings: &[SurfaceBinding],
) -> Result<CompiledJourneyProof> {
    spec.validate()?;
    for operation in &operations {
        if operation.timeout_seconds == Some(0) {
            bail!(
                "operation '{}' timeout_seconds must be positive",
                operation.id
            );
        }
    }
    // Derive capabilities from the typed operations before compiling them.
    // `read_only` is checked by this policy; it never decides confinement by
    // itself.
    crate::candidate_surface_policy::inspect_surface(
        spec,
        &spec.id,
        "compiled-surface",
        &operations,
        setup,
        bindings,
        crate::candidate_surface_policy::PolicyMode::Runtime,
    )?;
    let profile_name = profile;
    let profile = profile_for(spec, profile)?;
    if bindings.len() != spec.steps.len() {
        bail!(
            "compiled Journey requires exactly one primary operation per step ({} steps, {} bindings)",
            spec.steps.len(),
            bindings.len()
        );
    }
    let unique_steps: BTreeSet<&str> = bindings.iter().map(SurfaceBinding::step_id).collect();
    let unique_operations: BTreeSet<&str> = bindings
        .iter()
        .filter_map(SurfaceBinding::operation_id)
        .collect();
    let operation_binding_count = bindings
        .iter()
        .filter(|binding| binding.operation_id().is_some())
        .count();
    if unique_steps.len() != bindings.len() || unique_operations.len() != operation_binding_count {
        bail!("compiled Journey bindings repeat a step or primary operation");
    }
    let by_operation: BTreeMap<&str, &CliOperation> = operations
        .iter()
        .map(|operation| (operation.id.as_str(), operation))
        .collect();
    let by_step: BTreeMap<&str, &SurfaceBinding> = bindings
        .iter()
        .map(|binding| (binding.step_id(), binding))
        .collect();
    let input_by_id: BTreeMap<&str, &JourneyInput> = spec
        .inputs
        .iter()
        .map(|(id, input)| (id.as_str(), input))
        .collect();
    let has_human_decision = bindings
        .iter()
        .any(|binding| binding.human_decision().is_some());
    if has_human_decision && setup.is_none() {
        bail!("compiled human decision bindings require setup.graph=local_snapshot");
    }
    let compiled_setup = setup
        .map(|setup| {
            if setup.operations.is_empty()
                && !setup
                    .before_steps
                    .values()
                    .any(|actions| !actions.is_empty())
                && !has_human_decision
            {
                bail!(
                    "compiled Journey setup must contain an operation or before_steps file action"
                );
            }
            let bound_operations: BTreeSet<&str> =
                bindings.iter().filter_map(SurfaceBinding::operation_id).collect();
            let authored_steps: BTreeSet<&str> =
                spec.steps.iter().map(|step| step.id.as_str()).collect();
            for (step_id, actions) in &setup.before_steps {
                if !authored_steps.contains(step_id.as_str()) {
                    bail!("compiled Journey before_steps references unknown step '{step_id}'");
                }
                if actions.is_empty() {
                    bail!("compiled Journey before_steps.{step_id} has no file action");
                }
                let mut paths = BTreeSet::new();
                for action in actions {
                    action.validate()?;
                    if !paths.insert(action.path.as_str()) {
                        bail!(
                            "compiled Journey before_steps.{step_id} repeats path '{}'",
                            action.path
                        );
                    }
                }
            }
            let mut setup_ids = BTreeSet::new();
            let no_outputs = BTreeMap::new();
            let mut compiled = Vec::with_capacity(setup.operations.len());
            for operation_id in &setup.operations {
                if !setup_ids.insert(operation_id.as_str()) {
                    bail!("compiled Journey setup repeats operation '{operation_id}'");
                }
                if bound_operations.contains(operation_id.as_str()) {
                    bail!(
                        "compiled Journey setup operation '{operation_id}' is also a primary step operation"
                    );
                }
                let operation = by_operation.get(operation_id.as_str()).ok_or_else(|| {
                    anyhow!("compiled Journey setup operation '{operation_id}' is missing")
                })?;
                if operation.read_only {
                    bail!("compiled Journey setup operation '{operation_id}' must be mutable");
                }
                if !operation.output.captures.is_empty() {
                    bail!(
                        "compiled Journey setup operation '{operation_id}' must not capture authored outputs"
                    );
                }
                if operation.output.assertions.is_empty() {
                    bail!(
                        "compiled Journey setup operation '{operation_id}' must assert its fixture"
                    );
                }
                validate_sources(
                    &operation.argv,
                    &operation.arguments,
                    &operation.output.assertions,
                    &input_by_id,
                    &no_outputs,
                )?;
                compiled.push(CompiledSetupOperation {
                    operation_id: operation.id.clone(),
                    argv: operation.argv.clone(),
                    environment: canonical_environment(&operation.environment)?,
                    read_only: operation.read_only,
                    timeout_seconds: operation.timeout_seconds.unwrap_or(profile.timeout_seconds),
                    expected_exit: operation.expected_exit,
                    arguments: operation.arguments.clone(),
                    assertions: operation.output.assertions.clone(),
                    redact: operation.output.redact.clone(),
                });
            }
            Ok(CompiledSetup {
                graph: setup.graph,
                git: setup.git.clone(),
                before_steps: setup.before_steps.clone(),
                operations: compiled,
            })
        })
        .transpose()?;
    let mut available_outputs = BTreeMap::new();
    let mut steps = Vec::with_capacity(spec.steps.len());
    let mut assertion_count = 0usize;

    for semantic_step in &spec.steps {
        if let Some(actions) = setup.and_then(|setup| setup.before_steps.get(&semantic_step.id)) {
            for action in actions {
                validate_temporal_sources(action, &input_by_id, &available_outputs).with_context(
                    || {
                        format!(
                            "compiled before_steps.{} path '{}'",
                            semantic_step.id, action.path
                        )
                    },
                )?;
            }
        }
        let binding = by_step.get(semantic_step.id.as_str()).ok_or_else(|| {
            anyhow!(
                "Journey step '{}' has no primary operation",
                semantic_step.id
            )
        })?;
        match binding {
            SurfaceBinding::Operation(binding) => {
                let operation =
                    by_operation
                        .get(binding.operation_id.as_str())
                        .ok_or_else(|| {
                            anyhow!("surface operation '{}' is missing", binding.operation_id)
                        })?;
                validate_sources(
                    &operation.argv,
                    &operation.arguments,
                    &operation.output.assertions,
                    &input_by_id,
                    &available_outputs,
                )?;
                for capture in &operation.output.captures {
                    available_outputs.insert(
                        format!("steps.{}.outputs.{}", semantic_step.id, capture.id),
                        (capture.value_type, capture.redact),
                    );
                }
                assertion_count += operation.output.assertions.len();
                steps.push(CompiledStep {
                    step_id: semantic_step.id.clone(),
                    operation_id: operation.id.clone(),
                    argv: operation.argv.clone(),
                    environment: canonical_environment(&operation.environment)?,
                    read_only: operation.read_only,
                    timeout_seconds: Some(
                        operation.timeout_seconds.unwrap_or(profile.timeout_seconds),
                    ),
                    expected_exit: operation.expected_exit,
                    arguments: operation.arguments.clone(),
                    captures: operation.output.captures.clone(),
                    assertions: operation.output.assertions.clone(),
                    redact: operation.output.redact.clone(),
                    human_decision: None,
                });
            }
            SurfaceBinding::HumanDecision(binding) => {
                compile_human_decision_step(semantic_step, &binding.human_decision, &steps)?;
                steps.push(CompiledStep {
                    step_id: semantic_step.id.clone(),
                    operation_id: "human-decision".into(),
                    argv: Vec::new(),
                    environment: Vec::new(),
                    read_only: true,
                    timeout_seconds: None,
                    expected_exit: 0,
                    arguments: Vec::new(),
                    captures: Vec::new(),
                    assertions: Vec::new(),
                    redact: Vec::new(),
                    human_decision: Some(CompiledHumanDecision {
                        source_operation_id: binding.human_decision.operation_id.clone(),
                        pointer: binding.human_decision.pointer.clone(),
                    }),
                });
            }
        }
    }
    if assertion_count == 0 {
        bail!(
            "Journey '{}' surface has no typed output assertions; a compiled proof must check content",
            spec.id
        );
    }

    // Defaults are valid runtime sources even when the selected profile does
    // not override them, so the compiled shape records every authored input.
    let mut profile_input_ids: Vec<String> = spec.inputs.keys().cloned().collect();
    profile_input_ids.sort();
    let mut setup_directories = profile.workspace.directories.clone();
    setup_directories.sort();
    setup_directories.dedup();
    let mut setup_files: Vec<String> = profile
        .workspace
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    setup_files.sort();
    let setup_env = profile.workspace.env.keys().cloned().collect();

    Ok(CompiledJourneyProof {
        schema: COMPILED_PROOF_SCHEMA.into(),
        compiler_version: JOURNEY_COMPILER_VERSION.into(),
        journey_id: spec.id.clone(),
        journey_hash: spec.semantic_hash()?,
        surface_hash: surface_hash.into(),
        profile: profile_name.to_string(),
        profile_shape: CompiledProfileShape {
            input_ids: profile_input_ids,
            setup_directories,
            setup_files,
            setup_env,
        },
        setup: compiled_setup,
        steps,
    })
}

fn canonical_environment(environment: &[String]) -> Result<Vec<String>> {
    let mut environment = environment.to_vec();
    environment.sort();
    validate_compiled_environment("compiled operation", &environment)?;
    Ok(environment)
}

fn compile_human_decision_step(
    semantic_step: &crate::journey::JourneyStep,
    source: &HumanDecisionSource,
    prior_steps: &[CompiledStep],
) -> Result<()> {
    source.validate()?;
    if !prior_steps
        .iter()
        .any(|step| step.human_decision.is_none() && step.operation_id == source.operation_id)
    {
        bail!(
            "human decision step '{}' must reference an operation bound to an earlier authored step (found '{}')",
            semantic_step.id,
            source.operation_id
        );
    }
    if !semantic_step.produces.is_empty() {
        bail!(
            "human decision step '{}' cannot declare produced machine outputs",
            semantic_step.id
        );
    }
    Ok(())
}

fn validate_sources(
    argv: &[String],
    arguments: &[OperationArgument],
    assertions: &[OutputAssertion],
    inputs: &BTreeMap<&str, &JourneyInput>,
    prior_outputs: &BTreeMap<String, (ValueType, bool)>,
) -> Result<()> {
    for (index, token) in argv.iter().enumerate() {
        if let Some(source) = crate::journey::argv_token_source(token)? {
            if index == 0 {
                bail!("compiled Journey executable cannot be a runtime argv template");
            }
            validate_scalar_source(source, inputs, prior_outputs, false)
                .with_context(|| format!("argv token #{} source is unavailable", index + 1))?;
        }
    }
    for argument in arguments {
        let default_source = format!("inputs.{}", argument.id);
        let source = argument.source.as_deref().unwrap_or(&default_source);
        validate_source_reference(source, inputs, prior_outputs)
            .with_context(|| format!("argument '{}' source is unavailable", argument.id))?;
        if let RuntimeSource::Input(id) = crate::journey::parse_runtime_source(source)? {
            if inputs.get(id).is_some_and(|input| input.secret) {
                bail!(
                    "argument '{}' reads secret input '{}'; secret inputs are environment-only and must not enter CLI argv",
                    argument.id,
                    id
                );
            }
        }
    }
    for assertion in assertions {
        if let Some(source) = assertion.runtime_source() {
            validate_source_reference(source, inputs, prior_outputs)
                .with_context(|| format!("assertion '{}' source is unavailable", assertion.id))?;
        }
    }
    Ok(())
}

fn validate_temporal_sources(
    action: &SurfaceFileAction,
    inputs: &BTreeMap<&str, &JourneyInput>,
    prior_outputs: &BTreeMap<String, (ValueType, bool)>,
) -> Result<()> {
    action.validate()?;
    let Some(template) = &action.template else {
        return Ok(());
    };
    for source in crate::journey::template_references(template)? {
        validate_scalar_source(source, inputs, prior_outputs, true)?;
    }
    Ok(())
}

fn validate_scalar_source(
    source: &str,
    inputs: &BTreeMap<&str, &JourneyInput>,
    prior_outputs: &BTreeMap<String, (ValueType, bool)>,
    allow_run_id: bool,
) -> Result<()> {
    validate_source_reference(source, inputs, prior_outputs)?;
    match crate::journey::parse_runtime_source(source)? {
        RuntimeSource::RunId if allow_run_id => Ok(()),
        RuntimeSource::RunId => bail!("run.id cannot replace an argv token"),
        RuntimeSource::Input(id) => {
            let input = inputs
                .get(id)
                .expect("source existence validated before scalar policy");
            if input.secret {
                bail!("secret input '{id}' cannot enter argv or file content");
            }
            if !input.value_type.is_scalar() {
                bail!("input '{id}' is not scalar");
            }
            Ok(())
        }
        RuntimeSource::StepOutput { .. } => {
            let (value_type, redact) = prior_outputs
                .get(source)
                .expect("source availability validated before scalar policy");
            if *redact {
                bail!("redacted output '{source}' cannot enter argv or file content");
            }
            if !value_type.is_scalar() {
                bail!("output '{source}' is not scalar");
            }
            Ok(())
        }
    }
}

fn validate_source_reference(
    source: &str,
    inputs: &BTreeMap<&str, &JourneyInput>,
    prior_outputs: &BTreeMap<String, (ValueType, bool)>,
) -> Result<()> {
    match crate::journey::parse_runtime_source(source)? {
        RuntimeSource::RunId => Ok(()),
        RuntimeSource::Input(id) if inputs.contains_key(id) => Ok(()),
        RuntimeSource::Input(id) => bail!("unknown Journey input '{id}'"),
        RuntimeSource::StepOutput { .. } if prior_outputs.contains_key(source) => Ok(()),
        RuntimeSource::StepOutput { .. } => {
            bail!("'{source}' is not available from an earlier step")
        }
    }
}

pub fn canonical_bytes(proof: &CompiledJourneyProof) -> Result<Vec<u8>> {
    proof.validate()?;
    let canonical = canonicalize(serde_json::to_value(proof)?);
    let mut bytes = serde_json::to_vec_pretty(&canonical)?;
    bytes.push(b'\n');
    Ok(bytes)
}

impl CompiledJourneyProof {
    pub fn validate(&self) -> Result<()> {
        if self.schema != COMPILED_PROOF_SCHEMA {
            bail!("unsupported compiled Journey schema '{}'", self.schema);
        }
        if self.compiler_version != JOURNEY_COMPILER_VERSION {
            bail!(
                "compiled Journey compiler version '{}' is not current ('{}')",
                self.compiler_version,
                JOURNEY_COMPILER_VERSION
            );
        }
        crate::journey::validate_stable_id("journey", &self.journey_id)?;
        crate::journey::validate_stable_id("profile", &self.profile)?;
        if self.journey_hash.trim().is_empty() || self.surface_hash.trim().is_empty() {
            bail!("compiled Journey hashes must not be empty");
        }
        if self.steps.is_empty() {
            bail!("compiled Journey must contain at least one step");
        }
        if self.steps.iter().any(|step| {
            step.human_decision.is_some() != step.timeout_seconds.is_none()
                || step.timeout_seconds == Some(0)
        }) {
            bail!(
                "compiled Journey machine timeouts must be positive and human gates must have none"
            );
        }
        validate_compiled_step_shapes(self)?;
        if let Some(setup) = &self.setup {
            if setup
                .operations
                .iter()
                .any(|operation| operation.timeout_seconds == 0)
            {
                bail!("compiled setup operation timeout_seconds must be positive");
            }
            if setup.operations.is_empty()
                && !setup
                    .before_steps
                    .values()
                    .any(|actions| !actions.is_empty())
                && !self.steps.iter().any(|step| step.human_decision.is_some())
            {
                bail!(
                    "compiled Journey setup must contain an operation or before_steps file action"
                );
            }
            if let Some(git) = &setup.git {
                match setup.graph {
                    SetupGraph::LocalSnapshot => git.validate()?,
                }
            }
            let mut ids = BTreeSet::new();
            let step_operations: BTreeSet<&str> = self
                .steps
                .iter()
                .filter(|step| step.human_decision.is_none())
                .map(|step| step.operation_id.as_str())
                .collect();
            let step_ids: BTreeSet<&str> = self
                .steps
                .iter()
                .map(|step| step.step_id.as_str())
                .collect();
            for (step_id, actions) in &setup.before_steps {
                if !step_ids.contains(step_id.as_str()) {
                    bail!("compiled Journey before_steps references unknown step '{step_id}'");
                }
                if actions.is_empty() {
                    bail!("compiled Journey before_steps.{step_id} has no file action");
                }
                let mut paths = BTreeSet::new();
                for action in actions {
                    action.validate()?;
                    if !paths.insert(action.path.as_str()) {
                        bail!(
                            "compiled Journey before_steps.{step_id} repeats path '{}'",
                            action.path
                        );
                    }
                }
            }
            for operation in &setup.operations {
                crate::journey::validate_stable_id("setup operation", &operation.operation_id)?;
                if !ids.insert(operation.operation_id.as_str()) {
                    bail!(
                        "compiled Journey setup repeats operation '{}'",
                        operation.operation_id
                    );
                }
                if operation.read_only {
                    bail!(
                        "compiled Journey setup operation '{}' must be mutable",
                        operation.operation_id
                    );
                }
                if operation.assertions.is_empty() {
                    bail!(
                        "compiled Journey setup operation '{}' has no fixture assertion",
                        operation.operation_id
                    );
                }
                validate_compiled_environment(
                    &format!("compiled setup operation '{}'", operation.operation_id),
                    &operation.environment,
                )?;
                if step_operations.contains(operation.operation_id.as_str()) {
                    bail!(
                        "compiled Journey setup operation '{}' is also a primary step operation",
                        operation.operation_id
                    );
                }
            }
        }
        validate_compiled_runtime_sources(self)?;
        Ok(())
    }
}

fn validate_compiled_step_shapes(proof: &CompiledJourneyProof) -> Result<()> {
    let mut step_ids = BTreeSet::new();
    let mut prior_operations = BTreeSet::new();
    let mut operation_ids = BTreeSet::new();
    let mut semantic_assertions = 0usize;
    for step in &proof.steps {
        crate::journey::validate_stable_id("compiled Journey step", &step.step_id)?;
        if !step_ids.insert(step.step_id.as_str()) {
            bail!("compiled Journey repeats step '{}'", step.step_id);
        }
        match &step.human_decision {
            Some(gate) => {
                if step.operation_id != "human-decision"
                    || !step.argv.is_empty()
                    || !step.environment.is_empty()
                    || !step.arguments.is_empty()
                    || !step.captures.is_empty()
                    || !step.assertions.is_empty()
                    || !step.redact.is_empty()
                    || !step.read_only
                {
                    bail!(
                        "compiled human decision step '{}' must not contain a CLI operation, arguments, captures, assertions, or redactions",
                        step.step_id
                    );
                }
                let source = HumanDecisionSource {
                    operation_id: gate.source_operation_id.clone(),
                    pointer: gate.pointer.clone(),
                };
                source.validate()?;
                if !prior_operations.contains(gate.source_operation_id.as_str()) {
                    bail!(
                        "compiled human decision step '{}' references non-prior operation '{}'",
                        step.step_id,
                        gate.source_operation_id
                    );
                }
            }
            None => {
                crate::journey::validate_stable_id(
                    "compiled Journey operation",
                    &step.operation_id,
                )?;
                if step.argv.is_empty() || step.argv.iter().any(String::is_empty) {
                    bail!(
                        "compiled Journey operation '{}' has empty argv",
                        step.operation_id
                    );
                }
                validate_compiled_environment(
                    &format!("compiled Journey operation '{}'", step.operation_id),
                    &step.environment,
                )?;
                if !operation_ids.insert(step.operation_id.as_str()) {
                    bail!(
                        "compiled Journey repeats primary operation '{}'",
                        step.operation_id
                    );
                }
                semantic_assertions += step.assertions.len();
                prior_operations.insert(step.operation_id.as_str());
            }
        }
    }
    if semantic_assertions == 0 {
        bail!("compiled Journey has no typed output assertion");
    }
    Ok(())
}

fn validate_compiled_environment(label: &str, environment: &[String]) -> Result<()> {
    let mut previous: Option<&str> = None;
    for name in environment {
        crate::journey::validate_process_environment_name(name)
            .with_context(|| format!("{label} has invalid environment declaration"))?;
        if previous.is_some_and(|prior| prior >= name.as_str()) {
            bail!("{label} environment names must be unique and canonically ordered");
        }
        previous = Some(name);
    }
    Ok(())
}

fn validate_compiled_runtime_sources(proof: &CompiledJourneyProof) -> Result<()> {
    let input_ids: BTreeSet<&str> = proof
        .profile_shape
        .input_ids
        .iter()
        .map(String::as_str)
        .collect();
    if let Some(setup) = &proof.setup {
        for operation in &setup.operations {
            for (index, token) in operation.argv.iter().enumerate() {
                if let Some(source) = crate::journey::argv_token_source(token)? {
                    validate_compiled_scalar_source(source, &input_ids, &BTreeMap::new(), false)
                        .with_context(|| {
                            format!(
                                "compiled setup operation '{}' argv token #{}",
                                operation.operation_id,
                                index + 1
                            )
                        })?;
                }
            }
        }
    }
    let mut prior_outputs = BTreeMap::new();
    for step in &proof.steps {
        if let Some(actions) = proof
            .setup
            .as_ref()
            .and_then(|setup| setup.before_steps.get(&step.step_id))
        {
            for action in actions {
                if let Some(template) = &action.template {
                    for source in crate::journey::template_references(template)? {
                        validate_compiled_scalar_source(source, &input_ids, &prior_outputs, true)?;
                    }
                }
            }
        }
        for (index, token) in step.argv.iter().enumerate() {
            if let Some(source) = crate::journey::argv_token_source(token)? {
                validate_compiled_scalar_source(source, &input_ids, &prior_outputs, false)
                    .with_context(|| {
                        format!("compiled step '{}' argv token #{}", step.step_id, index + 1)
                    })?;
            }
        }
        for capture in &step.captures {
            prior_outputs.insert(
                format!("steps.{}.outputs.{}", step.step_id, capture.id),
                (capture.value_type, capture.redact),
            );
        }
    }
    Ok(())
}

fn validate_compiled_scalar_source(
    source: &str,
    input_ids: &BTreeSet<&str>,
    prior_outputs: &BTreeMap<String, (ValueType, bool)>,
    allow_run_id: bool,
) -> Result<()> {
    match crate::journey::parse_runtime_source(source)? {
        RuntimeSource::RunId if allow_run_id => Ok(()),
        RuntimeSource::RunId => bail!("run.id cannot replace an argv token"),
        RuntimeSource::Input(id) if input_ids.contains(id) => Ok(()),
        RuntimeSource::Input(id) => bail!("unknown Journey input '{id}'"),
        RuntimeSource::StepOutput { .. } => {
            let Some((value_type, redact)) = prior_outputs.get(source) else {
                bail!("'{source}' is not available from an earlier step");
            };
            if *redact {
                bail!("redacted output '{source}' cannot enter argv or file content");
            }
            if !value_type.is_scalar() {
                bail!("output '{source}' is not scalar");
            }
            Ok(())
        }
    }
}
