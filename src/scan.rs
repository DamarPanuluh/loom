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
/// A bare `file:line[:col]` location with no trailing message — the first half
/// of a two-line diagnostic (svelte-check-style human output).
const LOCATION_ONLY_MAP: &str = r"^(?P<file>[^:\s][^:]*?):(?P<line>\d+)(?::\d+)?\s*$";
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
/// Edit a registered scan adapter in place.
pub fn update_adapter(
    store: &Store,
    name: &str,
    command: Option<&str>,
    map: Option<&str>,
) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("scan adapter name must not be empty");
    }
    if command.is_none() && map.is_none() {
        bail!("nothing to update — pass --command and/or --map");
    }
    if let Some(command) = command {
        if command.trim().is_empty() {
            bail!("scan adapter command must not be empty");
        }
    }
    if let Some(map) = map {
        validate_map_regex(map)?;
    }

    let mut adapters = list_adapters(store)?;
    let adapter = adapters
        .iter_mut()
        .find(|a| a.name == name)
        .ok_or_else(|| anyhow!("no scan adapter named '{name}'"))?;
    if let Some(command) = command {
        adapter.command = command.to_string();
    }
    if let Some(map) = map {
        adapter.map = Some(map.to_string());
    }
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
        let (output, exit_code) = run_adapter_command(root, &adapter.command, &adapter.name)?;
        // A command that could not run (127 not-found / 126 not-executable) would
        // otherwise parse to zero diagnostics and converge every prior finding to
        // resolved — silent data loss. Fail loudly instead (M-8). Linters that
        // merely FOUND issues exit non-zero WITH output, so they pass this gate.
        if matches!(exit_code, Some(126) | Some(127)) {
            bail!(
                "scan adapter '{}' failed to run (exit {}) — check its command; findings left untouched: {}",
                adapter.name,
                exit_code.unwrap_or(-1),
                output.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim()
            );
        }
        let (parsed, skipped) = parse_output(&regex, &output, adapter.map.is_none());
        report.skipped_lines += skipped;
        // A non-zero exit that produced no recognizable diagnostics is a failed
        // run (a crash, a config error), not "every issue resolved" — a clean
        // pass exits 0. Converging then would wrongly resolve every prior finding
        // (M-8), so leave them untouched and let the next healthy run reconcile.
        let healthy_run = exit_code == Some(0) || !parsed.is_empty();
        let mut active = BTreeSet::new();

        for mut diagnostic in parsed {
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
        if healthy_run && !stale.is_empty() {
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
fn run_adapter_command(
    root: &Path,
    command: &str,
    adapter_name: &str,
) -> Result<(String, Option<i64>)> {
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
    Ok((
        String::from_utf8_lossy(&combined).into_owned(),
        output.status.code(),
    ))
}

/// Parse adapter output line by line. `pair_locations` (default parser only)
/// additionally accepts two-line diagnostics — a bare `file:line[:col]`
/// location line whose message is the immediately following line, the shape
/// svelte-check-style tools emit. A blank line drops the pending location:
/// pairing across gaps would stitch unrelated output blocks into bogus
/// diagnostics. A custom `--map` regex stays strictly per-line: the operator
/// owns that grammar.
fn parse_output(
    regex: &Regex,
    output: &str,
    pair_locations: bool,
) -> (Vec<ParsedDiagnostic>, usize) {
    let location_only =
        pair_locations.then(|| Regex::new(LOCATION_ONLY_MAP).expect("static location regex"));
    let mut diagnostics = Vec::new();
    let mut skipped = 0usize;
    // A location line still waiting for its message line.
    let mut pending: Option<(String, u64)> = None;
    for raw in output.lines() {
        let line = raw.trim_end_matches('\r');
        // Location-only is checked BEFORE the adapter map: on a bare
        // `file:line:col` line the default map would otherwise backtrack the
        // optional column group and misread the column as the message. A full
        // one-line diagnostic can never match here — location-only requires
        // end-of-line right after the column.
        if let Some(location_re) = &location_only {
            if let Some(captures) = location_re.captures(line) {
                skipped += usize::from(pending.take().is_some());
                let file = captures.name("file").map(|m| m.as_str().trim().to_string());
                let line_no = captures.name("line").and_then(|m| m.as_str().parse().ok());
                if let (Some(file), Some(line_no)) = (file, line_no) {
                    pending = Some((file, line_no));
                }
                continue;
            }
        }
        if let Some(diagnostic) = parse_diagnostic(regex, line) {
            // A full diagnostic abandons any unpaired location.
            skipped += usize::from(pending.take().is_some());
            diagnostics.push(diagnostic);
            continue;
        }
        match pending.take() {
            Some((file, line_no)) if !line.trim().is_empty() => {
                diagnostics.push(ParsedDiagnostic {
                    file,
                    line: line_no,
                    msg: line.trim().to_string(),
                    code: None,
                });
            }
            // A blank line right after a location: the location had no message.
            Some(_) => skipped += 2,
            None => skipped += 1,
        }
    }
    skipped += usize::from(pending.is_some());
    (diagnostics, skipped)
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

    // svelte-check-style two-line stream: a bare `file:line:col` location line
    // followed by the message on the next non-empty line becomes ONE diagnostic.
    // An indented code-excerpt line and a blank line between pairs are skipped,
    // and a second location+message pair also parses.
    #[test]
    fn svelte_check_two_line_pairs_into_diagnostics() -> Result<()> {
        let regex = Regex::new(DEFAULT_MAP)?;
        let (diags, skipped) = parse_output(
            &regex,
            concat!(
                "src/App.svelte:12:5\n",
                "Warn: unused export (svelte)\n",
                "  const x = 1\n",
                "\n",
                "src/App.svelte:13:2\n",
                "Error: missing semicolon",
            ),
            true,
        );
        assert_eq!(
            diags.len(),
            2,
            "two location+message pairs must yield exactly two diagnostics, got {diags:?}",
        );
        assert_eq!(diags[0].file, "src/App.svelte");
        assert_eq!(diags[0].line, 12);
        assert!(
            diags[0].msg.contains("unused export"),
            "paired message must carry the svelte warning text, got {:?}",
            diags[0].msg,
        );
        assert!(
            diags[0].msg != "5",
            "the column must never be misread as the message",
        );
        assert_eq!(diags[1].file, "src/App.svelte");
        assert_eq!(diags[1].line, 13);
        assert!(
            diags[1].msg.contains("missing semicolon"),
            "second pair message must carry its text, got {:?}",
            diags[1].msg,
        );
        // The indented code-excerpt line and the blank line between the pairs
        // are not diagnostics and must be counted as skipped.
        assert_eq!(
            skipped, 2,
            "code-excerpt line and blank line must both be skipped",
        );
        Ok(())
    }

    // A GCC one-liner and a two-line pair in the same stream both parse with
    // pair_locations=true: the GCC line keeps its inline message, the bare
    // location line is not misread as a standalone diagnostic.
    #[test]
    fn mixed_gcc_one_liner_and_two_line_pair_both_parse() -> Result<()> {
        let regex = Regex::new(DEFAULT_MAP)?;
        let (diags, skipped) = parse_output(
            &regex,
            "src/a.rs:3:1: error: boom\n\
             src/App.svelte:12:5\n\
             Warn: unused export (svelte)",
            true,
        );
        assert_eq!(
            diags.len(),
            2,
            "GCC one-liner plus a two-line pair must yield two diagnostics, got {diags:?}",
        );
        // GCC one-liner preserves its inline message verbatim.
        assert_eq!(diags[0].file, "src/a.rs");
        assert_eq!(diags[0].line, 3);
        assert_eq!(diags[0].msg, "error: boom");
        // Two-line pair carries the following line as its message.
        assert_eq!(diags[1].file, "src/App.svelte");
        assert_eq!(diags[1].line, 12);
        assert!(
            diags[1].msg.contains("unused export"),
            "paired message must carry the warning text, got {:?}",
            diags[1].msg,
        );
        assert_eq!(
            skipped, 0,
            "no lines should be skipped in a clean mixed stream"
        );
        Ok(())
    }

    // With pair_locations=false (a custom --map), the two-line shape is NOT
    // paired: the bare location line has no inline message for the default map
    // and pairing is disabled, so both lines count as skipped and no
    // diagnostic is emitted.
    #[test]
    fn pair_locations_false_skips_two_line_shape() -> Result<()> {
        let regex = Regex::new(DEFAULT_MAP)?;
        let (diags, skipped) = parse_output(
            &regex,
            "src/App.svelte:12\n\
             Warn: unused export (svelte)",
            false,
        );
        assert!(
            diags.is_empty(),
            "with pairing off a bare location line must not produce a diagnostic, got {diags:?}",
        );
        assert_eq!(
            skipped, 2,
            "both the unpaired location line and its orphan message must be skipped",
        );
        Ok(())
    }

    // A blank line directly after a location line drops the pending location:
    // pairing across gaps would stitch unrelated output into bogus diagnostics.
    #[test]
    fn blank_line_after_location_drops_pending() -> Result<()> {
        let regex = Regex::new(DEFAULT_MAP)?;
        let (diags, skipped) = parse_output(&regex, "src/App.svelte:12:5\n\n", true);
        assert!(
            diags.is_empty(),
            "a blank line after a location must drop the pending location, got {diags:?}",
        );
        assert_eq!(
            skipped, 2,
            "the location line and its blank terminator must both count as skipped",
        );
        Ok(())
    }

    // Output ending on a bare location line with no following message leaves a
    // dangling location: no diagnostic is synthesized from a location alone.
    #[test]
    fn dangling_location_at_end_of_output_is_not_a_diagnostic() -> Result<()> {
        let regex = Regex::new(DEFAULT_MAP)?;
        let (diags, skipped) = parse_output(&regex, "src/App.svelte:12:5", true);
        assert!(
            diags.is_empty(),
            "a dangling location with no message must not yield a diagnostic, got {diags:?}",
        );
        assert_eq!(
            skipped, 1,
            "the dangling location line must be counted as skipped",
        );
        Ok(())
    }
}
