//! Journey runner — the consumer-plane proof executor (ring 6).
//!
//! Plane: execution + graph write-back. A journey spec is an ordered chain of HTTP
//! requests, each naming the intent it proves, with values captured from one
//! response threaded into later requests. `run` executes the journey and stamps
//! `validates` edges: consecutive passing steps pass, the failing boundary fails
//! with the exact broken expectation, and never-reached steps are not executed;
//! previously passing validates edges for never-reached steps are reopened.
//!
//! JSONPath is a dotted-subset (`$.a.b`) — enough for capture/threading without
//! a full RFC 9535 engine.

use crate::model::{EdgeKind, InspectionStatus, Node, NodeType};
use crate::store::Store;
use crate::Result;
use anyhow::{anyhow, Context};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecKind {
    JourneyJson,
    HttpContractJson,
}

impl SpecKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SpecKind::JourneyJson => "journey_json",
            SpecKind::HttpContractJson => "http_contract_json",
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct JourneySpec {
    pub journey: String,
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

pub fn parse(path: &Path) -> Result<JourneySpec> {
    Ok(parse_with_kind(path)?.0)
}

pub fn parse_with_kind(path: &Path) -> Result<(JourneySpec, SpecKind)> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    // Try JSON first; fall back to YAML for `.yaml`/`.yml` specs.
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(json_err) => {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml") {
                serde_norway::from_str(&text).with_context(|| {
                    format!("parsing journey spec as YAML (JSON failed: {json_err})")
                })?
            } else {
                return Err(json_err).context("parsing journey spec (JSON)");
            }
        }
    };
    if value.get("routes").is_some() {
        let contract: HttpContract =
            serde_json::from_value(value).context("parsing HTTP contract")?;
        return Ok((
            http_contract_to_journey(contract),
            SpecKind::HttpContractJson,
        ));
    }
    let spec: JourneySpec = serde_json::from_value(value).context("parsing journey spec")?;
    Ok((spec, SpecKind::JourneyJson))
}

