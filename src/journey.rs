//! Strict semantic Journey artifacts and their accepted projections.
//!
//! A Journey is authored without transport or implementation detail. It says
//! who does what, in which order, and what must then be true. Technical intents
//! and reusable CLI surfaces are separate, hash-bound projections accepted by
//! dedicated commands.

use crate::Result;
use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

pub const JOURNEY_SCHEMA: &str = "loom.journey/v1";
pub const DERIVATION_SCHEMA: &str = "loom.journey-derivation/v1";
pub const SURFACE_SCHEMA: &str = "loom.journey.surface/v1";
pub const INTERFACE_SURFACE_SCHEMA: &str = "loom.interface-surface/v1";
pub const COMPILED_PROOF_SCHEMA: &str = "loom.journey.proof/v1";
pub const BASELINE_SCHEMA: &str = "loom.journey.baseline/v1";
pub const JOURNEY_COMPILER_VERSION: &str = "4";
pub const JOURNEY_LINT_REPORT_SCHEMA: &str = "loom.journey-lint/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JourneyLintSeverity {
    Blocking,
    Advisory,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct JourneyLintFinding {
    pub rule: String,
    pub severity: JourneyLintSeverity,
    pub journey_id: String,
    pub manifest_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assertion: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JourneyLintReport {
    pub schema: String,
    pub status: String,
    pub scanned: usize,
    pub blocking: usize,
    pub advisory: usize,
    pub findings: Vec<JourneyLintFinding>,
}

impl JourneyLintReport {
    pub fn new(scanned: usize, mut findings: Vec<JourneyLintFinding>) -> Self {
        findings.sort();
        let blocking = findings
            .iter()
            .filter(|f| f.severity == JourneyLintSeverity::Blocking)
            .count();
        let advisory = findings.len() - blocking;
        Self {
            schema: JOURNEY_LINT_REPORT_SCHEMA.into(),
            status: if blocking == 0 { "passed" } else { "blocked" }.into(),
            scanned,
            blocking,
            advisory,
            findings,
        }
    }
}

fn default_true() -> bool {
    true
}

pub const DEFAULT_JOURNEY_TIMEOUT_SECONDS: u64 = 2700;

fn default_journey_timeout_seconds() -> u64 {
    DEFAULT_JOURNEY_TIMEOUT_SECONDS
}

/// Value types shared by Journey inputs/outputs and CLI operation arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    String,
    Integer,
    Number,
    Boolean,
    Json,
}

impl ValueType {
    pub(crate) fn accepts(self, value: &Value) -> bool {
        match self {
            Self::String => value.is_string(),
            Self::Integer => value.as_i64().is_some() || value.as_u64().is_some(),
            Self::Number => value.is_number(),
            Self::Boolean => value.is_boolean(),
            Self::Json => true,
        }
    }

    pub(crate) fn is_scalar(self) -> bool {
        !matches!(self, Self::Json)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JourneyInput {
    #[serde(rename = "type")]
    pub value_type: ValueType,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub required: bool,
    /// Secret inputs are never authored as values. Every proof profile must
    /// resolve them from a named process environment variable.
    #[serde(default)]
    pub secret: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

/// One semantic action. Array order is the Journey's linear order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JourneyStep {
    pub id: String,
    pub name: String,
    pub action: String,
    pub expects: Vec<String>,
    pub produces: BTreeMap<String, JourneyOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JourneyOutput {
    #[serde(rename = "type")]
    pub value_type: ValueType,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemporaryFile {
    pub path: String,
    pub content: String,
}

/// Declarative, confined fixture setup. There is deliberately no command hook.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemporarySetup {
    #[serde(default)]
    pub directories: Vec<String>,
    #[serde(default)]
    pub files: Vec<TemporaryFile>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileInputBinding {
    /// A non-secret template. References use `inputs.<id>` or `run.id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Name of a process environment variable read only at proof execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JourneyProfile {
    pub inputs: BTreeMap<String, ProfileInputBinding>,
    pub workspace: TemporarySetup,
    #[serde(default = "default_journey_timeout_seconds")]
    pub timeout_seconds: u64,
}

/// The only authored Journey schema. It contains no HTTP, command, or Intent
/// fields; those implementation projections are accepted separately.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JourneySpec {
    pub schema: String,
    pub id: String,
    pub name: String,
    pub actor: String,
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub inputs: BTreeMap<String, JourneyInput>,
    pub preconditions: Vec<String>,
    pub steps: Vec<JourneyStep>,
    pub profiles: BTreeMap<String, JourneyProfile>,
}

impl JourneySpec {
    pub fn validate(&self) -> Result<()> {
        if self.schema != JOURNEY_SCHEMA {
            bail!(
                "unsupported Journey schema '{}' (expected '{JOURNEY_SCHEMA}')",
                self.schema
            );
        }
        validate_stable_id("journey", &self.id)?;
        nonempty("journey name", &self.name)?;
        nonempty("journey actor", &self.actor)?;
        nonempty("journey goal", &self.goal)?;
        if self.steps.is_empty() {
            bail!(
                "journey '{}' must contain at least one semantic step",
                self.id
            );
        }

        if self
            .description
            .as_deref()
            .is_some_and(|description| description.trim().is_empty())
        {
            bail!("journey description must not be empty when present");
        }
        // Every addressable semantic element has one globally unique stable id.
        let mut all_ids = BTreeSet::new();
        insert_unique(&mut all_ids, "journey", &self.id)?;
        for (input_id, input) in &self.inputs {
            insert_unique(&mut all_ids, "input", input_id)?;
            if let Some(default) = &input.default {
                if input.secret {
                    bail!(
                        "secret input '{}' must not declare a default; bind it with profiles.proof.inputs.{}.env",
                        input_id,
                        input_id
                    );
                }
                if !input.value_type.accepts(default) {
                    bail!(
                        "input '{}' default does not match type {:?}",
                        input_id,
                        input.value_type
                    );
                }
            }
        }
        let input_by_id: BTreeMap<&str, &JourneyInput> = self
            .inputs
            .iter()
            .map(|(id, input)| (id.as_str(), input))
            .collect();
        for (index, precondition) in self.preconditions.iter().enumerate() {
            nonempty(&format!("precondition #{}", index + 1), precondition)?;
            validate_template_references(precondition, &input_by_id, None)?;
        }
        let mut prior_outputs = BTreeSet::new();
        for step in &self.steps {
            insert_unique(&mut all_ids, "step", &step.id)?;
            nonempty(&format!("step '{}' name", step.id), &step.name)?;
            nonempty(&format!("step '{}' action", step.id), &step.action)?;
            validate_template_references(&step.action, &input_by_id, Some(&prior_outputs))?;
            for (index, expectation) in step.expects.iter().enumerate() {
                nonempty(
                    &format!("step '{}' expectation #{}", step.id, index + 1),
                    expectation,
                )?;
                validate_template_references(expectation, &input_by_id, Some(&prior_outputs))?;
            }
            for output_id in step.produces.keys() {
                validate_stable_id("step output", output_id)?;
                prior_outputs.insert(format!("steps.{}.outputs.{}", step.id, output_id));
            }
        }
        if !self.profiles.contains_key("proof") {
            bail!(
                "journey '{}' must declare the canonical proof profile at profiles.proof",
                self.id
            );
        }
        for (profile_id, profile) in &self.profiles {
            insert_unique(&mut all_ids, "profile", profile_id)?;
            for (input_id, binding) in &profile.inputs {
                let input = input_by_id.get(input_id.as_str()).ok_or_else(|| {
                    anyhow!(
                        "profile '{}' supplies unknown input '{}'",
                        profile_id,
                        input_id
                    )
                })?;
                match (&binding.template, &binding.env) {
                    (Some(_), Some(_)) | (None, None) => bail!(
                        "profiles.{}.inputs.{} must declare exactly one of template or env",
                        profile_id,
                        input_id
                    ),
                    (Some(_), None) if input.secret => bail!(
                        "secret input '{}' must bind via profiles.{}.inputs.{}.env, never template",
                        input_id,
                        profile_id,
                        input_id
                    ),
                    (Some(template), None) => {
                        validate_template_references(template, &input_by_id, None).with_context(
                            || format!("profiles.{}.inputs.{}.template", profile_id, input_id),
                        )?;
                        if !template.contains("{{") {
                            let value = parse_typed_text(template, input.value_type).with_context(
                                || {
                                    format!(
                                        "profiles.{}.inputs.{}.template has the wrong type",
                                        profile_id, input_id
                                    )
                                },
                            )?;
                            if !input.value_type.accepts(&value) {
                                bail!(
                                    "profiles.{}.inputs.{}.template does not match type {:?}",
                                    profile_id,
                                    input_id,
                                    input.value_type
                                );
                            }
                        }
                    }
                    (None, Some(env)) => {
                        validate_env_name(profile_id, input_id, env)?;
                        if input.secret && profile.workspace.env.contains_key(env) {
                            bail!(
                                "secret input '{}' environment '{}' must come from the process, not literal profile setup.env",
                                input_id,
                                env
                            );
                        }
                    }
                }
            }
            if profile_id == "proof" {
                for (input_id, input) in &self.inputs {
                    if input.required
                        && input.default.is_none()
                        && !profile.inputs.contains_key(input_id)
                    {
                        bail!(
                            "required input '{}' must bind at profiles.proof.inputs.{}",
                            input_id,
                            input_id
                        );
                    }
                    if input.secret
                        && profile
                            .inputs
                            .get(input_id)
                            .and_then(|binding| binding.env.as_ref())
                            .is_none()
                    {
                        bail!(
                            "secret input '{}' must bind at profiles.proof.inputs.{}.env",
                            input_id,
                            input_id
                        );
                    }
                }
            }
            for (input_id, binding) in &profile.inputs {
                let input = input_by_id.get(input_id.as_str()).expect("validated above");
                if input.secret && binding.template.is_some() {
                    bail!(
                        "secret input '{}' must not have a literal or template binding",
                        input_id
                    );
                }
            }
            validate_setup(profile_id, &profile.workspace)?;
            if profile.timeout_seconds == 0 {
                bail!("Journey profile '{profile_id}' timeout_seconds must be positive");
            }
        }
        Ok(())
    }

    /// Canonical semantic JSON. Only `steps` retains authored array order;
    /// other addressable collections sort by stable id.
    pub fn canonical_value(&self) -> Result<Value> {
        self.validate()?;
        let mut spec = self.clone();
        for profile in spec.profiles.values_mut() {
            profile.workspace.directories.sort();
            profile.workspace.directories.dedup();
            profile.workspace.files.sort_by(|a, b| a.path.cmp(&b.path));
        }
        Ok(canonicalize_value(serde_json::to_value(spec)?))
    }

    pub fn semantic_hash(&self) -> Result<String> {
        let mut canonical = self.canonical_value()?;
        // `name` is display-only. Stable id owns identity and a rename does not
        // stale authorized technical projections.
        let object = canonical
            .as_object_mut()
            .expect("Journey serializes as object");
        object.remove("name");
        if let Some(steps) = object.get_mut("steps").and_then(Value::as_array_mut) {
            for step in steps {
                step.as_object_mut()
                    .expect("JourneyStep serializes as object")
                    .remove("name");
            }
        }
        // Execution policy is bound by the authored artifact and compiled
        // proof, but is not part of the Journey's behavioral identity.
        if let Some(profiles) = object.get_mut("profiles").and_then(Value::as_object_mut) {
            for profile in profiles.values_mut() {
                profile
                    .as_object_mut()
                    .expect("JourneyProfile serializes as object")
                    .remove("timeout_seconds");
            }
        }
        let canonical = serde_json::to_string(&canonical)?;
        Ok(crate::artifact::fingerprint(&canonical))
    }

    /// Semantic context shared by every step projection. Profile/workspace and
    /// display labels are deliberately excluded so their drift only stales
    /// compiled/runtime artifacts.
    pub fn root_semantics_hash(&self) -> Result<String> {
        let value = json!({
            "actor": self.actor,
            "goal": self.goal,
            "description": self.description,
            "inputs": self.inputs,
            "preconditions": self.preconditions,
        });
        Ok(crate::artifact::fingerprint(&serde_json::to_string(
            &canonicalize_value(value),
        )?))
    }

    pub fn step_order_hash(&self) -> String {
        crate::artifact::fingerprint(&self.step_ids().join("\0"))
    }

    pub fn step_semantics_hash(&self) -> Result<String> {
        let by_id = self.step_hashes()?;
        Ok(crate::artifact::fingerprint(&serde_json::to_string(
            &by_id,
        )?))
    }

    pub fn step_hashes(&self) -> Result<BTreeMap<String, String>> {
        self.steps
            .iter()
            .map(|step| {
                let mut value = canonicalize_value(serde_json::to_value(step)?);
                // Step names, like the Journey name, are display labels. The
                // stable step id owns binding identity.
                value
                    .as_object_mut()
                    .expect("JourneyStep serializes as an object")
                    .remove("name");
                Ok((
                    step.id.clone(),
                    crate::artifact::fingerprint(&serde_json::to_string(&value)?),
                ))
            })
            .collect()
    }

    pub fn step_ids(&self) -> Vec<String> {
        self.steps.iter().map(|step| step.id.clone()).collect()
    }
}

pub fn proof_profiles() -> BTreeMap<String, JourneyProfile> {
    BTreeMap::from([(
        "proof".into(),
        JourneyProfile {
            inputs: BTreeMap::new(),
            workspace: TemporarySetup::default(),
            timeout_seconds: DEFAULT_JOURNEY_TIMEOUT_SECONDS,
        },
    )])
}

fn nonempty(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{label} must not be empty");
    }
    Ok(())
}

pub fn validate_stable_id(label: &str, id: &str) -> Result<()> {
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        bail!("{label} id must not be empty");
    };
    if !first.is_ascii_lowercase() {
        bail!("{label} id '{id}' must start with a lowercase ASCII letter");
    }
    let mut previous_separator = false;
    for ch in chars {
        let separator = matches!(ch, '.' | '-' | '_');
        if !(ch.is_ascii_lowercase() || ch.is_ascii_digit() || separator) {
            bail!(
                "{label} id '{id}' may contain only lowercase ASCII letters, digits, '.', '-' and '_'"
            );
        }
        if separator && previous_separator {
            bail!("{label} id '{id}' contains adjacent separators");
        }
        previous_separator = separator;
    }
    if previous_separator {
        bail!("{label} id '{id}' must not end with a separator");
    }
    Ok(())
}

fn insert_unique(ids: &mut BTreeSet<String>, label: &str, id: &str) -> Result<()> {
    validate_stable_id(label, id)?;
    if !ids.insert(id.to_string()) {
        bail!("duplicate stable id '{id}' ({label})");
    }
    Ok(())
}

fn validate_setup(profile_id: &str, setup: &TemporarySetup) -> Result<()> {
    let mut paths = BTreeSet::new();
    for dir in &setup.directories {
        validate_relative_setup_path(profile_id, dir)?;
        if !paths.insert(dir) {
            bail!("profile '{profile_id}' repeats setup path '{dir}'");
        }
    }
    for file in &setup.files {
        validate_relative_setup_path(profile_id, &file.path)?;
        if !paths.insert(&file.path) {
            bail!(
                "profile '{profile_id}' declares setup path '{}' more than once",
                file.path
            );
        }
    }
    for key in setup.env.keys() {
        if key.trim().is_empty() || key.contains('=') || key.contains('\0') {
            bail!("profile '{profile_id}' has invalid environment key '{key}'");
        }
    }
    Ok(())
}

fn validate_env_name(profile_id: &str, input_id: &str, env: &str) -> Result<()> {
    if validate_process_environment_name(env).is_err() {
        if env.is_empty() {
            bail!("profiles.{profile_id}.inputs.{input_id}.env must not be empty");
        }
        bail!(
            "profiles.{profile_id}.inputs.{input_id}.env '{env}' is not a valid environment variable name"
        );
    }
    Ok(())
}

pub(crate) fn validate_process_environment_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("environment variable name must not be empty");
    };
    if !(first == '_' || first.is_ascii_alphabetic())
        || !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        bail!("'{name}' is not a valid process environment variable name");
    }
    Ok(())
}

