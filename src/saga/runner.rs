//! The saga executor: eager, ordered, fresh — and halt-on-failure.
//!
//! Failure semantics (what the graph stamping in `commands::saga` relies on):
//! steps before the failure RAN and PASSED — they are real runtime evidence;
//! the failing step carries the exact expectation that broke ("expected 200,
//! got 502"); steps after it were NEVER REACHED and produce no outcome at all
//! (not-reached is not failing — stamping it would be a lie the graph cannot
//! distinguish later).
//!
//! Every failure mode is an *outcome*, not a process error: network refusal,
//! timeout, unparseable JSON, a capture that matched nothing. From the
//! consumer's vantage point those are all "I could not consume the system",
//! which is exactly what the proof exists to detect.

use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;

use super::spec::{interpolate, interpolate_json, required_env, BodyExpectation, SagaSpec, Step};

// ---------------------------------------------------------------------------
// Outcomes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct StepOutcome {
    /// 1-based step number, matching the spec order.
    pub step: usize,
    pub name: String,
    /// The intent binding as written in the spec.
    pub intent: String,
    /// HTTP method as written in the spec (uppercased).
    pub method: String,
    /// Resolved URL (after interpolation + base join), with `{{ env.X }}`
    /// values redacted before the outcome leaves the runner.
    pub url: String,
    /// HTTP status received, if the request got a response at all.
    pub status: Option<u16>,
    pub passed: bool,
    /// "ok (201)" on success; on failure, every expectation that broke.
    pub detail: String,
    /// Variables captured by this step.
    pub captured: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SagaRunReport {
    pub saga: String,
    pub passed: bool,
    pub total_steps: usize,
    /// How many steps ran (== total_steps on success; the failing step is the
    /// last executed one otherwise — everything after was never reached).
    pub executed: usize,
    pub outcomes: Vec<StepOutcome>,
}

impl SagaRunReport {
    /// The failing outcome, if any.
    pub fn failure(&self) -> Option<&StepOutcome> {
        self.outcomes.iter().find(|o| !o.passed)
    }
}

const RESPONSE_BODY_CAP: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
struct EnvRedactor {
    pairs: Vec<(String, String)>,
}

impl EnvRedactor {
    fn from_spec(spec: &SagaSpec) -> Result<Self> {
        let mut pairs = Vec::new();
        for key in required_env(spec) {
            let value = std::env::var(&key).with_context(|| {
                format!("Template references '{{{{ env.{key} }}}}' but ${key} is not set.")
            })?;
            if value.len() >= 4 {
                pairs.push((key, value));
            }
        }
        Ok(Self { pairs })
    }

    fn redact(&self, s: &str) -> String {
        let mut out = s.to_string();
        for (key, value) in &self.pairs {
            out = out.replace(value, &format!("{{{{ env.{key} }}}}"));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Run the saga start to finish (or to first failure). Only environment-level
/// problems (a broken HTTP client or unresolvable `base`) return `Err`;
/// everything observed *against the target* is an outcome.
pub fn run_saga(spec: &SagaSpec) -> Result<SagaRunReport> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(spec.timeout_secs))
        .build()
        .context("Could not build the HTTP client")?;

    let mut vars = spec.vars.clone();
    let redactor = EnvRedactor::from_spec(spec)?;
    let base = interpolate(&spec.base, &vars).context("Could not resolve the saga `base:` url")?;

    let mut outcomes: Vec<StepOutcome> = Vec::new();
    let mut passed = true;

    for (i, step) in spec.steps.iter().enumerate() {
        let outcome = run_step(&client, &base, &redactor, step, i + 1, &mut vars)?;
        let step_passed = outcome.passed;
        outcomes.push(outcome);
        if !step_passed {
            passed = false;
            break; // halt-on-failure: later steps are never reached
        }
    }

    Ok(SagaRunReport {
        saga: spec.saga.clone(),
        passed,
        total_steps: spec.steps.len(),
        executed: outcomes.len(),
        outcomes,
    })
}

fn run_step(
    client: &reqwest::blocking::Client,
    base: &str,
    redactor: &EnvRedactor,
    step: &Step,
    number: usize,
    vars: &mut BTreeMap<String, String>,
) -> Result<StepOutcome> {
    let method = step.request.method.to_uppercase();
    let fail = |url: String, status: Option<u16>, detail: String| StepOutcome {
        step: number,
        name: step.name.clone(),
        intent: step.intent.clone(),
        method: method.clone(),
        url: redactor.redact(&url),
        status,
        passed: false,
        detail: redactor.redact(&detail),
        captured: BTreeMap::new(),
    };

    // Resolve the URL. A relative url with no base is a failed step outcome,
    // not a hard process error, so earlier side effects are still recorded.
    let url = match interpolate(&step.request.url, vars) {
        Ok(u) => u,
        Err(e) => {
            return Ok(fail(
                step.request.url.clone(),
                None,
                format!("spec error: {e}"),
            ))
        }
    };
    let url = if url.starts_with("http://") || url.starts_with("https://") {
        url
    } else {
        if base.is_empty() {
            return Ok(fail(
                url.clone(),
                None,
                format!("relative url '{url}' cannot run because the saga has no `base:`"),
            ));
        }
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            url.trim_start_matches('/')
        )
    };

