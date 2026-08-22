use crate::journey::JourneySpec;
use crate::Result;
use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::compile::canonical_bytes;
use super::execute::{run_steps, ActiveRun};
use super::process::TemporaryRoot;
use super::types::{
    CompiledHumanDecision, CompiledJourneyProof, CompiledStep, ExecutionOutcome,
    PendingContinuation, StepReport,
};
use super::values::canonicalize;

const CONTINUATION_RUNTIME_SCHEMA: &str = "loom.journey-runtime-continuation/v2";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinuationState {
    schema: String,
    live_root: PathBuf,
    spec: JourneySpec,
    proof: CompiledJourneyProof,
    gate_binding: crate::journey_gate::GateBinding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_subject: Option<CurrentSubjectAnchor>,
    step_index: usize,
    active: ActiveRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentSubjectAnchor {
    kind: String,
    id: String,
    name: String,
    description: String,
    hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservedHumanPrompt {
    subject: crate::journey_gate::GateSubject,
    question: String,
    recommendation: String,
    options: Vec<crate::journey_gate::HumanOption>,
}

fn normalize_human_prompt(
    observed: &Value,
) -> Result<(
    crate::journey_gate::GateSubject,
    crate::journey_gate::HumanPrompt,
    bool,
    Option<CurrentSubjectAnchor>,
)> {
    if let Ok(prompt) = serde_json::from_value::<ObservedHumanPrompt>(observed.clone()) {
        let prompt = crate::journey_gate::HumanPrompt::new(
            prompt.question,
            prompt.recommendation,
            prompt.options,
        )?;
        return Ok((prompt_subject(observed)?, prompt, false, None));
    }

    // Native Ratify work packets already carry the complete human-facing
    // contract. Normalize that read-only projection without exposing its
    // write-back commands or inferring a decision.
    let target = observed
        .get("target")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("Ratify work item has no target object"))?;
    let kind = required_string(target.get("kind"), "Ratify target kind")?;
    let id = required_string(target.get("id"), "Ratify target id")?;
    let name = required_string(target.get("name"), "Ratify target name")?;
    let reason = required_string(observed.get("reason"), "Ratify reason")?;
    let (criterion, current_subject) = ratify_target_criterion(observed, kind, id, name)?;
    let gate = observed
        .pointer("/prompt_contract/human_gate")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("Ratify work item has no prompt_contract.human_gate object"))?;
    let question = required_string(gate.get("question"), "Ratify gate question")?;
    let recommendation = required_string(gate.get("recommendation"), "Ratify gate recommendation")?;
    required_string(
        gate.get("after_answer"),
        "Ratify gate after_answer guidance",
    )?;
    let options = gate
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Ratify gate options must be an array"))?;
    if options.len() != 3 {
        bail!("Ratify gate must expose exactly ratify, reject, and revise options");
    }
    let expected = ["ratify", "reject", "revise"];
    let mut normalized = Vec::with_capacity(3);
    for (option, expected_id) in options.iter().zip(expected) {
        let option = option
            .as_object()
            .ok_or_else(|| anyhow!("Ratify gate option must be an object"))?;
        let id = required_string(option.get("id"), "Ratify gate option id")?;
        if id != expected_id {
            bail!("Ratify gate option order must be ratify, reject, revise");
        }
        let label = required_string(option.get("label"), "Ratify gate option label")?;
        let description =
            required_string(option.get("description"), "Ratify gate option description")?;
        required_string(option.get("write_back"), "Ratify gate write_back")?;
        normalized.push(crate::journey_gate::HumanOption::new(
            id,
            label,
            description,
            expected_id == "revise",
        ));
    }
    let prompt = crate::journey_gate::HumanPrompt::new(
        question,
        format!(
            "{recommendation}\n\nCurrent criterion: {criterion}\nCurrent drift evidence: {reason}"
        ),
        normalized,
    )?;
    let canonical = serde_json::to_string(&canonicalize(observed.clone()))?;
    Ok((
        crate::journey_gate::GateSubject {
            kind: kind.to_string(),
            id: id.to_string(),
            hash: crate::artifact::fingerprint(&canonical),
        },
        prompt,
        true,
        Some(current_subject),
    ))
}

