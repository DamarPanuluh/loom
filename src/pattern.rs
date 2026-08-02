//! Strict Pattern body and the reusable applicability matcher.

use anyhow::{bail, Context};
use globset::{Glob, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::model::{Claim, EdgeKind, InspectionStatus, NodeType, TargetKind};
use crate::store::{Store, Subject};

/// Conservative packet budget. Omitted matches remain recoverable through the
/// exact lookup command, and excerpts disclose byte truncation explicitly.
pub const MAX_GUIDANCE_ITEMS: usize = 5;
pub const MAX_GUIDANCE_EXCERPT_BYTES: usize = 12 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PatternGuidance {
    pub offset: usize,
    pub matched: usize,
    pub included: usize,
    pub omitted: usize,
    pub items: Vec<PatternGuidanceItem>,
    pub lookup_command: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PatternGuidanceItem {
    pub name: String,
    pub rationale: String,
    pub when_to_use: String,
    pub when_not_to_use: String,
    pub path: String,
    pub locator: String,
    pub source_excerpt: String,
}

pub struct PatternView {
    pub node: crate::model::Node,
    pub health: &'static str,
    pub health_reason: String,
    pub exemplars: Vec<PatternGuidanceItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PatternBody {
    pub rationale: String,
    pub when_to_use: String,
    pub when_not_to_use: String,
    pub applicability: Applicability,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Applicability {
    #[serde(default)]
    pub path_globs: Vec<String>,
    #[serde(default)]
    pub intent_tags: Vec<String>,
}

impl PatternBody {
    pub fn parse(value: &serde_json::Value) -> crate::Result<Self> {
        let body: Self = serde_json::from_value(value.clone()).context("invalid Pattern body")?;
        for (field, prose) in [
            ("rationale", &body.rationale),
            ("when_to_use", &body.when_to_use),
            ("when_not_to_use", &body.when_not_to_use),
        ] {
            if crate::model::is_placeholder(prose) {
                bail!("Pattern {field} must contain nonempty semantic prose");
            }
        }
        let mut builder = GlobSetBuilder::new();
        for glob in &body.applicability.path_globs {
            if glob.trim().is_empty() || glob.starts_with('/') || glob.contains("..") {
                bail!("Pattern path glob must be canonical repo-relative: '{glob}'");
            }
            builder.add(
                Glob::new(glob).with_context(|| format!("invalid Pattern path glob '{glob}'"))?,
            );
        }
        builder.build().context("building Pattern path matcher")?;
        if body
            .applicability
            .intent_tags
            .iter()
            .any(|t| t.trim().is_empty())
        {
            bail!("Pattern intent tags must not be empty");
        }
        Ok(body)
    }

    /// OR within each family, AND across populated families. Empty selectors
    /// are manual-only and therefore never match lookup/routing.
    pub fn matches(&self, paths: &[String], tags: &[String]) -> crate::Result<bool> {
        let a = &self.applicability;
        if a.path_globs.is_empty() && a.intent_tags.is_empty() {
            return Ok(false);
        }
        let path_match = if a.path_globs.is_empty() {
            true
        } else {
            let mut builder = GlobSetBuilder::new();
            for glob in &a.path_globs {
                builder.add(Glob::new(glob)?);
            }
            let set = builder.build()?;
            paths
                .iter()
                .any(|p| !p.starts_with('/') && !p.contains("..") && set.is_match(p))
        };
        let tag_match = a.intent_tags.is_empty()
            || tags
                .iter()
                .any(|actual| a.intent_tags.iter().any(|wanted| wanted == actual));
        Ok(path_match && tag_match)
    }
}

/// Resolve all applicable, currently trusted Patterns. Both CLI lookup and
/// automatic packet enrichment call this function.
pub fn guidance(
    store: &Store,
    paths: &[String],
    tags: &[String],
) -> crate::Result<PatternGuidance> {
    guidance_page(store, paths, tags, 0)
}

pub fn guidance_page(
    store: &Store,
    paths: &[String],
    tags: &[String],
    offset: usize,
) -> crate::Result<PatternGuidance> {
    let mut matched = Vec::new();
    for node in store.list_nodes(Some(NodeType::Pattern), usize::MAX)? {
        let body = PatternBody::parse(&node.body)?;
        if body.matches(paths, tags)? {
            let view = inspect(store, &node)?;
            if view.health == "routable" {
                for mut exemplar in view.exemplars {
                    exemplar.name = node.name.clone();
                    matched.push(exemplar);
                }
            }
        }
    }
    matched.sort_by(|a, b| (&a.name, &a.path, &a.locator).cmp(&(&b.name, &b.path, &b.locator)));
    let matched_count = matched.len();
    let mut items = Vec::new();
    let mut bytes = 0;
    for mut item in matched.into_iter().skip(offset).take(MAX_GUIDANCE_ITEMS) {
        let remaining = MAX_GUIDANCE_EXCERPT_BYTES.saturating_sub(bytes);
        if remaining == 0 {
            break;
        }
        if item.source_excerpt.len() > remaining {
            let Some(truncated) = truncate_excerpt(&item.source_excerpt, remaining) else {
                break;
            };
            item.source_excerpt = truncated;
        }
        bytes += item.source_excerpt.len();
        items.push(item);
    }
    let next_offset = offset.saturating_add(items.len());
    Ok(PatternGuidance {
        offset,
        matched: matched_count,
        included: items.len(),
        omitted: matched_count.saturating_sub(next_offset),
        items,
        lookup_command: lookup_command(paths, tags, next_offset),
    })
}

fn truncate_excerpt(source: &str, budget: usize) -> Option<String> {
    if source.len() <= budget {
        return Some(source.to_string());
    }
    // Recompute the marker after choosing the prefix because the omitted byte
    // count affects marker width. Stop only when the complete rendered value is
    // within budget; tiny remainders omit the item rather than lying about the
    // byte cap.
    let mut end = budget.min(source.len());
    while end > 0 {
        while !source.is_char_boundary(end) {
            end -= 1;
        }
        let marker = format!(
            "\n…[{} bytes omitted by Pattern budget]…",
            source.len() - end
        );
        if end + marker.len() <= budget {
            return Some(format!("{}{}", &source[..end], marker));
        }
        if end == 0 {
            return None;
        }
        end -= 1;
    }
    None
}

pub fn lookup_command(paths: &[String], tags: &[String], offset: usize) -> String {
    let mut command = String::from("loom pattern lookup --json");
    for path in paths {
        command.push_str(&format!(" --path {}", shell_escape(path)));
    }
    for tag in tags {
        command.push_str(&format!(" --intent-tag {}", shell_escape(tag)));
    }
    command.push_str(&format!(" --offset {offset}"));
    command
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Derive health and actual live exemplar source without persisting either.
pub fn inspect(store: &Store, node: &crate::model::Node) -> crate::Result<PatternView> {
    let fail = |health, reason: String| {
        Ok(PatternView {
            node: node.clone(),
            health,
            health_reason: reason,
            exemplars: vec![],
        })
    };
    if node.status == "deprecated" {
        return fail("deprecated", "pattern is retired".into());
    }
    if store.ratification(&node.id)? != "ratified" {
        return fail("draft", "pattern is not directly human-ratified".into());
    }
    let body = PatternBody::parse(&node.body)?;
    if body.applicability.path_globs.is_empty() && body.applicability.intent_tags.is_empty() {
        return fail("manual_only", "applicability has no selectors".into());
    }
    let edges = store.edges_with(Some(EdgeKind::Exemplar), Some(&node.id), None)?;
    if edges.is_empty() {
        return fail("ungrounded", "pattern has no exemplars".into());
    }
    let floor = crate::policy::load(store)?.review_confidence_floor;
    let mut exemplars = Vec::new();
    for edge in edges {
        if edge.status != InspectionStatus::Passing {
            return fail("unreviewed", format!("exemplar {} is not passing", edge.id));
        }
        let fact = match store.fact(&Subject::Edge(edge.id.clone()), Claim::Verdict)? {
            Some(f) => f,
            None => return fail("unreviewed", format!("exemplar {} has no verdict", edge.id)),
        };
        if fact.fact.state != InspectionStatus::Passing.as_str() {
            return fail(
                "unreviewed",
                format!("exemplar {} fact is not passing", edge.id),
            );
        }
        if !fact.fact.verification.counts() || fact.fact.confidence < floor {
            return fail(
                "stale",
                format!("exemplar {} is stale or below confidence floor", edge.id),
            );
        }
        let file = store
            .get_node(&edge.to_id)?
            .ok_or_else(|| anyhow::anyhow!("missing exemplar endpoint"))?;
        let locator = store
            .get_facet(&edge.id, TargetKind::Edge, "locator")?
            .unwrap_or_default();
        let resolution =
            match crate::runner::resolve_locator(store.root(), &file.name, Some(&locator)) {
                Some(r) if r.run.exit_code == 0 && r.match_count == 1 => r,
                _ => {
                    return fail(
                        "stale",
                        format!("exemplar {} locator is missing or non-unique", edge.id),
                    )
                }
            };
        let recorded = fact.evidence.iter().find_map(|row| match &row.payload {
            crate::evidence::Evidence::Run(run)
                if run.producer == crate::model::RunProducer::Locator =>
            {
                Some(run.stdout_hash.as_str())
            }
            _ => None,
        });
        if recorded != Some(resolution.run.stdout_hash.as_str()) {
            return fail(
                "stale",
                format!("exemplar {} live fingerprint differs", edge.id),
            );
        }
        exemplars.push(PatternGuidanceItem {
            name: node.name.clone(),
            rationale: body.rationale.clone(),
            when_to_use: body.when_to_use.clone(),
            when_not_to_use: body.when_not_to_use.clone(),
            path: file.name,
            locator,
            source_excerpt: resolution.source_text.unwrap_or_default(),
        });
    }
    Ok(PatternView {
        node: node.clone(),
        health: "routable",
        health_reason: "all trust conditions hold against the live working tree".into(),
        exemplars,
    })
}

pub fn validate_node_body(
    kind: crate::model::NodeType,
    body: &serde_json::Value,
) -> crate::Result<()> {
    if kind == crate::model::NodeType::Pattern {
        PatternBody::parse(body)?;
    }
    if kind == crate::model::NodeType::TaskRecord {
        crate::research::validate_task_body(body)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(paths: &[&str], tags: &[&str]) -> PatternBody {
        PatternBody {
            rationale: "Keeps repository behavior consistent.".into(),
            when_to_use: "Use at this repository boundary.".into(),
            when_not_to_use: "Avoid outside that boundary.".into(),
            applicability: Applicability {
                path_globs: paths.iter().map(|s| s.to_string()).collect(),
                intent_tags: tags.iter().map(|s| s.to_string()).collect(),
            },
        }
    }

    #[test]
    fn strict_body_rejects_unknown_and_placeholder_fields() {
        let mut value = serde_json::to_value(body(&[], &[])).unwrap();
        value["snippet"] = serde_json::json!("do not persist me");
        assert!(PatternBody::parse(&value).is_err());
        let mut value = serde_json::to_value(body(&[], &[])).unwrap();
        value["rationale"] = serde_json::json!("");
        assert!(PatternBody::parse(&value).is_err());
    }

    #[test]
    fn matcher_is_or_within_and_across_families_and_manual_only() {
        let selected = body(&["src/**", "tests/**"], &["api", "db"]);
        assert!(selected
            .matches(&["src/x.rs".into()], &["db".into()])
            .unwrap());
        assert!(!selected
            .matches(&["docs/x.md".into()], &["db".into()])
            .unwrap());
        assert!(!selected
            .matches(&["src/x.rs".into()], &["database".into()])
            .unwrap());
        assert!(!body(&[], &[])
            .matches(&["src/x.rs".into()], &["db".into()])
            .unwrap());
    }
}
