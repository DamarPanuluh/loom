//! External diagnostic scan adapters.
//!
//! Plane: programmatic defect sensors. Adapters are repo-portable config stored in
//! meta, while their observations become ordinary derived Findings so the
//! existing finding adjudication flow stays the single triage surface.

use crate::model::{EdgeKind, Node, NodeType, TruthClass};
use crate::store::Store;
use crate::Result;
use anyhow::{anyhow, bail, Context};
use process_control::{ChildExt, Control};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};
use std::process::Stdio;
use std::time::Duration;

const ADAPTERS_META_KEY: &str = "scan_adapters";
const DEFAULT_MAP: &str = r"^(?P<file>[^:\s][^:]*?):(?P<line>\d+)(?::\d+)?:\s*(?P<msg>.+)$";
const SCAN_TIMEOUT_SECS: u64 = 120;
const TITLE_MSG_LIMIT: usize = 96;

/// A configured external diagnostic source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
}

/// Summary of one scan run.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScanReport {
    pub adapters_run: usize,
    pub diagnostics: usize,
    pub new_findings: usize,
    pub resolved_findings: usize,
    pub skipped_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedDiagnostic {
    file: String,
    line: u64,
    msg: String,
    code: Option<String>,
}

/// Register a scan adapter in the store meta registry.
pub fn add_adapter(store: &Store, name: &str, command: &str, map: Option<&str>) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("scan adapter name must not be empty");
    }
    if command.trim().is_empty() {
        bail!("scan adapter command must not be empty");
    }
    if let Some(map) = map {
        validate_map_regex(map)?;
    }

    let mut adapters = list_adapters(store)?;
    if adapters.iter().any(|a| a.name == name) {
        bail!("scan adapter '{name}' already exists");
    }
    adapters.push(Adapter {
        name: name.to_string(),
        command: command.to_string(),
        map: map.map(str::to_string),
    });
    write_adapters(store, &adapters)
}

/// Remove a scan adapter from the store meta registry.
pub fn remove_adapter(store: &Store, name: &str) -> Result<()> {
    let name = name.trim();
    let mut adapters = list_adapters(store)?;
    let before = adapters.len();
    adapters.retain(|a| a.name != name);
    if adapters.len() == before {
        bail!("no scan adapter named '{name}'");
    }
    write_adapters(store, &adapters)
}

/// List configured scan adapters in registration order.
pub fn list_adapters(store: &Store) -> Result<Vec<Adapter>> {
    let Some(raw) = store.get_meta(ADAPTERS_META_KEY)? else {
        return Ok(Vec::new());
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&raw).with_context(|| format!("parsing meta '{ADAPTERS_META_KEY}'"))
}

/// Run one adapter, or every configured adapter when `name` is `None`.
pub fn run(store: &Store, root: &Path, name: Option<&str>) -> Result<ScanReport> {
    let adapters = select_adapters(store, name)?;
    let codefiles = registered_codefiles(store, root)?;
    let mut report = ScanReport::default();

    for adapter in adapters {
        report.adapters_run += 1;
        let regex = adapter_regex(&adapter)?;
        let rule = ensure_adapter_rule(store, &adapter)?;
        let existing = adapter_finding_ids(store, &adapter.name)?;
        let output = run_adapter_command(root, &adapter.command, &adapter.name)?;
        let mut active = BTreeSet::new();

        for line in output.lines() {
            let line = line.trim_end_matches('\r');
            let Some(mut diagnostic) = parse_diagnostic(&regex, line) else {
                report.skipped_lines += 1;
                continue;
            };
            let Some(normalized) = normalize_path(root, &diagnostic.file) else {
                continue;
            };
            diagnostic.file = normalized;
            let Some(codefile) = codefiles.get(&diagnostic.file) else {
                continue;
            };

            let node = upsert_diagnostic(store, &adapter, &rule, codefile, &diagnostic)?;
            if active.insert(node.id.clone()) && !existing.contains(&node.id) {
                report.new_findings += 1;
            }
            report.diagnostics += 1;
        }

        let stale: Vec<String> = existing.difference(&active).cloned().collect();
        if !stale.is_empty() {
            store.remove_derived_findings(&stale)?;
            report.resolved_findings += stale.len();
        }
    }

    Ok(report)
}

fn write_adapters(store: &Store, adapters: &[Adapter]) -> Result<()> {
    let raw = serde_json::to_string(adapters)?;
    store.set_meta(ADAPTERS_META_KEY, &raw)
}

fn select_adapters(store: &Store, name: Option<&str>) -> Result<Vec<Adapter>> {
    let adapters = list_adapters(store)?;
    match name {
        Some(name) => adapters
            .into_iter()
            .find(|a| a.name == name)
            .map(|a| vec![a])
            .ok_or_else(|| anyhow!("no scan adapter named '{name}'")),
        None => Ok(adapters),
    }
}

