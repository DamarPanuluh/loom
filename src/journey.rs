//! Journey runner — the consumer-plane proof executor (ring 6).
//!
//! Plane: execution + graph write-back. A journey spec is an ordered chain of
//! steps, each naming the intent it proves. A step is either:
//! - **HTTP** — `request` + response expectations (API / contract surfaces)
//! - **CLI** — `run` (shell command) + exit/stdout expectations (CLI / tool surfaces)
//!
//! Values captured from one step thread into later steps. `run` stamps
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
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::Instant;

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
    /// Shell command for a CLI step (`sh -c`). Mutually exclusive with a
    /// meaningful HTTP `request` — when `run` is non-empty, the step is CLI.
    #[serde(default)]
    pub run: String,
    #[serde(default)]
    pub request: Request,
    #[serde(default)]
    pub expect: Expect,
    #[serde(default)]
    pub capture: BTreeMap<String, String>,
}

impl Step {
    /// CLI steps name a shell command; HTTP steps name a request URL/method.
    pub fn is_cli(&self) -> bool {
        !self.run.trim().is_empty()
    }
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
    /// HTTP response status (HTTP steps).
    pub status: Option<u16>,
    /// Process exit code (CLI steps). Defaults to `0` when omitted on a CLI step.
    pub exit_code: Option<i32>,
    /// Substrings that must appear in CLI stdout (CLI steps).
    #[serde(default)]
    pub stdout_contains: Vec<String>,
    /// Substrings that must appear in CLI stderr (CLI steps).
    #[serde(default)]
    pub stderr_contains: Vec<String>,
    /// JSONPath → expected value. HTTP: response body. CLI: stdout parsed as JSON.
    #[serde(default)]
    pub body: BTreeMap<String, serde_json::Value>,
    /// JSONPaths that must exist. HTTP: response body. CLI: stdout parsed as JSON.
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
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StepOutcome {
    pub name: String,
    pub intent: String,
    pub passed: bool,
    pub detail: String,
    #[serde(default)]
    pub transcript: String,
    /// Wall-clock execution time retained with a frozen baseline so an
    /// otherwise-passing replay can still surface a latency cliff.
    #[serde(default)]
    pub latency_ms: u128,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Baseline {
    pub journey: String,
    pub outcomes: Vec<StepOutcome>,
}

pub fn baseline_path(root: &Path, journey: &str) -> PathBuf {
    root.join(crate::LOOM_DIR)
        .join("baselines")
        .join(format!("{journey}.json"))
}

/// Every frozen baseline, sorted by journey name so they travel
/// deterministically in the export (baselines are local runtime state —
/// without them an imported graph's journeys cannot grade S4+).
pub fn read_baselines(root: &Path) -> Result<Vec<Baseline>> {
    let dir = root.join(crate::LOOM_DIR).join("baselines");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let text = match std::fs::read_to_string(entry.path()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if let Ok(b) = serde_json::from_str::<Baseline>(&text) {
            out.push(b);
        }
    }
    out.sort_by(|a, b| a.journey.cmp(&b.journey));
    Ok(out)
}

/// Restore exported baselines verbatim (the journey names are the keys).
/// Idempotent: an existing baseline for the same journey is left untouched.
pub fn restore_baselines(root: &Path, baselines: &[Baseline]) -> Result<usize> {
    let mut restored = 0;
    for b in baselines {
        let path = baseline_path(root, &b.journey);
        if path.exists() {
            continue;
        }
        write_baseline(root, &b.journey, &b.outcomes)?;
        restored += 1;
    }
    Ok(restored)
}

pub fn write_baseline(root: &Path, journey: &str, outcomes: &[StepOutcome]) -> Result<PathBuf> {
    let path = baseline_path(root, journey);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("baseline path '{}' has no parent", path.display()))?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&Baseline {
            journey: journey.into(),
            outcomes: outcomes.to_vec(),
        })?,
    )?;
    Ok(path)
}

