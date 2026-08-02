//! Strict, portable provenance for host-performed external research.

use crate::model::{Node, NodeType};
use crate::Result;
use anyhow::{bail, Context};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    OfficialDocs,
    Standard,
    Regulation,
    Maintainer,
    Primary,
    Secondary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResearchSource {
    pub url: String,
    pub title: String,
    pub publisher: String,
    pub source_kind: SourceKind,
    pub retrieved_at: String,
    pub quote: String,
    pub quote_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresh_until: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResearchBody {
    pub kind: String,
    pub research_schema: u8,
    pub why_external: String,
    #[serde(default)]
    pub preferred_sources: Vec<String>,
    #[serde(default)]
    pub sources: Vec<ResearchSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion_fresh_until: Option<String>,
}

fn substantive(label: &str, value: &str, min: usize) -> Result<()> {
    let v = value.trim();
    let lower = v.to_ascii_lowercase();
    if v.len() < min
        || matches!(
            lower.as_str(),
            "todo" | "tbd" | "unknown" | "placeholder" | "n/a"
        )
        || v.contains('<')
        || v.contains('>')
    {
        bail!("{label} must be substantive and must not be a placeholder");
    }
    Ok(())
}

fn timestamp(label: &str, value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("{label} must be an RFC 3339 timestamp"))
        .map(|v| v.with_timezone(&Utc))
}

/// Deterministic fingerprint of the exact quoted bytes. This proves only that
/// stored quote text was not changed after capture; it is not a page hash.
pub fn quote_fingerprint(quote: &str) -> String {
    format!("fnv:{}", crate::store::fnv_hex_digest(&[quote]))
}

impl ResearchSource {
    pub fn validate(&self) -> Result<()> {
        self.validate_url()?;
        substantive("source title", &self.title, 3)?;
        substantive("source publisher", &self.publisher, 2)?;
        substantive("source quote", &self.quote, 20)?;
        self.validate_dates()?;
        self.validate_fingerprint()
    }

    fn validate_url(&self) -> Result<()> {
        let url = Url::parse(&self.url).context("source URL must be an absolute URL")?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            bail!("source URL must be an actual http/https page");
        }
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let path = url.path().to_ascii_lowercase();
        if host.contains("google.") && path.contains("/search")
            || host.contains("bing.com") && path.contains("/search")
            || host.contains("duckduckgo.com")
            || host.contains("search.yahoo.com")
        {
            bail!("search-result URLs are discovery only; record the actual page read");
        }
        Ok(())
    }

    fn validate_dates(&self) -> Result<()> {
        let retrieved = timestamp("retrieved_at", &self.retrieved_at)?;
        if let Some(v) = &self.published_at {
            if timestamp("published_at", v)? > retrieved {
                bail!("published_at must not be after retrieved_at");
            }
        }
        if let Some(v) = &self.fresh_until {
            if timestamp("fresh_until", v)? < retrieved {
                bail!("fresh_until must not be before retrieved_at");
            }
        }
        Ok(())
    }

    fn validate_fingerprint(&self) -> Result<()> {
        let Some(hex) = self.quote_fingerprint.strip_prefix("fnv:") else {
            bail!("quote_fingerprint must be fnv:<lowercase hex>");
        };
        if hex.is_empty()
            || !hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            bail!("quote_fingerprint must be fnv:<lowercase hex>");
        }
        if self.quote_fingerprint != quote_fingerprint(&self.quote) {
            bail!("quote_fingerprint does not match the exact quoted text");
        }
        Ok(())
    }
}

impl ResearchBody {
    pub fn parse(value: &serde_json::Value) -> Result<Self> {
        let body: Self =
            serde_json::from_value(value.clone()).context("invalid research task body")?;
        if body.kind != "research" || body.research_schema != 1 {
            bail!("governed research body requires kind=research and research_schema=1");
        }
        substantive("why_external", &body.why_external, 12)?;
        for v in &body.preferred_sources {
            substantive("preferred source", v, 3)?;
        }
        for source in &body.sources {
            source.validate()?;
        }
        if let Some(v) = &body.conclusion_fresh_until {
            timestamp("conclusion_fresh_until", v)?;
        }
        Ok(body)
    }

    pub fn validate_close(&self, now: DateTime<Utc>) -> Result<()> {
        if self.sources.is_empty() {
            bail!("research task cannot close without at least one actual page source");
        }
        if !self.sources.iter().any(|s| {
            s.fresh_until
                .as_ref()
                .is_none_or(|v| timestamp("fresh_until", v).is_ok_and(|until| until >= now))
        }) {
            bail!("research task cannot close without at least one currently usable source");
        }
        Ok(())
    }