fn validate_template_references(
    template: &str,
    inputs: &BTreeMap<&str, &JourneyInput>,
    prior_step_outputs: Option<&BTreeSet<String>>,
) -> Result<()> {
    for reference in template_references(template)? {
        if reference == "run.id" {
            continue;
        }
        if let Some(id) = reference.strip_prefix("inputs.") {
            validate_stable_id("input reference", id)?;
            if let Some(input) = inputs.get(id) {
                if input.secret {
                    bail!(
                        "secret input '{}' cannot be interpolated by a template; bind and consume it through env/redacted input",
                        id
                    );
                }
                continue;
            }
            bail!("template references unknown input '{id}'");
        }
        if reference.starts_with("steps.") {
            if prior_step_outputs.is_some_and(|outputs| outputs.contains(reference)) {
                continue;
            }
            bail!("template reference '{reference}' is not an output of a prior Journey step");
        }
        bail!(
            "unsupported template reference '{reference}'; use inputs.<id>, a prior steps.<step>.outputs.<id>, or run.id"
        );
    }
    Ok(())
}

pub(crate) fn template_references(template: &str) -> Result<Vec<&str>> {
    let mut references = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or_else(|| anyhow!("template contains an unterminated '{{{{' reference"))?;
        let reference = after[..end].trim();
        if reference.is_empty() {
            bail!("template contains an empty reference");
        }
        references.push(reference);
        rest = &after[end + 2..];
    }
    if rest.contains("}}") {
        bail!("template contains an unmatched '}}}}'");
    }
    Ok(references)
}

pub(crate) fn parse_typed_text(text: &str, value_type: ValueType) -> Result<Value> {
    let value = match value_type {
        ValueType::String => Value::String(text.to_string()),
        _ => serde_json::from_str(text)
            .with_context(|| format!("'{text}' is not valid {:?} JSON", value_type))?,
    };
    if !value_type.accepts(&value) {
        bail!("'{text}' does not match type {:?}", value_type);
    }
    Ok(value)
}

fn validate_relative_setup_path(profile_id: &str, raw: &str) -> Result<()> {
    let path = Path::new(raw);
    if raw.trim().is_empty() || path.is_absolute() {
        bail!("profile '{profile_id}' setup path '{raw}' must be a non-empty relative path");
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        bail!("profile '{profile_id}' setup path '{raw}' escapes the temporary root");
    }
    Ok(())
}

/// Parse strict JSON, or YAML only for `.yaml`/`.yml` artifacts.
pub fn parse(path: &Path) -> Result<JourneySpec> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading Journey artifact {}", path.display()))?;
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    let spec: JourneySpec =
        if extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml") {
            serde_norway::from_str(&text)
                .with_context(|| format!("parsing {} as {JOURNEY_SCHEMA}", path.display()))?
        } else {
            serde_json::from_str(&text)
                .with_context(|| format!("parsing {} as {JOURNEY_SCHEMA}", path.display()))?
        };
    spec.validate()?;
    Ok(spec)
}

