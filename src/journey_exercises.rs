//! Canonical expected projection of compiler-owned Journey operation-exercise
//! provenance.
//!
//! Operation exercises (surface operation `exercises` entries) are downstream
//! code entries reached through bound public operations. They are not
//! additional surface owners: the surface's top-level `codefile`/`locator`
//! remains the single real public entrypoint, and `journey compile` turns
//! exercises into `Exercises` topology plus provenance facets.
//!
//! Compile, grading, sync currentness, and doctor all judge that compiled
//! topology. If each judged it from its own reading of the store, the layers
//! could disagree about what the compiler was supposed to write and forged
//! provenance could slip through the gap between them. This module owns the
//! ONE projection of what the accepted surface demands, derived from:
//!
//! * the current hash-bound surface the validation calls;
//! * the complete step bindings on that surface;
//! * only bound operations;
//! * exercises resolved to live CodeFiles by canonical name/id.
//!
//! `expected_projection` fails closed with an error whenever the accepted
//! surface cannot currently yield a complete projection (stale acceptance,
//! missing live code, unresolvable locators, incomplete bindings).
//! `topology_problems` reports any semantic disagreement between that
//! projection and the compiled Exercises topology/facets. Grading refuses S3
//! whenever either fails; sync stales the validation through the normal
//! compiler-owned mechanism; doctor reports the precise problem.

use std::collections::BTreeSet;

use anyhow::{anyhow, bail, Context, Result};

use crate::journey::{
    CliOperation, JourneyOperationExerciseFacet, INTERFACE_SURFACE_SCHEMA, JOURNEY_COMPILER_VERSION,
};
use crate::model::{EdgeKind, InspectionStatus, Node, NodeType, TargetKind};
use crate::store::Store;

/// One downstream exercise entry the accepted surface demands the compiler
/// realize. Every path here is resolved: `codefile_name` is the canonical
/// `CodeFile.name`, never the authored key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedExercise {
    pub operation_id: String,
    pub exercise_id: String,
    pub observed_by: String,
    pub locator: String,
    pub codefile_id: String,
    pub codefile_name: String,
}

/// The genuine top-level public entry of the accepted surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedPublicEntry {
    pub codefile_id: String,
    pub codefile_name: String,
    /// Empty/absent locator normalized to `None`.
    pub locator: Option<String>,
}

/// Canonical projection of the operation-exercise provenance a compiled
/// Journey validation's Exercises topology must agree with exactly.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExpectedExerciseProjection {
    pub public_entries: Vec<ExpectedPublicEntry>,
    pub exercises: Vec<ExpectedExercise>,
}

impl ExpectedExerciseProjection {
    /// CodeFile ids the compiler must realize as Exercises targets.
    pub fn target_ids(&self) -> BTreeSet<String> {
        self.public_entries
            .iter()
            .map(|entry| entry.codefile_id.clone())
            .chain(self.exercises.iter().map(|e| e.codefile_id.clone()))
            .collect()
    }

    /// The public surface locator for a codefile, when it is a public entry.
    pub fn public_locator(&self, codefile_id: &str) -> Option<&str> {
        self.public_entries
            .iter()
            .find(|entry| entry.codefile_id == codefile_id)
            .and_then(|entry| entry.locator.as_deref())
    }

    pub fn codefile_name(&self, codefile_id: &str) -> Option<&str> {
        self.public_entries
            .iter()
            .find(|entry| entry.codefile_id == codefile_id)
            .map(|entry| entry.codefile_name.as_str())
            .or_else(|| {
                self.exercises
                    .iter()
                    .find(|e| e.codefile_id == codefile_id)
                    .map(|e| e.codefile_name.as_str())
            })
    }

