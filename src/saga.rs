//! Saga runner — the consumer-plane proof executor (ring 6).
//!
//! Plane: execution + graph write-back. A saga spec is an ordered chain of HTTP
//! requests, each naming the intent it proves, with values captured from one
//! response threaded into later requests. `run` executes the journey and stamps
//! `validates` edges: consecutive passing steps pass; the boundary into a
//! failing step fails with the exact broken expectation; later steps untouched.
//! `diagnose` executes without writing the graph.
//!
//! JSONPath is a dotted-subset (`$.a.b`) — enough for capture/threading without
//! a full RFC 9535 engine.

use crate::model::{EdgeKind, InspectionStatus, NodeType};
use crate::store::Store;
use crate::Result;
use anyhow::{anyhow, Context};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecKind {
    SagaJson,
    HttpContractJson,
}

impl SpecKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SpecKind::SagaJson => "saga_json",
            SpecKind::HttpContractJson => "http_contract_json",
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SagaSpec {
    pub saga: String,
    #[serde(default)]
    pub base: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Step {
    #[serde(default)]
    pub name: String,
    pub intent: String,
    #[serde(default)]
    pub request: Request,
    #[serde(default)]
    pub expect: Expect,
    #[serde(default)]
    pub capture: BTreeMap<String, String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Request {
    #[serde(default = "get")]
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub query: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub json: Option<serde_json::Value>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            method: get(),
            url: String::new(),
            headers: BTreeMap::new(),
            query: BTreeMap::new(),
            json: None,
        }
    }
}