fn canonicalize_value(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize_value).collect()),
        Value::Object(object) => {
            let sorted: BTreeMap<String, Value> = object
                .into_iter()
                .map(|(key, value)| (key, canonicalize_value(value)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        scalar => scalar,
    }
}

/// Deterministic content hash of a Journey's accepted reusable surface
/// projection. Consumers use this as a compiler/routing cache key; it is
/// derived on read and is deliberately not stored as another stale facet.
pub fn surface_projection_hash(
    store: &crate::store::Store,
    journey: &crate::model::Node,
) -> Result<Option<String>> {
    use crate::model::{EdgeKind, TargetKind};

    let surface_edges = store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)?;
    if surface_edges.is_empty() {
        return Ok(None);
    }

    let mut surfaces: Vec<(String, Value)> = Vec::new();
    for edge in surface_edges {
        let surface = store.get_node(&edge.to_id)?.ok_or_else(|| {
            anyhow!(
                "Journey '{}' has a Surfaces edge to missing node '{}'",
                journey.name,
                edge.to_id
            )
        })?;
        let stable_id = surface
            .body
            .get("stable_id")
            .and_then(Value::as_str)
            .unwrap_or(&surface.id)
            .to_string();
        let operation_bindings =
            match store.get_facet(&edge.id, TargetKind::Edge, "operation_bindings")? {
                Some(raw) => canonicalize_value(serde_json::from_str(&raw).with_context(|| {
                    format!(
                        "Surfaces edge '{}' has malformed operation_bindings JSON",
                        edge.id
                    )
                })?),
                None => Value::Null,
            };
        let setup = match store.get_facet(&edge.id, TargetKind::Edge, "setup")? {
            Some(raw) => canonicalize_value(serde_json::from_str(&raw).with_context(|| {
                format!("Surfaces edge '{}' has malformed setup JSON", edge.id)
            })?),
            None => Value::Null,
        };

        let mut exposes: Vec<(String, Value)> = Vec::new();
        for exposed in store.edges_with(Some(EdgeKind::Exposes), Some(&surface.id), None)? {
            let codefile = store.get_node(&exposed.to_id)?.ok_or_else(|| {
                anyhow!(
                    "InterfaceSurface '{}' exposes missing node '{}'",
                    surface.name,
                    exposed.to_id
                )
            })?;
            let sort_key = format!("{}\0{}", codefile.name, codefile.id);
            exposes.push((
                sort_key,
                json!({
                    "codefile_name": codefile.name,
                    "codefile_id": codefile.id,
                    "locator": store.get_facet(&exposed.id, TargetKind::Edge, "locator")?,
                }),
            ));
        }
        exposes.sort_by(|a, b| a.0.cmp(&b.0));
        let sort_key = format!("{}\0{}", stable_id, surface.id);
        surfaces.push((
            sort_key,
            json!({
                "stable_id": stable_id,
                "surface_id": surface.id,
                "surface_body": canonicalize_value(surface.body),
                "journey_hash": store.get_facet(
                    &edge.id,
                    TargetKind::Edge,
                    "journey_hash"
                )?,
                "setup": setup,
                "operation_bindings": operation_bindings,
                "exposes": exposes.into_iter().map(|(_, row)| row).collect::<Vec<_>>(),
            }),
        ));
    }
    surfaces.sort_by(|a, b| a.0.cmp(&b.0));
    let projection = canonicalize_value(json!({
        "journey_semantic_hash": journey.body.get("semantic_hash"),
        "surfaces": surfaces.into_iter().map(|(_, row)| row).collect::<Vec<_>>(),
    }));
    Ok(Some(crate::artifact::fingerprint(&serde_json::to_string(
        &projection,
    )?)))
}

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
/// Every id, path, and locator inside is a deliberate placeholder: callers
/// must substitute repository-specific registered CodeFile keys, live
/// locators, and operation/assertion ids before acceptance. The template is
/// internally consistent by construction — in particular, the example
/// operation exercise's `observed_by` names an assertion that the example
/// operation actually declares.
pub fn surface_contract_template(journey_id: &str, journey_hash: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": SURFACE_SCHEMA,
        "journey_id": journey_id,
        "journey_hash": journey_hash,
        "surface": {
            "id": "stable-cli-surface-id",
            "title": "Reusable CLI surface title",
            "identity": "binary subcommand",
            "codefile": "required existing CodeFile key",
            "locator": "required live CLI entrypoint symbol or strict anchor:<id>",
            "operations": [{
                "id": "stable-operation-id",
                "summary": "One reusable CLI operation",
                "argv": ["binary", "subcommand"],
                "arguments": [{"id":"argument-id", "type":"string", "required":true, "flag":"--argument"}],
                "output": {
                    "format": "json",
                    "assertions": [{
                        "id": "assertion-id-in-this-operation",
                        "pointer": "/ok",
                        "type": "boolean",
                        "equals": true
                    }]
                },
                "exercises": [{
                    "id": "optional-downstream-entry",
                    "codefile": "path/to/handler.rs",
                    "locator": "handler_symbol",
                    "observed_by": "assertion-id-in-this-operation"
                }]
            }]
        },
        "setup": {
            "graph": "local_snapshot",
            "git": {
                "mode": "isolated_snapshot",
                "dirty_paths": ["registered/codefile.rs"]
            },
            "before_steps": {
                "authored-step-id": [{
                    "path": "registered/codefile.rs",
                    "expected_hash": "0123456789abcdef",
                    "content": "exact replacement content"
                }]
            },
            "operations": ["stable-setup-operation-id"]
        },
        "bindings": [{"step_id":"authored-step-id", "operation_id":"stable-operation-id"}]
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceManifest {
    pub schema: String,
    pub journey_id: String,
    pub journey_hash: String,
    pub surface: InterfaceSurfaceDefinition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<SurfaceSetup>,
    pub bindings: Vec<SurfaceBinding>,
}

/// Journey-specific preparation for an accepted reusable surface. The manifest
/// exposes only the graph source and ordered operation ids; cloning, confinement,
/// execution, and evidence accounting remain runtime implementation details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceSetup {
    pub graph: SetupGraph,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<SurfaceGitSetup>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub before_steps: BTreeMap<String, Vec<SurfaceFileAction>>,
    pub operations: Vec<String>,
}

/// One exact file transition applied only inside the trusted local snapshot,
/// immediately before the keyed authored step. Literal content and templates
/// are separate so source files containing `{{ ... }}` remain representable
/// without accidentally becoming runtime interpolation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceFileAction {
    pub path: String,
    pub expected_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupGraph {
    LocalSnapshot,
}

/// Optional Git state materialized only inside the runtime's trusted local
/// snapshot. The manifest names evidence paths; repository initialization,
/// history construction, confinement, and teardown remain runtime details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceGitSetup {
    pub mode: SurfaceGitMode,
    pub dirty_paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceGitMode {
    IsolatedSnapshot,
}

impl SurfaceGitSetup {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.dirty_paths.is_empty() {
            bail!("surface setup git.dirty_paths must not be empty");
        }
        let mut seen = BTreeSet::new();
        for path in &self.dirty_paths {
            validate_surface_git_path(path)?;
            if !seen.insert(path.as_str()) {
                bail!("surface setup git.dirty_paths repeats path '{path}'");
            }
        }
        Ok(())
    }

    pub(crate) fn validate_for_store(&self, store: &crate::store::Store) -> Result<()> {
        self.validate()?;
        let root = store
            .root()
            .canonicalize()
            .with_context(|| format!("canonicalizing graph root {}", store.root().display()))?;
        let registered: BTreeSet<String> = store
            .list_nodes(Some(crate::model::NodeType::CodeFile), usize::MAX)?
            .into_iter()
            .map(|node| node.name)
            .collect();
        for path in &self.dirty_paths {
            if !registered.contains(path) {
                bail!("surface setup git dirty path '{path}' is not a registered CodeFile");
            }
            let file = store.root().join(path);
            let metadata = std::fs::symlink_metadata(&file)
                .with_context(|| format!("reading surface setup git dirty path '{path}'"))?;
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                bail!("surface setup git dirty path '{path}' is not a file");
            }
            if !file
                .canonicalize()
                .with_context(|| format!("canonicalizing surface setup git dirty path '{path}'"))?
                .starts_with(&root)
            {
                bail!("surface setup git dirty path '{path}' escapes the graph root");
            }
        }
        Ok(())
    }
}

impl SurfaceFileAction {
    pub(crate) fn validate(&self) -> Result<()> {
        validate_surface_temporal_path(&self.path)?;
        if self.expected_hash.len() != 16
            || !self
                .expected_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            bail!(
                "surface setup temporal path '{}' expected_hash must be a lowercase 16-digit content fingerprint",
                self.path
            );
        }
        match (&self.content, &self.template) {
            (Some(content), None) => {
                if content.contains("${{") {
                    bail!(
                        "surface setup temporal path '{}' content must not contain '${{{{' syntax",
                        self.path
                    );
                }
                Ok(())
            }
            (None, Some(template)) => {
                if template.contains("${{") {
                    bail!(
                        "surface setup temporal path '{}' template must not contain '${{{{' syntax",
                        self.path
                    );
                }
                template_references(template).map(|_| ())
            }
            (Some(_), Some(_)) => bail!(
                "surface setup temporal path '{}' must declare exactly one of content or template",
                self.path
            ),
            (None, None) => bail!(
                "surface setup temporal path '{}' must declare content or template",
                self.path
            ),
        }
    }

    pub(crate) fn resolve_for_store(&self, store: &crate::store::Store) -> Result<PathBuf> {
        self.validate()?;
        let registered = store
            .list_nodes(Some(crate::model::NodeType::CodeFile), usize::MAX)?
            .into_iter()
            .any(|node| node.name == self.path);
        if !registered {
            bail!(
                "surface setup temporal path '{}' is not a registered CodeFile",
                self.path
            );
        }
        confined_regular_file(store.root(), &self.path, "surface setup temporal path")
    }
}

impl SurfaceSetup {
    fn has_temporal_actions(&self) -> bool {
        self.before_steps
            .values()
            .any(|actions| !actions.is_empty())
    }

    pub(crate) fn validate_for_store(&self, store: &crate::store::Store) -> Result<()> {
        if let Some(git) = &self.git {
            git.validate_for_store(store)?;
        }
        for actions in self.before_steps.values() {
            for action in actions {
                action.resolve_for_store(store)?;
            }
        }
        Ok(())
    }
}

fn validate_surface_temporal_path(path: &str) -> Result<()> {
    validate_confined_surface_path("surface setup temporal", path)
}

fn validate_surface_git_path(path: &str) -> Result<()> {
    validate_confined_surface_path("surface setup git dirty", path)
}

fn validate_confined_surface_path(label: &str, path: &str) -> Result<()> {
    if path.is_empty() || path.trim() != path || path.contains('\\') {
        bail!("{label} path '{path}' is not a normalized relative path");
    }
    let value = Path::new(path);
    if value.is_absolute()
        || value
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("{label} path '{path}' is not a normalized relative path");
    }
    let normalized = value
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if normalized != path {
        bail!("{label} path '{path}' is not a normalized relative path");
    }
    if value.components().any(|component| match component {
        Component::Normal(value) => matches!(value.to_str(), Some(".loom" | ".git")),
        _ => false,
    }) {
        bail!("{label} path '{path}' targets reserved state");
    }
    Ok(())
}

fn confined_regular_file(root: &Path, path: &str, label: &str) -> Result<PathBuf> {
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing graph root {}", root.display()))?;
    let mut current = root.to_path_buf();
    let components: Vec<_> = Path::new(path).components().collect();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            bail!("{label} '{path}' is not a normalized relative path");
        };
        current.push(component);
        let metadata = std::fs::symlink_metadata(&current)
            .with_context(|| format!("reading {label} '{path}'"))?;
        if metadata.file_type().is_symlink() {
            bail!("{label} '{path}' traverses a symlink");
        }
        let last = index + 1 == components.len();
        if (last && !metadata.file_type().is_file()) || (!last && !metadata.is_dir()) {
            bail!("{label} '{path}' is not a regular file");
        }
    }
    if !current
        .canonicalize()
        .with_context(|| format!("canonicalizing {label} '{path}'"))?
        .starts_with(&canonical_root)
    {
        bail!("{label} '{path}' escapes the graph root");
    }
    Ok(current)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceSurfaceDefinition {
    pub id: String,
    pub title: String,
    pub identity: String,
    /// Exactly one CodeFile owns this reusable CLI entrypoint.
    pub codefile: String,
    /// Live symbol locator for the entrypoint in `codefile`.
    pub locator: String,
    pub operations: Vec<CliOperation>,
}

/// Downstream code entry reached through a public CLI operation.
///
/// These are not additional surface owners. The InterfaceSurface still exposes
/// exactly one public executable entrypoint; an operation exercise names a
/// later boundary entry that the operation's observed assertion evidence
/// actually bridges to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationExercise {
    pub id: String,
    pub codefile: String,
    pub locator: String,
    /// Assertion id in this operation's `output.assertions` that observes the
    /// boundary crossing that reaches this entry.
    pub observed_by: String,
}

