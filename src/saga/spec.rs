//! Saga spec: the YAML format, its validation, and `{{ }}` interpolation.
//!
//! The graph binding is FIRST-CLASS: every step names the intent it proves
//! (`intent:` — id, exact name, or unique fragment), so the runner can
//! translate per-step results into per-edge verdicts without a sidecar
//! mapping. The spec file itself is registered as a CodeFile at `loom saga
//! add`, so it travels in the export and shows up in coverage.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::Deserialize;

/// Default per-request timeout. A consumer that waits forever proves nothing.
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// Format
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SagaSpec {
    /// Saga name — becomes the Validation node's name (addressable in
    /// `loom saga run <name>` / `loom validation mark`).
    pub saga: String,

    #[serde(default)]
    pub description: String,

    /// Base URL joined with relative step urls. Supports `{{ env.X }}` /
    /// `{{ var }}` so the live target stays out of the committed spec.
    #[serde(default)]
    pub base: String,

    /// Initial variables (captures add to these as steps execute).
    #[serde(default)]
    pub vars: BTreeMap<String, String>,

    /// Per-request timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    pub steps: Vec<Step>,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

const SPEC_SIZE_CAP: usize = 512 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub name: String,

    /// The intent this step proves — id, exact name, or unique fragment.
    pub intent: String,

    pub request: Request,

    #[serde(default)]
    pub expect: Expect,

    /// var name → JSONPath into the response body. Captured values are
    /// available to every later step as `{{ var }}`.
    #[serde(default)]
    pub capture: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// GET | POST | PUT | PATCH | DELETE | HEAD | OPTIONS
    pub method: String,

    /// Absolute, or relative to `base`.
    pub url: String,

    #[serde(default)]
    pub headers: BTreeMap<String, String>,

    /// JSON body (string leaves are interpolated). Mutually exclusive with `body`.
    #[serde(default)]
    pub json: Option<serde_json::Value>,

    /// Raw string body. Mutually exclusive with `json`.
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// Exact status code. Omitted = any 2xx.
    #[serde(default)]
    pub status: Option<u16>,

    /// header name → expected substring of its value.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,

    /// JSONPath → expectation on the (JSON) response body.
    #[serde(default)]
    pub body: BTreeMap<String, BodyExpectation>,
}

/// `{ exists: true }` / `{ contains: "…" }` / bare value = equals.
/// Untagged: the struct shapes are tried before the catch-all Equals, so a
/// bare mapping value still means "equals this object".
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum BodyExpectation {
    Exists { exists: bool },
    Contains { contains: String },
    Equals(serde_json::Value),
}

// ---------------------------------------------------------------------------
// Load + validate
// ---------------------------------------------------------------------------

const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// Parse a saga spec from YAML and reject structural nonsense up front, so
/// both `saga add` and `saga run` fail fast with a line-level reason instead
/// of mid-run.
pub fn load_spec(yaml: &str, origin: &str) -> Result<SagaSpec> {
    if yaml.len() > SPEC_SIZE_CAP {
        anyhow::bail!(
            "{origin}: saga spec is {} bytes, above the 512 KiB limit. Split the proof or remove generated payloads before running `loom saga add`.",
            yaml.len()
        );
    }
    let spec: SagaSpec =
        serde_yaml::from_str(yaml).with_context(|| format!("Invalid saga spec: {origin}"))?;
    if spec.saga.trim().is_empty() {
        anyhow::bail!("{origin}: `saga:` (the name) must not be empty.");
    }
    if spec.steps.is_empty() {
        anyhow::bail!("{origin}: a saga needs at least one step.");
    }
    if spec.timeout_secs == 0 {
        anyhow::bail!("{origin}: `timeout_secs:` must be at least 1 second.");
    }
    for (i, step) in spec.steps.iter().enumerate() {
        let at = format!("{origin}: step {} ('{}')", i + 1, step.name);
        if step.name.trim().is_empty() {
            anyhow::bail!("{origin}: step {} has an empty name.", i + 1);
        }
        if step.intent.trim().is_empty() {
            anyhow::bail!("{at}: `intent:` is required — every step names the intent it proves.");
        }
        let m = step.request.method.to_uppercase();
        if !METHODS.contains(&m.as_str()) {
            anyhow::bail!(
                "{at}: unknown method '{}'. Valid: {}.",
                step.request.method,
                METHODS.join(", ")
            );
        }
        if step.request.json.is_some() && step.request.body.is_some() {
            anyhow::bail!("{at}: `json` and `body` are mutually exclusive.");
        }
        if step.request.url.trim().is_empty() {
            anyhow::bail!("{at}: `url:` must not be empty.");
        }
        for path in step.expect.body.keys() {
            serde_json_path::JsonPath::parse(path)
                .with_context(|| format!("{at}: invalid JSONPath in expect.body: '{path}'"))?;
        }
        for (var, path) in &step.capture {
            serde_json_path::JsonPath::parse(path)
                .with_context(|| format!("{at}: invalid JSONPath for capture '{var}': '{path}'"))?;
        }
    }
    Ok(spec)
}