    /// The canonical facet value the compiler must store on the Exercises edge
    /// for `codefile_id`: the projection's exercises for that codefile, sorted
    /// by `(operation_id, exercise_id)` exactly as compile writes them. Empty
    /// for a codefile that is only the public entry.
    pub fn expected_facet(&self, codefile_id: &str) -> Vec<JourneyOperationExerciseFacet> {
        let mut entries: Vec<JourneyOperationExerciseFacet> = self
            .exercises
            .iter()
            .filter(|e| e.codefile_id == codefile_id)
            .map(|e| JourneyOperationExerciseFacet {
                operation_id: e.operation_id.clone(),
                exercise_id: e.exercise_id.clone(),
                observed_by: e.observed_by.clone(),
                locator: e.locator.clone(),
            })
            .collect();
        entries.sort_by(|left, right| {
            left.operation_id
                .cmp(&right.operation_id)
                .then_with(|| left.exercise_id.cmp(&right.exercise_id))
        });
        entries
    }

    /// Canonical covered-file evidence: resolved `CodeFile.name` paths only,
    /// sorted. Never an authored alias or node id.
    pub fn covered_files(&self) -> Vec<String> {
        let names: BTreeSet<String> = self
            .public_entries
            .iter()
            .map(|entry| entry.codefile_name.clone())
            .chain(self.exercises.iter().map(|e| e.codefile_name.clone()))
            .collect();
        names.into_iter().collect()
    }
}

fn projection_current(status: InspectionStatus) -> bool {
    matches!(
        status,
        InspectionStatus::Uninspected | InspectionStatus::Passing
    )
}

/// Operation exercises must name a live callable symbol. Navigation anchors and
/// non-callable declarations are refused so compile cannot invent an
/// S3-ineligible "entry" that looks authored.
pub fn require_callable_exercise_locator(
    store: &Store,
    codefile: &Node,
    locator: &str,
) -> Result<()> {
    if crate::locator::is_anchor_locator(locator) {
        bail!("navigation-only anchor locators are not valid operation exercise entries");
    }
    crate::locator::validate_for_codefile(store, codefile, locator)?;
    let symbols = crate::locator::symbols(locator);
    if symbols.is_empty() {
        bail!("operation exercise locator must name a callable symbol");
    }
    let path = store.root().join(&codefile.name);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("reading CodeFile '{}'", codefile.name))?;
    let extracted = crate::extract::extract(&codefile.name, &content);
    for symbol in symbols {
        let Some(entry) = extracted
            .symbols
            .iter()
            .find(|candidate| candidate.name == symbol)
        else {
            bail!(
                "operation exercise locator '{locator}' does not resolve in '{}'",
                codefile.name
            );
        };
        if !matches!(entry.kind.as_str(), "function" | "method") {
            bail!(
                "operation exercise locator '{locator}' resolves to non-callable '{}' in '{}'",
                entry.kind,
                codefile.name
            );
        }
    }
    Ok(())
}

