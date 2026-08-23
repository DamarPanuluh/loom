use crate::journey::{
    JourneyProfile, JourneySpec, OperationArgument, OutputAssertion, RuntimeSource, TemporarySetup,
};
use crate::Result;
use anyhow::{anyhow, bail, Context};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::types::{ResolvedInputs, RuntimeReport, REDACTED};

pub(crate) fn profile_for<'a>(spec: &'a JourneySpec, id: &str) -> Result<&'a JourneyProfile> {
    spec.profiles
        .get(id)
        .ok_or_else(|| anyhow!("Journey '{}' has no profile '{id}'", spec.id))
}

pub(crate) fn resolve_inputs(
    spec: &JourneySpec,
    profile_id: &str,
    profile: &JourneyProfile,
    overrides: &BTreeMap<String, Value>,
    run_id: &str,
) -> Result<ResolvedInputs> {
    let mut values = BTreeMap::new();
    let mut secrets = Vec::new();
    let mut bound_env = BTreeMap::new();
    for (id, input) in &spec.inputs {
        if let Some(value) = &input.default {
            values.insert(id.clone(), value.clone());
        }
    }

    // Resolve environment bindings immediately, then templates as their input
    // dependencies become available. Cycles and unavailable references fail
    // closed instead of silently interpolating an empty string.
    let mut pending: BTreeSet<String> = profile.inputs.keys().cloned().collect();
    while !pending.is_empty() {
        let mut progressed = false;
        for id in pending.clone() {
            let input = spec
                .inputs
                .get(&id)
                .ok_or_else(|| anyhow!("profile binds unknown input '{id}'"))?;
            let binding = profile.inputs.get(&id).expect("pending key came from map");
            let resolved = if let Some(env) = &binding.env {
                let raw = std::env::var(env).with_context(|| {
                    format!(
                        "required environment variable '{}' for Journey input '{}' is not set",
                        env, id
                    )
                })?;
                bound_env.insert(env.clone(), raw.clone());
                if input.secret {
                    // Register the raw value before parsing so neither type
                    // errors nor later evidence can disclose it.
                    secrets.push(raw.clone());
                    Some(
                        crate::journey::parse_typed_text(&raw, input.value_type).map_err(|_| {
                            anyhow!(
                                "secret Journey input '{}' from environment '{}' has the wrong type",
                                id,
                                env
                            )
                        })?,
                    )
                } else {
                    Some(crate::journey::parse_typed_text(&raw, input.value_type)?)
                }
            } else if let Some(template) = &binding.template {
                render_profile_template(template, input.value_type, &values, run_id)?
            } else {
                None
            };
            if let Some(value) = resolved {
                if !input.value_type.accepts(&value) {
                    bail!(
                        "profile '{}' input '{}' resolved to the wrong type",
                        profile_id,
                        id
                    );
                }
                if input.secret {
                    let rendered = scalar_text(&value).unwrap_or_else(|| value.to_string());
                    secrets.push(rendered);
                }
                values.insert(id.clone(), value);
                pending.remove(&id);
                progressed = true;
            }
        }
        if !progressed {
            bail!(
                "profile '{}' has cyclic or unavailable input template references: {}",
                profile_id,
                pending.into_iter().collect::<Vec<_>>().join(", ")
            );
        }
    }
    for (id, value) in overrides {
        let input = spec
            .inputs
            .get(id)
            .ok_or_else(|| anyhow!("diagnose override names unknown input '{id}'"))?;
        if input.secret {
            bail!(
                "secret Journey input '{}' cannot be supplied as a literal diagnose override; use profiles.proof.inputs.{}.env",
                id,
                id
            );
        }
        if !input.value_type.accepts(value) {
            bail!("diagnose override '{}' has the wrong type", id);
        }
        values.insert(id.clone(), value.clone());
    }
    for (id, input) in &spec.inputs {
        if input.required && !values.contains_key(id) {
            bail!(
                "required Journey input '{}' has no profile/default value",
                id
            );
        }
    }
    Ok((values, secrets, bound_env))
}

