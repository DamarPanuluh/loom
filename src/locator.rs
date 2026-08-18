//! Locator parsing and source-anchor resolution — one plane for what a
//! grounding locator names.
//!
//! Ordinary symbol resolution against a live file stays in [`crate::runner`].
//! Loom-issued `anchor:<id>` locators are different: their identity is an
//! exact source marker that must occur once across registered CodeFiles and be
//! attached to one smallest supported declaration/config entry. That stricter
//! repository-aware policy lives here so edge writes, surface acceptance,
//! doctor, impact, risk, and sync cannot disagree about the same anchor.

use crate::model::{EdgeKind, GroundingRole, Node, NodeType, TargetKind};
use crate::store::Store;
use crate::Result;
use anyhow::{bail, Context};
use std::path::Path;

pub const ANCHOR_LOCATOR_PREFIX: &str = "anchor:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedAnchor {
    pub id: String,
    pub locator: String,
    pub marker: String,
    pub file: String,
    pub entry_kind: String,
    pub entry_name: String,
    pub line_start: usize,
    pub line_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAnchor {
    pub id: String,
    pub locator: String,
    pub marker: String,
    pub file_id: String,
    pub file: String,
    pub entry_kind: String,
    pub entry_name: String,
    pub line_start: usize,
    pub line_end: usize,
    pub callable_symbol: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AnchorEntry {
    kind: String,
    name: String,
    line_start: usize,
    line_end: usize,
    callable_symbol: Option<String>,
}

#[derive(Debug, Clone)]
struct MarkerOccurrence {
    codefile: Node,
    marker: String,
    line: usize,
}

pub fn is_anchor_locator(locator: &str) -> bool {
    locator.trim().starts_with(ANCHOR_LOCATOR_PREFIX)
}

fn parse_anchor_id(locator: &str) -> Result<&str> {
    let locator = locator.trim();
    let Some(id) = locator.strip_prefix(ANCHOR_LOCATOR_PREFIX) else {
        bail!("locator '{locator}' is not an anchor locator");
    };
    crate::journey::validate_stable_id("source anchor", id)?;
    Ok(id)
}

/// Issue a deterministic source anchor without editing source or graph state.
///
/// `at_line` selects the smallest extracted declaration containing that line,
/// or the exact supported configuration entry on that line. If the selected
/// entry already has a valid marker, issuance is idempotent and returns it.
pub fn issue_anchor(store: &Store, codefile: &Node, at_line: usize) -> Result<IssuedAnchor> {
    if codefile.node_type != NodeType::CodeFile {
        bail!("source anchors can be issued only for a CodeFile");
    }
    if at_line == 0 {
        bail!("source anchor --at-line must be one-based and greater than zero");
    }
    let content = read_codefile(store, codefile)?;
    let prefix = marker_prefix(&codefile.name)?;
    let entry = selected_entry(&codefile.name, &content, at_line)?;
    let lines: Vec<&str> = content.lines().collect();

    if entry.line_start > 1 {
        let preceding = lines[entry.line_start - 2].trim();
        if preceding.contains("loom:anchor") {
            let (existing_prefix, existing_id) = parse_marker_line(preceding).ok_or_else(|| {
                anyhow::anyhow!(
                    "malformed source anchor marker at {}:{}",
                    codefile.name,
                    entry.line_start - 1
                )
            })?;
            if existing_prefix != prefix {
                bail!(
                    "source anchor '{}' uses comment marker '{}' but '{}' requires '{}'",
                    existing_id,
                    existing_prefix,
                    codefile.name,
                    prefix
                );
            }
            let locator = format!("{ANCHOR_LOCATOR_PREFIX}{existing_id}");
            let resolved = resolve_anchor(store, &locator)?;
            if resolved.file_id != codefile.id
                || resolved.line_start != entry.line_start
                || resolved.line_end != entry.line_end
            {
                bail!(
                    "source anchor '{}' is not attached to the selected entry in '{}'",
                    existing_id,
                    codefile.name
                );
            }
            return Ok(issued_from_resolved(resolved));
        }
    }

    let base = anchor_base_id(&codefile.name, &entry.name);
    let occurrences = scan_markers(store)?;
    let occupied: std::collections::BTreeSet<&str> = occurrences
        .iter()
        .map(|occurrence| occurrence_id(&occurrence.marker))
        .collect();
    let id = if !occupied.contains(base.as_str()) {
        base
    } else {
        let fingerprint = crate::artifact::fingerprint(&format!(
            "{}\0{}\0{}\0{}",
            codefile.id, entry.kind, entry.name, entry.line_start
        ));
        let stem = format!("{base}.{fingerprint}");
        if !occupied.contains(stem.as_str()) {
            stem
        } else {
            let mut ordinal = 2usize;
            loop {
                let candidate = format!("{stem}.{ordinal}");
                if !occupied.contains(candidate.as_str()) {
                    break candidate;
                }
                ordinal += 1;
            }
        }
    };
    let marker = format!("{prefix} loom:anchor {id}");
    Ok(IssuedAnchor {
        locator: format!("{ANCHOR_LOCATOR_PREFIX}{id}"),
        id,
        marker,
        file: codefile.name.clone(),
        entry_kind: entry.kind,
        entry_name: entry.name,
        line_start: entry.line_start,
        line_end: entry.line_end,
    })
}

/// Resolve one anchor globally across registered CodeFiles.
///
/// The marker must occur exactly once and attach to exactly one smallest
/// supported entry. The returned callable symbol is navigation data only; it
/// is deliberately absent from [`symbols`] and [`realizing_targets`], the
/// proof-facing projections.
pub fn resolve_anchor(store: &Store, locator: &str) -> Result<ResolvedAnchor> {
    let id = parse_anchor_id(locator)?;
    let wanted_marker = format!("loom:anchor {id}");
    let mut occurrences: Vec<MarkerOccurrence> = scan_markers(store)?
        .into_iter()
        .filter(|occurrence| occurrence.marker.ends_with(&wanted_marker))
        .collect();
    occurrences.sort_by(|left, right| {
        left.codefile
            .name
            .cmp(&right.codefile.name)
            .then(left.line.cmp(&right.line))
    });
    match occurrences.len() {
        0 => bail!("source anchor '{id}' is missing from registered CodeFiles"),
        1 => {}
        count => {
            let locations = occurrences
                .iter()
                .map(|occurrence| format!("{}:{}", occurrence.codefile.name, occurrence.line))
                .collect::<Vec<_>>()
                .join(", ");
            bail!("source anchor '{id}' is duplicated ({count} occurrences: {locations})");
        }
    }
    let occurrence = occurrences.pop().expect("one occurrence checked above");
    let content = read_codefile(store, &occurrence.codefile)?;
    let expected_prefix = marker_prefix(&occurrence.codefile.name)?;
    let (actual_prefix, _) = parse_marker_line(&occurrence.marker)
        .expect("scan_markers returns only exact valid marker lines");
    if actual_prefix != expected_prefix {
        bail!(
            "source anchor '{id}' uses comment marker '{}' but '{}' requires '{}'",
            actual_prefix,
            occurrence.codefile.name,
            expected_prefix
        );
    }
    let entry = attached_entry(&occurrence.codefile.name, &content, occurrence.line).with_context(
        || {
            format!(
                "source anchor '{id}' at {}:{} is detached",
                occurrence.codefile.name, occurrence.line
            )
        },
    )?;
    Ok(ResolvedAnchor {
        id: id.to_string(),
        locator: format!("{ANCHOR_LOCATOR_PREFIX}{id}"),
        marker: occurrence.marker,
        file_id: occurrence.codefile.id,
        file: occurrence.codefile.name,
        entry_kind: entry.kind,
        entry_name: entry.name,
        line_start: entry.line_start,
        line_end: entry.line_end,
        callable_symbol: entry.callable_symbol,
    })
}

/// Validate a locator against the CodeFile an asserted graph edge targets.
///
/// Legacy symbol/module semantics stay unchanged. Anchors additionally require
/// global uniqueness, a supported attachment, and exact target-file identity.
pub fn validate_for_codefile(store: &Store, codefile: &Node, locator: &str) -> Result<()> {
    if codefile.node_type != NodeType::CodeFile {
        bail!("locator target '{}' is not a CodeFile", codefile.name);
    }
    let locator = locator.trim();
    if locator.is_empty() || is_module_scope(locator) {
        return Ok(());
    }
    if is_anchor_locator(locator) {
        let anchor = resolve_anchor(store, locator)?;
        if anchor.file_id != codefile.id {
            bail!(
                "source anchor '{}' resolves in '{}', not target CodeFile '{}'",
                anchor.id,
                anchor.file,
                codefile.name
            );
        }
        return Ok(());
    }
    if crate::runner::grounding_locator_resolves(store.root(), &codefile.name, locator) {
        Ok(())
    } else {
        bail!(
            "locator must resolve to a live symbol in '{}' (no match for '{}'); use a symbol name, 'anchor:<id>', or 'module …' for whole-file scope",
            codefile.name,
            locator
        )
    }
}

fn issued_from_resolved(resolved: ResolvedAnchor) -> IssuedAnchor {
    IssuedAnchor {
        id: resolved.id,
        locator: resolved.locator,
        marker: resolved.marker,
        file: resolved.file,
        entry_kind: resolved.entry_kind,
        entry_name: resolved.entry_name,
        line_start: resolved.line_start,
        line_end: resolved.line_end,
    }
}

fn read_codefile(store: &Store, codefile: &Node) -> Result<String> {
    std::fs::read_to_string(store.root().join(&codefile.name))
        .with_context(|| format!("reading registered CodeFile '{}'", codefile.name))
}

fn scan_markers(store: &Store) -> Result<Vec<MarkerOccurrence>> {
    let mut codefiles = store.codefiles()?;
    codefiles.sort_by(|left, right| left.name.cmp(&right.name));
    let mut occurrences = Vec::new();
    for codefile in codefiles {
        let path = store.root().join(&codefile.name);
        if !path.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("reading registered CodeFile '{}'", codefile.name))?;
        for (index, line) in content.lines().enumerate() {
            let marker = line.trim();
            if parse_marker_line(marker).is_some() {
                occurrences.push(MarkerOccurrence {
                    codefile: codefile.clone(),
                    marker: marker.to_string(),
                    line: index + 1,
                });
            }
        }
    }
    Ok(occurrences)
}