fn ratify_target_criterion<'a>(
    observed: &'a Value,
    kind: &str,
    id: &str,
    name: &str,
) -> Result<(&'a str, CurrentSubjectAnchor)> {
    let linked = observed
        .pointer("/context/linked_entities")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Ratify work item has no context.linked_entities array"))?;
    let mut targets = linked.iter().filter(|entity| {
        entity.get("role").and_then(Value::as_str) == Some("target")
            && entity.get("kind").and_then(Value::as_str) == Some(kind)
            && entity.get("id").and_then(Value::as_str) == Some(id)
            && entity.get("name").and_then(Value::as_str) == Some(name)
    });
    let target = targets
        .next()
        .ok_or_else(|| anyhow!("Ratify context has no exact linked target criterion"))?;
    if targets.next().is_some() {
        bail!("Ratify context repeats the exact linked target criterion");
    }
    let description = required_string(target.get("description"), "Ratify linked target criterion")?;
    let canonical = serde_json::to_string(&json!({
        "kind": kind,
        "id": id,
        "name": name,
        "description": description,
    }))?;
    Ok((
        description,
        CurrentSubjectAnchor {
            kind: kind.to_string(),
            id: id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            hash: crate::artifact::fingerprint(&canonical),
        },
    ))
}

fn prompt_subject(observed: &Value) -> Result<crate::journey_gate::GateSubject> {
    let prompt: ObservedHumanPrompt = serde_json::from_value(observed.clone())?;
    Ok(prompt.subject)
}

fn required_string<'a>(value: Option<&'a Value>, label: &str) -> Result<&'a str> {
    let value = value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("{label} must be a substantive string"))?;
    if crate::model::is_placeholder(value) {
        bail!("{label} must not be a placeholder");
    }
    Ok(value)
}

fn scrub_ratify_control_fields(observed: &mut Value) {
    let Some(gate) = observed
        .pointer_mut("/prompt_contract/human_gate")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    gate.remove("after_answer");
    if let Some(options) = gate.get_mut("options").and_then(Value::as_array_mut) {
        for option in options {
            if let Some(option) = option.as_object_mut() {
                option.remove("write_back");
            }
        }
    }
}

pub(crate) struct GatePoint<'a> {
    pub(crate) step_index: usize,
    pub(crate) step: &'a CompiledStep,
    pub(crate) gate: &'a CompiledHumanDecision,
}

pub(crate) fn suspend_human_decision(
    root: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    temp: TemporaryRoot,
    point: GatePoint<'_>,
    mut active: ActiveRun,
) -> Result<ExecutionOutcome> {
    if proof.setup.is_none() {
        bail!("human decision continuation requires a local_snapshot workspace");
    }
    if !active.secrets.is_empty() || !active.redacted_captures.is_empty() {
        bail!(
            "human decision continuation cannot suspend a secret-bearing runtime; remove secret inputs or redacted captures from the gated profile"
        );
    }
    if !active.human_decisions.is_empty() {
        bail!(
            "human decision continuation cannot persist an earlier human answer; split sequential decisions into separate Journey runs"
        );
    }
    let source_index = active
        .reports
        .iter()
        .rposition(|report| report.operation_id == point.gate.source_operation_id)
        .ok_or_else(|| {
            anyhow!(
                "human decision step '{}' has no observed prior operation '{}'",
                point.step.step_id,
                point.gate.source_operation_id
            )
        })?;
    let observed = active.reports[source_index]
        .output
        .pointer(&point.gate.pointer)
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "human decision step '{}' source pointer '{}' is absent from operation '{}'",
                point.step.step_id,
                point.gate.pointer,
                point.gate.source_operation_id
            )
        })?;
    let (subject, prompt, scrub_ratify_controls, current_subject) =
        normalize_human_prompt(&observed).with_context(|| {
            format!(
                "human decision step '{}' source is not a structured prompt",
                point.step.step_id
            )
        })?;
    if scrub_ratify_controls {
        let selected = active.reports[source_index]
            .output
            .pointer_mut(&point.gate.pointer)
            .expect("gate source pointer was observed above");
        scrub_ratify_control_fields(selected);
    }
    let binding = crate::journey_gate::GateBinding {
        journey_id: proof.journey_id.clone(),
        profile: proof.profile.clone(),
        journey_hash: proof.journey_hash.clone(),
        surface_hash: proof.surface_hash.clone(),
        step_id: point.step.step_id.clone(),
        step_index: point.step_index,
        subject,
        prompt_hash: prompt.digest()?,
    };
    let store = capsule_store(root)?;
    let issued = store.issue(binding.clone(), prompt)?;
    let state = ContinuationState {
        schema: CONTINUATION_RUNTIME_SCHEMA.into(),
        live_root: root
            .canonicalize()
            .with_context(|| format!("canonicalizing live Journey root {}", root.display()))?,
        spec: spec.clone(),
        proof: proof.clone(),
        gate_binding: binding,
        current_subject,
        step_index: point.step_index,
        active,
    };
    let installed = (|| -> Result<()> {
        temp.persist_to(&issued.paths.workspace)?;
        write_new_continuation(&issued.paths.runtime_state, &state)
    })();
    if let Err(error) = installed {
        // Best-effort rollback of the half-installed continuation: the install
        // error is the caller's failure, but a directory the rollback could
        // not remove must leave a trace or the leak is invisible.
        if let Err(cleanup) = std::fs::remove_dir_all(&issued.paths.directory) {
            eprintln!(
                "warning: failed to remove half-installed continuation {}: {cleanup}",
                issued.paths.directory.display()
            );
        }
        return Err(error);
    }
    Ok(ExecutionOutcome::Pending(issued.pending))
}

