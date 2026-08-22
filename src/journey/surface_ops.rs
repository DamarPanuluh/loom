use super::sources::{argv_token_source, validate_json_pointer, validate_runtime_source};
use super::spec::{
    canonicalize_value, insert_unique, nonempty, validate_process_environment_name,
    validate_stable_id, ValueType,
};
use super::INTERFACE_SURFACE_SCHEMA;
use crate::Result;
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;

fn default_true() -> bool {
    true
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
    /// Process liveness contract: the exact exit code that counts as the
    /// operation having run. Omitted or `0` keeps the default rule (the child
    /// must exit 0). A non-zero value lets a structured-failure CLI — one
    /// that writes its single JSON envelope to stdout and then exits non-zero
    /// to signal the rejection — prove that failure as an observed result.
    /// Content checks such as `/ok`, `/error.kind`, and `/error.code` remain
    /// JSON assertions in `output.assertions`, never exit codes. Negative
    /// values are rejected at parse time, and a killed or signaled process
    /// never satisfies this contract.
    #[serde(default, skip_serializing_if = "exit_code_is_zero")]
    pub expected_exit: u32,
    #[serde(default)]
    pub arguments: Vec<OperationArgument>,
    pub output: OperationOutput,
    /// Optional downstream proof entries reached through this public operation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exercises: Vec<OperationExercise>,
}

fn exit_code_is_zero(code: &u32) -> bool {
    *code == 0
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
    /// The expected exit code lives on the operation itself
    /// ([`CliOperation::expected_exit`]); content checks such as `/ok` and
    /// `/error.kind` stay JSON assertions.
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

pub(super) const ASSERTION_NOT_EQUALS: &str = "$loom.assertion/not_equals/";
const ASSERTION_EXISTS: &str = "$loom.assertion/exists/";
const ASSERTION_CONTAINS: &str = "$loom.assertion/contains/";
const ASSERTION_MATCHES: &str = "$loom.assertion/matches/";
/// Lower bound for numeric pointers. Exact `equals` on a graph-shape metric
/// (a spare/stale count) pins incidental state and breaks on unrelated graph
/// growth; `minimum` expresses the behavior — "at least the sentinel survived"
/// — and stays true as the graph grows.
const ASSERTION_MINIMUM: &str = "$loom.assertion/minimum/";

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
    minimum: Option<Value>,
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
            wire.minimum.is_some(),
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
        } else if let Some(value) = wire.minimum {
            Some(format!(
                "{ASSERTION_MINIMUM}{}",
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
        } else if let Some(value) = self.minimum_value() {
            object.insert("minimum".into(), value);
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
                && !source.starts_with(ASSERTION_MINIMUM)
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

    pub(crate) fn minimum_value(&self) -> Option<Value> {
        decode_assertion_value(self.source.as_deref()?, ASSERTION_MINIMUM)
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
                    assertion.minimum_value().is_some(),
                    assertion.runtime_source().is_some(),
                ]
                .into_iter()
                .filter(|present| *present)
                .count();
                if comparisons > 1 {
                    bail!(
                        "assertion '{}' must declare at most one of equals, not_equals, contains, matches, minimum, or source",
                        assertion.id
                    );
                }
                if let Some(value) = assertion.minimum_value() {
                    if !value.is_number() {
                        bail!(
                            "assertion '{}' minimum operand must be a number",
                            assertion.id
                        );
                    }
                    if assertion.value_type.is_some_and(|value_type| {
                        !matches!(value_type, ValueType::Integer | ValueType::Number)
                    }) {
                        bail!(
                            "assertion '{}' minimum operator requires type integer, number, or no explicit type",
                            assertion.id
                        );
                    }
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
