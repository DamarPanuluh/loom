//! Source corpus coverage: structured docs are an enumerated source of truth.
//!
//! This is deliberately conservative. Programmatic extraction only treats
//! explicit requirement-like IDs as hard denominators. Docs without such IDs
//! remain visible as "unstructured" so an LLM/human can triage them through the
//! inbox instead of loom pretending a parser understood arbitrary prose.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::Serialize;

use crate::db::queries::comprehensiveness::is_doc_file;
use crate::db::queries::QuerySnapshot;
use crate::types::InboxItem;

pub const DEFAULT_CORPUS_PREFIXES: &[&str] = &["US", "E", "REQ", "NFR", "INV", "ADR"];
pub const AUTO_SEED_PREFIXES: &[&str] = &["US", "E"];

#[derive(Debug, Clone, Serialize)]
pub struct CorpusId {
    pub id: String,
    pub prefix: String,
    pub path: String,
    pub line: usize,
    pub occurrences: usize,
    pub modeled: bool,
    pub resolved: bool,
    pub inbox_status: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SourceCorpusCoverage {
    pub doc_files: usize,
    pub structured_doc_files: usize,
    pub unstructured_doc_files: usize,
    pub ids_total: usize,
    pub modeled: usize,
    pub resolved: usize,
    pub unresolved: usize,
    pub by_prefix: BTreeMap<String, usize>,
    pub ids: Vec<CorpusId>,
    pub examples: Vec<CorpusId>,
    pub warning: String,
}

impl SourceCorpusCoverage {
    pub fn has_signal(&self) -> bool {
        self.doc_files > 0
    }
}

pub fn source_corpus_coverage(
    root: &Path,
    snapshot: &QuerySnapshot,
    inbox: &[InboxItem],
) -> SourceCorpusCoverage {
    source_corpus_coverage_with_prefixes(root, snapshot, inbox, DEFAULT_CORPUS_PREFIXES)
}

pub fn source_corpus_coverage_with_prefixes(
    root: &Path,
    snapshot: &QuerySnapshot,
    inbox: &[InboxItem],
    prefixes: &[&str],
) -> SourceCorpusCoverage {
    let mut doc_files = Vec::new();
    if let Ok(files) = crate::repo::walk_files(root) {
        doc_files = files.into_iter().filter(|p| is_doc_file(p)).collect();
    }
    doc_files.sort();

    let mut by_id: BTreeMap<String, CorpusId> = BTreeMap::new();
    let mut structured_paths = std::collections::BTreeSet::new();
    for path in &doc_files {
        let Ok(content) = std::fs::read_to_string(root.join(path)) else {
            continue;
        };
        for (line_no, line) in unfenced_lines(&content) {
            for id in extract_ids(line, prefixes) {
                structured_paths.insert(path.clone());
                let prefix = id.split_once('-').map(|(p, _)| p).unwrap_or("").to_string();
                by_id
                    .entry(id.clone())
                    .and_modify(|item| item.occurrences += 1)
                    .or_insert(CorpusId {
                        id,
                        prefix,
                        path: path.clone(),
                        line: line_no,
                        occurrences: 1,
                        modeled: false,
                        resolved: false,
                        inbox_status: String::new(),
                    });
            }
        }
    }

    let modeled_haystacks: Vec<String> = snapshot
        .intents
        .iter()
        .filter(|intent| intent.status != "deprecated")
        .map(|intent| {
            format!(
                "{}\n{}\n{}\n{}",
                intent.name,
                intent.description,
                intent.criterion,
                intent.source_refs.join("\n")
            )
        })
        .collect();
    let inbox_by_id = corpus_inbox_index(inbox);

    let mut ids = Vec::new();
    let mut by_prefix = BTreeMap::new();
    let mut modeled = 0usize;
    let mut resolved = 0usize;
    for mut item in by_id.into_values() {
        item.modeled = modeled_haystacks
            .iter()
            .any(|haystack| contains_id_token(haystack, &item.id));
        if let Some(status) = inbox_by_id.get(item.id.as_str()) {
            item.inbox_status = status.clone();
            item.resolved = matches!(
                status.as_str(),
                "routed" | "rejected" | "duplicate" | "deferred"
            );
        }
        if item.modeled {
            modeled += 1;
        }
        if item.resolved {
            resolved += 1;
        }
        *by_prefix.entry(item.prefix.clone()).or_insert(0) += 1;
        ids.push(item);
    }
    let unresolved = ids
        .iter()
        .filter(|item| !item.modeled && !item.resolved)
        .count();
    let examples = ids
        .iter()
        .filter(|item| !item.modeled && !item.resolved)
        .take(10)
        .cloned()
        .collect();
    let structured_doc_files = structured_paths.len();
    let unstructured_doc_files = doc_files.len().saturating_sub(structured_doc_files);
    let warning = if unresolved > 0 {
        format!("{unresolved} documented requirement ID(s) are not modeled or resolved")
    } else if !doc_files.is_empty() && ids.is_empty() {
        "docs exist but no structured requirement IDs were detected; corpus completeness is unknown — use `loom seed --inbox` for LLM triage".to_string()
    } else if unstructured_doc_files > 0 {
        format!("{unstructured_doc_files} doc file(s) have no structured IDs; corpus completeness beyond explicit IDs is unknown")
    } else {
        String::new()
    };

    SourceCorpusCoverage {
        doc_files: doc_files.len(),
        structured_doc_files,
        unstructured_doc_files,
        ids_total: ids.len(),
        modeled,
        resolved,
        unresolved,
        by_prefix,
        ids,
        examples,
        warning,
    }
}

pub fn extract_ids(line: &str, prefixes: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if !bytes[i].is_ascii_uppercase() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_uppercase() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'-' {
            continue;
        }
        let prefix = &line[start..i];
        if !prefixes.iter().any(|p| *p == prefix) {
            i += 1;
            continue;
        }
        i += 1; // '-'
        let id_tail_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
            i += 1;
        }
        if i == id_tail_start || !bytes[id_tail_start].is_ascii_digit() {
            continue;
        }
        out.push(line[start..i].to_string());
    }
    out.sort();
    out.dedup();
    out
}

pub fn contains_id_token(haystack: &str, id: &str) -> bool {
    for (idx, _) in haystack.match_indices(id) {
        let before = haystack[..idx].chars().next_back();
        let after = haystack[idx + id.len()..].chars().next();
        if before.is_none_or(|c| !id_char(c)) && after.is_none_or(|c| !id_char(c)) {
            return true;
        }
    }
    false
}

fn id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

fn unfenced_lines(content: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            out.push((idx + 1, line));
        }
    }
    out
}

fn corpus_inbox_index(inbox: &[InboxItem]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for item in inbox {
        let text = format!(
            "{}\n{}\n{}",
            item.raw_text, item.normalized_claim, item.route_target_id
        );
        for id in extract_ids(&text, DEFAULT_CORPUS_PREFIXES) {
            if text.contains("corpus:") || item.kind == "docs_gap" {
                out.insert(id, item.status.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_default_requirement_ids() {
        assert_eq!(
            extract_ids(
                "US-12 and E-3 plus ADR-0004, but XX-1 no",
                DEFAULT_CORPUS_PREFIXES
            ),
            vec!["ADR-0004", "E-3", "US-12"]
        );
    }

    #[test]
    fn token_match_does_not_match_prefix_of_longer_id() {
        assert!(contains_id_token("implements US-12", "US-12"));
        assert!(!contains_id_token("implements US-123", "US-12"));
    }
}