fn get() -> String {
    "GET".into()
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct Expect {
    pub status: Option<u16>,
    #[serde(default)]
    pub body: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub exists: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct HttpContract {
    pub name: String,
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub auth: Option<HttpAuth>,
    #[serde(default)]
    pub routes: Vec<HttpRoute>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct HttpAuth {
    #[serde(default)]
    pub scheme: String,
    #[serde(default = "authorization")]
    pub header: String,
}

fn authorization() -> String {
    "Authorization".into()
}

#[derive(Debug, Clone, serde::Deserialize)]
struct HttpRoute {
    #[serde(default = "get")]
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub success_status: Option<u16>,
    #[serde(default)]
    pub query: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub example_request: Option<serde_json::Value>,
    #[serde(default)]
    pub extract: Vec<HttpExtract>,
    #[serde(default)]
    pub response_fields: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct HttpExtract {
    pub field: String,
    #[serde(rename = "as")]
    pub as_name: String,
}

/// The outcome of one executed step.
#[derive(Debug, Clone)]
pub struct StepOutcome {
    pub name: String,
    pub intent: String,
    pub passed: bool,
    pub detail: String,
}

pub fn parse(path: &Path) -> Result<SagaSpec> {
    Ok(parse_with_kind(path)?.0)
}

pub fn parse_with_kind(path: &Path) -> Result<(SagaSpec, SpecKind)> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).context("parsing saga spec (JSON)")?;
    if value.get("routes").is_some() {
        let contract: HttpContract =
            serde_json::from_value(value).context("parsing HTTP contract (JSON)")?;
        return Ok((http_contract_to_saga(contract), SpecKind::HttpContractJson));
    }
    let spec: SagaSpec = serde_json::from_value(value).context("parsing saga spec (JSON)")?;
    Ok((spec, SpecKind::SagaJson))
}

fn http_contract_to_saga(contract: HttpContract) -> SagaSpec {
    let auth_header = contract.auth.as_ref().and_then(|auth| {
        if auth.scheme.eq_ignore_ascii_case("bearer") {
            Some((
                auth.header.clone(),
                "Bearer {{ env.LOOM_SAGA_AUTH_TOKEN }}".to_string(),
            ))
        } else {
            None
        }
    });
    let steps = contract
        .routes
        .into_iter()
        .map(|route| {
            let method = route.method.to_uppercase();
            let name = route
                .name
                .unwrap_or_else(|| format!("{method} {}", route.path));
            let intent = route.intent.unwrap_or_else(|| name.clone());
            let mut headers = BTreeMap::new();
            if let Some((header, value)) = &auth_header {
                headers.insert(header.clone(), value.clone());
            }
            let capture = route
                .extract
                .into_iter()
                .map(|e| (e.as_name, field_path(&e.field)))
                .collect();
            Step {
                name,
                intent,
                request: Request {
                    method,
                    url: route.path,
                    headers,
                    query: route.query,
                    json: route.example_request,
                },
                expect: Expect {
                    status: route.success_status,
                    body: BTreeMap::new(),
                    exists: route
                        .response_fields
                        .into_iter()
                        .map(|field| field_path(&field))
                        .collect(),
                },
                capture,
            }
        })
        .collect();
    let base = if contract.base.trim().is_empty() {
        "{{ env.BASE_URL }}".to_string()
    } else {
        contract.base
    };
    SagaSpec {
        saga: contract.name,
        base,
        steps,
    }
}

fn field_path(field: &str) -> String {
    if field.starts_with('$') {
        field.to_string()
    } else {
        format!("$.{field}")
    }
}

/// Execute a saga. When `stamp` is true, write verdicts onto the saga's
/// `validates` edges; otherwise (diagnose) only report.
pub fn execute(store: &Store, spec: &SagaSpec, stamp: bool) -> Result<Vec<StepOutcome>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building http client")?;
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    let mut outcomes = Vec::new();
    let saga_val = if stamp {
        Some(store.resolve_node(&spec.saga, Some(NodeType::Validation))?)
    } else {
        None
    };
    let mut boundary_failed = false;

    for step in &spec.steps {
        if boundary_failed {
            break; // never-reached steps stay untouched
        }
        let url = interpolate(&format!("{}{}", spec.base, step.request.url), &vars);
        let method = step.request.method.to_uppercase();
        let mut req = client.request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET),
            &url,
        );
        for (name, value) in &step.request.headers {
            req = req.header(name, interpolate(value, &vars));
        }
        if !step.request.query.is_empty() {
            let query: Vec<(String, String)> = step
                .request
                .query
                .iter()
                .map(|(key, value)| (key.clone(), interpolate(&value_to_string(value), &vars)))
                .collect();
            req = req.query(&query);
        }
        if let Some(body) = &step.request.json {
            let interpolated = interpolate_json(body, &vars);
            req = req.json(&interpolated);
        }
        let outcome = match req.send() {
            Ok(resp) => check_response(step, resp, &mut vars),
            Err(e) => StepOutcome {
                name: step.name.clone(),
                intent: step.intent.clone(),
                passed: false,
                detail: format!("request error: {e}"),
            },
        };
        if !outcome.passed {
            boundary_failed = true;
        }
        // stamp the validates edge for this step's intent
        if let Some(saga) = &saga_val {
            if let Ok(intent) = store.resolve_node(&step.intent, Some(NodeType::Intent)) {
                for e in
                    store.edges_with(Some(EdgeKind::Validates), Some(&saga.id), Some(&intent.id))?
                {
                    let status = if outcome.passed {
                        InspectionStatus::Passing
                    } else {
                        InspectionStatus::Failing
                    };
                    store.record_verdict(
                        &e.id,
                        status,
                        "saga step",
                        &outcome.detail,
                        1.0,
                        "saga",
                    )?;
                }
            }
        }
        outcomes.push(outcome);
    }
    if stamp {
        let all_pass = outcomes.iter().all(|o| o.passed) && !outcomes.is_empty();
        if let Some(saga) = &saga_val {
            store.set_node_status(&saga.id, if all_pass { "passed" } else { "failed" })?;
        }
    }
    Ok(outcomes)
}