fn render_profile_template(
    template: &str,
    value_type: crate::journey::ValueType,
    inputs: &BTreeMap<String, Value>,
    run_id: &str,
) -> Result<Option<Value>> {
    let references = crate::journey::template_references(template)?;
    let exact = references.len() == 1 && template.trim() == format!("{{{{ {} }}}}", references[0]);
    if exact {
        return match references[0] {
            "run.id" => Ok(Some(crate::journey::parse_typed_text(run_id, value_type)?)),
            reference if reference.starts_with("inputs.") => {
                let id = &reference["inputs.".len()..];
                Ok(inputs.get(id).cloned())
            }
            _ => Ok(None),
        };
    }

    let mut rendered = template.to_string();
    for reference in references {
        let replacement = match reference {
            "run.id" => run_id.to_string(),
            reference if reference.starts_with("inputs.") => {
                let id = &reference["inputs.".len()..];
                let Some(value) = inputs.get(id) else {
                    return Ok(None);
                };
                scalar_text(value).unwrap_or_else(|| value.to_string())
            }
            _ => return Ok(None),
        };
        rendered = rendered.replace(&format!("{{{{ {reference} }}}}"), &replacement);
        rendered = rendered.replace(&format!("{{{{{reference}}}}}"), &replacement);
    }
    Ok(Some(crate::journey::parse_typed_text(
        &rendered, value_type,
    )?))
}

pub(crate) fn resolve_argv(
    base_argv: &[String],
    arguments: &[OperationArgument],
    inputs: &BTreeMap<String, Value>,
    captures: &BTreeMap<String, Value>,
    run_id: &str,
    secrets: &mut Vec<String>,
) -> Result<(Vec<String>, Vec<String>)> {
    let mut argv = Vec::with_capacity(base_argv.len() + arguments.len() * 2 + 2);
    let mut display = Vec::with_capacity(argv.capacity());
    for (index, token) in base_argv.iter().enumerate() {
        let resolved = match crate::journey::argv_token_source(token)? {
            Some(source) => {
                if index == 0 {
                    bail!("executable argv token cannot be a runtime template");
                }
                let value = source_value(source, inputs, captures, run_id)
                    .ok_or_else(|| anyhow!("argv token source '{source}' is unavailable"))?;
                let rendered = runtime_scalar_text(value.as_ref())
                    .ok_or_else(|| anyhow!("argv token source '{source}' is not scalar"))?;
                if rendered.contains('\0') {
                    bail!("argv token source '{source}' resolved a NUL byte");
                }
                if secrets
                    .iter()
                    .any(|secret| !secret.is_empty() && secret == &rendered)
                {
                    bail!("argv token source '{source}' resolved protected secret material");
                }
                rendered
            }
            None => token.clone(),
        };
        argv.push(resolved.clone());
        display.push(resolved);
    }
    for argument in arguments {
        let default_source = format!("inputs.{}", argument.id);
        let value = source_value(
            argument.source.as_deref().unwrap_or(&default_source),
            inputs,
            captures,
            run_id,
        );
        let Some(value) = value else {
            if argument.required {
                bail!("required argument '{}' has no value", argument.id);
            }
            continue;
        };
        if !argument.value_type.accepts(value.as_ref()) {
            bail!("argument '{}' source has the wrong type", argument.id);
        }
        let rendered = scalar_text(value.as_ref()).unwrap_or_else(|| value.to_string());
        if let Some(flag) = &argument.flag {
            argv.push(flag.clone());
            display.push(flag.clone());
        }
        argv.push(rendered.clone());
        if argument.redact {
            secrets.push(rendered);
            display.push(REDACTED.into());
        } else {
            display.push(rendered);
        }
    }
    Ok((argv, display))
}

pub(crate) fn runtime_scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

pub(crate) fn runtime_run_id(journey_id: &str, profile: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{journey_id}.{profile}.{}.{nanos}", std::process::id())
}

