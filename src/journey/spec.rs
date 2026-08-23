use crate::Result;
use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

pub const JOURNEY_SCHEMA: &str = "loom.journey/v1";

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

pub(super) fn nonempty(label: &str, value: &str) -> Result<()> {
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

pub(super) fn insert_unique(ids: &mut BTreeSet<String>, label: &str, id: &str) -> Result<()> {
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

/// Canonical JSON key ordering. The rule lives in `crate::canonical` — it used
/// to exist five times under four names, each feeding a hash another module
/// compared against.
pub(super) use crate::canonical::canonicalize as canonicalize_value;

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
}