/// Read + parse a spec file.
pub fn load_spec_file(path: &std::path::Path) -> Result<SagaSpec> {
    let size = std::fs::metadata(path)
        .with_context(|| format!("Cannot stat saga spec '{}'", path.display()))?
        .len();
    if size > SPEC_SIZE_CAP as u64 {
        anyhow::bail!(
            "Saga spec '{}' is {size} bytes, above the 512 KiB limit. Split the proof or remove generated payloads before running `loom saga add`.",
            path.display()
        );
    }
    let yaml = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read saga spec '{}'", path.display()))?;
    load_spec(&yaml, &path.display().to_string())
}

// ---------------------------------------------------------------------------
// Interpolation: {{ var }} and {{ env.NAME }}
// ---------------------------------------------------------------------------

/// Substitute `{{ var }}` from `vars` and `{{ env.NAME }}` from the process
/// environment. Unknown names are hard errors — a silently-empty URL segment
/// would turn a broken spec into a confusing HTTP 404.
pub fn interpolate(template: &str, vars: &BTreeMap<String, String>) -> Result<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            anyhow::bail!("Unclosed '{{{{' in template: '{template}'");
        };
        let name = after[..end].trim();
        if let Some(env_name) = name.strip_prefix("env.") {
            let v = std::env::var(env_name).map_err(|_| {
                anyhow::anyhow!(
                    "Template references '{{{{ env.{env_name} }}}}' but ${env_name} is not set."
                )
            })?;
            out.push_str(&v);
        } else {
            let v = vars.get(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "Template references '{{{{ {name} }}}}' but no such variable. \
                     Available: {avail}. Variables come from `vars:` and earlier steps' `capture:`.",
                    avail = if vars.is_empty() { "(none)".to_string() } else {
                        vars.keys().cloned().collect::<Vec<_>>().join(", ")
                    },
                )
            })?;
            out.push_str(v);
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Every `{{ env.NAME }}` reference in one template string.
fn env_refs_in(template: &str, out: &mut Vec<String>) {
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { return };
        if let Some(name) = after[..end].trim().strip_prefix("env.") {
            if !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
        }
        rest = &after[end + 2..];
    }
}

fn env_refs_in_json(value: &serde_json::Value, out: &mut Vec<String>) {
    use serde_json::Value;
    match value {
        Value::String(s) => env_refs_in(s, out),
        Value::Array(items) => items.iter().for_each(|v| env_refs_in_json(v, out)),
        Value::Object(map) => map.values().for_each(|v| env_refs_in_json(v, out)),
        _ => {}
    }
}

/// Every environment variable a spec references (`{{ env.NAME }}` anywhere a
/// template is interpolated: base, urls, headers, bodies). This is the saga's
/// declared dependency on the OUTSIDE world — values are passed at invocation
/// (`BASE_URL=… loom saga run …`), never stored in the graph.
pub fn required_env(spec: &SagaSpec) -> Vec<String> {
    let mut out = Vec::new();
    env_refs_in(&spec.base, &mut out);
    for step in &spec.steps {
        env_refs_in(&step.request.url, &mut out);
        for v in step.request.headers.values() {
            env_refs_in(v, &mut out);
        }
        if let Some(json) = &step.request.json {
            env_refs_in_json(json, &mut out);
        }
        if let Some(body) = &step.request.body {
            env_refs_in(body, &mut out);
        }
    }
    out
}

/// The subset of `required_env` not set in this process's environment.
pub fn missing_env(spec: &SagaSpec) -> Vec<String> {
    required_env(spec)
        .into_iter()
        .filter(|name| std::env::var(name).is_err())
        .collect()
}

/// Interpolate every string leaf of a JSON value (for `request.json` bodies).
pub fn interpolate_json(
    value: &serde_json::Value,
    vars: &BTreeMap<String, String>,
) -> Result<serde_json::Value> {
    use serde_json::Value;
    Ok(match value {
        Value::String(s) => Value::String(interpolate(s, vars)?),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| interpolate_json(v, vars))
                .collect::<Result<_>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), interpolate_json(v, vars)?)))
                .collect::<Result<_>>()?,
        ),
        other => other.clone(),
    })
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
saga: checkout-flow
base: "{{ env.BASE_URL }}"
vars:
  user: alice
steps:
  - name: create cart
    intent: cart-creation
    request: { method: POST, url: /carts, json: { owner: "{{ user }}" } }
    expect:
      status: 201
      body:
        "$.id": { exists: true }
    capture: { cart_id: "$.id" }
  - name: pay
    intent: payment-capture
    request: { method: POST, url: "/carts/{{ cart_id }}/payment" }
    expect:
      status: 200
      body:
        "$.state": paid