pub fn pending_continuation(token: &str) -> Result<PendingContinuation> {
    let store = capsule_store_without_graph()?;
    let state = read_continuation(&pending_runtime_state_path(&store, token)?)?;
    state.validate()?;
    Ok(PendingContinuation {
        binding: state.gate_binding,
        live_root: state.live_root,
    })
}

pub fn resume_interactive(
    root: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    token: &str,
    answer: crate::journey_gate::ResumeAnswer,
    executor: &str,
) -> Result<ExecutionOutcome> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing Journey repository root {}", root.display()))?;
    proof.validate()?;
    let store = capsule_store(&root)?;
    let pending_paths = store.inspect_pending(token)?;
    let pending = read_continuation(&pending_paths.runtime_state)?;
    pending.validate()?;
    validate_current_continuation(&root, &pending_paths.workspace, spec, proof, &pending)?;
    verify_continuation_anchors(&root, &pending)?;

    let claimed = store.claim(token, &pending.gate_binding, answer, executor)?;
    let resumed = (|| -> Result<ExecutionOutcome> {
        let claimed_paths = store.inspect_claimed(token)?;
        let mut state = read_continuation(&claimed_paths.runtime_state)?;
        state.validate()?;
        validate_current_continuation(&root, &claimed_paths.workspace, spec, proof, &state)?;
        verify_continuation_anchors(&root, &state)?;
        if claimed.receipt.binding != state.gate_binding {
            bail!("claimed human decision does not match its runtime continuation");
        }
        let step = proof
            .steps
            .get(state.step_index)
            .ok_or_else(|| anyhow!("human decision continuation step index is no longer valid"))?;
        if step.human_decision.is_none() || step.step_id != state.gate_binding.step_id {
            bail!("human decision continuation no longer names the compiled gate step");
        }
        let receipt = serde_json::to_value(&claimed.receipt)?;
        state.active.reports.push(StepReport {
            step_id: step.step_id.clone(),
            operation_id: "human-decision".into(),
            argv: Vec::new(),
            exit_code: 0,
            output: receipt.clone(),
            assertions_passed: 1,
            assertions_failed: 0,
        });
        state.active.assertions_passed += 1;
        state.active.human_decisions.push(receipt);
        let workspace = TemporaryRoot::adopt(claimed.paths.workspace.clone())?;
        run_steps(
            &root,
            spec,
            proof,
            workspace,
            true,
            state.step_index + 1,
            state.active,
        )
    })();
    std::fs::remove_dir_all(&claimed.paths.directory).with_context(|| {
        format!(
            "destroying claimed Journey continuation {}",
            claimed.paths.directory.display()
        )
    })?;
    resumed
}

