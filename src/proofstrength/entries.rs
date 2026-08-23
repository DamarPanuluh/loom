use super::command::command_entries;
use super::CallEvidenceWitness;
use crate::model::{EdgeKind, InspectionStatus, Node, NodeType, TargetKind};
use crate::store::Store;
use crate::Result;

/// File-qualified realizing targets. Grading uses these so a same-named
/// symbol in another file cannot share a call witness.
fn grounded_targets(store: &Store, intent_id: &str) -> Result<Vec<(String, String)>> {
    crate::locator::realizing_targets(store, intent_id)
}

/// How far [`call_witness`] walks the call graph.
///
/// Cap of 4 hid exact callers at 6 hops (finding `d3107a6d`: ring32 research
/// tests → `push_notes`). `loom impact <sym> --depth 8` already contradicted
/// the S2 "nothing this proof runs reaches the symbol" grade. Eight matches
/// that diagnostic depth and clears the documented 6-hop case with headroom
/// for a layer or two of helpers.
pub const CALL_WITNESS_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EntryEvidence {
    pub source: &'static str,
    pub file: String,
    pub entry_symbol: Option<String>,
    pub s3_eligible: bool,
    pub operation_id: Option<String>,
    pub exercise_id: Option<String>,
    pub observed_by: Option<String>,
}

impl EntryEvidence {
    pub(crate) fn plain(
        source: &'static str,
        file: String,
        entry_symbol: Option<String>,
        s3_eligible: bool,
    ) -> Self {
        Self {
            source,
            file,
            entry_symbol,
            s3_eligible,
            operation_id: None,
            exercise_id: None,
            observed_by: None,
        }
    }

    pub(super) fn into_call_evidence(self, grounded_symbol: Option<String>) -> CallEvidenceWitness {
        CallEvidenceWitness {
            source: self.source.into(),
            file: self.file,
            entry_symbol: self.entry_symbol,
            grounded_symbol,
            s3_eligible: self.s3_eligible,
            operation_id: self.operation_id,
            exercise_id: self.exercise_id,
            observed_by: self.observed_by,
        }
    }
}

/// Does this validation-specific entry reach a symbol the intent is grounded
/// in? `impact` walks callers backwards; narrowing by `entry_symbol` prevents a
/// broad file match from crediting a different test in the same file.
pub(super) fn call_witness(
    store: &Store,
    graph: &crate::callgraph::CallGraph,
    intent_id: &str,
    entries: &[EntryEvidence],
) -> Result<Option<CallEvidenceWitness>> {
    for (file, symbol) in grounded_targets(store, intent_id)? {
        // Exact path from this realizing definition site only — never every
        // same-named symbol in the repo.
        let reach = graph.exact_impact_at(&file, &symbol, CALL_WITNESS_DEPTH);
        for entry in entries {
            if !entry.s3_eligible {
                continue;
            }
            // The entry may itself be the grounded handler. `exact_impact_at`
            // returns callers, so that valid zero-hop path is not present in
            // `reach.callers`; recognize it only by the same exact file+symbol
            // qualification used for multi-hop witnesses. Requiring a symbol
            // keeps bare-file evidence out, while `s3_eligible` above keeps the
            // intent-wide diagnostic fallback out.
            let zero_hop = entry
                .entry_symbol
                .as_deref()
                .is_some_and(|expected| entry.file == file && expected == symbol);
            let reaches = zero_hop
                || reach.callers.iter().any(|caller| {
                    caller.file == entry.file
                        && entry
                            .entry_symbol
                            .as_deref()
                            .is_none_or(|expected| caller.symbol == expected)
                });
            if reaches {
                return Ok(Some(entry.clone().into_call_evidence(Some(symbol))));
            }
        }
    }
    Ok(None)
}