fn parse_marker_line(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    let (prefix, id) = if let Some(id) = line.strip_prefix("// loom:anchor ") {
        ("//", id)
    } else {
        ("#", line.strip_prefix("# loom:anchor ")?)
    };
    crate::journey::validate_stable_id("source anchor", id)
        .is_ok()
        .then_some((prefix, id))
}

fn occurrence_id(marker: &str) -> &str {
    parse_marker_line(marker)
        .map(|(_, id)| id)
        .expect("marker occurrence was parsed before storage")
}

fn marker_prefix(path: &str) -> Result<&'static str> {
    use crate::extract::Language;
    match Language::detect(path) {
        Language::Rust | Language::Go | Language::JavaScript | Language::TypeScript => Ok("//"),
        Language::Python => Ok("#"),
        Language::Other if is_supported_config(path) => Ok("#"),
        Language::Other => bail!(
            "source anchors are unsupported for commentless or unknown format '{}'",
            path
        ),
    }
}

fn is_supported_config(path: &str) -> bool {
    let path = Path::new(path);
    let file = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    matches!(ext, "toml" | "yaml" | "yml" | "ini" | "cfg")
        || file == "Dockerfile"
        || file.ends_with(".dockerfile")
}

/// Resolve a declaration by NAME to the one-based line an anchor should be
/// issued for.
///
/// A line number is a coordinate, not an identity: inserting anything above a
/// declaration silently moves it, and a pinned line then resolves to whatever
/// now occupies it — a neighbouring function, reported as a confident answer
/// rather than an error. Callers that mean a specific declaration can name it
/// and stay correct across edits.
pub fn line_for_symbol(store: &Store, codefile: &Node, symbol: &str) -> Result<usize> {
    if codefile.node_type != NodeType::CodeFile {
        bail!("source anchors can be issued only for a CodeFile");
    }
    let symbol = symbol.trim();
    if symbol.is_empty() {
        bail!("source anchor --at-symbol requires a declaration name");
    }
    let content = read_codefile(store, codefile)?;
    if crate::extract::Language::detect(&codefile.name) == crate::extract::Language::Other {
        bail!(
            "'{}' has no extractable declarations; use --at-line",
            codefile.name
        );
    }
    let extraction = crate::extract::extract(&codefile.name, &content);
    let mut matches: Vec<_> = extraction
        .symbols
        .iter()
        .filter(|candidate| candidate.name == symbol)
        .collect();
    matches.sort_by_key(|candidate| candidate.line_start);
    match matches.as_slice() {
        [] => bail!("'{}' declares no symbol named '{symbol}'", codefile.name),
        [only] => Ok(only.line_start),
        several => bail!(
            "'{}' declares {} symbols named '{symbol}' (lines {}); use --at-line to select one",
            codefile.name,
            several.len(),
            several
                .iter()
                .map(|candidate| candidate.line_start.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn selected_entry(path: &str, content: &str, at_line: usize) -> Result<AnchorEntry> {
    if crate::extract::Language::detect(path) == crate::extract::Language::Other {
        if !is_supported_config(path) {
            marker_prefix(path)?;
        }
        return config_entry(path, content, at_line);
    }
    let extraction = crate::extract::extract(path, content);
    let mut candidates: Vec<_> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.line_start <= at_line && at_line <= symbol.line_end)
        .collect();
    candidates.sort_by(|left, right| {
        (left.line_end - left.line_start)
            .cmp(&(right.line_end - right.line_start))
            .then(left.line_start.cmp(&right.line_start))
            .then(left.name.cmp(&right.name))
    });
    let Some(symbol) = candidates.first() else {
        bail!("line {at_line} in '{path}' is not inside a supported declaration");
    };
    if candidates.get(1).is_some_and(|other| {
        other.line_start == symbol.line_start && other.line_end == symbol.line_end
    }) {
        bail!("line {at_line} in '{path}' belongs to more than one smallest declaration");
    }
    Ok(symbol_entry(symbol))
}

fn attached_entry(path: &str, content: &str, marker_line: usize) -> Result<AnchorEntry> {
    let entry_line = marker_line + 1;
    if crate::extract::Language::detect(path) == crate::extract::Language::Other {
        if !is_supported_config(path) {
            marker_prefix(path)?;
        }
        return config_entry(path, content, entry_line);
    }
    let extraction = crate::extract::extract(path, content);
    let candidates: Vec<_> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.line_start == entry_line)
        .collect();
    match candidates.as_slice() {
        [symbol] => Ok(symbol_entry(symbol)),
        [] => bail!("marker is not immediately before a supported declaration"),
        _ => bail!("marker attaches to more than one declaration"),
    }
}

fn symbol_entry(symbol: &crate::extract::Symbol) -> AnchorEntry {
    AnchorEntry {
        kind: symbol.kind.clone(),
        name: symbol.name.clone(),
        line_start: symbol.line_start,
        line_end: symbol.line_end,
        callable_symbol: matches!(symbol.kind.as_str(), "function" | "method")
            .then(|| symbol.name.clone()),
    }
}

fn config_entry(path: &str, content: &str, line_number: usize) -> Result<AnchorEntry> {
    marker_prefix(path)?;
    let line = content
        .lines()
        .nth(line_number.saturating_sub(1))
        .ok_or_else(|| anyhow::anyhow!("line {line_number} is outside '{path}'"))?;
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        bail!("line {line_number} in '{path}' is not a configuration entry");
    }
    let file = Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let ext = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let (kind, name) = if file == "Dockerfile" || file.ends_with(".dockerfile") {
        let instruction = trimmed.split_whitespace().next().unwrap_or("");
        if instruction.is_empty()
            || !instruction
                .chars()
                .all(|character| character.is_ascii_alphabetic())
        {
            bail!("line {line_number} in '{path}' is not a Dockerfile instruction");
        }
        ("config_instruction", instruction.to_ascii_lowercase())
    } else if matches!(ext, "toml" | "ini" | "cfg") {
        if trimmed.starts_with('[') {
            bail!("line {line_number} in '{path}' is a section, not the smallest key/value entry");
        }
        let Some((key, _)) = trimmed.split_once('=') else {
            bail!("line {line_number} in '{path}' is not a key/value entry");
        };
        let key = key.trim().trim_matches(['\'', '"']);
        if key.is_empty() {
            bail!("line {line_number} in '{path}' has an empty configuration key");
        }
        ("config_entry", key.to_string())
    } else {
        let candidate = trimmed.strip_prefix("- ").unwrap_or(trimmed);
        let Some((key, _)) = candidate.split_once(':') else {
            bail!("line {line_number} in '{path}' is not a YAML mapping entry");
        };
        let key = key.trim().trim_matches(['\'', '"']);
        if key.is_empty() {
            bail!("line {line_number} in '{path}' has an empty configuration key");
        }
        ("config_entry", key.to_string())
    };
    Ok(AnchorEntry {
        kind: kind.into(),
        name,
        line_start: line_number,
        line_end: line_number,
        callable_symbol: None,
    })
}

fn anchor_base_id(path: &str, entry_name: &str) -> String {
    let path = Path::new(path);
    let mut parts: Vec<String> = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(slug_part)
        .filter(|part| !part.is_empty())
        .collect();
    if parts
        .first()
        .is_some_and(|part| matches!(part.as_str(), "src" | "lib" | "app"))
    {
        parts.remove(0);
    }
    if let Some(last) = parts.last_mut() {
        if let Some(stem) = Path::new(last).file_stem().and_then(|value| value.to_str()) {
            *last = slug_part(stem);
        }
    }
    parts.push(slug_part(entry_name));
    parts.retain(|part| !part.is_empty());
    let mut id = parts.join(".");
    if id.is_empty() || !id.starts_with(|character: char| character.is_ascii_lowercase()) {
        id = format!("source.{id}").trim_end_matches('.').to_string();
    }
    id
}

fn slug_part(value: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            slug.push(character);
            separator = false;
        } else if !slug.is_empty() && !separator {
            slug.push('.');
            separator = true;
        }
    }
    slug.trim_matches('.').to_string()
}