fn check_response(
    step: &Step,
    resp: reqwest::blocking::Response,
    vars: &mut BTreeMap<String, String>,
) -> StepOutcome {
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().unwrap_or(serde_json::Value::Null);
    // status check (default: any 2xx)
    let status_ok = match step.expect.status {
        Some(want) => status == want,
        None => (200..300).contains(&status),
    };
    if !status_ok {
        return StepOutcome {
            name: step.name.clone(),
            intent: step.intent.clone(),
            passed: false,
            detail: format!("expected status {:?}, got {status}", step.expect.status),
        };
    }
    // body expectations
    for (path, want) in &step.expect.body {
        let got = jsonpath(&body, path);
        if got.as_ref() != Some(want) {
            return StepOutcome {
                name: step.name.clone(),
                intent: step.intent.clone(),
                passed: false,
                detail: format!("expected {path}={want}, got {got:?}"),
            };
        }
    }
    for path in &step.expect.exists {
        if jsonpath(&body, path).is_none() {
            return StepOutcome {
                name: step.name.clone(),
                intent: step.intent.clone(),
                passed: false,
                detail: format!("expected field {path} to exist"),
            };
        }
    }
    // captures
    for (var, path) in &step.capture {
        if let Some(v) = jsonpath(&body, path) {
            let s = match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            vars.insert(var.clone(), s);
        }
    }
    StepOutcome {
        name: step.name.clone(),
        intent: step.intent.clone(),
        passed: true,
        detail: format!("status {status}"),
    }
}

/// Replace `{{ var }}` and `{{ env.NAME }}` in a string.
fn interpolate(s: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let key = after[..end].trim();
            let value = if let Some(env_key) = key.strip_prefix("env.") {
                std::env::var(env_key).unwrap_or_default()
            } else {
                vars.get(key).cloned().unwrap_or_default()
            };
            out.push_str(&value);
            rest = &after[end + 2..];
        } else {
            out.push_str("{{");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn interpolate_json(v: &serde_json::Value, vars: &BTreeMap<String, String>) -> serde_json::Value {
    match v {
        serde_json::Value::String(s) => serde_json::Value::String(interpolate(s, vars)),
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.iter().map(|x| interpolate_json(x, vars)).collect())
        }
        serde_json::Value::Object(o) => serde_json::Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), interpolate_json(x, vars)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Dotted JSONPath subset: `$.a.b` / `$.id`. Returns the value if present.
fn jsonpath(v: &serde_json::Value, path: &str) -> Option<serde_json::Value> {
    let p = path
        .strip_prefix("$.")
        .or_else(|| path.strip_prefix('$'))
        .unwrap_or(path);
    if p.is_empty() {
        return Some(v.clone());
    }
    let mut cur = v;
    for seg in p.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur.clone())
}

/// Diagnose common failure roots before/without stamping.
pub fn diagnose_hints(spec: &SagaSpec) -> Vec<String> {
    let mut hints = Vec::new();
    if spec.base.is_empty() {
        hints.push("no `base` url set — relative step urls will fail".into());
    }
    let blob = serde_json::to_string(
        &spec
            .steps
            .iter()
            .map(|s| &s.request.url)
            .collect::<Vec<_>>(),
    )
    .unwrap_or_default();
    if blob.contains("{{ env.") || blob.contains("{{env.") {
        hints.push("steps reference env vars — ensure they are set before running".into());
    }
    hints
}

pub fn require(store: &Store, name: &str) -> Result<()> {
    store
        .resolve_node(name, Some(NodeType::Validation))
        .map(|_| ())
        .map_err(|_| anyhow!("no saga validation '{name}' — add it first with `loom saga add`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_threads_vars_and_env() {
        let mut vars = BTreeMap::new();
        vars.insert("cart_id".to_string(), "abc".to_string());
        assert_eq!(
            interpolate("/carts/{{ cart_id }}/pay", &vars),
            "/carts/abc/pay"
        );
        std::env::set_var("LOOM_SAGA_TEST_BASE", "http://x");
        assert_eq!(
            interpolate("{{ env.LOOM_SAGA_TEST_BASE }}/y", &vars),
            "http://x/y"
        );
    }

    #[test]
    fn jsonpath_dotted_subset() {
        let v = serde_json::json!({"id": "c1", "state": {"paid": true}});
        assert_eq!(jsonpath(&v, "$.id"), Some(serde_json::json!("c1")));
        assert_eq!(jsonpath(&v, "$.state.paid"), Some(serde_json::json!(true)));
        assert_eq!(jsonpath(&v, "$.missing"), None);
    }
}