/// Build the canonical expected projection from the CURRENT accepted surface
/// of `journey`. Fails closed (Err) when no complete, current projection
/// exists: no/ambiguous hash-bound surface, malformed surface content,
/// incomplete bindings, unresolvable exercises, missing live files, or
/// duplicate exercise provenance.
pub fn expected_projection(store: &Store, journey: &Node) -> Result<ExpectedExerciseProjection> {
    if journey.node_type != NodeType::Journey {
        bail!("projection source '{}' is not a Journey", journey.name);
    }
    let Some(journey_hash) = journey
        .body
        .get("semantic_hash")
        .and_then(serde_json::Value::as_str)
    else {
        bail!("Journey '{}' has no semantic hash", journey.name);
    };
    let step_ids: Vec<&str> = journey
        .body
        .get("step_ids")
        .and_then(serde_json::Value::as_array)
        .map(|steps| steps.iter().filter_map(serde_json::Value::as_str).collect())
        .ok_or_else(|| anyhow!("Journey '{}' has no step_ids", journey.name))?;

    let mut current_surfaces = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)? {
        if !projection_current(edge.status)
            || store
                .get_facet(&edge.id, TargetKind::Edge, "journey_hash")?
                .as_deref()
                != Some(journey_hash)
        {
            continue;
        }
        current_surfaces.push(edge);
    }
    let [surface_edge] = current_surfaces.as_slice() else {
        bail!(
            "Journey '{}' has {} current hash-bound surface(s); exactly one is required",
            journey.name,
            current_surfaces.len()
        );
    };
    let surface = store.get_node(&surface_edge.to_id)?.ok_or_else(|| {
        anyhow!(
            "Journey '{}' accepted surface '{}' is missing",
            journey.name,
            surface_edge.to_id
        )
    })?;
    if surface.node_type != NodeType::InterfaceSurface
        || surface.status == "quarantined"
        || surface
            .body
            .get("schema")
            .and_then(serde_json::Value::as_str)
            != Some(INTERFACE_SURFACE_SCHEMA)
        || surface.body.get("kind").and_then(serde_json::Value::as_str) != Some("cli")
    {
        bail!(
            "Journey '{}' accepted surface is not a current reusable CLI",
            journey.name
        );
    }
    let operations: Vec<CliOperation> = serde_json::from_value(
        surface
            .body
            .get("operations")
            .cloned()
            .ok_or_else(|| anyhow!("InterfaceSurface '{}' has no operations", surface.name))?,
    )
    .with_context(|| format!("decoding InterfaceSurface '{}' operations", surface.name))?;

    // Complete step bindings only: every authored step bound exactly once,
    // every bound operation declared on the surface.
    let bindings =
        match store.get_facet(&surface_edge.id, TargetKind::Edge, "operation_bindings")? {
            Some(raw) => crate::completeness::exact_surface_bindings(&raw, &step_ids, &operations)
                .ok_or_else(|| {
                    anyhow!(
                        "Journey '{}' surface lacks canonical complete operation bindings",
                        journey.name
                    )
                })?,
            None => bail!(
                "Journey '{}' surface has no operation bindings",
                journey.name
            ),
        };

    let mut public_entries = Vec::new();
    for exposes in store.edges_with(Some(EdgeKind::Exposes), Some(&surface.id), None)? {
        if !projection_current(exposes.status) {
            continue;
        }
        // Fail closed on any malformed current exposure: a missing target, a
        // non-CodeFile target, or a non-live file are corrupt acceptance data,
        // never silently skipped.
        let codefile = store.get_node(&exposes.to_id)?.ok_or_else(|| {
            anyhow!(
                "InterfaceSurface '{}' exposes missing node '{}'",
                surface.name,
                exposes.to_id
            )
        })?;
        if codefile.node_type != NodeType::CodeFile {
            bail!(
                "InterfaceSurface '{}' exposes non-CodeFile '{}'",
                surface.name,
                codefile.name
            );
        }
        if !store.root().join(&codefile.name).is_file() {
            bail!(
                "InterfaceSurface '{}' exposed CodeFile '{}' is not a live file",
                surface.name,
                codefile.name
            );
        }
        let locator = store
            .edge_locator(&exposes.id)?
            .filter(|value| !value.trim().is_empty());
        public_entries.push(ExpectedPublicEntry {
            codefile_id: codefile.id,
            codefile_name: codefile.name,
            locator,
        });
    }
    // The surface's top-level codefile/locator is the single real public
    // entrypoint. Exactly one live CodeFile exposure is the contract.
    if public_entries.len() != 1 {
        bail!(
            "Journey '{}' CLI surface must expose exactly one live CodeFile (found {})",
            journey.name,
            public_entries.len()
        );
    }
    public_entries.sort_by(|left, right| left.codefile_name.cmp(&right.codefile_name));

    let mut exercises = Vec::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for binding in &bindings {
        let Some(operation_id) = binding.operation_id() else {
            continue;
        };
        let operation = operations
            .iter()
            .find(|candidate| candidate.id == operation_id)
            .ok_or_else(|| {
                anyhow!("bound operation '{operation_id}' is not declared on the surface")
            })?;
        for exercise in &operation.exercises {
            if !seen.insert((operation.id.clone(), exercise.id.clone())) {
                bail!(
                    "operation '{}' declares duplicate exercise '{}'",
                    operation.id,
                    exercise.id
                );
            }
            for (field, value) in [
                ("operation id", operation.id.as_str()),
                ("exercise id", exercise.id.as_str()),
                ("observed_by", exercise.observed_by.as_str()),
                ("locator", exercise.locator.as_str()),
            ] {
                if value.trim().is_empty() {
                    bail!(
                        "operation '{}' exercise '{}' has an empty {field}",
                        operation.id,
                        exercise.id
                    );
                }
            }
            if crate::locator::is_anchor_locator(&exercise.locator) {
                bail!(
                    "operation '{}' exercise '{}' locator must not be a navigation-only anchor",
                    operation.id,
                    exercise.id
                );
            }
            if !operation
                .output
                .assertions
                .iter()
                .any(|assertion| assertion.id == exercise.observed_by)
            {
                bail!(
                    "operation '{}' exercise '{}' observed_by '{}' is not an assertion in the same operation",
                    operation.id,
                    exercise.id,
                    exercise.observed_by
                );
            }
            let codefile = store
                .resolve_node(&exercise.codefile, Some(NodeType::CodeFile))
                .with_context(|| {
                    format!(
                        "operation '{}' exercise '{}' codefile '{}'",
                        operation.id, exercise.id, exercise.codefile
                    )
                })?;
            if !store.root().join(&codefile.name).is_file() {
                bail!(
                    "operation '{}' exercise '{}' codefile '{}' is not a live file",
                    operation.id,
                    exercise.id,
                    codefile.name
                );
            }
            require_callable_exercise_locator(store, &codefile, &exercise.locator).with_context(
                || {
                    format!(
                        "operation '{}' exercise '{}' locator '{}'",
                        operation.id, exercise.id, exercise.locator
                    )
                },
            )?;
            exercises.push(ExpectedExercise {
                operation_id: operation.id.clone(),
                exercise_id: exercise.id.clone(),
                observed_by: exercise.observed_by.clone(),
                locator: exercise.locator.clone(),
                codefile_id: codefile.id,
                codefile_name: codefile.name,
            });
        }
    }
    // `exact_surface_bindings` already enforced completeness and uniqueness:
    // every authored step bound exactly once and every machine operation
    // unique. Only machine (Operation) bindings can declare exercises;
    // HumanDecision bindings reference a prior bound operation and own none.
    exercises.sort_by(|left, right| {
        left.codefile_name
            .cmp(&right.codefile_name)
            .then_with(|| left.operation_id.cmp(&right.operation_id))
            .then_with(|| left.exercise_id.cmp(&right.exercise_id))
    });
    Ok(ExpectedExerciseProjection {
        public_entries,
        exercises,
    })
}