pub fn read_baseline(root: &Path, journey: &str) -> Result<Option<Baseline>> {
    let path = baseline_path(root, journey);
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn deviations(baseline: &Baseline, outcomes: &[StepOutcome]) -> Vec<String> {
    let mut deviations = Vec::new();
    for (index, outcome) in outcomes.iter().enumerate() {
        let Some(before) = baseline.outcomes.get(index) else {
            deviations.push(format!("new step '{}'", outcome.name));
            continue;
        };
        if before.transcript != outcome.transcript {
            deviations.push(format!("{}: verbatim output changed", outcome.name));
        }
        if before.passed != outcome.passed {
            deviations.push(format!("{}: pass/fail changed", outcome.name));
        }
        // Avoid flagging scheduler noise: a cliff is at least 100 ms slower
        // and at least twice the frozen execution time.
        if outcome.latency_ms >= before.latency_ms.saturating_mul(2)
            && outcome.latency_ms.saturating_sub(before.latency_ms) >= 100
        {
            deviations.push(format!(
                "{}: latency cliff ({}ms → {}ms)",
                outcome.name, before.latency_ms, outcome.latency_ms
            ));
        }
    }
    if baseline.outcomes.len() > outcomes.len() {
        deviations.push("one or more baseline steps were not reached".into());
    }
    deviations
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
                run: String::new(),
                request: Request {
                    method,
                    url: normalize_path_params(&route.path),
                    headers,
                    query: route.query,
                    json: route.example_request,
                },
                expect: Expect {
                    status: route.success_status,
                    exit_code: None,
                    stdout_contains: Vec::new(),
                    stderr_contains: Vec::new(),
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
///
/// `cwd` is the working directory for CLI steps (`run:`). HTTP steps ignore it.
/// When `None`, CLI steps use the process current directory.
///
/// Prefer [`execute_steps`] + [`record_outcomes`] from `journey run` so the
/// exclusive graph lock is not held while nested CLI steps touch the same graph.
pub fn execute(
    store: Option<&Store>,
    spec: &JourneySpec,
    record: bool,
) -> Result<Vec<StepOutcome>> {
    execute_in(store, spec, record, None)
}

/// Like [`execute`], but CLI steps run with `cwd` as the working directory.
pub fn execute_in(
    store: Option<&Store>,
    spec: &JourneySpec,
    record: bool,
    cwd: Option<&Path>,
) -> Result<Vec<StepOutcome>> {
    let mut outcomes = execute_steps(spec, cwd, !record)?;
    if record {
        let store =
            store.ok_or_else(|| anyhow!("recording a journey run requires a graph store"))?;
        record_outcomes(store, spec, &mut outcomes)?;
    }
    Ok(outcomes)
}

/// Run every step without holding a graph store (no verdict write-back).
///
/// Use this from `journey run` after dropping the exclusive lock so CLI steps
/// that invoke the same repo's loom (or any other graph writer) can open the
/// graph. Call [`record_outcomes`] afterward to stamp proofs.
pub fn execute_steps(
    spec: &JourneySpec,
    cwd: Option<&Path>,
    diagnose_style: bool,
) -> Result<Vec<StepOutcome>> {
    let has_http = spec.steps.iter().any(|s| !s.is_cli());
    let client = if has_http {
        Some(
            reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .context("building http client")?,
        )
    } else {
        None
    };
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    let mut outcomes = Vec::new();
    for step in &spec.steps {
        if step.is_cli() && !step.request.url.trim().is_empty() {
            anyhow::bail!(
                "step '{}': set either `run` (CLI) or `request` (HTTP), not both",
                step.name
            );
        }
        let started = Instant::now();
        let mut outcome = if step.is_cli() {
            run_cli_step(step, &mut vars, cwd, diagnose_style)?
        } else {
            let client = client
                .as_ref()
                .ok_or_else(|| anyhow!("internal: HTTP client missing for HTTP step"))?;
            run_http_step(client, spec, step, &mut vars, diagnose_style)?
        };
        outcome.latency_ms = started.elapsed().as_millis();
        let passed = outcome.passed;
        outcomes.push(outcome);
        if !passed {
            break;
        }
    }
    Ok(outcomes)
}

fn run_http_step(
    client: &reqwest::blocking::Client,
    spec: &JourneySpec,
    step: &Step,
    vars: &mut BTreeMap<String, String>,
    diagnose_style: bool,
) -> Result<StepOutcome> {
    let base = interpolate(&spec.base, vars);
    if diagnose_style && (base.is_empty() || base.contains("{{")) {
        bail_no_usable_base(spec, &base)?;
    }
    let url = interpolate(&format!("{base}{}", step.request.url), vars);
    let method_name = step.request.method.to_uppercase();
    let method = reqwest::Method::from_bytes(method_name.as_bytes())
        .map_err(|_| anyhow!("step '{}': invalid HTTP method '{method_name}'", step.name))?;
    let mut request = client.request(method, &url);
    for (name, value) in &step.request.headers {
        request = request.header(name, interpolate(value, vars));
    }
    if !step.request.query.is_empty() {
        let query: Vec<(String, String)> = step
            .request
            .query
            .iter()
            .map(|(key, value)| (key.clone(), interpolate(&value_to_string(value), vars)))
            .collect();
        request = request.query(&query);
    }
    if let Some(body) = &step.request.json {
        request = request.json(&interpolate_json(body, vars));
    }
    Ok(match request.send() {
        Ok(response) => check_response(step, response, vars, diagnose_style),
        Err(error) => failed_step(
            step,
            if diagnose_style {
                format!("request failed: {error}")
            } else {
                format!("request error: {error}")
            },
        ),
    })
}

/// Stamp `validates` verdicts and journey status from already-executed outcomes.
///
/// Resolves the journey validation (repairing duplicates when needed). Mutates
/// `outcomes` when a step intent cannot be resolved (marks that step failed).
/// The intent NAME a step targets, for matching spec steps back to the intent
/// their verdict aggregates.
fn intent_name<'a>(store: &Store, intent_id: &'a str) -> std::borrow::Cow<'a, str> {
    match store.get_node(intent_id) {
        Ok(Some(n)) => std::borrow::Cow::Owned(n.name),
        _ => std::borrow::Cow::Borrowed(intent_id),
    }
}

pub fn record_outcomes(
    store: &Store,
    spec: &JourneySpec,
    outcomes: &mut [StepOutcome],
) -> Result<Node> {
    let journey = resolve_validation(store, &spec.journey, true)?;
    let mut first_fail_idx: Option<usize> = None;
    let mut intent_outcomes: BTreeMap<String, (InspectionStatus, Vec<String>)> = BTreeMap::new();
    for (idx, outcome) in outcomes.iter_mut().enumerate() {
        match store.resolve_node(&outcome.intent, Some(NodeType::Intent)) {
            Ok(intent) => {
                let entry = intent_outcomes
                    .entry(intent.id)
                    .or_insert_with(|| (InspectionStatus::Passing, Vec::new()));
                if !outcome.passed {
                    entry.0 = InspectionStatus::Failing;
                }
                entry
                    .1
                    .push(format!("{}: {}", outcome.name, outcome.detail));
            }
            Err(e) => {
                outcome.passed = false;
                outcome.detail = format!("unresolved step intent '{}': {e}", outcome.intent);
            }
        }
        if !outcome.passed && first_fail_idx.is_none() {
            first_fail_idx = Some(idx);
        }
    }
    // Several protocol steps may exercise the same behavior. Record one
    // aggregate verdict per intent instead of repeatedly cycling the same edge
    // through each step's evidence; otherwise a logically identical rerun
    // dirties updated_at even when the final evidence is byte-identical.
    for (intent_id, (status, details)) in intent_outcomes {
        let evidence = details.join(" | ");
        // loom RAN these steps, so the verdict is anchored to the run — not to
        // a sentence about it. `assertions` counts the content checks the spec
        // actually made (`exit_code` deliberately excluded: an exit code proves
        // liveness, not behavior), and `covered` is the code the proof
        // exercised, so the proof expires when that code moves.
        let assertions: usize = spec
            .steps
            .iter()
            .filter(|st| st.intent == *intent_name(store, &intent_id))
            .map(|st| {
                st.expect.body.len()
                    + st.expect.exists.len()
                    + st.expect.stdout_contains.len()
                    + st.expect.stderr_contains.len()
            })
            .sum();
        let covered = crate::runner::files_grounding(store, &intent_id)?;
        let transcript: String = outcomes
            .iter()
            .map(|o| o.transcript.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let run = crate::runner::record(
            store.root(),
            crate::model::RunProducer::Journey,
            &format!("loom journey run '{}'", spec.journey),
            &covered,
            assertions,
            i64::from(status != InspectionStatus::Passing),
            transcript.as_bytes(),
            &[],
            0,
        );
        for e in store.edges_with(
            Some(EdgeKind::Validates),
            Some(&journey.id),
            Some(&intent_id),
        )? {
            store.assert_fact(
                crate::store::Assertion::new(
                    crate::store::Subject::Edge(e.id.clone()),
                    crate::model::Claim::Verdict,
                    status.as_str(),
                    "journey",
                )
                .criterion("journey steps")
                .confidence(1.0)
                .cited(crate::evidence::cite(store.root(), &evidence)?)
                .observed(run.clone()),
            )?;
        }
    }
    if let Some(idx) = first_fail_idx {
        stale_unreached_passing_steps(store, spec, &journey.id, idx + 1)?;
    }
    let all_pass = !outcomes.is_empty() && outcomes.iter().all(|o| o.passed);
    store.set_node_status(&journey.id, if all_pass { "passed" } else { "failed" })?;
    Ok(journey)
}

fn run_cli_step(
    step: &Step,
    vars: &mut BTreeMap<String, String>,
    cwd: Option<&Path>,
    diagnose_style: bool,
) -> Result<StepOutcome> {
    let command = interpolate(&step.run, vars);
    // The same default the runner uses, so a journey step and a validation obey
    // one timeout policy rather than contradicting each other.
    let timeout_secs = crate::runner::DEFAULT_TIMEOUT_SECS;
    let output = execute_cli_command(step, &command, cwd, timeout_secs)?;
    // A timeout is a failure to OBSERVE, not a failing behavior — recording it
    // as a failed step would attribute breakage to code loom never got to watch.
    // Surface it as an error so the journey run stops honestly instead.
    let Some(output) = output else {
        anyhow::bail!(
            "step '{}': `{command}` timed out after {timeout_secs}s — could not observe",
            step.name
        );
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let code = output.status.code().unwrap_or(-1) as i32;
    let want = step.expect.exit_code.unwrap_or(0);
    if code != want {
        return Ok(failed_step(
            step,
            if diagnose_style {
                format!(
                    "exit {code} (want {want}); stderr: {}",
                    truncate(&stderr, 200)
                )
            } else {
                format!("exit {code} (want {want})")
            },
        ));
    }
    if let Some(detail) = cli_text_failure(step, &stdout, &stderr, vars) {
        return Ok(failed_step(step, detail));
    }
    let stdout_json = serde_json::from_str::<serde_json::Value>(stdout.trim()).ok();
    if let Some(detail) = apply_cli_json_contract(step, stdout_json.as_ref(), vars) {
        return Ok(failed_step(step, detail));
    }
    // Always expose raw streams for later interpolation when useful.
    vars.insert("stdout".into(), stdout.clone());
    vars.insert("stderr".into(), stderr.clone());
    Ok(StepOutcome {
        name: step.name.clone(),
        intent: step.intent.clone(),
        passed: true,
        // Persist the authored template, not runtime substitutions such as a
        // captured random id. The exit code is the observed fact; volatile
        // values would make an identical passing journey dirty the export and
        // can also disclose data that only needed to flow between steps.
        detail: format!("`{}` exit {code}", step.run),
        transcript: format!("stdout:\n{stdout}\nstderr:\n{stderr}\nexit:{code}"),
        latency_ms: 0,
    })
}

fn execute_cli_command(
    step: &Step,
    command: &str,
    cwd: Option<&Path>,
    timeout_secs: u64,
) -> Result<Option<crate::subprocess::Captured>> {
    let dir = cwd.unwrap_or_else(|| Path::new("."));
    crate::subprocess::run(command, dir, Duration::from_secs(timeout_secs))
        .with_context(|| format!("step '{}': running `{command}`", step.name))
}

fn cli_text_failure(
    step: &Step,
    stdout: &str,
    stderr: &str,
    vars: &BTreeMap<String, String>,
) -> Option<String> {
    for needle in &step.expect.stdout_contains {
        let expected = interpolate(needle, vars);
        if !stdout.contains(&expected) {
            return Some(format!("stdout missing `{expected}`"));
        }
    }
    for needle in &step.expect.stderr_contains {
        let expected = interpolate(needle, vars);
        if !stderr.contains(&expected) {
            return Some(format!("stderr missing `{expected}`"));
        }
    }
    None
}

fn apply_cli_json_contract(
    step: &Step,
    body: Option<&serde_json::Value>,
    vars: &mut BTreeMap<String, String>,
) -> Option<String> {
    let needs_json =
        !step.expect.body.is_empty() || !step.expect.exists.is_empty() || !step.capture.is_empty();
    let Some(body) = body else {
        return needs_json.then(|| {
            "CLI step expects JSON stdout for body/exists/capture, but stdout was not JSON".into()
        });
    };
    for path in &step.expect.exists {
        if jsonpath(body, path).is_none() {
            return Some(format!("stdout JSON missing `{path}`"));
        }
    }
    for (path, expected) in &step.expect.body {
        let Some(got) = jsonpath(body, path) else {
            return Some(format!("stdout JSON missing `{path}`"));
        };
        let expected = interpolate_json(expected, vars);
        if got != expected {
            return Some(format!("stdout JSON `{path}`: got {got}, want {expected}"));
        }
    }
    for (name, path) in &step.capture {
        let Some(value) = jsonpath(body, path) else {
            return Some(format!(
                "capture `{name}` from `{path}` missing in stdout JSON"
            ));
        };
        vars.insert(name.clone(), value_to_string(&value));
    }
    None
}

fn failed_step(step: &Step, detail: impl Into<String>) -> StepOutcome {
    StepOutcome {
        name: step.name.clone(),
        intent: step.intent.clone(),
        passed: false,
        detail: detail.into(),
        transcript: String::new(),
        latency_ms: 0,
    }
}

fn truncate(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        t.to_string()
    } else {
        let cut: String = t.chars().take(max).collect();
        format!("{cut}…")
    }
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
    // Keep the raw body in the baseline transcript even when no assertion
    // currently inspects it; a changed response is still operational drift.
    let body_text = resp.text().unwrap_or_default();
    let body_parse: Result<serde_json::Value, _> = serde_json::from_str(&body_text);
    let transcript = || format!("status:{status}\nbody:\n{body_text}");
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
            transcript: transcript(),
            latency_ms: 0,
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
                transcript: transcript(),
                latency_ms: 0,
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
                transcript: transcript(),
                latency_ms: 0,
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
                transcript: transcript(),
                latency_ms: 0,
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
        transcript: transcript(),
        latency_ms: 0,
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
    let has_http = spec.steps.iter().any(|s| !s.is_cli());
    let has_cli = spec.steps.iter().any(|s| s.is_cli());
    if has_http && spec.base.is_empty() {
        hints.push("no `base` url set — relative HTTP step urls will fail".into());
    }
    if has_cli {
        hints.push(
            "CLI steps run via `sh -c` in the graph root with the exclusive lock released — put the binary on PATH (or use a repo-relative path)"
                .into(),
        );
    }
    let mut blob = String::new();
    for s in &spec.steps {
        blob.push_str(&s.run);
        blob.push_str(&s.request.url);
        for v in s.request.headers.values() {
            blob.push_str(v);
        }
        for n in &s.expect.stdout_contains {
            blob.push_str(n);
        }
    }
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

    #[test]
    fn successful_cli_evidence_preserves_the_template_not_runtime_values() {
        let step = Step {
            name: "use captured id".into(),
            intent: "captured values remain ephemeral".into(),
            run: "true # {{ item_id }}".into(),
            request: Request::default(),
            expect: Expect::default(),
            capture: BTreeMap::new(),
        };
        let mut vars = BTreeMap::from([("item_id".into(), "random-runtime-id".into())]);

        let outcome = run_cli_step(&step, &mut vars, None, false).unwrap();

        assert!(outcome.passed);
        assert_eq!(outcome.detail, "`true # {{ item_id }}` exit 0");
        assert!(!outcome.detail.contains("random-runtime-id"));
    }
}