pub(crate) fn source_value<'a>(
    source: &str,
    inputs: &'a BTreeMap<String, Value>,
    captures: &'a BTreeMap<String, Value>,
    run_id: &'a str,
) -> Option<std::borrow::Cow<'a, Value>> {
    match crate::journey::parse_runtime_source(source).ok()? {
        RuntimeSource::Input(id) => inputs.get(id).map(std::borrow::Cow::Borrowed),
        RuntimeSource::StepOutput { .. } => captures.get(source).map(std::borrow::Cow::Borrowed),
        RuntimeSource::RunId => Some(std::borrow::Cow::Owned(Value::String(run_id.to_string()))),
    }
}

pub(crate) fn assertion_holds(
    assertion: &OutputAssertion,
    output: &Value,
    inputs: &BTreeMap<String, Value>,
    captures: &BTreeMap<String, Value>,
    run_id: &str,
) -> bool {
    use crate::journey::Resolved;
    // Ambiguity is not absence: a selector matching two elements fails an
    // `exists: true` and an `exists: false` alike, because it has identified
    // nothing either way.
    if let Some(expected) = assertion.exists_value() {
        return match crate::journey::resolve_pointer(output, &assertion.pointer) {
            Resolved::Unique(_) => expected,
            Resolved::Missing => !expected,
            Resolved::Ambiguous(_) => false,
        };
    }
    let Resolved::Unique(actual) = crate::journey::resolve_pointer(output, &assertion.pointer)
    else {
        return false;
    };
    if assertion
        .value_type
        .is_some_and(|value_type| !value_type.accepts(actual))
    {
        return false;
    }
    if let Some(expected) = &assertion.equals {
        if actual != expected {
            return false;
        }
    }
    if assertion
        .not_equals_value()
        .as_ref()
        .is_some_and(|unexpected| actual == unexpected)
    {
        return false;
    }
    if assertion
        .contains_value()
        .as_ref()
        .is_some_and(|expected| !value_contains(actual, expected))
    {
        return false;
    }
    if let Some(pattern) = assertion.matches_pattern() {
        let Some(actual) = actual.as_str() else {
            return false;
        };
        if !regex::Regex::new(&pattern).is_ok_and(|regex| regex.is_match(actual)) {
            return false;
        }
    }
    if let Some(expected) = assertion.minimum_value() {
        // Numeric lower bound: a non-numeric actual fails closed, exactly as a
        // non-string actual fails `matches`.
        let (Some(actual), Some(expected)) = (actual.as_f64(), expected.as_f64()) else {
            return false;
        };
        if actual < expected {
            return false;
        }
    }
    if let Some(source) = assertion.runtime_source() {
        if source_value(source, inputs, captures, run_id).as_deref() != Some(actual) {
            return false;
        }
    }
    true
}

