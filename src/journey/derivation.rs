use super::spec::{insert_unique, nonempty, validate_stable_id, JourneySpec};
use super::SURFACE_SCHEMA;
use crate::Result;
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;
use std::path::Path;

pub const DERIVATION_SCHEMA: &str = "loom.journey-derivation/v1";

// -------------------------------------------------------------------------
// Derivation manifest

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivationManifest {
    pub schema: String,
    pub journey_id: String,
    pub journey_hash: String,
    pub proposal_id: String,
    pub proposal_rationale: String,
    pub intents: Vec<DerivedIntent>,
    pub relationships: Vec<DerivedRelationship>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unresolved_question: Option<DerivationQuestion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivedIntent {
    /// Stable projection-local id used by relationship endpoints.
    pub id: String,
    pub operation: DerivedIntentOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criterion: Option<String>,
    pub level: String,
    pub visibility: String,
    pub rationale: String,
    pub step_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DerivedIntentOperation {
    Create,
    Reuse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivationQuestion {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DerivedRelationshipKind {
    Requires,
    Hierarchy,
}

impl DerivedRelationshipKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requires => "requires",
            Self::Hierarchy => "hierarchy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivedRelationship {
    pub id: String,
    pub kind: DerivedRelationshipKind,
    pub from: String,
    pub to: String,
    pub rationale: String,
}

impl DerivationManifest {
    pub fn parse_json(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading derivation manifest {}", path.display()))?;
        let manifest: Self = serde_json::from_str(&text)
            .with_context(|| format!("parsing {} as {DERIVATION_SCHEMA}", path.display()))?;
        Ok(manifest)
    }

    pub fn validate_for(&self, journey: &JourneySpec, hash: &str) -> Result<()> {
        if self.schema != DERIVATION_SCHEMA {
            bail!(
                "unsupported derivation schema '{}' (expected '{DERIVATION_SCHEMA}')",
                self.schema
            );
        }
        if self.journey_id != journey.id {
            bail!(
                "derivation targets journey '{}', not '{}'",
                self.journey_id,
                journey.id
            );
        }
        if self.journey_hash != hash {
            bail!(
                "derivation manifest is stale for journey '{}' (hash mismatch)",
                journey.id
            );
        }
        if self.intents.is_empty() {
            bail!("derivation manifest must contain at least one technical intent");
        }
        validate_stable_id("derivation proposal", &self.proposal_id)?;
        nonempty("derivation proposal rationale", &self.proposal_rationale)?;
        let journey_steps: BTreeSet<&str> =
            journey.steps.iter().map(|step| step.id.as_str()).collect();
        let mut derivation_ids = BTreeSet::new();
        let mut covered = BTreeSet::new();
        for intent in &self.intents {
            insert_unique(&mut derivation_ids, "derived intent", &intent.id)?;
            nonempty(
                &format!("derived intent '{}' rationale", intent.id),
                &intent.rationale,
            )?;
            nonempty(
                &format!("derived intent '{}' level", intent.id),
                &intent.level,
            )?;
            nonempty(
                &format!("derived intent '{}' visibility", intent.id),
                &intent.visibility,
            )?;
            if intent.step_ids.is_empty() {
                bail!(
                    "derived intent '{}' must cover at least one step",
                    intent.id
                );
            }
            let mut local = BTreeSet::new();
            for step_id in &intent.step_ids {
                if !journey_steps.contains(step_id.as_str()) {
                    bail!(
                        "derived intent '{}' references unknown journey step '{}'",
                        intent.id,
                        step_id
                    );
                }
                if !local.insert(step_id) {
                    bail!(
                        "derived intent '{}' repeats journey step '{}'",
                        intent.id,
                        step_id
                    );
                }
                covered.insert(step_id.as_str());
            }
            match intent.operation {
                DerivedIntentOperation::Reuse => {
                    nonempty(
                        &format!("derived intent '{}' intent_id", intent.id),
                        intent.intent_id.as_deref().unwrap_or(""),
                    )?;
                    if intent.name.is_some() || intent.criterion.is_some() {
                        bail!(
                            "derived intent '{}' operation=reuse must not define name/criterion",
                            intent.id
                        );
                    }
                }
                DerivedIntentOperation::Create => {
                    if intent.intent_id.is_some() {
                        bail!(
                            "derived intent '{}' operation=create must not define intent_id",
                            intent.id
                        );
                    }
                    nonempty(
                        &format!("derived intent '{}' name", intent.id),
                        intent.name.as_deref().unwrap_or(""),
                    )?;
                    nonempty(
                        &format!("derived intent '{}' criterion", intent.id),
                        intent.criterion.as_deref().unwrap_or(""),
                    )?;
                }
            }
        }
        let mut relationship_ids = BTreeSet::new();
        let mut relationships = BTreeSet::new();
        for (index, relationship) in self.relationships.iter().enumerate() {
            let label = format!("relationship #{}", index + 1);
            insert_unique(
                &mut relationship_ids,
                "derived relationship",
                &relationship.id,
            )?;
            nonempty(
                &format!("derived relationship '{}' rationale", relationship.id),
                &relationship.rationale,
            )?;
            if !derivation_ids.contains(&relationship.from) {
                bail!(
                    "{label} references unknown from intent entry '{}'",
                    relationship.from
                );
            }
            if !derivation_ids.contains(&relationship.to) {
                bail!(
                    "{label} references unknown to intent entry '{}'",
                    relationship.to
                );
            }
            if relationship.from == relationship.to {
                bail!("{label} must not link an Intent to itself");
            }
            if !relationships.insert((
                relationship.kind,
                relationship.from.as_str(),
                relationship.to.as_str(),
            )) {
                bail!("{label} duplicates an earlier declared relationship");
            }
        }
        validate_declared_relationship_cycles(&self.relationships)?;
        if let Some(question) = &self.unresolved_question {
            validate_stable_id("derivation question", &question.id)?;
            nonempty("derivation question text", &question.text)?;
        }
        let missing: Vec<&str> = journey_steps.difference(&covered).copied().collect();
        if !missing.is_empty() {
            bail!(
                "derivation does not cover journey step(s): {}",
                missing.join(", ")
            );
        }
        Ok(())
    }
}

fn validate_declared_relationship_cycles(relationships: &[DerivedRelationship]) -> Result<()> {
    for kind in [
        DerivedRelationshipKind::Requires,
        DerivedRelationshipKind::Hierarchy,
    ] {
        let edges: Vec<(String, String)> = relationships
            .iter()
            .filter(|relationship| relationship.kind == kind)
            .map(|relationship| (relationship.from.clone(), relationship.to.clone()))
            .collect();
        for (from, to) in &edges {
            if relationship_path_exists(to, from, &edges, &mut BTreeSet::new()) {
                bail!(
                    "declared {} relationships contain a cycle through '{}' and '{}'",
                    kind.as_str(),
                    from,
                    to
                );
            }
        }
    }
    Ok(())
}

fn relationship_path_exists(
    current: &str,
    target: &str,
    edges: &[(String, String)],
    seen: &mut BTreeSet<String>,
) -> bool {
    if current == target {
        return true;
    }
    if !seen.insert(current.to_string()) {
        return false;
    }
    edges
        .iter()
        .filter(|(from, _)| from == current)
        .any(|(_, to)| relationship_path_exists(to, target, edges, seen))
}

// -------------------------------------------------------------------------
// Reusable CLI surface manifest

/// The reusable surface-contract template emitted by `loom journey surface`.
///
/// Operations and bindings are generated from the authored Journey steps, so
/// the document is a complete [`SurfaceManifest`] shape for that Journey.
/// What is generated, and — just as importantly — what is deliberately NOT:
///
/// - `output.captures`: one typed capture per `JourneyStep.produces` entry,
///   with the authored `type` and a `/<output-id>` pointer scaffold. The
///   pointer must be aligned with the real tool's stdout document — loom
///   cannot infer it, so the scaffold is structural, not final.
/// - `output.assertions`: one sample boolean assertion per step, again with a
///   pointer the author must align with the real output.
/// - `exercises`: NEVER generated. Operation exercises are provenance of a
///   real downstream process boundary, and the authored JourneyStep model
///   does not express one; fabricating an entry would manufacture S3
///   provenance that no process earned. Add `exercises` entries by hand only
///   for an operation whose output a downstream process actually consumes.
/// - Human decisions: the authored JourneyStep model carries no
///   human-decision marker, so the template cannot know which steps are
///   host-mediated. A Journey with human-gated steps therefore requires
///   STRUCTURAL editing of the generated manifest: replace the step's
///   operation binding with a `HumanDecision` binding (whose observing
///   operation's stdout carries the prompt object) and add a `setup` block
///   with the local_snapshot graph. Only CodeFile keys, locators, argv, and
///   pointers can be replaced as-is; human-gated Journeys cannot.
///
/// There is no `setup` block. Operation, assertion, and surface ids are
/// derived from the authored step ids and need not change unless the caller
/// wants different stable ids.
pub fn surface_contract_template(spec: &JourneySpec) -> Result<serde_json::Value> {
    spec.validate()?;
    let journey_hash = spec.semantic_hash()?;
    let mut operations = Vec::new();
    let mut bindings = Vec::new();
    for step in &spec.steps {
        let operation_id = format!("{}-operation", step.id);
        let assertion_id = format!("{}-ok", step.id);
        let captures: Vec<serde_json::Value> = step
            .produces
            .iter()
            .map(|(output_id, output)| {
                json!({
                    "id": output_id,
                    "pointer": format!("/{output_id}"),
                    "type": output.value_type,
                    "redact": false,
                })
            })
            .collect();
        operations.push(json!({
            "id": operation_id,
            "summary": format!("CLI operation for step '{}'", step.name),
            "argv": ["binary", "subcommand"],
            "arguments": [],
            "output": {
                "format": "json",
                "captures": captures,
                "assertions": [{
                    "id": assertion_id,
                    "pointer": "/ok",
                    "type": "boolean",
                    "equals": true
                }]
            }
        }));
        bindings.push(json!({
            "step_id": step.id,
            "operation_id": operation_id
        }));
    }
    Ok(json!({
        "schema": SURFACE_SCHEMA,
        "journey_id": spec.id,
        "journey_hash": journey_hash,
        "surface": {
            "id": "stable-cli-surface-id",
            "title": "Reusable CLI surface title",
            "identity": "binary subcommand",
            "codefile": "required existing CodeFile key",
            "locator": "required live CLI entrypoint symbol or strict anchor:<id>",
            "operations": operations
        },
        "bindings": bindings
    }))
}
