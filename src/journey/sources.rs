use super::spec::{template_references, validate_stable_id, JourneyInput, ValueType};
use super::surface_ops::CliOperation;
use super::surface_setup::SurfaceFileAction;
use crate::Result;
use anyhow::{bail, Context};
use serde_json::Value;
use std::collections::BTreeMap;

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

pub(super) fn validate_runtime_source(source: &str) -> Result<()> {
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

pub(super) fn validate_temporal_action_references(
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

pub(super) fn validate_operation_references(
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

/// Parse a selector segment, or `None` if this is an ordinary segment.
pub(crate) fn parse_selector(segment: &str) -> Option<(&str, &str)> {
    let inner = segment.strip_prefix('[')?.strip_suffix(']')?;
    let (key, value) = inner.split_once('=')?;
    Some((key, value))
}

/// What a pointer addressed in one document.
///
/// Ambiguity is its own answer rather than a silent first-match: a selector
/// that names two elements has not identified anything, and letting it read as
/// "found" would settle a proof on whichever element happened to be first.
#[derive(Debug, PartialEq)]
pub(crate) enum Resolved<'a> {
    Unique(&'a Value),
    Missing,
    Ambiguous(usize),
}

/// Resolve a pointer that may carry `[key=value]` selector segments.
///
/// A pointer with no selector is delegated verbatim to `serde_json`, so every
/// proof authored before selectors existed resolves through exactly the code it
/// always did.
pub(crate) fn resolve_pointer<'a>(document: &'a Value, pointer: &str) -> Resolved<'a> {
    if !pointer.contains('[') {
        return match document.pointer(pointer) {
            Some(value) => Resolved::Unique(value),
            None => Resolved::Missing,
        };
    }
    let mut current = document;
    for segment in pointer.split('/').skip(1) {
        if let Some((key, value)) = parse_selector(segment) {
            let Some(array) = current.as_array() else {
                return Resolved::Missing;
            };
            let mut hits = array.iter().filter(|element| {
                element
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|found| found == value)
            });
            let Some(first) = hits.next() else {
                return Resolved::Missing;
            };
            let extra = hits.count();
            if extra > 0 {
                return Resolved::Ambiguous(extra + 1);
            }
            current = first;
            continue;
        }
        let unescaped = segment.replace("~1", "/").replace("~0", "~");
        let next = match current {
            Value::Object(map) => map.get(&unescaped),
            Value::Array(items) => unescaped.parse::<usize>().ok().and_then(|i| items.get(i)),
            _ => None,
        };
        match next {
            Some(value) => current = value,
            None => return Resolved::Missing,
        }
    }
    Resolved::Unique(current)
}

/// Selector values are matched literally, so a value carrying pointer syntax is
/// ambiguous. Loom refuses ambiguity rather than inventing a quoting grammar.
fn validate_selector(label: &str, id: &str, segment: &str) -> Result<()> {
    let Some((key, value)) = parse_selector(segment) else {
        bail!("{label} '{id}' selector segment '{segment}' must read [key=value]");
    };
    if key.is_empty() {
        bail!("{label} '{id}' selector segment '{segment}' names no key");
    }
    for (what, text) in [("key", key), ("value", value)] {
        if text.contains([']', '[', '=', '/', '~']) {
            bail!(
                "{label} '{id}' selector {what} '{text}' must not contain ']', '[', '=', '/' or '~' — \
                 select on a field whose value is free of pointer syntax"
            );
        }
    }
    Ok(())
}

pub(super) fn validate_json_pointer(label: &str, id: &str, pointer: &str) -> Result<()> {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        bail!("{label} '{id}' JSON pointer must be empty or start with '/'");
    }
    for segment in pointer.split('/').skip(1) {
        if segment.starts_with('[') || segment.ends_with(']') {
            validate_selector(label, id, segment)?;
        }
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

    #[test]
    fn identity_selectors_resolve_uniquely_or_refuse() {
        use serde_json::json;
        let doc = json!({
            "tools": [
                {"name": "loom_status", "arity": 0},
                {"name": "loom_context", "arity": 1},
                {"name": "loom_next", "arity": 2}
            ],
            "twins": [{"name": "same"}, {"name": "same"}],
            "scalar": 3
        });

        // Names the element, and keeps naming it wherever the list moves.
        assert_eq!(
            resolve_pointer(&doc, "/tools/[name=loom_context]/arity"),
            Resolved::Unique(&json!(1))
        );
        let mut reordered = doc.clone();
        reordered["tools"].as_array_mut().unwrap().reverse();
        assert_eq!(
            resolve_pointer(&reordered, "/tools/[name=loom_context]/arity"),
            Resolved::Unique(&json!(1))
        );
        // The coordinate does not survive the same reorder.
        assert_eq!(
            resolve_pointer(&doc, "/tools/1/name"),
            Resolved::Unique(&json!("loom_context"))
        );
        assert_eq!(
            resolve_pointer(&reordered, "/tools/1/name"),
            Resolved::Unique(&json!("loom_context"))
        );
        assert_eq!(
            resolve_pointer(&reordered, "/tools/0/name"),
            Resolved::Unique(&json!("loom_next"))
        );

        // Fail closed: no match, two matches, and a selector on a non-array.
        assert_eq!(
            resolve_pointer(&doc, "/tools/[name=absent]/arity"),
            Resolved::Missing
        );
        assert_eq!(
            resolve_pointer(&doc, "/twins/[name=same]"),
            Resolved::Ambiguous(2)
        );
        assert_eq!(resolve_pointer(&doc, "/scalar/[name=x]"), Resolved::Missing);

        // A selector-free pointer is delegated verbatim, escapes included.
        let escaped = json!({"a/b": {"c~d": 7}});
        assert_eq!(
            resolve_pointer(&escaped, "/a~1b/c~0d"),
            Resolved::Unique(&json!(7))
        );

        // Validation refuses values carrying pointer syntax rather than guessing.
        assert!(
            validate_json_pointer("assertion", "x", "/tools/[name=loom_context]/arity").is_ok()
        );
        assert!(validate_json_pointer("assertion", "x", "/tools/[name=a/b]").is_err());
        assert!(validate_json_pointer("assertion", "x", "/tools/[name]").is_err());
        assert!(validate_json_pointer("assertion", "x", "/tools/[=v]").is_err());
    }
}