    /// Bind the synthesized conclusion to the sources that are current at
    /// close. Historical stale sources remain useful provenance but do not
    /// shorten a conclusion supported by their current replacements.
    pub fn stamp_conclusion_freshness(&mut self, now: DateTime<Utc>) -> Result<()> {
        self.validate_close(now)?;
        let usable: Vec<_> = self
            .sources
            .iter()
            .filter(|source| {
                source
                    .fresh_until
                    .as_ref()
                    .is_none_or(|v| timestamp("fresh_until", v).is_ok_and(|until| until >= now))
            })
            .collect();
        self.conclusion_fresh_until = if usable.iter().any(|source| source.fresh_until.is_none()) {
            None
        } else {
            usable
                .iter()
                .filter_map(|source| source.fresh_until.as_ref())
                .min_by_key(|value| timestamp("fresh_until", value).ok())
                .cloned()
        };
        Ok(())
    }
}

pub fn is_governed(node: &Node) -> bool {
    node.node_type == NodeType::TaskRecord
        && node.body.get("kind").and_then(|v| v.as_str()) == Some("research")
        && node.body.get("research_schema").and_then(|v| v.as_u64()) == Some(1)
}

pub fn is_open_research(node: &Node) -> bool {
    is_governed(node) && matches!(node.status.as_str(), "proposed" | "active")
}

/// Validate a prospective persisted record and, when supplied, its mutation.
/// `historical` skips only today's freshness check for already-completed imports.
pub fn validate_record(
    old: Option<&Node>,
    new: &Node,
    now: DateTime<Utc>,
    historical: bool,
) -> Result<()> {
    if !is_governed(new) && !old.is_some_and(is_governed) {
        return Ok(());
    }
    if !is_governed(new) {
        bail!("research discriminator and brief fields are immutable");
    }
    validate_status(new)?;
    let body = ResearchBody::parse(&new.body)?;
    if new.status == "completed" {
        validate_completion(new, &body, now, historical)?;
    }
    if let Some(old) = old {
        validate_mutation(old, new, &body)?;
    }
    Ok(())
}

fn validate_status(node: &Node) -> Result<()> {
    if !matches!(
        node.status.as_str(),
        "proposed" | "active" | "completed" | "abandoned"
    ) {
        bail!("governed research status must be proposed, active, completed, or abandoned");
    }
    Ok(())
}

fn validate_completion(
    node: &Node,
    body: &ResearchBody,
    now: DateTime<Utc>,
    historical: bool,
) -> Result<()> {
    substantive("research result", &node.description, 12)?;
    if !historical {
        body.validate_close(now)?;
    } else if body.sources.is_empty() {
        bail!("completed research requires historical source provenance");
    }
    Ok(())
}

fn validate_mutation(old: &Node, new: &Node, body: &ResearchBody) -> Result<()> {
    if matches!(old.status.as_str(), "completed" | "abandoned") && old != new {
        bail!("completed or abandoned governed research is immutable except deletion");
    }
    if !matches!(
        (old.status.as_str(), new.status.as_str()),
        (
            "proposed",
            "proposed" | "active" | "completed" | "abandoned"
        ) | ("active", "active" | "completed" | "abandoned")
    ) {
        bail!(
            "invalid governed research transition {} -> {}",
            old.status,
            new.status
        );
    }
    let before = ResearchBody::parse(&old.body)?;
    if before.kind != body.kind
        || before.research_schema != body.research_schema
        || before.why_external != body.why_external
        || before.preferred_sources != body.preferred_sources
        || before.target_id != body.target_id
        || !body.sources.starts_with(&before.sources)
    {
        bail!("governed research brief is immutable and sources are append-only");
    }
    if before.conclusion_fresh_until.is_some()
        && before.conclusion_fresh_until != body.conclusion_fresh_until
    {
        bail!("governed research conclusion freshness is immutable once stamped");
    }
    Ok(())
}

pub fn validate_task_body(value: &serde_json::Value) -> Result<()> {
    if value.get("kind").and_then(|v| v.as_str()) == Some("research")
        && value.get("research_schema").and_then(|v| v.as_u64()) == Some(1)
    {
        ResearchBody::parse(value)?;
    }
    Ok(())
}