/// Explicit Validation→CodeFile entry evidence for generic (non-Journey)
/// validations. This is the schema-level form of a per-validation grounding:
/// unlike `implements`, it owns no behavior/code coverage and exists only to
/// say which code surface this proof exercises. Compiler-owned Journey
/// validations never take this path — see `journey_owned_entries`.
pub(super) fn validation_entries(store: &Store, validation_id: &str) -> Result<Vec<EntryEvidence>> {
    let mut out = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Exercises), Some(validation_id), None)? {
        let Some(file) = store.get_node(&edge.to_id)? else {
            continue;
        };
        let locator = store.edge_locator(&edge.id)?;
        if locator
            .as_deref()
            .is_some_and(crate::locator::is_anchor_locator)
        {
            // Source anchors stabilize navigation only. Even when attached to
            // a callable entry, they must not become an S3 proof declaration.
            out.push(EntryEvidence::plain(
                "anchor_navigation",
                file.name.clone(),
                None,
                false,
            ));
            continue;
        }
        let locators = locator
            .map(|locator| crate::locator::symbols(&locator))
            .unwrap_or_default();
        // Bare file claim: diagnostic only. Locator-bound exercises is the
        // product's validation-specific entry declaration (see module docs):
        // the operator names the entry surface this validation exercises.
        // Command-derived entries are the other S3 path. Both require a call
        // witness to the realizing symbol — the locator alone is not enough
        // without that reachability check in `call_witness`.
        if locators.is_empty() {
            out.push(EntryEvidence::plain(
                "validation_grounding",
                file.name,
                None,
                false,
            ));
        } else {
            out.extend(locators.into_iter().map(|symbol| {
                EntryEvidence::plain(
                    "validation_grounding",
                    file.name.clone(),
                    Some(symbol),
                    true,
                )
            }));
        }
    }
    Ok(out)
}

/// Compiler-owned Journey entries: minted from the canonical projection of the
/// accepted surface, and only after the compiled Exercises topology/facets
/// agree with it exactly. Any disagreement, malformed provenance, or a missing
/// projection fails closed — entries stay diagnostic-only and can never earn
/// S3. A Journey proof is compiler-owned graph structure, never an authored spec
/// inferred from a path on the Validation. This deliberately duplicates the
/// readiness signature at the grading boundary so a raw Journey artifact, or a
/// hand-authored sibling Validation, cannot borrow compiled proof strength.
/// This also refuses the public-entry interpretation of the aggregate `locator` facet.
///
/// Returns the entries plus a human-readable description of any provenance
/// disagreement (empty when the topology agreed exactly).
pub(super) fn journey_owned_entries(
    store: &Store,
    validation_id: &str,
    projection: &crate::journey_exercises::ExpectedExerciseProjection,
) -> Result<(Vec<EntryEvidence>, Option<String>)> {
    let problems = crate::journey_exercises::topology_problems(store, validation_id, projection)?;
    if !problems.is_empty() {
        let mut out = Vec::new();
        for codefile_id in projection.target_ids() {
            if let Some(name) = projection.codefile_name(&codefile_id) {
                out.push(EntryEvidence {
                    source: "journey_provenance_mismatch",
                    file: name.to_string(),
                    entry_symbol: None,
                    s3_eligible: false,
                    operation_id: None,
                    exercise_id: None,
                    observed_by: None,
                });
            }
        }
        return Ok((out, Some(problems.join("; "))));
    }

    let passed = passed_journey_assertions(store, validation_id)?;
    let mut out = Vec::new();
    // The genuine top-level public entry stays the one surface owner. Its
    // entries follow the ordinary validation_grounding rules, but are read
    // from the projection — never from a facet an operator could edit.
    for entry in &projection.public_entries {
        let Some(locator) = &entry.locator else {
            out.push(EntryEvidence::plain(
                "validation_grounding",
                entry.codefile_name.clone(),
                None,
                false,
            ));
            continue;
        };
        if crate::locator::is_anchor_locator(locator) {
            out.push(EntryEvidence::plain(
                "anchor_navigation",
                entry.codefile_name.clone(),
                None,
                false,
            ));
            continue;
        }
        let symbols = crate::locator::symbols(locator);
        if symbols.is_empty() {
            out.push(EntryEvidence::plain(
                "validation_grounding",
                entry.codefile_name.clone(),
                None,
                false,
            ));
        } else {
            out.extend(symbols.into_iter().map(|symbol| {
                EntryEvidence::plain(
                    "validation_grounding",
                    entry.codefile_name.clone(),
                    Some(symbol),
                    true,
                )
            }));
        }
    }
    for exercise in &projection.exercises {
        let assertion_passed =
            passed.contains(&(exercise.operation_id.clone(), exercise.observed_by.clone()));
        let symbols = crate::locator::symbols(&exercise.locator);
        if symbols.is_empty() {
            // Defensive: the projection already validated callability, so this
            // only happens if the file changed between projection and grading
            // within one call — still fail closed.
            out.push(EntryEvidence {
                source: "journey_operation_exercise",
                file: exercise.codefile_name.clone(),
                entry_symbol: None,
                s3_eligible: false,
                operation_id: Some(exercise.operation_id.clone()),
                exercise_id: Some(exercise.exercise_id.clone()),
                observed_by: Some(exercise.observed_by.clone()),
            });
            continue;
        }
        out.extend(symbols.into_iter().map(|symbol| EntryEvidence {
            source: "journey_operation_exercise",
            file: exercise.codefile_name.clone(),
            entry_symbol: Some(symbol),
            s3_eligible: assertion_passed,
            operation_id: Some(exercise.operation_id.clone()),
            exercise_id: Some(exercise.exercise_id.clone()),
            observed_by: Some(exercise.observed_by.clone()),
        }));
    }
    Ok((out, None))
}