fn validate_map_regex(map: &str) -> Result<()> {
    let regex = Regex::new(map).with_context(|| "compiling scan adapter map regex")?;
    let names: BTreeSet<&str> = regex.capture_names().flatten().collect();
    for required in ["file", "line"] {
        if !names.contains(required) {
            bail!("scan adapter map regex must contain named group '{required}'");
        }
    }
    Ok(())
}

fn adapter_regex(adapter: &Adapter) -> Result<Regex> {
    let map = adapter.map.as_deref().unwrap_or(DEFAULT_MAP);
    validate_map_regex(map)?;
    Regex::new(map)
        .with_context(|| format!("compiling map regex for scan adapter '{}'", adapter.name))
}

fn registered_codefiles(store: &Store, root: &Path) -> Result<BTreeMap<String, Node>> {
    let mut out = BTreeMap::new();
    for codefile in store.codefiles()? {
        if let Some(path) = normalize_path(root, &codefile.name) {
            out.insert(path, codefile);
        }
    }
    Ok(out)
}

fn ensure_adapter_rule(store: &Store, adapter: &Adapter) -> Result<Node> {
    let key = format!("scan:{}", adapter.name);
    store.upsert_builtin_node(
        NodeType::CodeRule,
        &key,
        &key,
        &format!(
            "external diagnostics emitted by scan adapter '{}'",
            adapter.name
        ),
        serde_json::json!({
            "category": "external",
            "adapter": adapter.name,
        }),
    )
}

fn run_adapter_command(root: &Path, command: &str, adapter_name: &str) -> Result<String> {
    let script = format!("exec 2>&1\n{command}");
    let child = std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning scan adapter '{adapter_name}'"))?;

    let Some(output) = child
        .controlled_with_output()
        .time_limit(Duration::from_secs(SCAN_TIMEOUT_SECS))
        .terminate_for_timeout()
        .wait()
        .with_context(|| format!("waiting for scan adapter '{adapter_name}'"))?
    else {
        bail!("scan adapter '{adapter_name}' timed out after {SCAN_TIMEOUT_SECS}s");
    };

    let mut combined = output.stdout;
    combined.extend(output.stderr);
    Ok(String::from_utf8_lossy(&combined).into_owned())
}

fn parse_diagnostic(regex: &Regex, line: &str) -> Option<ParsedDiagnostic> {
    let captures = regex.captures(line)?;
    let file = captures.name("file")?.as_str().trim().to_string();
    if file.is_empty() {
        return None;
    }
    let line_no = captures.name("line")?.as_str().parse().ok()?;
    let msg = captures
        .name("msg")
        .map(|m| m.as_str().trim())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| line.trim())
        .to_string();
    let code = captures
        .name("code")
        .map(|m| m.as_str().trim())
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    Some(ParsedDiagnostic {
        file,
        line: line_no,
        msg,
        code,
    })
}

fn normalize_path(root: &Path, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let portable = raw.replace('\\', "/");
    let path = Path::new(&portable);
    let relative = if path.is_absolute() {
        match path.strip_prefix(root) {
            Ok(stripped) => stripped.to_path_buf(),
            Err(_) => match root.canonicalize() {
                Ok(canonical_root) => path.strip_prefix(canonical_root).ok()?.to_path_buf(),
                Err(_) => return None,
            },
        }
    } else {
        path.to_path_buf()
    };
    normalize_relative(&relative)
}