    // Build the request.
    let method_parsed = match reqwest::Method::from_bytes(method.as_bytes()) {
        Ok(method) => method,
        Err(e) => {
            return Ok(fail(
                url,
                None,
                format!("invalid HTTP method '{method}': {e}"),
            ))
        }
    };
    let mut req = client.request(method_parsed, &url);
    for (k, v) in &step.request.headers {
        match interpolate(v, vars) {
            Ok(v) => req = req.header(k, v),
            Err(e) => return Ok(fail(url, None, format!("spec error in header '{k}': {e}"))),
        }
    }
    if let Some(json) = &step.request.json {
        match interpolate_json(json, vars) {
            Ok(j) => req = req.json(&j),
            Err(e) => return Ok(fail(url, None, format!("spec error in json body: {e}"))),
        }
    } else if let Some(body) = &step.request.body {
        match interpolate(body, vars) {
            Ok(b) => req = req.body(b),
            Err(e) => return Ok(fail(url, None, format!("spec error in body: {e}"))),
        }
    }

    // Execute. A refused/timed-out request is a failed consumption, recorded
    // as the step's outcome.
    let resp = match req.send() {
        Ok(r) => r,
        Err(e) => return Ok(fail(url, None, format!("request failed: {e}"))),
    };
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let text = match read_capped_body(resp) {
        Ok(t) => t,
        Err(e) => return Ok(fail(url, Some(status), e)),
    };

    // Evaluate expectations — collect EVERY broken one (better debugging than
    // first-failure-only).
    let mut problems: Vec<String> = Vec::new();

    match step.expect.status {
        Some(want) if status != want => {
            problems.push(format!("expected status {want}, got {status}"));
        }
        None if !(200..300).contains(&status) => {
            problems.push(format!("expected a 2xx status, got {status}"));
        }
        _ => {}
    }

    for (name, want_substr) in &step.expect.headers {
        let got = headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !got.contains(want_substr.as_str()) {
            problems.push(format!(
                "header '{name}': expected to contain '{want_substr}', got '{got}'"
            ));
        }
    }

    // Body asserts + captures need a JSON body; only demand one when used.
    let needs_json = !step.expect.body.is_empty() || !step.capture.is_empty();
    let json_body: Option<serde_json::Value> = if needs_json {
        match serde_json::from_str(&text) {
            Ok(v) => Some(v),
            Err(e) => {
                problems.push(format!(
                    "response body is not JSON ({e}); body starts: '{}'",
                    text.chars().take(80).collect::<String>()
                ));
                None
            }
        }
    } else {
        None
    };

