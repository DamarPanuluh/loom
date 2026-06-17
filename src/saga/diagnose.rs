//! Generic saga failure diagnosis.
//!
//! This layer explains the saga runner's structured outcomes. It deliberately
//! does not know Grid, app/person tables, or product-specific fix commands.
//! Repo-specific auth/state probes can sit above these generic diagnoses later.

use serde::Serialize;

use super::runner::{SagaRunReport, StepOutcome};
use super::spec::{BodyExpectation, SagaSpec, Step};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SagaDiagnosis {
    pub saga: String,
    pub passed: bool,
    pub total_steps: usize,
    pub executed: usize,
    pub steps: Vec<StepDiagnosis>,
    pub summary: DiagnosisSummary,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StepDiagnosis {
    pub step: usize,
    pub name: String,
    pub intent: String,
    pub outcome: String,
    pub method: String,
    pub url: String,
    pub http_status: Option<u16>,
    pub detail: String,
    pub root_cause: Option<RootCause>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RootCause {
    pub kind: String,
    pub title: String,
    pub fields: Vec<DiagnosisField>,
    pub fix: String,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiagnosisField {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct DiagnosisSummary {
    pub diagnosed_sagas: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped_steps: usize,
    pub by_kind: Vec<DiagnosisCount>,
    pub suggested_order: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiagnosisCount {
    pub kind: String,
    pub count: usize,
}

pub fn diagnose_report(spec: &SagaSpec, report: &SagaRunReport) -> SagaDiagnosis {
    let mut steps = Vec::new();
    for outcome in &report.outcomes {
        let spec_step = spec.steps.get(outcome.step.saturating_sub(1));
        steps.push(diagnose_outcome(outcome, spec_step));
    }

    let failed_step = report.failure().map(|outcome| outcome.step);
    for idx in report.executed..report.total_steps {
        let step = &spec.steps[idx];
        steps.push(StepDiagnosis {
            step: idx + 1,
            name: step.name.clone(),
            intent: step.intent.clone(),
            outcome: "skipped".to_string(),
            method: step.request.method.to_uppercase(),
            url: step.request.url.clone(),
            http_status: None,
            detail: failed_step
                .map(|n| format!("skipped because step {n} failed"))
                .unwrap_or_else(|| "skipped".to_string()),
            root_cause: Some(RootCause {
                kind: "state_dependency".to_string(),
                title: "Skipped after earlier failed step".to_string(),
                fields: failed_step
                    .map(|n| {
                        vec![field(
                            "depends_on",
                            format!("step {n} must pass before this step can run"),
                        )]
                    })
                    .unwrap_or_default(),
                fix: "Fix the first failing step, then rerun the saga.".to_string(),
                confidence: "high".to_string(),
            }),
        });
    }

    let summary = summarize(&steps, report.passed);
    SagaDiagnosis {
        saga: report.saga.clone(),
        passed: report.passed,
        total_steps: report.total_steps,
        executed: report.executed,
        steps,
        summary,
    }
}

pub fn diagnose_missing_env(
    spec: &SagaSpec,
    missing: &[String],
    invocation: String,
) -> SagaDiagnosis {
    let mut steps = Vec::new();
    for (idx, step) in spec.steps.iter().enumerate() {
        steps.push(StepDiagnosis {
            step: idx + 1,
            name: step.name.clone(),
            intent: step.intent.clone(),
            outcome: if idx == 0 { "failed" } else { "skipped" }.to_string(),
            method: step.request.method.to_uppercase(),
            url: step.request.url.clone(),
            http_status: None,
            detail: if idx == 0 {
                format!("missing environment value(s): {}", missing.join(", "))
            } else {
                "skipped because environment prerequisites are missing".to_string()
            },
            root_cause: Some(if idx == 0 {
                RootCause {
                    kind: "env_var_missing".to_string(),
                    title: "Environment variable missing".to_string(),
                    fields: vec![field("missing", missing.join(", "))],
                    fix: format!("Run with the required environment values: {invocation}"),
                    confidence: "high".to_string(),
                }
            } else {
                RootCause {
                    kind: "state_dependency".to_string(),
                    title: "Skipped after environment prerequisite failed".to_string(),
                    fields: vec![field("depends_on", "environment values must be supplied")],
                    fix: format!("Run with the required environment values: {invocation}"),
                    confidence: "high".to_string(),
                }
            }),
        });
    }
    let summary = summarize(&steps, false);
    SagaDiagnosis {
        saga: spec.saga.clone(),
        passed: false,
        total_steps: spec.steps.len(),
        executed: 0,
        steps,
        summary,
    }
}

fn diagnose_outcome(outcome: &StepOutcome, spec_step: Option<&Step>) -> StepDiagnosis {
    let root_cause = if outcome.passed {
        None
    } else {
        Some(classify_failure(outcome, spec_step))
    };
    StepDiagnosis {
        step: outcome.step,
        name: outcome.name.clone(),
        intent: outcome.intent.clone(),
        outcome: if outcome.passed { "passed" } else { "failed" }.to_string(),
        method: outcome.method.clone(),
        url: outcome.url.clone(),
        http_status: outcome.status,
        detail: outcome.detail.clone(),
        root_cause,
    }
}

fn classify_failure(outcome: &StepOutcome, spec_step: Option<&Step>) -> RootCause {
    if let Some(root) = template_not_expanded(outcome, spec_step) {
        return root;
    }

    let detail = outcome.detail.as_str();
    if let Some(name) = extract_between(detail, "Template references '{{ env.", " }}'") {
        return RootCause {
            kind: "env_var_missing".to_string(),
            title: "Environment variable missing".to_string(),
            fields: vec![field("missing", name)],
            fix: "Set the missing environment variable, then rerun the saga.".to_string(),
            confidence: "high".to_string(),
        };
    }
    if detail.contains("but no such variable") {
        return RootCause {
            kind: "template_variable_missing".to_string(),
            title: "Template variable missing".to_string(),
            fields: vec![field("detail", detail)],
            fix: "Create the variable in `vars:` or capture it from an earlier passing step."
                .to_string(),
            confidence: "high".to_string(),
        };
    }
    if detail.contains("relative url") && detail.contains("no `base:`") {
        return RootCause {
            kind: "missing_base_url".to_string(),
            title: "Relative URL needs saga base".to_string(),
            fields: vec![field("url", outcome.url.clone())],
            fix: "Add `base:` to the saga spec or make this step URL absolute.".to_string(),
            confidence: "high".to_string(),
        };
    }
    if let Some(status) = outcome.status {
        match status {
            401 => {
                return RootCause {
                    kind: "auth_unauthorized".to_string(),
                    title: "Unauthorized request".to_string(),
                    fields: vec![field("status", "401 Unauthorized")],
                    fix: "Check the auth header, token validity, actor identity, and any repo-specific auth binding.".to_string(),
                    confidence: "medium".to_string(),
                };
            }
            403 => {
                return RootCause {
                    kind: "auth_forbidden".to_string(),
                    title: "Forbidden request".to_string(),
                    fields: vec![field("status", "403 Forbidden")],
                    fix: "Check token scopes/roles and the endpoint's declared authorization requirements.".to_string(),
                    confidence: "medium".to_string(),
                };
            }
            404 => {
                return RootCause {
                    kind: "resource_not_found".to_string(),
                    title: "Resource not found".to_string(),
                    fields: vec![field("status", "404 Not Found"), field("url", &outcome.url)],
                    fix: "Check the identifier in the URL and whether an earlier step was supposed to create it.".to_string(),
                    confidence: "medium".to_string(),
                };
            }
            _ => {}
        }
    }
    if detail.contains("expected status") {
        return RootCause {
            kind: "status_expectation".to_string(),
            title: "Status expectation mismatch".to_string(),
            fields: vec![
                field(
                    "status",
                    outcome.status.map(|s| s.to_string()).unwrap_or_default(),
                ),
                field("detail", detail),
            ],
            fix: "Confirm whether the endpoint changed or the saga expectation is stale."
                .to_string(),
            confidence: "high".to_string(),
        };
    }
    if detail.contains("request failed:") {
        return RootCause {
            kind: "request_failed".to_string(),
            title: "Request could not reach the target".to_string(),
            fields: vec![field("detail", detail)],
            fix: "Start the target service, fix BASE_URL, or resolve the network/TLS error, then rerun.".to_string(),
            confidence: "high".to_string(),
        };
    }
    if detail.contains("response body is not JSON") {
        return RootCause {
            kind: "response_not_json".to_string(),
            title: "Response body is not JSON".to_string(),
            fields: vec![field("detail", detail)],
            fix: "Confirm the endpoint response format or remove JSON body expectations/captures."
                .to_string(),
            confidence: "high".to_string(),
        };
    }
    if detail.contains("matched nothing") {
        return RootCause {
            kind: "resource_or_body_shape_missing".to_string(),
            title: "Expected body field was missing".to_string(),
            fields: vec![field("detail", detail)],
            fix: "Check whether the endpoint response shape changed or earlier state was not created.".to_string(),
            confidence: "medium".to_string(),
        };
    }
    if detail.contains("body ") {
        return RootCause {
            kind: "body_mismatch".to_string(),
            title: "Body expectation mismatch".to_string(),
            fields: vec![field("detail", detail)],
            fix: "Compare the endpoint response to the saga expectation; update the endpoint or the spec.".to_string(),
            confidence: "high".to_string(),
        };
    }
    RootCause {
        kind: "unknown_failure".to_string(),
        title: "Unclassified saga failure".to_string(),
        fields: vec![field("detail", detail)],
        fix: "Inspect the failing request/response and add a more specific diagnosis rule if this recurs.".to_string(),
        confidence: "low".to_string(),
    }
}

fn template_not_expanded(outcome: &StepOutcome, spec_step: Option<&Step>) -> Option<RootCause> {
    let step = spec_step?;
    let mut fields = Vec::new();
    for (path, expectation) in &step.expect.body {
        for template in expectation_templates(expectation) {
            if !template.contains("{{") {
                continue;
            }
            fields.push(field("template", template.clone()));
            fields.push(field("in_field", format!("expect.body.{path}")));
            if let Some(env_name) = template
                .split("{{")
                .nth(1)
                .and_then(|rest| rest.split("}}").next())
                .map(str::trim)
                .and_then(|name| name.strip_prefix("env."))
            {
                if let Ok(value) = std::env::var(env_name) {
                    fields.push(field("env_value", value));
                }
            }
        }
    }
    if fields.is_empty() || !outcome.detail.contains("{{") {
        return None;
    }
    Some(RootCause {
        kind: "template_not_expanded".to_string(),
        title: "Template not expanded in expectation".to_string(),
        fields,
        fix: "Bug or unsupported feature: expectation templates are still literal. Report to loom dev or avoid templates in `expect.body`.".to_string(),
        confidence: "high".to_string(),
    })
}

fn expectation_templates(expectation: &BodyExpectation) -> Vec<String> {
    match expectation {
        BodyExpectation::Exists { .. } => Vec::new(),
        BodyExpectation::Contains { contains } => vec![contains.clone()],
        BodyExpectation::Equals(value) => string_leaves(value),
    }
}

fn string_leaves(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(items) => items.iter().flat_map(string_leaves).collect(),
        serde_json::Value::Object(map) => map.values().flat_map(string_leaves).collect(),
        _ => Vec::new(),
    }
}

fn summarize(steps: &[StepDiagnosis], passed: bool) -> DiagnosisSummary {
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut suggested_order = Vec::new();
    let mut skipped = 0;
    for step in steps {
        if step.outcome == "skipped" {
            skipped += 1;
        }
        if let Some(root) = &step.root_cause {
            *counts.entry(root.kind.clone()).or_insert(0) += 1;
            if step.outcome == "failed" {
                suggested_order.push(root.kind.clone());
            }
        }
    }
    suggested_order.dedup();
    DiagnosisSummary {
        diagnosed_sagas: 1,
        passed: usize::from(passed),
        failed: usize::from(!passed),
        skipped_steps: skipped,
        by_kind: counts
            .into_iter()
            .map(|(kind, count)| DiagnosisCount { kind, count })
            .collect(),
        suggested_order,
    }
}

fn field(name: impl Into<String>, value: impl Into<String>) -> DiagnosisField {
    DiagnosisField {
        name: name.into(),
        value: value.into(),
    }
}

fn extract_between(haystack: &str, before: &str, after: &str) -> Option<String> {
    let rest = haystack.split_once(before)?.1;
    let value = rest.split_once(after)?.0;
    Some(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(yaml: &str) -> SagaSpec {
        crate::saga::spec::load_spec(yaml, "test").unwrap()
    }

    #[test]
    fn status_failure_is_classified_before_skipped_steps() {
        let spec = spec(
            r#"
saga: auth-flow
steps:
  - name: write
    intent: write
    request: { method: POST, url: https://example.test/write }
    expect: { status: 200 }
  - name: read
    intent: read
    request: { method: GET, url: https://example.test/read }
"#,
        );
        let report = SagaRunReport {
            saga: "auth-flow".into(),
            passed: false,
            total_steps: 2,
            executed: 1,
            outcomes: vec![StepOutcome {
                step: 1,
                name: "write".into(),
                intent: "write".into(),
                method: "POST".into(),
                url: "https://example.test/write".into(),
                status: Some(403),
                passed: false,
                detail: "expected status 200, got 403".into(),
                captured: Default::default(),
            }],
        };
        let diagnosis = diagnose_report(&spec, &report);
        assert_eq!(
            diagnosis.steps[0].root_cause.as_ref().unwrap().kind,
            "auth_forbidden"
        );
        assert_eq!(diagnosis.steps[1].outcome, "skipped");
        assert_eq!(
            diagnosis.steps[1].root_cause.as_ref().unwrap().kind,
            "state_dependency"
        );
    }

    #[test]
    fn expect_body_template_mismatch_is_engine_bug() {
        std::env::set_var("LOOM_DIAGNOSE_EXPECT_TEMPLATE", "app_marketplace");
        let spec = spec(
            r#"
saga: template-flow
steps:
  - name: create
    intent: create
    request: { method: POST, url: https://example.test/create }
    expect:
      status: 201
      body:
        "$.issuer_app_id": "{{ env.LOOM_DIAGNOSE_EXPECT_TEMPLATE }}"
"#,
        );
        let report = SagaRunReport {
            saga: "template-flow".into(),
            passed: false,
            total_steps: 1,
            executed: 1,
            outcomes: vec![StepOutcome {
                step: 1,
                name: "create".into(),
                intent: "create".into(),
                method: "POST".into(),
                url: "https://example.test/create".into(),
                status: Some(201),
                passed: false,
                detail: r#"body $.issuer_app_id: expected "{{ env.LOOM_DIAGNOSE_EXPECT_TEMPLATE }}", got "app_marketplace""#.into(),
                captured: Default::default(),
            }],
        };
        let diagnosis = diagnose_report(&spec, &report);
        let root = diagnosis.steps[0].root_cause.as_ref().unwrap();
        assert_eq!(root.kind, "template_not_expanded");
        assert!(root.fields.iter().any(|f| f.name == "env_value"));
    }
}