/// Observed assertion ids from the latest compiled Journey run evidence.
/// Read from the run record's structured `observed_assertions` — machine
/// evidence minted by the Journey settlement — never parsed from the
/// human-facing stdout excerpt, which is truncated for large reports and must
/// never be parsed for trust decisions. Only `RunProducer::Journey` runs
/// qualify: the CLI mints that producer solely in the compiler-owned Journey
/// settlement path, so a generic validation run (whose producer comes from
/// its command shape) can never carry Journey assertion provenance.
fn passed_journey_assertions(
    store: &Store,
    validation_id: &str,
) -> Result<std::collections::BTreeSet<(String, String)>> {
    let mut out = std::collections::BTreeSet::new();
    for edge in store.edges_with(Some(EdgeKind::Validates), Some(validation_id), None)? {
        let Some(view) = store.fact(
            &crate::store::Subject::Edge(edge.id.clone()),
            crate::model::Claim::Verdict,
        )?
        else {
            continue;
        };
        for row in &view.evidence {
            let crate::evidence::Evidence::Run(run) = &row.payload else {
                continue;
            };
            // Trusted Journey assertion provenance is locally minted by the
            // compiler-owned settlement, then reloaded from the store. A
            // deserialized or imported RunRecord can carry assertion names for
            // audit, but those names never earn S3.
            if !run.has_trusted_journey_assertions() {
                continue;
            }
            for observed in run.observed_assertions() {
                out.insert((observed.group.clone(), observed.assertion.clone()));
            }
        }
    }
    Ok(out)
}

/// Legacy intent-level verifying files. Kept visible so migrated graphs explain
/// what the old grader used, but never eligible for S3 under this model.
pub(super) fn intent_wide_entries(store: &Store, intent_id: &str) -> Result<Vec<EntryEvidence>> {
    let mut out = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Implements), Some(intent_id), None)? {
        if store.edge_superseded(&edge.id)?
            || store.grounding_role(&edge.id)? != crate::model::GroundingRole::Verifies
        {
            continue;
        }
        if let Some(file) = store.get_node(&edge.to_id)? {
            out.push(EntryEvidence::plain(
                "intent_wide_fallback",
                file.name,
                None,
                false,
            ));
        }
    }
    Ok(out)
}

pub(super) fn derived_entries(
    validation: &Node,
    graph: &crate::callgraph::CallGraph,
) -> Vec<EntryEvidence> {
    validation
        .body
        .get("command")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(|command| command_entries(command, graph, "validation_command"))
        .unwrap_or_default()
}

fn projection_current(status: InspectionStatus) -> bool {
    matches!(
        status,
        InspectionStatus::Uninspected | InspectionStatus::Passing
    )
}