    if let Some(body) = &json_body {
        for (path, expectation) in &step.expect.body {
            let jp = match serde_json_path::JsonPath::parse(path) {
                Ok(jp) => jp,
                Err(e) => {
                    problems.push(format!("body {path}: invalid JSONPath: {e}"));
                    continue;
                }
            };
            let nodes = jp.query(body).all();
            // Expectation values are templates too — `{{ env.X }}` / `{{ var }}`
            // in `expect.body` must be resolved against the process env and
            // earlier captures before comparison, mirroring request.* expansion.
            let expectation = match interpolate_expectation(expectation, vars) {
                Ok(e) => e,
                Err(e) => {
                    problems.push(format!("body {path}: spec error: {e}"));
                    continue;
                }
            };
            match expectation {
                BodyExpectation::Exists { exists } => {
                    if nodes.is_empty() == exists {
                        problems.push(format!(
                            "body {path}: expected exists={exists}, found {} node(s)",
                            nodes.len()
                        ));
                    }
                }
                BodyExpectation::Contains { contains } => match nodes.first() {
                    None => problems.push(format!("body {path}: matched nothing")),
                    Some(node) => {
                        let got = node_as_string(node);
                        if !got.contains(contains.as_str()) {
                            problems.push(format!(
                                "body {path}: expected to contain '{contains}', got '{got}'"
                            ));
                        }
                    }
                },
                BodyExpectation::Equals(want) => match nodes.first() {
                    None => problems.push(format!("body {path}: matched nothing")),
                    Some(node) => {
                        if *node != &want {
                            problems.push(format!(
                                "body {path}: expected {want}, got {got}",
                                got = node
                            ));
                        }
                    }
                },
            }
        }
    }

    // Captures only count on an otherwise-passing step — capturing out of a
    // failed response would thread garbage into later steps.
    let mut captured = BTreeMap::new();
    if problems.is_empty() {
        if let Some(body) = &json_body {
            for (var, path) in &step.capture {
                let jp = match serde_json_path::JsonPath::parse(path) {
                    Ok(jp) => jp,
                    Err(e) => {
                        problems.push(format!("capture '{var}': invalid JSONPath {path}: {e}"));
                        continue;
                    }
                };
                match jp.query(body).all().first() {
                    None => problems.push(format!("capture '{var}': {path} matched nothing")),
                    Some(node) => {
                        captured.insert(var.clone(), node_as_string(node));
                    }
                }
            }
        }
    }

    let passed = problems.is_empty();
    if passed {
        vars.extend(captured.clone());
    }
    let detail = if passed {
        format!("ok ({status})")
    } else {
        problems.join("; ")
    };
    Ok(StepOutcome {
        step: number,
        name: step.name.clone(),
        intent: step.intent.clone(),
        method,
        url: redactor.redact(&url),
        status: Some(status),
        passed,
        detail: redactor.redact(&detail),
        captured,
    })
}