/// The projection for a compiler-owned Journey validation, or `None` when the
/// validation is not a current compiler-v6 Journey proof at all. Errors when
/// the accepted surface itself cannot yield a projection — callers fail closed
/// on that too.
pub fn expected_projection_for_validation(
    store: &Store,
    validation: &Node,
) -> Result<Option<ExpectedExerciseProjection>> {
    if validation.node_type != NodeType::Validation
        || validation
            .body
            .get("type")
            .and_then(serde_json::Value::as_str)
            != Some("journey")
        || validation
            .body
            .get("profile")
            .and_then(serde_json::Value::as_str)
            != Some("proof")
        || validation
            .body
            .get("compiler_version")
            .and_then(serde_json::Value::as_str)
            != Some(JOURNEY_COMPILER_VERSION)
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
    if journey.node_type != NodeType::Journey
        || validation
            .body
            .get("journey_hash")
            .and_then(serde_json::Value::as_str)
            != journey
                .body
                .get("semantic_hash")
                .and_then(serde_json::Value::as_str)
    {
        return Ok(None);
    }
    let expected_surface_hash = crate::journey::surface_projection_hash(store, &journey)?;
    if expected_surface_hash.is_none()
        || validation
            .body
            .get("surface_hash")
            .and_then(serde_json::Value::as_str)
            != expected_surface_hash.as_deref()
    {
        return Ok(None);
    }
    Ok(Some(expected_projection(store, &journey)?))
}