/// A Journey proof is compiler-owned graph structure, never an authored spec
/// inferred from a path on the Validation. This deliberately duplicates the
/// readiness signature at the grading boundary so a raw Journey artifact, or a
/// hand-authored sibling Validation, cannot borrow compiled proof strength.
pub(super) fn compiled_journey_proves_edge(
    store: &Store,
    validation: &Node,
) -> Result<Option<crate::model::Edge>> {
    if validation.body.get("type").and_then(|value| value.as_str()) != Some("journey")
        || validation
            .body
            .get("profile")
            .and_then(|value| value.as_str())
            != Some("proof")
        || validation
            .body
            .get("compiler_version")
            .and_then(|value| value.as_str())
            != Some(crate::journey::JOURNEY_COMPILER_VERSION)
    {
        return Ok(None);
    }

    let proves: Vec<_> = store
        .edges_with(Some(EdgeKind::Proves), Some(&validation.id), None)?
        .into_iter()
        .filter(|edge| projection_current(edge.status))
        .collect();
    let [proves] = proves.as_slice() else {
        return Ok(None);
    };
    let Some(journey) = store.get_node(&proves.to_id)? else {
        return Ok(None);
    };
    if journey.node_type != NodeType::Journey {
        return Ok(None);
    }
    let Some(journey_hash) = journey
        .body
        .get("semantic_hash")
        .and_then(|value| value.as_str())
    else {
        return Ok(None);
    };
    if validation
        .body
        .get("journey_hash")
        .and_then(|value| value.as_str())
        != Some(journey_hash)
    {
        return Ok(None);
    }
    let Some(surface_hash) = crate::journey::surface_projection_hash(store, &journey)? else {
        return Ok(None);
    };
    if validation
        .body
        .get("surface_hash")
        .and_then(|value| value.as_str())
        != Some(surface_hash.as_str())
    {
        return Ok(None);
    }

    let mut accepted_surfaces = std::collections::BTreeSet::new();
    for edge in store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)? {
        if projection_current(edge.status)
            && store
                .get_facet(&edge.id, TargetKind::Edge, "journey_hash")?
                .as_deref()
                == Some(journey_hash)
        {
            accepted_surfaces.insert(edge.to_id);
        }
    }
    let calls_current_surface = store
        .edges_with(Some(EdgeKind::Calls), Some(&validation.id), None)?
        .into_iter()
        .any(|edge| projection_current(edge.status) && accepted_surfaces.contains(&edge.to_id));
    if !calls_current_surface {
        return Ok(None);
    }
    let mut exercises_live_code = false;
    for edge in store.edges_with(Some(EdgeKind::Exercises), Some(&validation.id), None)? {
        if projection_current(edge.status)
            && store
                .get_node(&edge.to_id)?
                .is_some_and(|node| node.node_type == NodeType::CodeFile)
        {
            exercises_live_code = true;
            break;
        }
    }
    Ok(exercises_live_code.then(|| proves.clone()))
}

pub(super) fn dedup_entries(entries: &mut Vec<EntryEvidence>) {
    entries.sort();
    entries.dedup();
}

/// Journey-specific S2 guidance. Never recommends `loom edge exercises` for
/// compiler-owned proofs.
pub(super) fn journey_s2_next(
    entries: &[EntryEvidence],
    provenance_problems: Option<&str>,
) -> String {
    let suffix = " Update the authored surface manifest, then run `loom journey surface-accept`, `loom journey compile`, and `loom journey run`.";
    if let Some(problems) = provenance_problems {
        return format!(
            "compiled Journey is S2: compiled operation-exercise provenance does not match the accepted surface ({problems}).{suffix}"
        );
    }
    let operation_entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.source == "journey_operation_exercise")
        .collect();
    if operation_entries.is_empty() {
        return format!(
            "compiled Journey is S2: no operation exercise was declared for a downstream entry that reaches the realizing symbol.{suffix}"
        );
    }
    if operation_entries.iter().any(|entry| {
        entry.entry_symbol.is_none()
            || entry
                .entry_symbol
                .as_deref()
                .is_some_and(|symbol| symbol.trim().is_empty())
    }) {
        return format!(
            "compiled Journey is S2: an operation exercise CodeFile/locator is stale or unresolved.{suffix}"
        );
    }
    if operation_entries.iter().any(|entry| !entry.s3_eligible) {
        return format!(
            "compiled Journey is S2: an operation exercise's observed_by assertion was missing or did not pass on the compiled run.{suffix}"
        );
    }
    format!(
        "compiled Journey is S2: the declared operation exercise entry does not reach a realizing grounding for the Intent.{suffix}"
    )
}