fn read_capped_body(resp: reqwest::blocking::Response) -> std::result::Result<String, String> {
    let mut bytes = Vec::new();
    let mut limited = resp.take((RESPONSE_BODY_CAP + 1) as u64);
    limited
        .read_to_end(&mut bytes)
        .map_err(|e| format!("could not read response body: {e}"))?;
    if bytes.len() > RESPONSE_BODY_CAP {
        return Err("response body exceeds 8 MiB cap".to_string());
    }
    String::from_utf8(bytes).map_err(|e| format!("response body is not valid UTF-8: {e}"))
}
/// A captured/compared node as a string: raw for JSON strings, compact JSON
/// for everything else.
fn node_as_string(node: &serde_json::Value) -> String {
    match node {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
/// Resolve `{{ env.NAME }}` / `{{ var }}` inside an `expect.body` value before
/// it is compared against the response. `Equals` holds a JSON value (string
/// leaves interpolated via `interpolate_json`); `Contains` holds a single
/// template string; `Exists` carries no template.
fn interpolate_expectation(
    expectation: &BodyExpectation,
    vars: &BTreeMap<String, String>,
) -> Result<BodyExpectation> {
    Ok(match expectation {
        BodyExpectation::Exists { exists } => BodyExpectation::Exists { exists: *exists },
        BodyExpectation::Contains { contains } => BodyExpectation::Contains {
            contains: interpolate(contains, vars)?,
        },
        BodyExpectation::Equals(value) => {
            BodyExpectation::Equals(interpolate_json(value, vars)?)
        }
    })
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// Tiny scripted HTTP server: serves the canned responses in order (one
    /// connection each) and records the request line + body it saw.
    fn serve_script(responses: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            for resp in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut buf = vec![0u8; 65536];
                let mut read = 0usize;
                // Read until end of headers, then any Content-Length body.
                let (head_end, content_len) = loop {
                    let n = stream.read(&mut buf[read..]).unwrap_or(0);
                    if n == 0 {
                        break (read, 0);
                    }
                    read += n;
                    let text = String::from_utf8_lossy(&buf[..read]);
                    if let Some(pos) = text.find("\r\n\r\n") {
                        let len = text.lines().find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        });
                        break (pos + 4, len.unwrap_or(0));
                    }
                };
                while read < head_end + content_len {
                    let n = stream.read(&mut buf[read..]).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    read += n;
                }
                seen2
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..read]).to_string());
                stream.write_all(resp.as_bytes()).unwrap();
            }
        });
        (addr, seen)
    }

    fn http(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    const SPEC: &str = r#"
saga: checkout
base: "__BASE__"
steps:
  - name: create cart
    intent: cart-creation
    request: { method: POST, url: /carts, json: { owner: alice } }
    expect:
      status: 201
      headers: { content-type: json }
      body:
        "$.id": { exists: true }
        "$.state": open
    capture: { cart_id: "$.id" }
  - name: pay
    intent: payment-capture
    request: { method: POST, url: "/carts/{{ cart_id }}/payment" }
    expect:
      status: 200
      body:
        "$.state": paid
  - name: receipt
    intent: receipts
    request: { method: GET, url: "/carts/{{ cart_id }}/receipt" }
"#;

    // Tests run in parallel — substitute the base textually rather than via a
    // shared env var (a `set_var` here races the other tests' interpolation).
    fn load(base: &str) -> SagaSpec {
        crate::saga::spec::load_spec(&SPEC.replace("__BASE__", base), "test").unwrap()
    }

    #[test]
    fn happy_path_threads_captures_and_passes() {
        let (base, seen) = serve_script(vec![
            http("201 Created", r#"{"id":"c-42","state":"open"}"#),
            http("200 OK", r#"{"state":"paid"}"#),
            http("200 OK", r#"{"total":12}"#),
        ]);
        let report = run_saga(&load(&base)).unwrap();
        assert!(report.passed, "outcomes: {:?}", report.outcomes);
        assert_eq!(report.executed, 3);
        assert_eq!(report.outcomes[0].captured["cart_id"], "c-42");
        // The captured id was threaded into step 2's URL.
        let seen = seen.lock().unwrap();
        assert!(
            seen[1].starts_with("POST /carts/c-42/payment"),
            "got: {}",
            seen[1]
        );
        assert!(seen[0].contains(r#""owner":"alice""#), "got: {}", seen[0]);
    }

    #[test]
    fn failure_halts_and_names_the_broken_expectation() {
        let (base, _seen) = serve_script(vec![
            http("201 Created", r#"{"id":"c-42","state":"open"}"#),
            http("502 Bad Gateway", r#"{"error":"upstream"}"#),
            // step 3's response is never requested
        ]);
        let report = run_saga(&load(&base)).unwrap();
        assert!(!report.passed);
        assert_eq!(report.executed, 2, "halt-on-failure: step 3 never reached");
        let failure = report.failure().unwrap();
        assert_eq!(failure.step, 2);
        assert!(
            failure.detail.contains("expected status 200, got 502"),
            "got: {}",
            failure.detail
        );
        assert!(
            failure.detail.contains("$.state"),
            "all broken expectations listed: {}",
            failure.detail
        );
    }

    #[test]
    fn body_assert_failure_is_a_step_failure() {
        let (base, _seen) = serve_script(vec![http(
            "201 Created",
            r#"{"id":"c-42","state":"locked"}"#,
        )]);
        let report = run_saga(&load(&base)).unwrap();
        assert!(!report.passed);
        assert_eq!(report.executed, 1);
        let d = &report.failure().unwrap().detail;
        assert!(d.contains(r#"expected "open", got "locked""#), "got: {d}");
    }

    #[test]
    fn connection_refused_is_an_outcome_not_a_crash() {
        // Nothing listens on this port (bind then drop to find a free one).
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let spec = load(&format!("http://127.0.0.1:{port}"));
        let report = run_saga(&spec).unwrap();
        assert!(!report.passed);
        assert_eq!(report.executed, 1);
        assert!(report.failure().unwrap().detail.contains("request failed"));
    }

    #[test]
    fn relative_url_without_base_is_step_failure() {
        let spec = load("");
        let report = run_saga(&spec).unwrap();
        assert!(!report.passed);
        assert_eq!(report.executed, 1);
        let failure = report.failure().unwrap();
        assert!(
            failure.detail.contains("no `base:`"),
            "got: {}",
            failure.detail
        );
        assert_eq!(failure.url, "/carts");
    }

    #[test]
    fn env_values_are_redacted_from_outcomes() {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let secret_base = format!("http://127.0.0.1:{port}");
        std::env::set_var("LOOM_SAGA_REDACT_BASE", &secret_base);
        let spec = crate::saga::spec::load_spec(
            &SPEC.replace("__BASE__", "{{ env.LOOM_SAGA_REDACT_BASE }}"),
            "test",
        )
        .unwrap();
        let report = run_saga(&spec).unwrap();
        let failure = report.failure().unwrap();
        assert!(
            failure.url.contains("{{ env.LOOM_SAGA_REDACT_BASE }}"),
            "got: {}",
            failure.url
        );
        assert!(!failure.url.contains(&secret_base), "got: {}", failure.url);
        assert!(
            !failure.detail.contains(&secret_base),
            "got: {}",
            failure.detail
        );
    }

    // Unique env var names per test: `std::env::set_var` is process-global,
    // so shared names race when tests run in parallel.
    const ENV_EXPECT_SPEC: &str = r#"
saga: env-expect-ok
base: "__BASE__"
steps:
  - name: check policy
    intent: policy-read
    request: { method: GET, url: /policy }
    expect:
      status: 200
      body:
        "$.policy_version_id": "{{ env.LOOM_SAGA_EXPECT_ENV_OK_POLICY }}"
        "$.actor_person_id": { contains: "{{ env.LOOM_SAGA_EXPECT_ENV_OK_ACTOR }}" }
"#;

    // `{{ var }}` (from `vars:`) in expect.body is expanded like `{{ env.X }}`,
    // but — unlike env values — vars are NOT redacted from outcomes, so a
    // mismatch surfaces the *expanded* expectation in the detail. This proves
    // the value was actually substituted (pre-fix the detail held the literal
    // `{{ expected_state }}` template).
    const VAR_EXPECT_MISMATCH_SPEC: &str = r#"
saga: var-expect-mismatch
base: "__BASE__"
vars:
  expected_state: locked
steps:
  - name: check state
    intent: state-read
    request: { method: GET, url: /state }
    expect:
      status: 200
      body:
        "$.state": "{{ expected_state }}"
"#;

    #[test]
    fn env_in_expect_body_is_expanded_before_comparison() {
        let (base, _seen) = serve_script(vec![http(
            "200 OK",
            r#"{"policy_version_id":"grid-v1.0.0","actor_person_id":"grd_p_actor"}"#,
        )]);
        std::env::set_var("LOOM_SAGA_EXPECT_ENV_OK_POLICY", "grid-v1.0.0");
        std::env::set_var("LOOM_SAGA_EXPECT_ENV_OK_ACTOR", "grd_p_actor");
        let spec =
            crate::saga::spec::load_spec(&ENV_EXPECT_SPEC.replace("__BASE__", &base), "test")
                .unwrap();
        let report = run_saga(&spec).unwrap();
        assert!(report.passed, "outcomes: {:?}", report.outcomes);
    }

    #[test]
    fn var_in_expect_body_mismatch_surfaces_expanded_value() {
        let (base, _seen) = serve_script(vec![http("200 OK", r#"{"state":"open"}"#)]);
        let spec = crate::saga::spec::load_spec(
            &VAR_EXPECT_MISMATCH_SPEC.replace("__BASE__", &base),
            "test",
        )
        .unwrap();
        let report = run_saga(&spec).unwrap();
        assert!(!report.passed);
        let d = &report.failure().unwrap().detail;
        // The expanded expectation ("locked"), not the literal template, is
        // what the response was compared against — and no `{{` leaks through.
        assert!(d.contains("locked"), "got: {d}");
        assert!(!d.contains("{{"), "got: {d}");
    }
}