/// Whether this locator is a whole-file scope (`module …`), not a symbol.
pub fn is_module_scope(locator: &str) -> bool {
    let t = locator.trim().to_ascii_lowercase();
    t == "module" || t.starts_with("module ")
}

/// Parse the symbol names carried by one locator.
///
/// A locator may name several symbols with `;`. Each member must still look
/// like a locator, not prose: a bare/qualified symbol, optionally with a line
/// suffix, or a symbol preceded only by declaration modifiers (`fn`, `enum`,
/// `pub`, ...). Taking the final word of prose such as
/// `subject case state-machine tests` would let an unrelated symbol named
/// `tests` manufacture a witness or inflate blast radius.
pub fn symbols(locator: &str) -> Vec<String> {
    // Anchor ids are navigation identities, never proof symbols. Reserve the
    // whole prefix, including malformed forms, so `anchor:bad` cannot degrade
    // to an ordinary symbol named `anchor` and manufacture a call witness.
    if is_anchor_locator(locator) {
        return Vec::new();
    }
    locator.split(';').filter_map(symbol).collect()
}

/// Parse one locator member (one side of `;`).
pub fn symbol(member: &str) -> Option<String> {
    let member = member.trim();
    if member.is_empty() || is_module_scope(member) || is_anchor_locator(member) {
        return None;
    }

    let words: Vec<&str> = member.split_whitespace().collect();
    let token = *words.last()?;
    if words.len() > 1 {
        let prefixes = &words[..words.len() - 1];
        let declaration = prefixes.iter().any(|word| {
            matches!(
                *word,
                "fn" | "struct"
                    | "enum"
                    | "trait"
                    | "impl"
                    | "class"
                    | "def"
                    | "function"
                    | "interface"
                    | "type"
                    // JS/TS declarations commonly use a bound const as the
                    // callable surface (`export const load = …`).
                    | "const"
            )
        });
        let all_are_declaration_words = prefixes.iter().all(|word| {
            matches!(
                *word,
                "fn" | "struct"
                    | "enum"
                    | "trait"
                    | "impl"
                    | "class"
                    | "def"
                    | "function"
                    | "interface"
                    | "type"
                    | "async"
                    | "unsafe"
                    | "extern"
                    | "const"
                    // TS/JS/JVM visibility + member modifiers — `export` is
                    // TypeScript's `pub`; without these, a locator such as
                    // `export async function getDeck` parses as prose and the
                    // grounded symbol silently drops out of every consumer.
                    | "export"
                    | "default"
                    | "public"
                    | "private"
                    | "protected"
                    | "static"
                    | "readonly"
                    | "abstract"
                    | "override"
            ) || *word == "pub"
                || word.starts_with("pub(")
        });
        if !declaration || !all_are_declaration_words {
            return None;
        }
    }

    // Qualification must be removed before a `:line` suffix: splitting
    // `Type::method` at `:` first yields `Type` and makes the method branch
    // unreachable.
    let token = token.rsplit("::").next().unwrap_or(token);
    let token = token.split(':').next().unwrap_or(token);
    is_symbol_name(token).then(|| token.to_string())
}