fn value_contains(actual: &Value, expected: &Value) -> bool {
    match actual {
        Value::String(actual) => expected
            .as_str()
            .is_some_and(|expected| actual.contains(expected)),
        Value::Array(actual) => actual.iter().any(|value| value == expected),
        Value::Object(actual) => expected.as_object().is_some_and(|expected| {
            expected
                .iter()
                .all(|(key, value)| actual.get(key) == Some(value))
        }),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

pub(crate) fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null => Some("null".into()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

pub(crate) fn redact_capture_map(
    mut captures: BTreeMap<String, Value>,
    redacted: &BTreeSet<String>,
    secrets: &[String],
) -> BTreeMap<String, Value> {
    for id in redacted {
        if let Some(value) = captures.get_mut(id) {
            *value = Value::String(REDACTED.into());
        }
    }
    for value in captures.values_mut() {
        redact_json_secrets(value, secrets);
    }
    captures
}

pub(crate) fn redact_json_secrets(value: &mut Value, secrets: &[String]) {
    match value {
        Value::String(text) => *text = redact_text(text, secrets),
        Value::Array(values) => {
            for value in values {
                redact_json_secrets(value, secrets);
            }
        }
        Value::Object(values) => {
            let original = std::mem::take(values);
            let mut preserved = serde_json::Map::new();
            let mut renamed = Vec::new();
            for (key, mut value) in original {
                redact_json_secrets(&mut value, secrets);
                let redacted_key = redact_text(&key, secrets);
                if redacted_key == key {
                    preserved.insert(key, value);
                } else {
                    renamed.push((key, redacted_key, value));
                }
            }
            // Preserve every unrelated key first, then deterministically
            // allocate collision-safe names for redacted keys. The original
            // secret-bearing key orders equal redactions without being kept.
            renamed.sort_by(|left, right| left.0.cmp(&right.0));
            for (_, base, value) in renamed {
                let mut candidate = base.clone();
                let mut suffix = 2usize;
                while preserved.contains_key(&candidate) {
                    candidate = format!("{base}#{suffix}");
                    suffix += 1;
                }
                preserved.insert(candidate, value);
            }
            *values = preserved;
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

pub(crate) fn redact_pointer(value: &mut Value, pointer: &str) {
    if pointer.is_empty() {
        *value = Value::String(REDACTED.into());
        return;
    }
    let mut segments: Vec<String> = pointer
        .split('/')
        .skip(1)
        .map(|segment| segment.replace("~1", "/").replace("~0", "~"))
        .collect();
    let Some(last) = segments.pop() else {
        return;
    };
    let mut current = value;
    for segment in segments {
        current = match current {
            Value::Object(map) => match map.get_mut(&segment) {
                Some(value) => value,
                None => return,
            },
            Value::Array(values) => match segment
                .parse::<usize>()
                .ok()
                .and_then(|index| values.get_mut(index))
            {
                Some(value) => value,
                None => return,
            },
            _ => return,
        };
    }
    match current {
        Value::Object(map) => {
            if let Some(value) = map.get_mut(&last) {
                *value = Value::String(REDACTED.into());
            }
        }
        Value::Array(values) => {
            if let Some(value) = last
                .parse::<usize>()
                .ok()
                .and_then(|index| values.get_mut(index))
            {
                *value = Value::String(REDACTED.into());
            }
        }
        _ => {}
    }
}

pub(crate) fn redact_text(text: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(text.to_string(), |text, secret| {
            text.replace(secret, REDACTED)
        })
}

pub(crate) fn materialize_setup(root: &Path, setup: &TemporarySetup) -> Result<()> {
    for directory in &setup.directories {
        std::fs::create_dir_all(root.join(directory))
            .with_context(|| format!("creating temporary setup directory '{directory}'"))?;
    }
    for file in &setup.files {
        let path = root.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &file.content)
            .with_context(|| format!("writing temporary setup file '{}'", file.path))?;
    }
    Ok(())
}

/// Canonical JSON key ordering. The rule lives in `crate::canonical` — it used
/// to exist five times under four names, each feeding a hash another module
/// compared against.
pub(crate) use crate::canonical::canonicalize;

pub fn report_observation_json(report: &RuntimeReport) -> Result<Vec<u8>> {
    // This is structured evidence from checks Loom actually performed. The
    // proof-strength layer should consume RunRecord.assertions directly; it
    // must not depend on a synthetic test-runner sentence hidden in stdout.
    Ok(serde_json::to_vec(&json!({
        "journey": report.journey_id,
        "profile": report.profile,
        "status": report.status,
        "assertions_passed": report.assertions_passed,
        "assertions_failed": report.assertions_failed,
        "passed_assertions": report.passed_assertions,
        "failed_assertions": report.failed_assertions,
    }))?)
}

pub fn parse_overrides(raw: &[String]) -> Result<BTreeMap<String, Value>> {
    let mut overrides = BTreeMap::new();
    for item in raw {
        let (key, encoded) = item
            .split_once('=')
            .ok_or_else(|| anyhow!("--input '{item}' must be KEY=JSON"))?;
        crate::journey::validate_stable_id("input", key)?;
        if overrides
            .insert(
                key.to_string(),
                serde_json::from_str(encoded)
                    .with_context(|| format!("parsing --input '{key}' as JSON"))?,
            )
            .is_some()
        {
            bail!("--input '{key}' was supplied more than once");
        }
    }
    Ok(overrides)
}