/// Compiler-owned provenance for one operation exercise on an Exercises edge.
/// Multiple entries that target the same CodeFile are aggregated on one edge;
/// this facet preserves the exact operation mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JourneyOperationExerciseFacet {
    pub operation_id: String,
    pub exercise_id: String,
    pub observed_by: String,
    pub locator: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CliOperation {
    pub id: String,
    pub summary: String,
    /// Structured argv passed directly to a process by a future executor.
    pub argv: Vec<String>,
    /// Host environment names this operation is permitted to inherit, apart
    /// from the runtime's fixed process-launch infrastructure allowlist.
    /// Values are resolved only at execution time and never enter the manifest
    /// hash or compiled proof artifact.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<String>,
    /// Read-only operations may inspect the repository graph in place. Any
    /// surface containing a mutable operation executes against a temporary
    /// Loom-owned workspace/graph instead.
    #[serde(default)]
    pub read_only: bool,
    /// Optional positive override of the selected proof profile's timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub arguments: Vec<OperationArgument>,
    pub output: OperationOutput,
    /// Optional downstream proof entries reached through this public operation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exercises: Vec<OperationExercise>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationArgument {
    pub id: String,
    #[serde(rename = "type")]
    pub value_type: ValueType,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Never persist or print the resolved value of this argument.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub redact: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationOutput {
    pub format: OutputFormat,
    /// Typed values extracted from the JSON document for later operations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub captures: Vec<OutputCapture>,
    /// Machine-checkable content expectations. Exit status is deliberately not
    /// represented here: a process exiting zero is liveness, not an assertion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assertions: Vec<OutputAssertion>,
    /// JSON pointers whose values must be removed from reports and baselines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redact: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputCapture {
    pub id: String,
    /// RFC 6901 JSON pointer into the operation's stdout document.
    pub pointer: String,
    #[serde(rename = "type")]
    pub value_type: ValueType,
    #[serde(default)]
    pub redact: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputAssertion {
    pub id: String,
    /// RFC 6901 JSON pointer into the operation's stdout document.
    pub pointer: String,
    /// Optional type assertion for the selected value.
    pub value_type: Option<ValueType>,
    /// Compare the selected value with this literal.
    pub equals: Option<Value>,
    /// Compare the selected value with a prior input/capture source.
    pub source: Option<String>,
}

const ASSERTION_NOT_EQUALS: &str = "$loom.assertion/not_equals/";
const ASSERTION_EXISTS: &str = "$loom.assertion/exists/";
const ASSERTION_CONTAINS: &str = "$loom.assertion/contains/";
const ASSERTION_MATCHES: &str = "$loom.assertion/matches/";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputAssertionWire {
    id: String,
    pointer: String,
    #[serde(default, rename = "type")]
    value_type: Option<ValueType>,
    #[serde(default)]
    equals: Option<Value>,
    #[serde(default)]
    not_equals: Option<Value>,
    #[serde(default)]
    exists: Option<bool>,
    #[serde(default)]
    contains: Option<Value>,
    #[serde(default)]
    matches: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

impl<'de> Deserialize<'de> for OutputAssertion {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        let wire = OutputAssertionWire::deserialize(deserializer)?;
        let comparisons = [
            wire.equals.is_some(),
            wire.not_equals.is_some(),
            wire.exists.is_some(),
            wire.contains.is_some(),
            wire.matches.is_some(),
            wire.source.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count();
        if comparisons > 1 {
            return Err(D::Error::custom(
                "output assertion must declare at most one comparison operator",
            ));
        }
        let source = if let Some(value) = wire.not_equals {
            Some(format!(
                "{ASSERTION_NOT_EQUALS}{}",
                serde_json::to_string(&value).map_err(D::Error::custom)?
            ))
        } else if let Some(value) = wire.exists {
            Some(format!("{ASSERTION_EXISTS}{value}"))
        } else if let Some(value) = wire.contains {
            Some(format!(
                "{ASSERTION_CONTAINS}{}",
                serde_json::to_string(&value).map_err(D::Error::custom)?
            ))
        } else if let Some(value) = wire.matches {
            Some(format!(
                "{ASSERTION_MATCHES}{}",
                serde_json::to_string(&value).map_err(D::Error::custom)?
            ))
        } else {
            wire.source
        };
        Ok(Self {
            id: wire.id,
            pointer: wire.pointer,
            value_type: wire.value_type,
            equals: wire.equals,
            source,
        })
    }
}

impl Serialize for OutputAssertion {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::Error;

        let mut object = serde_json::Map::new();
        object.insert("id".into(), Value::String(self.id.clone()));
        object.insert("pointer".into(), Value::String(self.pointer.clone()));
        if let Some(value_type) = self.value_type {
            object.insert(
                "type".into(),
                serde_json::to_value(value_type).map_err(S::Error::custom)?,
            );
        }
        if let Some(equals) = &self.equals {
            object.insert("equals".into(), equals.clone());
        } else if let Some(value) = self.not_equals_value() {
            object.insert("not_equals".into(), value);
        } else if let Some(value) = self.exists_value() {
            object.insert("exists".into(), Value::Bool(value));
        } else if let Some(value) = self.contains_value() {
            object.insert("contains".into(), value);
        } else if let Some(pattern) = self.matches_pattern() {
            object.insert("matches".into(), Value::String(pattern));
        } else if let Some(source) = self.runtime_source() {
            object.insert("source".into(), Value::String(source.to_string()));
        }
        Value::Object(object).serialize(serializer)
    }
}

impl OutputAssertion {
    pub(crate) fn runtime_source(&self) -> Option<&str> {
        self.source.as_deref().filter(|source| {
            !source.starts_with(ASSERTION_NOT_EQUALS)
                && !source.starts_with(ASSERTION_EXISTS)
                && !source.starts_with(ASSERTION_CONTAINS)
                && !source.starts_with(ASSERTION_MATCHES)
        })
    }

    pub(crate) fn not_equals_value(&self) -> Option<Value> {
        decode_assertion_value(self.source.as_deref()?, ASSERTION_NOT_EQUALS)
    }

    pub(crate) fn exists_value(&self) -> Option<bool> {
        self.source
            .as_deref()?
            .strip_prefix(ASSERTION_EXISTS)?
            .parse()
            .ok()
    }

    pub(crate) fn contains_value(&self) -> Option<Value> {
        decode_assertion_value(self.source.as_deref()?, ASSERTION_CONTAINS)
    }

    pub(crate) fn matches_pattern(&self) -> Option<String> {
        let encoded = self.source.as_deref()?.strip_prefix(ASSERTION_MATCHES)?;
        serde_json::from_str(encoded).ok()
    }
}

fn decode_assertion_value(source: &str, prefix: &str) -> Option<Value> {
    serde_json::from_str(source.strip_prefix(prefix)?).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationBinding {
    pub step_id: String,
    pub operation_id: String,
}

/// A surface step is either executed through one structured CLI operation or
/// suspended at an intrinsic host-mediated human decision. The untagged
/// variants are individually strict, so a manifest cannot combine both forms
/// or smuggle command/template/default-answer fields into a gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SurfaceBinding {
    Operation(OperationBinding),
    HumanDecision(HumanDecisionBinding),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanDecisionBinding {
    pub step_id: String,
    pub human_decision: HumanDecisionSource,
}

/// The structured prompt is observed from an earlier operation. `pointer`
/// selects a JSON object containing `subject`, `question`, `recommendation`,
/// and two or three ordered `options`; the manifest never contains an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanDecisionSource {
    pub operation_id: String,
    pub pointer: String,
}

impl From<OperationBinding> for SurfaceBinding {
    fn from(binding: OperationBinding) -> Self {
        Self::Operation(binding)
    }
}

impl SurfaceBinding {
    pub fn step_id(&self) -> &str {
        match self {
            Self::Operation(binding) => &binding.step_id,
            Self::HumanDecision(binding) => &binding.step_id,
        }
    }

    pub fn operation_id(&self) -> Option<&str> {
        match self {
            Self::Operation(binding) => Some(&binding.operation_id),
            Self::HumanDecision(_) => None,
        }
    }

    pub fn human_decision(&self) -> Option<&HumanDecisionSource> {
        match self {
            Self::Operation(_) => None,
            Self::HumanDecision(binding) => Some(&binding.human_decision),
        }
    }

    pub fn identity(&self) -> String {
        match self {
            Self::Operation(binding) => format!("operation:{}", binding.operation_id),
            Self::HumanDecision(binding) => format!(
                "human_decision:{}:{}",
                binding.human_decision.operation_id, binding.human_decision.pointer
            ),
        }
    }
}

impl HumanDecisionSource {
    pub fn validate(&self) -> Result<()> {
        validate_stable_id("human decision source operation", &self.operation_id)?;
        validate_json_pointer("human decision source", &self.operation_id, &self.pointer)
    }
}

impl InterfaceSurfaceDefinition {
    pub fn validate(&self) -> Result<()> {
        validate_stable_id("interface surface", &self.id)?;
        nonempty("interface surface title", &self.title)?;
        nonempty("interface surface identity", &self.identity)?;
        nonempty("interface surface codefile", &self.codefile)?;
        nonempty("interface surface locator", &self.locator)?;
        if self.operations.is_empty() {
            bail!("interface surface '{}' must define an operation", self.id);
        }
        let mut operation_ids = BTreeSet::new();
        for operation in &self.operations {
            insert_unique(&mut operation_ids, "operation", &operation.id)?;
            nonempty(
                &format!("operation '{}' summary", operation.id),
                &operation.summary,
            )?;
            if operation.timeout_seconds == Some(0) {
                bail!(
                    "operation '{}' timeout_seconds must be positive",
                    operation.id
                );
            }
            if operation.argv.is_empty() || operation.argv.iter().any(|part| part.is_empty()) {
                bail!(
                    "operation '{}' argv must contain non-empty structured arguments",
                    operation.id
                );
            }
            let mut environment = BTreeSet::new();
            for name in &operation.environment {
                validate_process_environment_name(name).with_context(|| {
                    format!(
                        "operation '{}' has invalid environment declaration",
                        operation.id
                    )
                })?;
                if !environment.insert(name.as_str()) {
                    bail!(
                        "operation '{}' repeats environment name '{}'",
                        operation.id,
                        name
                    );
                }
            }
            for (index, token) in operation.argv.iter().enumerate() {
                if argv_token_source(token)?.is_some() && index == 0 {
                    bail!(
                        "operation '{}' executable argv token cannot be a runtime template",
                        operation.id
                    );
                }
            }
            let executable = Path::new(&operation.argv[0])
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(
                executable.as_str(),
                "sh" | "bash" | "zsh" | "fish" | "cmd" | "powershell" | "pwsh"
            ) {
                bail!(
                    "operation '{}' uses a shell executable; operations must be structured argv",
                    operation.id
                );
            }
            if matches!(
                executable.as_str(),
                "curl" | "wget" | "http" | "https" | "httpie"
            ) || operation
                .argv
                .iter()
                .any(|part| part.starts_with("http://") || part.starts_with("https://"))
            {
                bail!(
                    "operation '{}' declares an HTTP client/URL; Journey proofs execute reusable CLI surfaces only",
                    operation.id
                );
            }
            if executable == "loom" && operation.argv[1..].iter().any(|part| part == "--graph") {
                bail!(
                    "operation '{}' must not supply --graph; the Journey runtime owns graph confinement",
                    operation.id
                );
            }
            let mut argument_ids = BTreeSet::new();
            for argument in &operation.arguments {
                insert_unique(&mut argument_ids, "operation argument", &argument.id)?;
                if let Some(flag) = &argument.flag {
                    if !flag.starts_with('-') || flag.chars().any(char::is_whitespace) {
                        bail!(
                            "operation '{}' argument '{}' has invalid flag '{}'",
                            operation.id,
                            argument.id,
                            flag
                        );
                    }
                }
                if let Some(source) = &argument.source {
                    validate_runtime_source(source).with_context(|| {
                        format!(
                            "operation '{}' argument '{}' has invalid source",
                            operation.id, argument.id
                        )
                    })?;
                }
            }
            let mut output_ids = BTreeSet::new();
            for capture in &operation.output.captures {
                insert_unique(&mut output_ids, "output capture", &capture.id)?;
                validate_json_pointer("capture", &capture.id, &capture.pointer)?;
            }
            for assertion in &operation.output.assertions {
                insert_unique(&mut output_ids, "output assertion", &assertion.id)?;
                validate_json_pointer("assertion", &assertion.id, &assertion.pointer)?;
                let comparisons = [
                    assertion.equals.is_some(),
                    assertion.not_equals_value().is_some(),
                    assertion.contains_value().is_some(),
                    assertion.matches_pattern().is_some(),
                    assertion.runtime_source().is_some(),
                ]
                .into_iter()
                .filter(|present| *present)
                .count();
                if comparisons > 1 {
                    bail!(
                        "assertion '{}' must declare at most one of equals, not_equals, contains, matches, or source",
                        assertion.id
                    );
                }
                if assertion.exists_value().is_some()
                    && (comparisons > 0 || assertion.value_type.is_some())
                {
                    bail!(
                        "assertion '{}' exists operator must not be combined with a type or value comparison",
                        assertion.id
                    );
                }
                if assertion.exists_value().is_none()
                    && comparisons == 0
                    && assertion.value_type.is_none()
                {
                    bail!(
                        "assertion '{}' must declare a type or comparison",
                        assertion.id
                    );
                }
                if assertion.matches_pattern().is_some()
                    && assertion
                        .value_type
                        .is_some_and(|value_type| value_type != ValueType::String)
                {
                    bail!(
                        "assertion '{}' matches operator is compatible only with type string",
                        assertion.id
                    );
                }
                if let Some(pattern) = assertion.matches_pattern() {
                    regex::Regex::new(&pattern).with_context(|| {
                        format!("assertion '{}' has invalid matches regex", assertion.id)
                    })?;
                }
                if assertion.contains_value().is_some()
                    && assertion.value_type.is_some_and(|value_type| {
                        !matches!(value_type, ValueType::String | ValueType::Json)
                    })
                {
                    bail!(
                        "assertion '{}' contains operator requires type string, json, or no explicit type",
                        assertion.id
                    );
                }
                if assertion.value_type == Some(ValueType::String)
                    && assertion
                        .contains_value()
                        .is_some_and(|value| !value.is_string())
                {
                    bail!(
                        "assertion '{}' string contains operand must be a string",
                        assertion.id
                    );
                }
                if let Some(source) = assertion.runtime_source() {
                    validate_runtime_source(source).with_context(|| {
                        format!("assertion '{}' has invalid source", assertion.id)
                    })?;
                }
            }
            for pointer in &operation.output.redact {
                validate_json_pointer("redaction", &operation.id, pointer)?;
            }
            let assertion_ids: BTreeSet<&str> = operation
                .output
                .assertions
                .iter()
                .map(|assertion| assertion.id.as_str())
                .collect();
            let mut exercise_ids = BTreeSet::new();
            for exercise in &operation.exercises {
                insert_unique(
                    &mut exercise_ids,
                    &format!("operation '{}' exercise", operation.id),
                    &exercise.id,
                )?;
                nonempty(
                    &format!(
                        "operation '{}' exercise '{}' codefile",
                        operation.id, exercise.id
                    ),
                    &exercise.codefile,
                )?;
                nonempty(
                    &format!(
                        "operation '{}' exercise '{}' locator",
                        operation.id, exercise.id
                    ),
                    &exercise.locator,
                )?;
                nonempty(
                    &format!(
                        "operation '{}' exercise '{}' observed_by",
                        operation.id, exercise.id
                    ),
                    &exercise.observed_by,
                )?;
                if crate::locator::is_anchor_locator(&exercise.locator) {
                    bail!(
                        "operation '{}' exercise '{}' locator must not be a navigation-only anchor",
                        operation.id,
                        exercise.id
                    );
                }
                if crate::locator::symbols(&exercise.locator).is_empty() {
                    bail!(
                        "operation '{}' exercise '{}' locator must name a resolvable callable symbol",
                        operation.id,
                        exercise.id
                    );
                }
                if !assertion_ids.contains(exercise.observed_by.as_str()) {
                    bail!(
                        "operation '{}' exercise '{}' observed_by '{}' is not an assertion in the same operation",
                        operation.id,
                        exercise.id,
                        exercise.observed_by
                    );
                }
            }
        }
        Ok(())
    }

    pub fn canonical_operations(&self) -> Result<Value> {
        self.validate()?;
        let mut operations = self.operations.clone();
        operations.sort_by(|a, b| a.id.cmp(&b.id));
        for operation in &mut operations {
            operation.arguments.sort_by(|a, b| a.id.cmp(&b.id));
            operation.environment.sort();
            operation.exercises.sort_by(|a, b| a.id.cmp(&b.id));
        }
        Ok(canonicalize_value(serde_json::to_value(operations)?))
    }

    pub fn node_body(&self) -> Result<Value> {
        Ok(json!({
            "schema": INTERFACE_SURFACE_SCHEMA,
            "stable_id": self.id,
            "title": self.title,
            "kind": "cli",
            "identity": self.identity,
            "codefile": self.codefile,
            "locator": self.locator,
            "operations": self.canonical_operations()?,
        }))
    }
}

fn is_exact_graph_identity(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_undeclared_graph_identity(store: &crate::store::Store, text: &str) -> Result<bool> {
    if !is_exact_graph_identity(text) {
        return Ok(false);
    }
    Ok(store.get_node(text)?.is_none() && store.get_edge(text)?.is_none())
}

fn value_contains_undeclared_graph_identity(
    store: &crate::store::Store,
    value: &Value,
) -> Result<bool> {
    match value {
        Value::String(text) => is_undeclared_graph_identity(store, text),
        Value::Array(values) => {
            for value in values {
                if value_contains_undeclared_graph_identity(store, value)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Value::Object(values) => {
            for value in values.values() {
                if value_contains_undeclared_graph_identity(store, value)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn exact_census_pin(assertion: &OutputAssertion) -> bool {
    let Some(value) = &assertion.equals else {
        return false;
    };
    let census_name = assertion
        .pointer
        .split('/')
        .next_back()
        .is_some_and(|segment| {
            let segment = segment.to_ascii_lowercase();
            matches!(
                segment.as_str(),
                "count" | "counts" | "total" | "totals" | "census"
            ) || segment.ends_with("_count")
                || segment.ends_with("_total")
        });
    census_name && (value.is_number() || value.is_array() || value.is_object())
}

fn positional_census_pointer(pointer: &str) -> bool {
    pointer
        .split('/')
        .skip(1)
        .any(|segment| !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit()))
}

fn relies_on_real_clock_minute_bucket(operation: &CliOperation) -> bool {
    // The known self-audit fixture creates many adjudications and audits the
    // resulting judgment burst in one MCP transcript. Audit groups that burst
    // by the host clock's current minute, so crossing a minute is correctness-
    // affecting. Requiring both structural signals avoids matching prose.
    let joined = operation.argv.join(" ");
    joined.contains("\"adjudications\"")
        && (joined.contains("[\"loom\",\"audit\",\"--json\"]")
            || operation
                .argv
                .windows(2)
                .any(|parts| parts == ["loom", "audit"]))
}

impl SurfaceManifest {
    /// Static portability and durability policy shared by lint and acceptance.
    /// Call only after schema validation and setup confinement validation.
    pub fn lint(
        &self,
        store: &crate::store::Store,
        journey: &JourneySpec,
        manifest_path: &str,
    ) -> Result<JourneyLintReport> {
        let mut findings = Vec::new();
        let mut add = |rule: &str,
                       severity,
                       operation: Option<&str>,
                       assertion: Option<&str>,
                       message: String| {
            findings.push(JourneyLintFinding {
                rule: rule.into(),
                severity,
                journey_id: self.journey_id.clone(),
                manifest_path: manifest_path.into(),
                operation: operation.map(str::to_owned),
                assertion: assertion.map(str::to_owned),
                message,
            });
        };
        for operation in &self.surface.operations {
            for arg in &operation.argv {
                if is_undeclared_graph_identity(store, arg)? {
                    add("graph-local-identity", JourneyLintSeverity::Blocking, Some(&operation.id), None, "replace the undeclared 32-hex identity in argv with a repository-declared identity, stable name, or captured value".into());
                }
            }
            if relies_on_real_clock_minute_bucket(operation) {
                add("real-clock-minute-bucket", JourneyLintSeverity::Advisory, Some(&operation.id), None, "replace the real-clock judgment-burst/minute-bucket fixture with deterministic clock-controlled evidence".into());
            }
            for assertion in &operation.output.assertions {
                let undeclared_equals = match &assertion.equals {
                    Some(value) => value_contains_undeclared_graph_identity(store, value)?,
                    None => false,
                };
                let mut undeclared_pointer = false;
                for segment in assertion.pointer.split('/') {
                    if is_undeclared_graph_identity(store, segment)? {
                        undeclared_pointer = true;
                        break;
                    }
                }
                if undeclared_equals || undeclared_pointer {
                    add("graph-local-identity", JourneyLintSeverity::Blocking, Some(&operation.id), Some(&assertion.id), "replace the undeclared 32-hex identity with a repository-declared identity, stable name, or captured value".into());
                }
                if exact_census_pin(assertion) {
                    add("exact-census-pin", JourneyLintSeverity::Advisory, Some(&operation.id), Some(&assertion.id), "assert an invariant or bounded relationship instead of an exact whole-graph count or total".into());
                }
                if positional_census_pointer(&assertion.pointer) {
                    add("positional-census-pointer", JourneyLintSeverity::Advisory, Some(&operation.id), Some(&assertion.id), "select census data by stable identity instead of a numeric JSON-pointer position".into());
                }
                if assertion.not_equals_value() == Some(Value::String(String::new())) {
                    add("not-equals-empty", JourneyLintSeverity::Advisory, Some(&operation.id), Some(&assertion.id), "use an explicit existence, type, or semantic assertion instead of not_equals empty string".into());
                }
            }
        }
        if let Some(setup) = &self.setup {
            let mut transitioned_paths = BTreeSet::new();
            for step in &journey.steps {
                let Some(actions) = setup.before_steps.get(&step.id) else {
                    continue;
                };
                for action in actions {
                    if !transitioned_paths.insert(action.path.clone()) {
                        continue;
                    }
                    let content = std::fs::read_to_string(action.resolve_for_store(store)?)?;
                    if crate::artifact::fingerprint(&content) != action.expected_hash {
                        add("stale-temporal-expected-hash", JourneyLintSeverity::Blocking, None, None, format!("update setup path '{}' expected_hash to the current repository content fingerprint", action.path));
                    }
                }
            }
        }
        Ok(JourneyLintReport::new(1, findings))
    }

    pub fn parse_json(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading surface manifest {}", path.display()))?;
        let manifest: Self = serde_json::from_str(&text)
            .with_context(|| format!("parsing {} as {SURFACE_SCHEMA}", path.display()))?;
        Ok(manifest)
    }

    pub fn validate_for(&self, journey: &JourneySpec, hash: &str) -> Result<()> {
        if self.schema != SURFACE_SCHEMA {
            bail!(
                "unsupported surface schema '{}' (expected '{SURFACE_SCHEMA}')",
                self.schema
            );
        }
        if self.journey_id != journey.id {
            bail!(
                "surface manifest targets journey '{}', not '{}'",
                self.journey_id,
                journey.id
            );
        }
        if self.journey_hash != hash {
            bail!(
                "surface manifest is stale for journey '{}' (hash mismatch)",
                journey.id
            );
        }
        self.surface.validate()?;
        let operations: BTreeSet<&str> = self
            .surface
            .operations
            .iter()
            .map(|operation| operation.id.as_str())
            .collect();
        let operation_by_id: BTreeMap<&str, &CliOperation> = self
            .surface
            .operations
            .iter()
            .map(|operation| (operation.id.as_str(), operation))
            .collect();
        let input_by_id: BTreeMap<&str, &JourneyInput> = journey
            .inputs
            .iter()
            .map(|(id, input)| (id.as_str(), input))
            .collect();
        let journey_steps: BTreeSet<&str> =
            journey.steps.iter().map(|step| step.id.as_str()).collect();
        let mut bound = BTreeSet::new();
        let mut bound_operations = BTreeSet::new();
        let mut has_human_decision = false;
        for binding in &self.bindings {
            let step_id = binding.step_id();
            if !journey_steps.contains(step_id) {
                bail!("surface binding references unknown step '{}'", step_id);
            }
            if !bound.insert(step_id) {
                bail!(
                    "journey step '{}' has more than one surface binding",
                    step_id
                );
            }
            match binding {
                SurfaceBinding::Operation(binding) => {
                    if !operations.contains(binding.operation_id.as_str()) {
                        bail!(
                            "surface binding for step '{}' references unknown operation '{}'",
                            binding.step_id,
                            binding.operation_id
                        );
                    }
                    if !bound_operations.insert(binding.operation_id.as_str()) {
                        bail!(
                            "surface operation '{}' is bound more than once; each Journey step requires one primary operation",
                            binding.operation_id
                        );
                    }
                }
                SurfaceBinding::HumanDecision(binding) => {
                    has_human_decision = true;
                    binding.human_decision.validate()?;
                }
            }
        }
        let missing: Vec<&str> = journey_steps.difference(&bound).copied().collect();
        if !missing.is_empty() {
            bail!(
                "surface manifest does not bind journey step(s): {}",
                missing.join(", ")
            );
        }

        if has_human_decision && self.setup.is_none() {
            bail!("human decision bindings require setup.graph=local_snapshot");
        }
        if let Some(setup) = &self.setup {
            if setup.operations.is_empty() && !setup.has_temporal_actions() && !has_human_decision {
                bail!("surface setup must name an operation or declare a before_steps file action");
            }
            if let Some(git) = &setup.git {
                match setup.graph {
                    SetupGraph::LocalSnapshot => git.validate()?,
                }
            }
            let mut setup_operations = BTreeSet::new();
            let no_outputs = BTreeMap::new();
            for (step_id, actions) in &setup.before_steps {
                if !journey_steps.contains(step_id.as_str()) {
                    bail!("surface setup before_steps references unknown step '{step_id}'");
                }
                if actions.is_empty() {
                    bail!("surface setup before_steps.{step_id} must contain a file action");
                }
                let mut paths = BTreeSet::new();
                for action in actions {
                    action.validate()?;
                    if !paths.insert(action.path.as_str()) {
                        bail!(
                            "surface setup before_steps.{step_id} repeats path '{}'",
                            action.path
                        );
                    }
                }
            }
            for operation_id in &setup.operations {
                if !setup_operations.insert(operation_id.as_str()) {
                    bail!("surface setup repeats operation '{operation_id}'");
                }
                let operation = operation_by_id.get(operation_id.as_str()).ok_or_else(|| {
                    anyhow!("surface setup references unknown operation '{operation_id}'")
                })?;
                if bound_operations.contains(operation_id.as_str()) {
                    bail!(
                        "surface setup operation '{operation_id}' is also bound to an authored step"
                    );
                }
                if operation.read_only {
                    bail!(
                        "surface setup operation '{operation_id}' must be mutable so it can establish the isolated fixture"
                    );
                }
                if !operation.output.captures.is_empty() {
                    bail!(
                        "surface setup operation '{operation_id}' must not capture authored step outputs"
                    );
                }
                if operation.output.assertions.is_empty() {
                    bail!(
                        "surface setup operation '{operation_id}' must assert the fixture it establishes"
                    );
                }
                validate_operation_references(operation, &input_by_id, &no_outputs)?;
            }
        }

        // References are checked in semantic step order. A surface may read
        // authored inputs, this execution's run.id, or typed outputs captured
        // by an operation bound to an earlier step—never a forward/global
        // capture name.
        let binding_by_step: BTreeMap<&str, &SurfaceBinding> = self
            .bindings
            .iter()
            .map(|binding| (binding.step_id(), binding))
            .collect();
        let mut prior_outputs = BTreeMap::new();
        let mut prior_operations = BTreeSet::new();
        for step in &journey.steps {
            if let Some(actions) = self
                .setup
                .as_ref()
                .and_then(|setup| setup.before_steps.get(&step.id))
            {
                for action in actions {
                    validate_temporal_action_references(action, &input_by_id, &prior_outputs)
                        .with_context(|| {
                            format!(
                                "surface setup before_steps.{} path '{}'",
                                step.id, action.path
                            )
                        })?;
                }
            }
            let binding = binding_by_step
                .get(step.id.as_str())
                .expect("complete bindings validated above");
            match binding {
                SurfaceBinding::Operation(binding) => {
                    let operation = operation_by_id
                        .get(binding.operation_id.as_str())
                        .expect("operation binding validated above");
                    validate_operation_references(operation, &input_by_id, &prior_outputs)?;
                    for capture in &operation.output.captures {
                        let authored = step.produces.get(&capture.id).ok_or_else(|| {
                            anyhow!(
                                "operation '{}' captures undeclared output '{}' for Journey step '{}'",
                                operation.id,
                                capture.id,
                                step.id
                            )
                        })?;
                        if authored.value_type != capture.value_type {
                            bail!(
                                "operation '{}' capture '{}' type does not match Journey step '{}' output type",
                                operation.id,
                                capture.id,
                                step.id
                            );
                        }
                        prior_outputs.insert(
                            format!("steps.{}.outputs.{}", step.id, capture.id),
                            (capture.value_type, capture.redact),
                        );
                    }
                    let captured: BTreeSet<&str> = operation
                        .output
                        .captures
                        .iter()
                        .map(|capture| capture.id.as_str())
                        .collect();
                    let missing: Vec<&str> = step
                        .produces
                        .keys()
                        .map(String::as_str)
                        .filter(|id| !captured.contains(id))
                        .collect();
                    if !missing.is_empty() {
                        bail!(
                            "operation '{}' does not capture Journey step '{}' output(s): {}",
                            operation.id,
                            step.id,
                            missing.join(", ")
                        );
                    }
                    prior_operations.insert(operation.id.as_str());
                }
                SurfaceBinding::HumanDecision(binding) => {
                    if !prior_operations.contains(binding.human_decision.operation_id.as_str()) {
                        bail!(
                            "human decision step '{}' must reference an operation bound to an earlier authored step (found '{}')",
                            step.id,
                            binding.human_decision.operation_id
                        );
                    }
                    if !step.produces.is_empty() {
                        bail!(
                            "human decision step '{}' cannot declare produced machine outputs",
                            step.id
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Validate setup paths against local graph authority before accepting a
    /// reusable surface. Compilation repeats this check in the isolated clone.
    pub fn validate_setup_for_store(&self, store: &crate::store::Store) -> Result<()> {
        if let Some(setup) = &self.setup {
            setup.validate_for_store(store)?;
        }
        self.validate_exercises_for_store(store)?;
        Ok(())
    }

    /// Resolve every operation exercise against live CodeFiles and callable
    /// locators. Schema validation already checked ids/assertions; this binds
    /// the declaration to repository content.
    pub fn validate_exercises_for_store(&self, store: &crate::store::Store) -> Result<()> {
        for operation in &self.surface.operations {
            for exercise in &operation.exercises {
                let codefile = store
                    .resolve_node(&exercise.codefile, Some(crate::model::NodeType::CodeFile))
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
                crate::journey_exercises::require_callable_exercise_locator(
                    store,
                    &codefile,
                    &exercise.locator,
                )
                .with_context(|| {
                    format!(
                        "operation '{}' exercise '{}' locator '{}'",
                        operation.id, exercise.id, exercise.locator
                    )
                })?;
            }
        }
        Ok(())
    }

    pub fn canonical_bindings(&self, journey: &JourneySpec) -> Value {
        let by_step: BTreeMap<&str, &SurfaceBinding> = self
            .bindings
            .iter()
            .map(|binding| (binding.step_id(), binding))
            .collect();
        Value::Array(
            journey
                .steps
                .iter()
                .filter_map(|step| by_step.get(step.id.as_str()))
                .map(|binding| {
                    serde_json::to_value(binding).expect("surface binding is serializable")
                })
                .collect(),
        )
    }

    pub fn canonical_setup(&self) -> Result<Option<Value>> {
        self.setup
            .as_ref()
            .map(|setup| serde_json::to_value(setup).map(canonicalize_value))
            .transpose()
            .map_err(Into::into)
    }
}

/// Settle a compiled Journey Validation from the observation made by the
/// direct-argv runtime. Kept beside the Journey proof vocabulary so every proof
/// status write remains on the repository's proof-stability chokepoint.
pub fn settle_compiled_validation(
    store: &crate::store::Store,
    validation_id: &str,
    report: &crate::journey_runtime::RuntimeReport,
    covered_files: &[String],
) -> Result<()> {
    use crate::model::{Claim, EdgeKind, InspectionStatus, RunProducer};
    use crate::store::{Assertion, Subject};

    // The observation must be bound to the exact compiled validation it
    // settles: a report carrying any other journey/surface identity cannot
    // mint this validation's run evidence.
    let validation = store
        .get_node(validation_id)?
        .ok_or_else(|| anyhow!("validation '{validation_id}' is missing"))?;
    if validation
        .body
        .get("type")
        .and_then(serde_json::Value::as_str)
        != Some("journey")
        || validation
            .body
            .get("journey_hash")
            .and_then(serde_json::Value::as_str)
            != Some(report.journey_hash.as_str())
        || validation
            .body
            .get("surface_hash")
            .and_then(serde_json::Value::as_str)
            != Some(report.surface_hash.as_str())
    {
        bail!(
            "compiled Journey report does not match validation '{}' hashes",
            validation.name
        );
    }

    let (node_status, edge_status) = match report.status {
        crate::journey_runtime::RuntimeStatus::Passed => ("passed", InspectionStatus::Passing),
        crate::journey_runtime::RuntimeStatus::Failed => ("failed", InspectionStatus::Failing),
        crate::journey_runtime::RuntimeStatus::Blocked => ("blocked", InspectionStatus::Blocked),
    };
    let evidence = match &report.detail {
        Some(detail) => format!(
            "compiled Journey '{}:{}' observed {}: {detail}",
            report.journey_id, report.profile, node_status
        ),
        None => format!(
            "compiled Journey '{}:{}' observed {} with {} typed assertion(s)",
            report.journey_id, report.profile, node_status, report.assertions_passed
        ),
    };
    let run = if report.status == crate::journey_runtime::RuntimeStatus::Blocked {
        None
    } else {
        let stdout = crate::journey_runtime::report_observation_json(report)?;
        let mut run = crate::runner::record(
            store.root(),
            RunProducer::Journey,
            &format!(
                "loom journey run {} --profile {}",
                report.journey_id, report.profile
            ),
            covered_files,
            report.assertions_passed,
            if report.status == crate::journey_runtime::RuntimeStatus::Passed {
                0
            } else {
                1
            },
            &stdout,
            report.detail.as_deref().unwrap_or("").as_bytes(),
            0,
        );
        // The typed assertions this exact run observed are structured machine
        // evidence on the run record — never recovered from the (truncatable)
        // human-facing stdout excerpt.
        run.observed_assertions = report
            .passed_assertions
            .iter()
            .map(|passed| crate::evidence::ObservedAssertion {
                group: passed.operation_id.clone(),
                assertion: passed.assertion_id.clone(),
            })
            .collect();
        Some(run)
    };

    for kind in [EdgeKind::Validates, EdgeKind::Proves] {
        for edge in store.edges_with(Some(kind), Some(validation_id), None)? {
            let mut assertion = Assertion::new(
                Subject::Edge(edge.id),
                Claim::Verdict,
                edge_status.as_str(),
                "loom",
            )
            .criterion("compiled Journey profile")
            .confidence(1.0)
            .cited(crate::evidence::cite(store.root(), &evidence)?);
            if let Some(run) = &run {
                assertion = assertion.observed(run.clone());
            }
            store.assert_fact(assertion)?;
        }
    }
    store.record_proof_stability(validation_id, node_status)?;
    store.set_node_status(validation_id, node_status)?;
    store.append_journal(
        "journey_run",
        validation_id,
        json!({
            "journey_id": report.journey_id,
            "profile": report.profile,
            "outcome": node_status,
            "journey_hash": report.journey_hash,
            "surface_hash": report.surface_hash,
            "assertions_passed": report.assertions_passed,
            "assertions_failed": report.assertions_failed,
            "detail": report.detail,
        }),
    )?;
    regrade_compiled_validation(store, validation_id)
}

fn regrade_compiled_validation(store: &crate::store::Store, validation_id: &str) -> Result<()> {
    use crate::model::EdgeKind;
    let Some(validation) = store.get_node(validation_id)? else {
        return Ok(());
    };
    let callgraph = crate::callgraph::build(store)?;
    let mut best: Option<crate::proofstrength::StrengthWitness> = None;
    for edge in store.edges_with(Some(EdgeKind::Validates), Some(validation_id), None)? {
        let witness =
            crate::proofstrength::grade(store, store.root(), &validation, &edge.to_id, &callgraph)?;
        let stronger = best
            .as_ref()
            .map(|current| {
                crate::proofstrength::Strength::parse(&witness.grade)
                    > crate::proofstrength::Strength::parse(&current.grade)
            })
            .unwrap_or(true);
        if stronger {
            best = Some(witness);
        }
    }
    if let Some(witness) = best {
        crate::proofstrength::store_witness(store, validation_id, &witness)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeSource<'a> {
    Input(&'a str),
    StepOutput {
        step_id: &'a str,
        output_id: &'a str,
    },
    RunId,
}

pub(crate) fn parse_runtime_source(source: &str) -> Result<RuntimeSource<'_>> {
    if source == "run.id" {
        return Ok(RuntimeSource::RunId);
    }
    if let Some(id) = source.strip_prefix("inputs.") {
        validate_stable_id("input source", id)?;
        return Ok(RuntimeSource::Input(id));
    }
    if let Some(rest) = source.strip_prefix("steps.") {
        let Some((step_id, output_id)) = rest.split_once(".outputs.") else {
            bail!("source '{source}' must use steps.<step-id>.outputs.<output-id>");
        };
        validate_stable_id("step source", step_id)?;
        validate_stable_id("step output source", output_id)?;
        return Ok(RuntimeSource::StepOutput { step_id, output_id });
    }
    bail!("source '{source}' must be inputs.<id>, steps.<prior-step>.outputs.<id>, or run.id")
}

/// Return the runtime source named by one whole argv token. Runtime values may
/// replace a token, never splice into one: that keeps argv ordering explicit
/// and prevents a value from becoming shell syntax or changing token count.
pub(crate) fn argv_token_source(token: &str) -> Result<Option<&str>> {
    if !token.contains("{{") {
        return Ok(None);
    }
    let references = template_references(token)?;
    if references.is_empty() {
        return Ok(None);
    }
    let whole_source = token
        .strip_prefix("${{")
        .and_then(|inner| inner.strip_suffix("}}"))
        // Retain the pre-v1 spelling as read compatibility. Newly emitted
        // manifests use the canonical `${{ ... }}` form.
        .or_else(|| {
            token
                .strip_prefix("{{")
                .and_then(|inner| inner.strip_suffix("}}"))
        })
        .map(str::trim);
    if references.len() != 1 || whole_source != Some(references[0]) {
        bail!(
            "argv token templates must be exactly one '${{{{ inputs.<id> }}}}' or '${{{{ steps.<prior-step>.outputs.<id> }}}}' source"
        );
    }
    match parse_runtime_source(references[0])? {
        RuntimeSource::Input(_) | RuntimeSource::StepOutput { .. } => Ok(Some(references[0])),
        RuntimeSource::RunId => {
            bail!("argv token templates may not use run.id; use an authored input or prior output")
        }
    }
}

fn validate_runtime_source(source: &str) -> Result<()> {
    parse_runtime_source(source).map(|_| ())
}

fn validate_available_source(
    source: &str,
    inputs: &BTreeMap<&str, &JourneyInput>,
    prior_step_outputs: &BTreeMap<String, (ValueType, bool)>,
) -> Result<()> {
    match parse_runtime_source(source)? {
        RuntimeSource::RunId => Ok(()),
        RuntimeSource::Input(id) if inputs.contains_key(id) => Ok(()),
        RuntimeSource::Input(id) => bail!("unknown Journey input '{id}'"),
        RuntimeSource::StepOutput { .. } if prior_step_outputs.contains_key(source) => Ok(()),
        RuntimeSource::StepOutput { .. } => {
            bail!("'{source}' is not an output of a prior Journey step")
        }
    }
}

fn validate_interpolated_source(
    source: &str,
    inputs: &BTreeMap<&str, &JourneyInput>,
    prior_outputs: &BTreeMap<String, (ValueType, bool)>,
    allow_run_id: bool,
) -> Result<()> {
    validate_available_source(source, inputs, prior_outputs)?;
    match parse_runtime_source(source)? {
        RuntimeSource::RunId if allow_run_id => Ok(()),
        RuntimeSource::RunId => bail!("run.id is not allowed in this runtime template"),
        RuntimeSource::Input(id) => {
            let input = inputs
                .get(id)
                .expect("input existence validated before interpolation policy");
            if input.secret {
                bail!("secret input '{id}' cannot enter a runtime template");
            }
            if !input.value_type.is_scalar() {
                bail!("input '{id}' is not scalar and cannot replace one argv/content token");
            }
            Ok(())
        }
        RuntimeSource::StepOutput { .. } => {
            let (value_type, redact) = prior_outputs
                .get(source)
                .expect("output availability validated before interpolation policy");
            if *redact {
                bail!("redacted output '{source}' cannot enter a runtime template");
            }
            if !value_type.is_scalar() {
                bail!("output '{source}' is not scalar and cannot enter a runtime template");
            }
            Ok(())
        }
    }
}

fn validate_temporal_action_references(
    action: &SurfaceFileAction,
    inputs: &BTreeMap<&str, &JourneyInput>,
    prior_outputs: &BTreeMap<String, (ValueType, bool)>,
) -> Result<()> {
    let Some(template) = &action.template else {
        return Ok(());
    };
    for source in template_references(template)? {
        validate_interpolated_source(source, inputs, prior_outputs, true)?;
    }
    Ok(())
}

fn validate_operation_references(
    operation: &CliOperation,
    inputs: &BTreeMap<&str, &JourneyInput>,
    prior_outputs: &BTreeMap<String, (ValueType, bool)>,
) -> Result<()> {
    for (index, token) in operation.argv.iter().enumerate() {
        if let Some(source) = argv_token_source(token)? {
            if index == 0 {
                bail!(
                    "operation '{}' executable argv token cannot be a runtime template",
                    operation.id
                );
            }
            validate_interpolated_source(source, inputs, prior_outputs, false).with_context(
                || format!("operation '{}' argv token #{}", operation.id, index + 1),
            )?;
        }
    }
    for argument in &operation.arguments {
        let default_source = format!("inputs.{}", argument.id);
        let source = argument.source.as_deref().unwrap_or(&default_source);
        validate_available_source(source, inputs, prior_outputs).with_context(|| {
            format!(
                "operation '{}' argument '{}' source",
                operation.id, argument.id
            )
        })?;
        if let RuntimeSource::Input(id) = parse_runtime_source(source)? {
            if inputs.get(id).is_some_and(|input| input.secret) {
                bail!(
                    "operation '{}' argument '{}' reads secret input '{}'; secret inputs are environment-only and must not enter CLI argv",
                    operation.id,
                    argument.id,
                    id
                );
            }
        }
    }
    for assertion in &operation.output.assertions {
        if let Some(source) = assertion.runtime_source() {
            validate_available_source(source, inputs, prior_outputs).with_context(|| {
                format!(
                    "operation '{}' assertion '{}' source",
                    operation.id, assertion.id
                )
            })?;
        }
    }
    Ok(())
}

fn validate_json_pointer(label: &str, id: &str, pointer: &str) -> Result<()> {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        bail!("{label} '{id}' JSON pointer must be empty or start with '/'");
    }
    // RFC 6901 allows only ~0 and ~1 escapes. Rejecting malformed escapes here
    // keeps compile/run parity deterministic across serde_json versions.
    let bytes = pointer.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'~' {
            if index + 1 >= bytes.len() || !matches!(bytes[index + 1], b'0' | b'1') {
                bail!("{label} '{id}' has malformed JSON pointer escape");
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> JourneySpec {
        serde_json::from_value(json!({
            "schema": JOURNEY_SCHEMA,
            "id": "checkout.happy",
            "name": "Checkout succeeds",
            "actor": "shopper",
            "goal": "Pay for an order",
            "inputs": {
                "sku": {"type":"string", "description":"Item SKU"},
                "quantity": {"type":"integer", "description":"Quantity", "default":1}
            },
            "preconditions": [],
            "steps": [
                {
                    "id":"add-item",
                    "name":"Add item",
                    "action":"adds an item",
                    "expects":[],
                    "produces":{}
                },
                {
                    "id":"pay",
                    "name":"Pay",
                    "action":"pays for the order",
                    "expects":["the order is paid"],
                    "produces":{"order-id":{"type":"string","description":"Created order id"}}
                }
            ],
            "profiles": {
                "proof": {
                    "inputs":{"sku":{"template":"sku-1"}},
                    "workspace":{"files":[{"path":"fixtures/cart.json","content":"{}"}]}
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn canonical_hash_ignores_display_name_but_tracks_step_order() {
        let a = example();
        let mut b = a.clone();
        b.name = "A renamed checkout".into();
        assert_eq!(a.semantic_hash().unwrap(), b.semantic_hash().unwrap());
        b.steps.reverse();
        assert_ne!(a.semantic_hash().unwrap(), b.semantic_hash().unwrap());
    }

    #[test]
    fn setup_is_confined() {
        let mut spec = example();
        spec.profiles.get_mut("proof").unwrap().workspace.files[0].path = "../escape".into();
        assert!(spec.validate().unwrap_err().to_string().contains("escapes"));
    }

    #[test]
    fn surface_file_actions_reject_dollar_template_syntax_without_changing_valid_semantics() {
        let action = |content: Option<&str>, template: Option<&str>| SurfaceFileAction {
            path: "src/example.rs".into(),
            expected_hash: "0123456789abcdef".into(),
            content: content.map(str::to_owned),
            template: template.map(str::to_owned),
        };

        let content_error = action(Some("literal ${{ inputs.topic }}"), None)
            .validate()
            .unwrap_err();
        assert!(content_error
            .to_string()
            .contains("content must not contain"));

        let template_error = action(None, Some("${{ inputs.topic }}"))
            .validate()
            .unwrap_err();
        assert!(template_error
            .to_string()
            .contains("template must not contain"));

        action(Some("literal {{ inputs.topic }}"), None)
            .validate()
            .unwrap();
        action(None, Some("runtime {{ inputs.topic }}"))
            .validate()
            .unwrap();
    }

    #[test]
    fn journey_lint_static_predicates_are_narrow() {
        assert!(is_exact_graph_identity("0123456789abcdef0123456789ABCDEF"));
        assert!(!is_exact_graph_identity(
            "id=0123456789abcdef0123456789abcdef"
        ));

        let assertion =
            |pointer: &str, equals: Option<Value>, source: Option<String>| OutputAssertion {
                id: "check".into(),
                pointer: pointer.into(),
                value_type: None,
                equals,
                source,
            };
        assert!(exact_census_pin(&assertion(
            "/request_count",
            Some(json!(16)),
            None
        )));
        for pointer in [
            "/entry_count",
            "/file_count",
            "/tombstone_count",
            "/operation_count",
            "/byte_total",
        ] {
            assert!(exact_census_pin(&assertion(pointer, Some(json!(1)), None)));
        }
        assert!(!exact_census_pin(&assertion(
            "/exit_code",
            Some(json!(0)),
            None
        )));
        assert!(positional_census_pointer("/findings/0/kind"));
        assert!(!positional_census_pointer("/finding/by-id/kind"));
        assert_eq!(
            assertion("/name", None, Some(format!("{ASSERTION_NOT_EQUALS}\"\"")))
                .not_equals_value(),
            Some(json!(""))
        );

        let operation = CliOperation {
            id: "audit-burst".into(),
            summary: String::new(),
            argv: vec![
                "loom".into(),
                "mcp".into(),
                "{\"adjudications\":[],\"command\":[\"loom\",\"audit\",\"--json\"]}".into(),
            ],
            environment: Vec::new(),
            read_only: false,
            timeout_seconds: None,
            arguments: Vec::new(),
            output: OperationOutput {
                format: OutputFormat::Json,
                captures: Vec::new(),
                assertions: Vec::new(),
                redact: Vec::new(),
            },
            exercises: Vec::new(),
        };
        assert!(relies_on_real_clock_minute_bucket(&operation));
        let mut merely_mentions_audit = operation;
        merely_mentions_audit.argv[2] = "audit prose without adjudications".into();
        assert!(!relies_on_real_clock_minute_bucket(&merely_mentions_audit));
    }

    #[test]
    fn journey_lint_reports_every_source_and_stable_contract() {
        let root =
            std::env::temp_dir().join(format!("loom-journey-lint-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("current.rs"), "current\n").unwrap();
        std::fs::write(root.join("stale.rs"), "changed\n").unwrap();
        let store = crate::store::Store::init(&root, Some("lint-unit"), false).unwrap();
        for path in ["current.rs", "stale.rs"] {
            store
                .add_node(
                    crate::model::NodeType::CodeFile,
                    path,
                    "lint fixture",
                    "active",
                    json!({}),
                )
                .unwrap();
        }
        let current_hash = crate::artifact::fingerprint("current\n");
        let graph_id = "0123456789abcdef0123456789abcdef";
        let manifest: SurfaceManifest = serde_json::from_value(json!({
            "schema": SURFACE_SCHEMA,
            "journey_id": "lint.fixture",
            "journey_hash": "hash",
            "surface": {
                "id": "lint-cli", "title": "Lint CLI", "identity": "lint",
                "codefile": "src/main.rs", "locator": "main",
                "operations": [{
                    "id": "inspect", "summary": "inspect",
                    "argv": ["tool", graph_id], "arguments": [],
                    "output": {"format": "json", "assertions": [
                        {"id":"equals-id", "pointer":"/id", "equals":{"nested":graph_id}},
                        {"id":"pointer-id", "pointer":format!("/nodes/{graph_id}/name"), "equals":"ok"},
                        {"id":"count", "pointer":"/entry_count", "equals":2},
                        {"id":"position", "pointer":"/entries/0/name", "equals":"first"},
                        {"id":"non-empty", "pointer":"/name", "not_equals":""},
                        {"id":"exit", "pointer":"/exit_code", "equals":0}
                    ]}
                }]
            },
            "setup": {
                "graph":"local_snapshot", "operations":[],
                "before_steps": {"step": [
                    {"path":"current.rs", "expected_hash":current_hash, "content":"next"},
                    {"path":"stale.rs", "expected_hash":"0000000000000000", "content":"next"}
                ]}
            },
            "bindings": []
        })).unwrap();
        let journey = JourneySpec {
            schema: JOURNEY_SCHEMA.into(),
            id: "lint.fixture".into(),
            name: "Lint fixture".into(),
            actor: "tester".into(),
            goal: "Exercise lint".into(),
            description: None,
            inputs: BTreeMap::new(),
            preconditions: Vec::new(),
            steps: vec![JourneyStep {
                id: "step".into(),
                name: "Step".into(),
                action: "Inspect".into(),
                expects: Vec::new(),
                produces: BTreeMap::new(),
            }],
            profiles: BTreeMap::new(),
        };
        let report = manifest
            .lint(&store, &journey, "surfaces/lint.fixture.surface.json")
            .unwrap();
        let rules: Vec<_> = report
            .findings
            .iter()
            .map(|finding| finding.rule.as_str())
            .collect();
        assert_eq!(
            rules
                .iter()
                .filter(|rule| **rule == "graph-local-identity")
                .count(),
            3
        );
        for rule in [
            "exact-census-pin",
            "positional-census-pointer",
            "not-equals-empty",
        ] {
            assert!(rules.contains(&rule), "missing {rule}: {rules:?}");
        }
        assert_eq!(
            rules
                .iter()
                .filter(|rule| **rule == "stale-temporal-expected-hash")
                .count(),
            1
        );
        assert!(report.findings.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            json!({
                "schema":"loom.journey-lint/v1", "status":"blocked", "scanned":1,
                "blocking":4, "advisory":3, "findings": report.findings
            })
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn self_audit_real_clock_rule_matches_only_authorized_batch() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let path = root.join("journeys/surfaces/self-audit.surface.json");
        let manifest = SurfaceManifest::parse_json(&path).unwrap();
        for operation in &manifest.surface.operations {
            let expected = operation.id == "audit-authorized-batch";
            if matches!(
                operation.id.as_str(),
                "audit-authorized-batch" | "audit-clean-graph" | "audit-defective-graph"
            ) {
                assert_eq!(
                    relies_on_real_clock_minute_bucket(operation),
                    expected,
                    "{}",
                    operation.id
                );
            }
        }
    }
}