fn http_contract_to_journey(contract: HttpContract) -> JourneySpec {
    let auth_header = contract.auth.as_ref().and_then(|auth| {
        if auth.scheme.eq_ignore_ascii_case("bearer") {
            Some((
                auth.header.clone(),
                "Bearer {{ env.LOOM_JOURNEY_AUTH_TOKEN }}".to_string(),
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
                    url: normalize_path_params(&route.path),
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
    JourneySpec {
        journey: contract.name,
        base,
        steps,
    }
}

/// Normalize OpenAPI/REST-style single-brace path params (`{person_id}`) to
/// loom's canonical `{{ person_id }}` interpolation syntax, so a contract's
/// path template threads a prior step's `extract`/`capture` without the
/// author having to hand-rewrite braces. A path that already uses `{{ }}` is
/// left untouched (its inner text starts with `{`, which this skips).
fn normalize_path_params(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(brace_pos) = rest.find('{') {
        out.push_str(&rest[..brace_pos]);
        let after_brace = &rest[brace_pos + 1..];
        // Already double-braced (`{{ ... }}`) — copy through and continue past it.
        if let Some(stripped) = after_brace.strip_prefix('{') {
            out.push_str("{{");
            rest = stripped;
            continue;
        }
        match after_brace.find('}') {
            Some(rel_end) => {
                let ident = &after_brace[..rel_end];
                let is_bare_ident = !ident.is_empty()
                    && ident
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
                if is_bare_ident {
                    out.push_str("{{ ");
                    out.push_str(ident);
                    out.push_str(" }}");
                } else {
                    // Not a bare identifier — leave the original braces as-is.
                    out.push('{');
                    out.push_str(ident);
                    out.push('}');
                }
                rest = &after_brace[rel_end + 1..];
            }
            None => {
                // Unterminated brace — copy the rest verbatim and stop.
                out.push('{');
                out.push_str(after_brace);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn field_path(field: &str) -> String {
    if field.starts_with('$') {
        field.to_string()
    } else {
        format!("$.{field}")
    }
}

/// Execute a journey. When `record` is true, write verdicts onto the journey's
/// `validates` edges; otherwise (diagnose) only report.
pub fn execute(
    store: Option<&Store>,
    spec: &JourneySpec,
    record: bool,
) -> Result<Vec<StepOutcome>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building http client")?;
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    let mut outcomes = Vec::new();
    let journey_val = if record {
        let store =
            store.ok_or_else(|| anyhow!("recording a journey run requires a graph store"))?;
        Some(resolve_validation(store, &spec.journey, true)?)
    } else {
        None
    };
    for (idx, step) in spec.steps.iter().enumerate() {
        let base = interpolate(&spec.base, &vars);
        if !record && (base.is_empty() || base.contains("{{")) {
            bail_no_usable_base(spec, &base)?;
        }
        let url = interpolate(&format!("{base}{}", step.request.url), &vars);
        let method = step.request.method.to_uppercase();
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| anyhow!("step '{}': invalid HTTP method '{method}'", step.name))?;
        let mut req = client.request(method, &url);
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
        let mut outcome = match req.send() {
            Ok(resp) => check_response(step, resp, &mut vars, !record),
            Err(e) => StepOutcome {
                name: step.name.clone(),
                intent: step.intent.clone(),
                passed: false,
                detail: if record {
                    format!("request error: {e}")
                } else {
                    format!("request failed: {e}")
                },
            },
        };
        if let (Some(store), Some(journey)) = (store, &journey_val) {
            match store.resolve_node(&step.intent, Some(NodeType::Intent)) {
                Ok(intent) => {
                    for e in store.edges_with(
                        Some(EdgeKind::Validates),
                        Some(&journey.id),
                        Some(&intent.id),
                    )? {
                        let status = if outcome.passed {
                            InspectionStatus::Passing
                        } else {
                            InspectionStatus::Failing
                        };
                        store.record_verdict(
                            &e.id,
                            status,
                            "journey step",
                            &outcome.detail,
                            1.0,
                            "journey",
                        )?;
                    }
                }
                Err(e) => {
                    outcome.passed = false;
                    outcome.detail = format!("unresolved step intent '{}': {e}", step.intent);
                }
            }
        }
        let passed = outcome.passed;
        outcomes.push(outcome);
        if !passed {
            if let (Some(store), Some(journey)) = (store, &journey_val) {
                stale_unreached_passing_steps(store, spec, &journey.id, idx + 1)?;
            }
            break;
        }
    }
    if record {
        let all_pass = outcomes.iter().all(|o| o.passed) && !outcomes.is_empty();
        if let (Some(store), Some(journey)) = (store, &journey_val) {
            store.set_node_status(&journey.id, if all_pass { "passed" } else { "failed" })?;
        }
    }
    Ok(outcomes)
}

fn bail_no_usable_base(spec: &JourneySpec, resolved_base: &str) -> Result<()> {
    anyhow::bail!(
        "journey '{}' has no usable base URL (spec base='{}' resolved to '{resolved_base}'). \
         Pass --base-url, set BASE_URL in the environment, or add a \"base\" field to the spec.",
        spec.journey,
        spec.base
    )
}

fn stale_unreached_passing_steps(
    store: &Store,
    spec: &JourneySpec,
    journey_id: &str,
    start: usize,
) -> Result<()> {
    for step in spec.steps.iter().skip(start) {
        let Ok(intent) = store.resolve_node(&step.intent, Some(NodeType::Intent)) else {
            continue;
        };
        for e in store.edges_with(
            Some(EdgeKind::Validates),
            Some(journey_id),
            Some(&intent.id),
        )? {
            store.stale_passing_edge(&e.id)?;
        }
    }
    Ok(())
}

fn check_response(
    step: &Step,
    resp: reqwest::blocking::Response,
    vars: &mut BTreeMap<String, String>,
    diagnose_style: bool,
) -> StepOutcome {
    let status = resp.status().as_u16();
    let body_parse: Result<serde_json::Value, _> = resp.json();
    let status_ok = if diagnose_style {
        status == step.expect.status.unwrap_or(200)
    } else {
        match step.expect.status {
            Some(want) => status == want,
            None => (200..300).contains(&status),
        }
    };
    if !status_ok {
        let detail = if diagnose_style {
            format!(
                "expected status {}, got {status}",
                step.expect.status.unwrap_or(200)
            )
        } else {
            format!("expected status {:?}, got {status}", step.expect.status)
        };
        return StepOutcome {
            name: step.name.clone(),
            intent: step.intent.clone(),
            passed: false,
            detail,
        };
    }
    // A step that never reads the body (status-only check) tolerates a
    // non-JSON response; a step that checks fields or captures variables
    // must fail honestly instead of matching against a silent Null.
    let needs_body =
        !step.expect.body.is_empty() || !step.expect.exists.is_empty() || !step.capture.is_empty();
    let body = match body_parse {
        Ok(v) => v,
        Err(e) if needs_body => {
            return StepOutcome {
                name: step.name.clone(),
                intent: step.intent.clone(),
                passed: false,
                detail: format!("response body is not valid JSON ({e}); status {status}"),
            };
        }
        Err(_) => serde_json::Value::Null,
    };
    for (path, want) in &step.expect.body {
        let want_resolved = interpolate_json(want, vars);
        let got = jsonpath(&body, path);
        if got.as_ref() != Some(&want_resolved) {
            let detail = if diagnose_style {
                format!(
                    "body {path}: expected {want_resolved}, got {}",
                    got.map(|v| v.to_string()).unwrap_or_else(|| "null".into())
                )
            } else {
                format!("expected {path}={want_resolved}, got {got:?}")
            };
            return StepOutcome {
                name: step.name.clone(),
                intent: step.intent.clone(),
                passed: false,
                detail,
            };
        }
    }
    for path in &step.expect.exists {
        if jsonpath(&body, path).is_none() {
            let detail = if diagnose_style {
                format!("missing field {path}")
            } else {
                format!("expected field {path} to exist")
            };
            return StepOutcome {
                name: step.name.clone(),
                intent: step.intent.clone(),
                passed: false,
                detail,
            };
        }
    }
    for (var, path) in &step.capture {
        if let Some(v) = jsonpath(&body, path) {
            let s = match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            vars.insert(var.clone(), s);
        }
    }
    let detail = if step.expect.exists.is_empty() && step.expect.body.is_empty() {
        if diagnose_style {
            format!("status {status} ok")
        } else {
            format!("status {status}")
        }
    } else {
        let mut checked: Vec<&str> = step.expect.body.keys().map(String::as_str).collect();
        checked.extend(step.expect.exists.iter().map(String::as_str));
        if diagnose_style {
            format!("status {status} ok, verified: {}", checked.join(", "))
        } else {
            format!("status {status}, verified: {}", checked.join(", "))
        }
    };
    StepOutcome {
        name: step.name.clone(),
        intent: step.intent.clone(),
        passed: true,
        detail,
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
                env_value(env_key)
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

fn env_value(key: &str) -> String {
    std::env::var(key)
        .or_else(|_| {
            if key == "LOOM_JOURNEY_AUTH_TOKEN" {
                std::env::var("LOOM_SAGA_AUTH_TOKEN")
            } else {
                Err(std::env::VarError::NotPresent)
            }
        })
        .unwrap_or_default()
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

/// Diagnose common failure roots before/without recording.
pub fn diagnose_hints(spec: &JourneySpec) -> Vec<String> {
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

fn journey_status_rank(status: &str) -> u8 {
    match status {
        "passed" => 0,
        "not_run" => 1,
        _ => 2,
    }
}

/// Every journey validation registered for `journey_id`, canonical-sorted: a
/// passed proof first, then not_run, then by id. A non-idempotent `add` could
/// leave several duplicates for one id; the first is the one to keep.
pub fn journey_validations(store: &Store, journey_id: &str) -> Result<Vec<Node>> {
    let mut vals: Vec<Node> = store
        .list_nodes(Some(NodeType::Validation), usize::MAX)?
        .into_iter()
        .filter(|n| {
            // A journey validation created by `journey add` carries body.journey_id;
            // a name-based one (e.g. `validation add --proof-kind journey`, or a
            // legacy node) is matched by name. The is-journey guard keeps a
            // same-named non-journey validation out.
            let is_journey = n.body.get("type").and_then(|t| t.as_str()) == Some("journey")
                || n.body.get("proof_kind").and_then(|t| t.as_str()) == Some("journey");
            is_journey
                && (n.body.get("journey_id").and_then(|v| v.as_str()) == Some(journey_id)
                    || n.name == journey_id)
        })
        .collect();
    vals.sort_by(|a, b| {
        journey_status_rank(&a.status)
            .cmp(&journey_status_rank(&b.status))
            .then(a.id.cmp(&b.id))
    });
    Ok(vals)
}

/// Resolve the single journey validation for `journey_id`. Tolerates duplicates
/// left by a non-idempotent add (picks the canonical one); when `repair`, removes
/// the duplicates so the graph self-heals on run. Errors only when none exists —
/// the honest "add it first" case (never the misleading ambiguous-name error).
pub fn resolve_validation(store: &Store, journey_id: &str, repair: bool) -> Result<Node> {
    let mut vals = journey_validations(store, journey_id)?;
    if vals.is_empty() {
        anyhow::bail!(
            "no journey validation '{journey_id}' — add it first with `loom journey add`"
        );
    }
    let canonical = vals.remove(0);
    if repair {
        for dup in &vals {
            // Inherit the duplicate's step links before removing it, so a later
            // fixed add that linked more/different steps loses no coverage.
            for e in store.edges_with(Some(EdgeKind::Validates), Some(&dup.id), None)? {
                store.ensure_edge(EdgeKind::Validates, &canonical.id, &e.to_id)?;
            }
            store.delete_node(&dup.id)?;
        }
    }
    Ok(canonical)
}

pub fn require(store: &Store, name: &str) -> Result<()> {
    resolve_validation(store, name, false).map(|_| ())
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
        std::env::set_var("LOOM_JOURNEY_TEST_BASE", "http://x");
        assert_eq!(
            interpolate("{{ env.LOOM_JOURNEY_TEST_BASE }}/y", &vars),
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

    #[test]
    fn normalize_path_params_converts_single_brace_idents() {
        assert_eq!(
            normalize_path_params("/v1/grid/standing/{person_id}"),
            "/v1/grid/standing/{{ person_id }}"
        );
        assert_eq!(
            normalize_path_params("/a/{x}/b/{y}/c"),
            "/a/{{ x }}/b/{{ y }}/c"
        );
    }

    #[test]
    fn normalize_path_params_leaves_already_double_braced_alone() {
        assert_eq!(
            normalize_path_params("/v1/persons/{{ person_id }}"),
            "/v1/persons/{{ person_id }}"
        );
    }

    #[test]
    fn normalize_path_params_ignores_non_identifier_braces() {
        // Not a bare identifier (contains a space/symbol not in the allowed
        // set) — left untouched rather than guessed at.
        assert_eq!(normalize_path_params("/x/{a b}/y"), "/x/{a b}/y");
    }
}