fn is_symbol_name(candidate: &str) -> bool {
    let mut chars = candidate.chars();
    matches!(chars.next(), Some(first) if first == '_' || first == '$' || first.is_alphabetic())
        && chars.all(|c| c == '_' || c == '$' || c.is_alphanumeric())
}

/// Symbol names an intent is grounded in, via its realizing locators only.
///
/// Non-realizing roles (`consumes`, `configures`, `verifies`) are seams and
/// proofs, not the behavior's home — counting them as blast-radius / proof
/// symbols lets a test helper inflate urgency and manufacture witnesses.
pub fn realizing_symbols(store: &Store, intent_id: &str) -> Result<Vec<String>> {
    let mut out: Vec<String> = realizing_targets(store, intent_id)?
        .into_iter()
        .map(|(_, symbol)| symbol)
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

/// Navigation/blast-radius symbol names for an Intent's realizing groundings.
///
/// Unlike [`realizing_symbols`], this resolves source anchors to the declaration
/// currently attached to their marker. Config entries have no callable symbol
/// and therefore contribute no call-graph target. Resolution errors propagate:
/// risk must fail closed rather than guess which duplicate marker was meant.
pub fn realizing_navigation_symbols(store: &Store, intent_id: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Implements), Some(intent_id), None)? {
        if store.edge_superseded(&edge.id)?
            || store.grounding_role(&edge.id)? != GroundingRole::Realizes
        {
            continue;
        }
        let Some(locator) = store.get_facet(&edge.id, TargetKind::Edge, "locator")? else {
            continue;
        };
        if is_anchor_locator(&locator) {
            if let Some(symbol) = resolve_anchor(store, &locator)?.callable_symbol {
                out.push(symbol);
            }
        } else {
            out.extend(symbols(&locator));
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Realizing groundings as `(codefile path, symbol)` pairs.
///
/// Grading must keep the file: a bare symbol shared by two definitions can
/// otherwise pull callers of the wrong definition into the call witness.
pub fn realizing_targets(store: &Store, intent_id: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Implements), Some(intent_id), None)? {
        if store.edge_superseded(&e.id)? {
            continue;
        }
        if store.grounding_role(&e.id)? != GroundingRole::Realizes {
            continue;
        }
        let Some(file) = store.get_node(&e.to_id)? else {
            continue;
        };
        if let Some(loc) = store.get_facet(&e.id, TargetKind::Edge, "locator")? {
            for symbol in symbols(&loc) {
                out.push((file.name.clone(), symbol));
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeType, TruthClass};
    use crate::store::Store;

    /// Shared regression table: every consumer of locator parsing must agree
    /// with these answers. Multi-symbol, Type::method, prose, declaration
    /// modifiers, and grounding roles are all covered here so a second parser
    /// cannot quietly reappear elsewhere.
    #[test]
    fn shared_locator_regression_table() {
        let cases: &[(&str, &[&str])] = &[
            // multi-symbol
            (
                "getSubjectCase; listSubjectCases; a;b;c",
                &["getSubjectCase", "listSubjectCases", "a", "b", "c"],
            ),
            // Type::method and line suffixes — path qualification before `:line`
            (
                "DurableSignalLedger::rotate_checkpoint_authority_exact",
                &["rotate_checkpoint_authority_exact"],
            ),
            ("Type::method:42-57", &["method"]),
            ("capture_payment:88", &["capture_payment"]),
            // declaration modifiers
            ("fn perform_behavior", &["perform_behavior"]),
            ("pub async fn perform_behavior", &["perform_behavior"]),
            ("export function getDeck", &["getDeck"]),
            ("export async function getDeck", &["getDeck"]),
            ("export default class RoomDeck", &["RoomDeck"]),
            ("public static def render", &["render"]),
            // prose / module scopes — must not invent a symbol
            ("subject case state-machine tests", &[]),
            ("private-CA PostgreSQL acceptance runner", &[]),
            ("module proof strength grading", &[]),
            ("export the deck roster", &[]),
        ];
        for (locator, expected) in cases {
            let got = symbols(locator);
            assert_eq!(
                got, *expected,
                "locator `{locator}`: got {got:?}, expected {expected:?}"
            );
        }
        assert!(is_module_scope("module the thing this file is about"));
        assert!(!is_module_scope("mod_helper"));
    }

    #[test]
    fn realizing_symbols_ignores_non_realizing_roles() {
        let root = std::env::temp_dir().join(format!(
            "loom-locator-roles-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let store = Store::init_with_identity(
            &root,
            Some("locator roles"),
            false,
            crate::identity::ExecutionIdentity::solo(),
        )
        .unwrap();
        let intent = store
            .add_node(
                NodeType::Intent,
                "behavior",
                "d",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();

        for (role, symbol) in [
            (GroundingRole::Realizes, "real_symbol"),
            (GroundingRole::Consumes, "consumed_symbol"),
            (GroundingRole::Configures, "configured_symbol"),
            (GroundingRole::Verifies, "verifying_symbol"),
        ] {
            let file = store
                .add_node(
                    NodeType::CodeFile,
                    &format!("{symbol}.rs"),
                    "",
                    "",
                    serde_json::json!({}),
                )
                .unwrap();
            let edge = store
                .add_edge(
                    EdgeKind::Implements,
                    &intent.id,
                    &file.id,
                    TruthClass::Asserted,
                )
                .unwrap();
            store.set_grounding_role(&edge.id, role).unwrap();
            store
                .set_facet(
                    &edge.id,
                    TargetKind::Edge,
                    "locator",
                    symbol,
                    TruthClass::Asserted,
                )
                .unwrap();
        }

        assert_eq!(
            realizing_symbols(&store, &intent.id).unwrap(),
            ["real_symbol"]
        );
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }
}
