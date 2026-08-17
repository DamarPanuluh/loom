use super::*;

/// Observe a command and return what loom saw. The shared core: the CLI prints
/// it, the MCP tool returns it, and neither can report an outcome loom did not
/// witness because neither is given the chance to supply one.
pub(crate) fn observe_run(
    graph: Option<&Path>,
    target: Option<&str>,
    timeout: u64,
    command: &[String],
) -> Result<serde_json::Value> {
    // Re-quote every argument. Joining on spaces looks right and is wrong: it
    // hands `python3 -c "import sys; ..."` to the shell as several statements,
    // so the command loom "observed" is not the command the caller asked for.
    let command = command
        .iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ");

    // Resolve what we need, then CLOSE the graph before running anything.
    //
    // Holding the write lock across the child is fatal for the commands most
    // worth observing: loom's own journeys are `loom journey run …`, and a
    // child blocked on the lock its parent holds exits non-zero. That does not
    // merely fail — it records a FALSE FAILING verdict against a behavior that
    // passes, which is the one outcome this whole spine exists to prevent.
    let (intent, covered, root, execution) = {
        let store = open(graph)?;
        let root = store.root().to_path_buf();
        let execution = store.execution_identity();
        // What this run covers: the files the target behavior is grounded in,
        // so an edit to any of them expires it. With no target, the run covers
        // nothing and stands only as a journal record — honest about being
        // unattached.
        match target {
            Some(key) => {
                let node = store.resolve_node(key, Some(NodeType::Intent))?;
                let files = store.files_grounding(&node.id)?;
                (Some(node), files, root, execution)
            }
            None => (None, Vec::new(), root, execution),
        }
    };

    let _harness = crate::harness::acquire(&root, "observe", &execution)?;
    let observation = crate::runner::observe_command(
        &root,
        crate::model::RunProducer::Command,
        &command,
        &covered,
        0,
        timeout,
    )?;
    let run = match &observation {
        crate::runner::Observation::Ran(run) => (**run).clone(),
        crate::runner::Observation::Blocked { reason } => {
            // Keep the store open through the journal append so the graph lock
            // is held while the blocked proof is recorded; the binding is
            // intentionally unused beyond its drop.
            let store = crate::store::Store::open_with_identity(&root, execution.clone())?;
            // A command loom could not run is not a failing proof. Recorded as
            // blocked, visible, never green.
            store.append_journal(
                "observe",
                intent.as_ref().map(|n| n.id.as_str()).unwrap_or(""),
                serde_json::json!({ "command": command, "blocked": reason }),
            )?;
            return Ok(serde_json::json!({ "observed": false, "blocked": reason }));
        }
    };

    // The child is done; take the lock back to record what happened.
    let store = crate::store::Store::open_with_identity(&root, execution.clone())?;
    let entry = store.append_journal(
        "observe",
        intent.as_ref().map(|n| n.id.as_str()).unwrap_or(""),
        serde_json::json!({
            "command": command,
            "exit_code": run.exit_code,
            "covered": run.covered.len(),
        }),
    )?;

    // Bind it, when there is something to bind it to.
    let mut bound: Option<String> = None;
    let mut bound_id: Option<String> = None;
    if let Some(node) = &intent {
        let validation = existing_or_new_proof(&store, &node.id, &command)?;
        let result = if run.exit_code == 0 {
            "passed"
        } else {
            "failed"
        };
        mark_validation(
            &store,
            &validation.id,
            result,
            &format!("observed by loom: `{command}` exited {}", run.exit_code),
            "",
            Some(run.clone()),
        )?;
        regrade(&store, &validation.id)?;
        bound = Some(validation.name.clone());
        bound_id = Some(validation.id.clone());
    }

    // Read the grade off the proof this run actually bound to. Looking it up by
    // the name loom WOULD have minted reports S0 for every run that reused an
    // existing proof — which is most of them, since the proof is keyed on the
    // command precisely so repeat runs land on one node.
    let grade = match &bound_id {
        Some(id) => crate::proofstrength::of(&store, id)?.as_str(),
        None => "-",
    };

    Ok(serde_json::json!({
        "observed": true,
        "command": command,
        "exit_code": run.exit_code,
        "stdout_excerpt": run.stdout_excerpt,
        "stderr_excerpt": run.stderr_excerpt,
        "covered": run.covered.keys().collect::<Vec<_>>(),
        "journal": entry.id,
        "bound_to": bound,
        "strength": grade,
    }))
}

/// Quote one argument for `sh -c`, so what runs is what was typed.
fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_alphanumeric() || "._-/=:@+,".contains(c))
    {
        return arg.to_string();
    }
    // Single quotes protect everything except a single quote, which has to be
    // closed, escaped, and reopened.
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// The stable name loom gives a proof it minted from an observed command.
///
/// Short and stable: the full command lives in `body.command`, and a node name
/// that is 200 characters of shell is unreadable everywhere it appears.
fn command_proof_name(command: &str) -> String {
    let head: String = command
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
    let head: String = head.chars().take(48).collect();
    format!(
        "observed: {head} [{}]",
        &crate::artifact::fingerprint(command)[..8]
    )
}

/// The validation this command already proves, or a new one for it.
///
/// Keyed on the COMMAND, so running the same command twice updates one proof
/// instead of littering the graph with near-duplicates.
fn existing_or_new_proof(
    store: &Store,
    intent_id: &str,
    command: &str,
) -> Result<crate::model::Node> {
    for e in store.edges_with(Some(EdgeKind::Validates), None, Some(intent_id))? {
        if let Some(v) = store.get_node(&e.from_id)? {
            if v.body.get("command").and_then(|c| c.as_str()) == Some(command) {
                if let Some((journey, profile)) =
                    crate::completeness::compiler_owned_journey_validation(store, &v)?
                {
                    bail!(
                        "loom observe cannot reuse compiler-owned Journey validation '{}'; use `loom journey run {} --profile {}`",
                        v.name,
                        journey.id,
                        profile
                    );
                }
                return Ok(v);
            }
        }
    }
    let val = store.add_node(
        NodeType::Validation,
        &command_proof_name(command),
        "registered by `loom observe`",
        "not_run",
        serde_json::json!({ "type": "test", "command": command }),
    )?;
    store.ensure_edge(EdgeKind::Validates, &val.id, intent_id)?;
    Ok(val)
}
