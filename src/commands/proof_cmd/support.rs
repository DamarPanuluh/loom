use super::*;
pub(crate) fn validation_targets(store: &Store, val_id: &str) -> Result<Vec<serde_json::Value>> {
    let mut out = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Validates), Some(val_id), None)? {
        let target = store.get_node(&e.to_id)?;
        out.push(serde_json::json!({
            "id": e.to_id,
            "name": target.as_ref().map(|n| n.name.as_str()).unwrap_or(e.to_id.as_str()),
            "edge_id": e.id,
            "edge_status": e.status,
        }));
    }
    Ok(out)
}
/// Record a validation's outcome.
///
/// `run` is the observation loom made. When it is `Some`, the verdict is
/// anchored `verified` — loom watched this happen. When it is `None`, the
/// caller is REPORTING an outcome, which for a command-shaped proof is exactly
/// the move that produced 54 unearned green proofs in this graph; the anchor
/// floor refuses it.
/// The file→hash set a proof over this intent depends on: every file grounding
/// it. This is what makes a passing proof expire when the code moves.
pub(crate) fn covered_hashes(
    store: &Store,
    intent_id: &str,
) -> Result<std::collections::BTreeMap<String, String>> {
    let files = store.files_grounding(intent_id)?;
    let mut hashes = std::collections::BTreeMap::new();
    for file in files {
        let path = store.root().join(&file);
        let contents = std::fs::read_to_string(&path).map_err(|error| {
            anyhow::anyhow!(
                "reading grounded file '{}' for proof coverage: {error}",
                path.display()
            )
        })?;
        hashes.insert(file, crate::artifact::fingerprint(&contents));
    }
    Ok(hashes)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct PriorProofBehavior {
    id: String,
    name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ProofCommandCollision {
    detected: bool,
    command: String,
    prior_behavior_count: usize,
    prior_behaviors: Vec<PriorProofBehavior>,
}

impl ProofCommandCollision {
    pub(crate) fn none(command: &str) -> Self {
        Self {
            detected: false,
            command: command.to_string(),
            prior_behavior_count: 0,
            prior_behaviors: Vec::new(),
        }
    }

    fn warn(&self) {
        if !self.detected {
            return;
        }
        eprintln!(
            "warning: `{}` is already the proof of {} other behavior(s): {}.\n\
             \x20        One command exercises at most one of them; the rest stand on whatever it\n\
             \x20        really tests. Narrow this proof to the test that asserts THIS behavior,\n\
             \x20        or accept it knowingly — `loom smells` will keep reporting it.",
            self.command,
            self.prior_behavior_count,
            self.prior_behaviors
                .iter()
                .map(|behavior| format!("'{}'", behavior.name))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

/// Report and say so when a command already proves another behavior.
///
/// A warning, never a refusal. A ring genuinely covering several behaviors is a
/// legitimate shape — fifteen of this repo's shared commands are exactly that —
/// so refusing would break honest work to catch dishonest work. But the shape
/// is also how a claim goes green over code it never touches: an intent
/// claiming "a locator that cannot resolve falls back to file-scope reopening"
/// carried two passing validations, both running `cargo test --test ring6 -q`,
/// while the behavior did not exist at all.
///
/// Said at write time, which is the only moment it is cheap. Afterwards it
/// costs a smell, a triage verdict, and eventually someone re-deriving why.
pub(crate) fn warn_if_command_already_proves_another(
    store: &Store,
    command: &str,
    intent_id: &str,
    skip_validation: Option<&str>,
) -> Result<ProofCommandCollision> {
    let command = command.trim();
    if command.is_empty() {
        return Ok(ProofCommandCollision::none(command));
    }
    let mut prior_behaviors = Vec::new();
    for val_id in store.validations_with_command(command, skip_validation)? {
        for e in store.edges_with(Some(EdgeKind::Validates), Some(&val_id), None)? {
            if e.to_id == intent_id {
                continue;
            }
            if let Some(other) = store.get_node(&e.to_id)? {
                prior_behaviors.push(PriorProofBehavior {
                    id: other.id,
                    name: other.name,
                });
            }
        }
    }
    prior_behaviors.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });
    prior_behaviors.dedup_by(|left, right| left.id == right.id);
    let collision = ProofCommandCollision {
        detected: !prior_behaviors.is_empty(),
        command: command.to_string(),
        prior_behavior_count: prior_behaviors.len(),
        prior_behaviors,
    };
    collision.warn();
    Ok(collision)
}

pub(crate) fn mark_validation(
    store: &Store,
    val_id: &str,
    result: &str,
    evidence: &str,
    reason: &str,
    run: Option<crate::evidence::RunRecord>,
) -> Result<()> {
    let (node_status, edge_status, ev) = match result {
        "passed" => ("passed", InspectionStatus::Passing, evidence),
        "failed" => ("failed", InspectionStatus::Failing, evidence),
        // A blocked mark's reason lives in --reason, but a worker following the
        // packet may pass it as --evidence; accept either so the blocker text is
        // never silently dropped (M-2). record_verdict still requires it non-empty.
        "blocked" => (
            "blocked",
            InspectionStatus::Blocked,
            if reason.trim().is_empty() {
                evidence
            } else {
                reason
            },
        ),
        other => bail!("unknown result '{other}' (use passed|failed|blocked)"),
    };
    // Record the edge verdicts FIRST: record_verdict enforces INV-6 (a
    // passing/failing verdict needs non-empty evidence) and will bail on, e.g.,
    // an empty `--evidence`. Setting the node status only after they all succeed
    // keeps the mark atomic — a rejected verdict never leaves the validation
    // showing `passed` while the command exits non-zero.
    for e in store.edges_with(Some(EdgeKind::Validates), Some(val_id), None)? {
        // A proof run anchors the code it exercised: every file grounding the
        // intent it validates. Any later edit to one of those expires the run,
        // so a passing proof stops counting the moment the behavior moves
        // beneath it.
        let mut assertion = crate::store::Assertion::new(
            crate::store::Subject::Edge(e.id.clone()),
            crate::model::Claim::Verdict,
            edge_status.as_str(),
            "loom",
        )
        .criterion("proof")
        .confidence(1.0)
        .cited(crate::evidence::cite(store.root(), ev)?);
        if let Some(run) = run.clone() {
            let mut run = run;
            run.covered = covered_hashes(store, &e.to_id)?;
            assertion = assertion.observed(run);
        }
        store.assert_fact(assertion)?;
    }
    store.record_proof_stability(val_id, node_status)?;
    store.set_node_status(val_id, node_status)?;
    store.append_journal(
        "validation_verdict",
        val_id,
        serde_json::json!({ "outcome": result, "evidence": ev, "reason": reason }),
    )?;
    Ok(())
}
/// Run one validation through loom and record what loom observed.
///
/// The library path behind `loom validation run` — the ONLY way a `validates`
/// verdict reaches `verified`. Public because "let loom run it" is the correct
/// move for every caller, not just the CLI: `absorb` binds observed runs, and a
/// test fixture that wants a proven graph should get one the same way a
/// production graph does, rather than through a seam that fabricates the record.
pub fn observe_validation(
    store: &Store,
    val: &crate::model::Node,
) -> Result<crate::proof::ProofOutcome> {
    use crate::proof::ProofOutcome;
    if let Some((journey, profile)) =
        crate::completeness::compiler_owned_journey_validation(store, val)?
    {
        bail!(
            "compiler-owned Journey validations require `loom journey run {} --profile {}`",
            journey.id,
            profile
        );
    }
    if val.body.get("type").and_then(serde_json::Value::as_str) == Some("journey") {
        bail!("Journey validations cannot run through the generic proof runner; remove an orphaned proof or use `loom journey run <journey> --profile <profile>`");
    }
    let ty = match val.body.get("type") {
        None => crate::model::ValidationType::Test,
        Some(value) => {
            let Some(raw) = value.as_str() else {
                bail!("validation '{}' has a non-string type", val.id);
            };
            match raw.parse::<crate::model::ValidationType>() {
                Ok(ty) => ty,
                Err(_) => bail!("validation '{}' has unknown type '{raw}'", val.id),
            }
        }
    };
    let outcome = crate::proof::runner_for(ty).run(store.root(), val);
    match &outcome {
        ProofOutcome::Passed { evidence, run } => {
            mark_validation(
                store,
                &val.id,
                "passed",
                evidence,
                "",
                Some((**run).clone()),
            )?;
        }
        ProofOutcome::Failed { evidence, run, .. } => {
            mark_validation(
                store,
                &val.id,
                "failed",
                evidence,
                "",
                Some((**run).clone()),
            )?;
        }
        ProofOutcome::Blocked { reason } => {
            mark_validation(store, &val.id, "blocked", "", reason, None)?;
        }
        // No runner applies. loom records nothing rather than guessing — a
        // manual check is attested by a human, never inferred.
        ProofOutcome::Manual { .. } => {}
    }
    // Running a proof changes the inputs to its own grade, so re-grade it here
    // rather than leaving a stale figure until the next sync. Without this,
    // `loom validation run` followed by any command that reads strength would
    // report the grade from BEFORE the run.
    regrade(store, &val.id)?;
    Ok(outcome)
}

/// Recompute one validation's derived grade in place.
///
/// Must be called by EVERY path that settles a proof's outcome. It was called
/// only from `observe_validation`, which the `loom validation run` CLI bypasses
/// (see the dispatch at `ValidationCmd::Run`), so the documented way to run a
/// proof left the grade at whatever it was before the run. That is not a
/// cosmetic staleness: `sync` grades a reset validation S0, the run then passes
/// it, and the S0 stands — this session watched `proven` report 19 unproven
/// intents with all 189 proofs green, and a bare `loom sync` fix 26 grades at
/// once. Grade where the status is written, or the two drift.
pub(crate) fn regrade(store: &Store, validation_id: &str) -> Result<()> {
    let Some(val) = store.get_node(validation_id)? else {
        return Ok(());
    };
    let graph = crate::callgraph::build(store)?;
    let root = store.root().to_path_buf();
    let mut best: Option<crate::proofstrength::StrengthWitness> = None;
    for e in store.edges_with(Some(EdgeKind::Validates), Some(validation_id), None)? {
        let w = crate::proofstrength::grade(store, &root, &val, &e.to_id, &graph)?;
        let better = best
            .as_ref()
            .map(|b| {
                crate::proofstrength::Strength::parse(&w.grade)
                    > crate::proofstrength::Strength::parse(&b.grade)
            })
            .unwrap_or(true);
        if better {
            best = Some(w);
        }
    }
    if let Some(witness) = best {
        crate::proofstrength::store_witness(store, validation_id, &witness)?;
    }
    Ok(())
}

/// Register a command-shaped proof for an intent and run it. One call for the
/// common case: "this behavior is proven, and here is loom watching it be so."
pub fn prove_intent(store: &Store, intent_id: &str, name: &str, command: &str) -> Result<()> {
    let val = store.add_node(
        NodeType::Validation,
        name,
        "",
        "not_run",
        serde_json::json!({ "type": "test", "command": command }),
    )?;
    store.ensure_edge(EdgeKind::Validates, &val.id, intent_id)?;
    observe_validation(store, &val)?;
    Ok(())
}