/// Exact semantic agreement between the compiled Exercises topology/facets of
/// `validation_id` and the expected projection. Returns a list of disagreement
/// descriptions; empty means exact agreement. Never bails on mismatches — a
/// disagreement is a report, and every caller fails closed on a nonempty list.
pub fn topology_problems(
    store: &Store,
    validation_id: &str,
    projection: &ExpectedExerciseProjection,
) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    let edges = store.edges_with(Some(EdgeKind::Exercises), Some(validation_id), None)?;
    // The compiler writes exactly one current edge per target. A duplicate or
    // a stale edge is corrupted topology: it must not be able to agree with
    // the projection and quietly earn S3.
    let mut seen_targets = BTreeSet::new();
    for edge in &edges {
        if !seen_targets.insert(edge.to_id.clone()) {
            problems.push(format!(
                "duplicate Exercises edges target CodeFile '{}'",
                edge.to_id
            ));
        }
        if !projection_current(edge.status) {
            problems.push(format!(
                "Exercises edge to '{}' is not current (status '{}')",
                edge.to_id,
                edge.status.as_str()
            ));
        }
    }
    let compiled_targets: BTreeSet<String> = edges.iter().map(|edge| edge.to_id.clone()).collect();
    let expected_targets = projection.target_ids();
    if compiled_targets != expected_targets {
        problems.push(format!(
            "Exercises targets differ from the accepted surface (compiled: {}, expected: {})",
            compiled_targets
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            expected_targets
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for edge in &edges {
        let Some(codefile_name) = projection.codefile_name(&edge.to_id) else {
            problems.push(format!(
                "Exercises edge targets CodeFile '{}' that the accepted surface does not declare",
                edge.to_id
            ));
            continue;
        };
        let surface_locator = store
            .get_facet(&edge.id, TargetKind::Edge, "surface_locator")?
            .filter(|value| !value.trim().is_empty());
        let expected_surface_locator = projection.public_locator(&edge.to_id);
        if surface_locator.as_deref() != expected_surface_locator {
            problems.push(format!(
                "Exercises edge to '{codefile_name}' has surface_locator {:?}; the accepted surface requires {:?}",
                surface_locator, expected_surface_locator
            ));
        }
        let expected_facet = projection.expected_facet(&edge.to_id);
        let actual: Option<Vec<JourneyOperationExerciseFacet>> = match store.get_facet(
            &edge.id,
            TargetKind::Edge,
            "journey_operation_exercises",
        )? {
            None => None,
            Some(raw) => match serde_json::from_str::<Vec<JourneyOperationExerciseFacet>>(&raw) {
                Ok(entries) => Some(entries),
                Err(error) => {
                    problems.push(format!(
                        "Exercises edge to '{codefile_name}' has malformed journey_operation_exercises JSON: {error}"
                    ));
                    continue;
                }
            },
        };
        match actual {
            Some(mut entries) => {
                entries.sort_by(|left, right| {
                    left.operation_id
                        .cmp(&right.operation_id)
                        .then_with(|| left.exercise_id.cmp(&right.exercise_id))
                });
                if entries != expected_facet {
                    problems.push(format!(
                        "Exercises edge to '{codefile_name}' provenance disagrees with the accepted surface (compiled: {}, expected: {})",
                        serde_json::to_string(&entries).unwrap_or_default(),
                        serde_json::to_string(&expected_facet).unwrap_or_default()
                    ));
                }
            }
            None => {
                if !expected_facet.is_empty() {
                    problems.push(format!(
                        "Exercises edge to '{codefile_name}' has no journey_operation_exercises provenance; the accepted surface requires {}",
                        serde_json::to_string(&expected_facet).unwrap_or_default()
                    ));
                }
            }
        }
        // The compiler-written aggregate `locator` facet must equal the exact
        // canonical union: the public locator (when present) plus every
        // exercise locator, sorted and semicolon-joined. A hand-edited
        // aggregate is corruption, not a navigation hint.
        let mut expected_locators = BTreeSet::new();
        if let Some(public_locator) = projection.public_locator(&edge.to_id) {
            expected_locators.insert(public_locator.to_string());
        }
        for exercise in &expected_facet {
            expected_locators.insert(exercise.locator.clone());
        }
        let expected_aggregate: String =
            expected_locators.into_iter().collect::<Vec<_>>().join(";");
        let actual_aggregate = store
            .edge_locator(&edge.id)?
            .unwrap_or_default();
        if actual_aggregate != expected_aggregate {
            problems.push(format!(
                "Exercises edge to '{codefile_name}' aggregate locator '{actual_aggregate}' disagrees with the canonical '{expected_aggregate}'"
            ));
        }
    }
    Ok(problems)
}