impl ContinuationState {
    fn validate(&self) -> Result<()> {
        if self.schema != CONTINUATION_RUNTIME_SCHEMA {
            bail!("unsupported Journey runtime continuation schema");
        }
        self.proof.validate()?;
        if self.proof.journey_id != self.spec.id
            || self.proof.journey_hash != self.spec.semantic_hash()?
        {
            bail!("Journey runtime continuation has mismatched authored semantics");
        }
        let step = self
            .proof
            .steps
            .get(self.step_index)
            .ok_or_else(|| anyhow!("Journey runtime continuation step index is invalid"))?;
        if step.human_decision.is_none()
            || step.step_id != self.gate_binding.step_id
            || self.gate_binding.journey_id != self.proof.journey_id
            || self.gate_binding.profile != self.proof.profile
            || self.gate_binding.journey_hash != self.proof.journey_hash
            || self.gate_binding.surface_hash != self.proof.surface_hash
            || self.gate_binding.step_index != self.step_index
        {
            bail!("Journey runtime continuation binding is inconsistent");
        }
        if let Some(subject) = &self.current_subject {
            if subject.kind != self.gate_binding.subject.kind
                || subject.id != self.gate_binding.subject.id
                || subject.hash.len() != 16
                || !subject.hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                bail!("Journey runtime continuation current-subject anchor is inconsistent");
            }
        }
        self.gate_binding.validate()
    }
}

fn validate_current_continuation(
    root: &Path,
    workspace: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    state: &ContinuationState,
) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing Journey root {}", root.display()))?;
    if root != state.live_root {
        bail!("Journey gate resume token belongs to a different graph root");
    }
    if spec.semantic_hash()? != state.spec.semantic_hash()?
        || canonical_bytes(proof)? != canonical_bytes(&state.proof)?
    {
        bail!("Journey gate resume token is stale for the current compiled projection");
    }
    if let Some(subject) = &state.current_subject {
        validate_current_subject(workspace, subject)?;
    }
    Ok(())
}

/// Recheck the execution-time covered hashes persisted by the paused run
/// against the files on disk, before any resumed step may execute. A covered
/// file changed between the pause and the resume invalidates the evidence the
/// final observation would claim.
fn verify_continuation_anchors(root: &Path, state: &ContinuationState) -> Result<()> {
    if let Some(anchors) = &state.active.anchors {
        if !anchors.covered_still_match(root) {
            bail!(
                "Journey gate resume token is stale: a covered file changed since the run paused"
            );
        }
    }
    Ok(())
}

fn validate_current_subject(root: &Path, subject: &CurrentSubjectAnchor) -> Result<()> {
    let store = crate::store::Store::open_read(root)?;
    let node = store
        .get_node(&subject.id)?
        .ok_or_else(|| anyhow!("Journey gate current subject '{}' is missing", subject.id))?;
    if node.node_type.as_str() != subject.kind
        || node.name != subject.name
        || node.description != subject.description
    {
        bail!("Journey gate resume token is stale for the current subject");
    }
    let canonical = serde_json::to_string(&json!({
        "kind": node.node_type.as_str(),
        "id": node.id,
        "name": node.name,
        "description": node.description,
    }))?;
    if crate::artifact::fingerprint(&canonical) != subject.hash {
        bail!("Journey gate resume token is stale for the current subject");
    }
    Ok(())
}

fn capsule_store(root: &Path) -> Result<crate::journey_gate::CapsuleStore> {
    let store = capsule_store_without_graph()?;
    let live = root
        .canonicalize()
        .with_context(|| format!("canonicalizing Journey root {}", root.display()))?;
    if store.root() == live || store.root().starts_with(&live) {
        bail!("Journey continuation storage must be outside the live graph root");
    }
    Ok(store)
}

fn capsule_store_without_graph() -> Result<crate::journey_gate::CapsuleStore> {
    crate::journey_gate::CapsuleStore::new(
        std::env::temp_dir().join("loom-journey-runtime-continuations-v1"),
    )
}

fn pending_runtime_state_path(
    store: &crate::journey_gate::CapsuleStore,
    token: &str,
) -> Result<PathBuf> {
    let digest = crate::journey_gate::digest_token(token)?;
    Ok(store
        .root()
        .join("pending")
        .join(digest)
        .join("runtime-state.json"))
}

fn read_continuation(path: &Path) -> Result<ContinuationState> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("opening Journey runtime continuation {}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("Journey runtime continuation is not a confined regular file");
    }
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes).context("decoding Journey runtime continuation")
}

fn write_new_continuation(path: &Path, state: &ContinuationState) -> Result<()> {
    let bytes = serde_json::to_vec(state)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("creating Journey runtime continuation {}", path.display()))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}