"#;

    #[test]
    fn good_spec_parses() {
        let spec = load_spec(GOOD, "test").unwrap();
        assert_eq!(spec.saga, "checkout-flow");
        assert_eq!(spec.steps.len(), 2);
        assert_eq!(spec.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert!(matches!(
            spec.steps[0].expect.body["$.id"],
            BodyExpectation::Exists { exists: true }
        ));
        assert!(matches!(
            spec.steps[1].expect.body["$.state"],
            BodyExpectation::Equals(_)
        ));
    }

    #[test]
    fn rejects_missing_intent_binding() {
        let bad = GOOD.replace("    intent: cart-creation\n", "");
        let err = load_spec(&bad, "test").unwrap_err().to_string();
        assert!(err.contains("Invalid saga spec"), "got: {err}");
    }

    #[test]
    fn rejects_zero_timeout() {
        let bad = GOOD.replace(
            "saga: checkout-flow",
            "saga: checkout-flow\ntimeout_secs: 0",
        );
        let err = format!("{:#}", load_spec(&bad, "t").unwrap_err());
        assert!(
            err.contains("`timeout_secs:` must be at least 1 second"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_oversized_spec_before_yaml_parse() {
        let huge = "x".repeat(SPEC_SIZE_CAP + 1);
        let err = format!("{:#}", load_spec(&huge, "huge.yaml").unwrap_err());
        assert!(err.contains("above the 512 KiB limit"), "got: {err}");
    }

    #[test]
    fn rejects_empty_intent_and_bad_method_and_bad_jsonpath() {
        let empty_intent = GOOD.replace("intent: cart-creation", "intent: \"\"");
        let err = format!("{:#}", load_spec(&empty_intent, "t").unwrap_err());
        assert!(err.contains("`intent:` is required"), "got: {err}");

        let bad_method = GOOD.replace("method: POST, url: /carts,", "method: YEET, url: /carts,");
        let err = format!("{:#}", load_spec(&bad_method, "t").unwrap_err());
        assert!(err.contains("unknown method 'YEET'"), "got: {err}");

        let bad_path = GOOD.replace("cart_id: \"$.id\"", "cart_id: \"$[\"");
        let err = format!("{:#}", load_spec(&bad_path, "t").unwrap_err());
        assert!(err.contains("invalid JSONPath"), "got: {err}");
    }

    #[test]
    fn rejects_json_and_body_together() {
        let bad = GOOD.replace(
            "request: { method: POST, url: \"/carts/{{ cart_id }}/payment\" }",
            "request: { method: POST, url: /pay, json: {}, body: \"raw\" }",
        );
        let err = format!("{:#}", load_spec(&bad, "t").unwrap_err());
        assert!(err.contains("mutually exclusive"), "got: {err}");
    }

    #[test]
    fn interpolation_resolves_vars_and_env() {
        let mut vars = BTreeMap::new();
        vars.insert("cart_id".to_string(), "c-42".to_string());
        assert_eq!(
            interpolate("/carts/{{ cart_id }}/payment", &vars).unwrap(),
            "/carts/c-42/payment"
        );
        std::env::set_var("LOOM_SAGA_TEST_VAR", "hello");
        assert_eq!(
            interpolate("{{ env.LOOM_SAGA_TEST_VAR }}/x", &vars).unwrap(),
            "hello/x"
        );
        // Unknown variable names which steps could provide are listed.
        let err = interpolate("{{ nope }}", &vars).unwrap_err().to_string();
        assert!(
            err.contains("no such variable") && err.contains("cart_id"),
            "got: {err}"
        );
        // Unclosed braces are an error, not silent passthrough.
        assert!(interpolate("{{ oops", &vars).is_err());
    }

    #[test]
    fn required_env_finds_every_reference_once() {
        let spec = load_spec(GOOD, "test").unwrap();
        assert_eq!(required_env(&spec), vec!["BASE_URL"]);

        let multi = GOOD
            .replace(
                "request: { method: POST, url: /carts, json: { owner: \"{{ user }}\" } }",
                "request: { method: POST, url: /carts, headers: { X-Auth: \"{{ env.API_TOKEN }}\" }, json: { owner: \"{{ env.OWNER }}\", base: \"{{ env.BASE_URL }}\" } }",
            );
        let spec = load_spec(&multi, "test").unwrap();
        assert_eq!(required_env(&spec), vec!["BASE_URL", "API_TOKEN", "OWNER"]);
    }

    #[test]
    fn interpolate_json_walks_string_leaves() {
        let mut vars = BTreeMap::new();
        vars.insert("user".to_string(), "alice".to_string());
        let body = serde_json::json!({"owner": "{{ user }}", "n": 3, "tags": ["{{ user }}"]});
        let out = interpolate_json(&body, &vars).unwrap();
        assert_eq!(
            out,
            serde_json::json!({"owner": "alice", "n": 3, "tags": ["alice"]})
        );
    }
}