fn normalize_relative(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn upsert_diagnostic(
    store: &Store,
    adapter: &Adapter,
    rule: &Node,
    codefile: &Node,
    diagnostic: &ParsedDiagnostic,
) -> Result<Node> {
    let detail = match diagnostic.code.as_deref() {
        Some(code) => format!("{}\ncode: {code}", diagnostic.msg),
        None => diagnostic.msg.clone(),
    };
    let title = format!(
        "{}:{} {}",
        diagnostic.file,
        diagnostic.line,
        truncate_chars(&diagnostic.msg, TITLE_MSG_LIMIT)
    );
    let det_key = diagnostic_det_key(adapter, diagnostic);
    let node = store.add_derived_node(
        NodeType::Finding,
        &det_key,
        &title,
        &detail,
        "external_diagnostic",
        serde_json::json!({
            "kind": "external_diagnostic",
            "adapter": adapter.name,
            "file": diagnostic.file,
            "line": diagnostic.line,
            "msg": diagnostic.msg,
            "code": diagnostic.code,
        }),
    )?;
    store.add_derived_edge(EdgeKind::Flags, &node.id, &codefile.id)?;
    store.add_derived_edge(EdgeKind::Assesses, &node.id, &rule.id)?;
    Ok(node)
}

fn diagnostic_det_key(adapter: &Adapter, diagnostic: &ParsedDiagnostic) -> String {
    format!(
        "scan:{}:{}:{}:{}:{}",
        adapter.name,
        diagnostic.file,
        diagnostic.line,
        diagnostic.code.as_deref().unwrap_or(""),
        diagnostic.msg
    )
}

fn adapter_finding_ids(store: &Store, adapter_name: &str) -> Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    for node in store.list_nodes(Some(NodeType::Finding), usize::MAX)? {
        if node.truth_class != TruthClass::Derived || node.status != "external_diagnostic" {
            continue;
        }
        let is_adapter = node
            .body
            .get("kind")
            .and_then(|v| v.as_str())
            .is_some_and(|kind| kind == "external_diagnostic")
            && node
                .body
                .get("adapter")
                .and_then(|v| v.as_str())
                .is_some_and(|adapter| adapter == adapter_name);
        if is_adapter {
            ids.insert(node.id);
        }
    }
    Ok(ids)
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx == limit {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeType;
    use crate::store::Store;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(name: &str) -> Result<Self> {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "loom_scan_{name}_{}_{}",
                std::process::id(),
                nonce
            ));
            std::fs::create_dir_all(root.join("src"))?;
            std::fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n")?;
            Ok(Self(root))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn default_map_parses_gcc_style_line() -> Result<()> {
        let regex = Regex::new(DEFAULT_MAP)?;
        let diagnostic = parse_diagnostic(&regex, "src/a.rs:12:5: warning: unused variable")
            .ok_or_else(|| anyhow!("expected diagnostic"))?;
        assert_eq!(diagnostic.file, "src/a.rs");
        assert_eq!(diagnostic.line, 12);
        assert_eq!(diagnostic.msg, "warning: unused variable");
        assert_eq!(diagnostic.code, None);
        Ok(())
    }

    #[test]
    fn custom_map_with_named_groups_parses_code_and_message() -> Result<()> {
        let map = r"^\[(?P<code>[A-Z]\d+)\] (?P<file>.+?)@(?P<line>\d+) (?P<msg>.+)$";
        validate_map_regex(map)?;
        let regex = Regex::new(map)?;
        let diagnostic = parse_diagnostic(&regex, "[E42] src/a.py@7 bad import")
            .ok_or_else(|| anyhow!("expected diagnostic"))?;
        assert_eq!(diagnostic.file, "src/a.py");
        assert_eq!(diagnostic.line, 7);
        assert_eq!(diagnostic.msg, "bad import");
        assert_eq!(diagnostic.code.as_deref(), Some("E42"));
        Ok(())
    }

    #[test]
    fn non_registered_file_is_dropped() -> Result<()> {
        let root = TestRoot::new("drop")?;
        let store = Store::init(root.path(), Some("scan-test"), false)?;
        store.add_node(
            NodeType::CodeFile,
            "src/lib.rs",
            "",
            "",
            serde_json::json!({}),
        )?;
        add_adapter(
            &store,
            "fake",
            "printf 'src/lib.rs:1: boom\\nsrc/missing.rs:2: nope\\n'",
            None,
        )?;

        let report = run(&store, root.path(), Some("fake"))?;
        assert_eq!(report.adapters_run, 1);
        assert_eq!(report.diagnostics, 1);
        assert_eq!(report.new_findings, 1);
        assert_eq!(report.resolved_findings, 0);
        assert_eq!(report.skipped_lines, 0);

        let findings = crate::signal::findings_view(&store)?;
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].node.status, "external_diagnostic");
        assert!(findings[0].node.name.starts_with("src/lib.rs:1 boom"));
        Ok(())
    }

    #[test]
    fn fake_tool_findings_are_visible_and_converge() -> Result<()> {
        let root = TestRoot::new("converge")?;
        let store = Store::init(root.path(), Some("scan-test"), false)?;
        store.add_node(
            NodeType::CodeFile,
            "src/lib.rs",
            "",
            "",
            serde_json::json!({}),
        )?;
        add_adapter(&store, "fake", "printf 'src/lib.rs:1: boom\\n'", None)?;

        let first = run(&store, root.path(), Some("fake"))?;
        assert_eq!(first.diagnostics, 1);
        assert_eq!(first.new_findings, 1);
        let findings = crate::signal::findings_view(&store)?;
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].state, "untriaged");
        assert!(findings[0].node.name.contains("src/lib.rs:1 boom"));

        remove_adapter(&store, "fake")?;
        add_adapter(&store, "fake", "printf ''", None)?;
        let second = run(&store, root.path(), Some("fake"))?;
        assert_eq!(second.diagnostics, 0);
        assert_eq!(second.new_findings, 0);
        assert_eq!(second.resolved_findings, 1);
        assert!(crate::signal::findings_view(&store)?.is_empty());
        Ok(())
    }
}
