use serde::{Deserialize, Serialize};

pub const JOURNEY_LINT_REPORT_SCHEMA: &str = "loom.journey-lint/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JourneyLintSeverity {
    Blocking,
    Advisory,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct JourneyLintFinding {
    pub rule: String,
    pub severity: JourneyLintSeverity,
    pub journey_id: String,
    pub manifest_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assertion: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JourneyLintReport {
    pub schema: String,
    pub status: String,
    pub scanned: usize,
    pub blocking: usize,
    pub advisory: usize,
    pub findings: Vec<JourneyLintFinding>,
}

impl JourneyLintReport {
    pub fn new(scanned: usize, mut findings: Vec<JourneyLintFinding>) -> Self {
        findings.sort();
        let blocking = findings
            .iter()
            .filter(|f| f.severity == JourneyLintSeverity::Blocking)
            .count();
        let advisory = findings.len() - blocking;
        Self {
            schema: JOURNEY_LINT_REPORT_SCHEMA.into(),
            status: if blocking == 0 { "passed" } else { "blocked" }.into(),
            scanned,
            blocking,
            advisory,
            findings,
        }
    }
}
