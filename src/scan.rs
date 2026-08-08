//! External diagnostic scan adapters.
//!
//! Plane: programmatic defect sensors. Adapters are repo-portable config stored in
//! meta, while their observations become ordinary derived Findings so the
//! existing finding adjudication flow stays the single triage surface.

use crate::model::{EdgeKind, Node, NodeType, TruthClass};
use crate::store::Store;
use crate::Result;
use anyhow::{anyhow, bail, Context};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};
use std::time::Duration;

const ADAPTERS_META_KEY: &str = "scan_adapters";
const DEFAULT_MAP: &str =
    r"^(?P<file>(?:[A-Za-z]:)?[^:\s][^:]*?):(?P<line>\d+)(?::\d+)?:\s*(?P<msg>.+)$";
/// A bare `file:line[:col]` location with no trailing message — the first half
/// of a two-line diagnostic (svelte-check-style human output).
const LOCATION_ONLY_MAP: &str = r"^(?P<file>(?:[A-Za-z]:)?[^:\s][^:]*?):(?P<line>\d+)(?::\d+)?\s*$";
pub(crate) const SCAN_TIMEOUT_SECS: u64 = 120;
const TITLE_MSG_LIMIT: usize = 96;

/// How an adapter's output is parsed into diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanFormat {
    /// Line-oriented text matched by a regex map (GCC-style default).
    #[default]
    Lines,
    /// A JSON array (or JSONL stream) of finding objects; `map` renames the
    /// looked-up fields (`file=…,line=…,msg=…,code=…`, dotted paths allowed).
    Json,
}

fn is_lines(format: &ScanFormat) -> bool {
    matches!(format, ScanFormat::Lines)
}

fn trusted_by_default() -> bool {
    true
}

fn is_trusted(trusted: &bool) -> bool {
    *trusted
}

/// A configured external diagnostic source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    /// Output format; `lines` is skipped in the export so pre-existing
    /// adapter configs stay byte-identical (INV-2 export determinism).
    #[serde(default, skip_serializing_if = "is_lines")]
    pub format: ScanFormat,
    /// Imported executable config is quarantined until its command is
    /// re-entered through `loom scan update --command` locally. `true` is
    /// omitted so existing/local adapter config stays byte-compatible.
    #[serde(default = "trusted_by_default", skip_serializing_if = "is_trusted")]
    pub trusted: bool,
}

/// Summary of one scan run.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScanReport {
    pub adapters_run: usize,
    pub diagnostics: usize,
    pub new_findings: usize,
    pub resolved_findings: usize,
    pub skipped_lines: usize,
    /// Diagnostics that parsed but pointed at a path loom does not track as a
    /// CodeFile (unregistered path, or a normalization mismatch). Counted so an
    /// all-unattached run is visible instead of silently resolving everything.
    pub unattached: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedDiagnostic {
    file: String,
    line: u64,
    msg: String,
    code: Option<String>,
}

/// Register a scan adapter in the store meta registry.
pub fn add_adapter(
    store: &Store,
    name: &str,
    command: &str,
    map: Option<&str>,
    format: ScanFormat,
) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("scan adapter name must not be empty");
    }
    if command.trim().is_empty() {
        bail!("scan adapter command must not be empty");
    }
    validate_map(format, map)?;

    let mut adapters = list_adapters(store)?;
    if adapters.iter().any(|a| a.name == name) {
        bail!("scan adapter '{name}' already exists");
    }
    adapters.push(Adapter {
        name: name.to_string(),
        command: command.to_string(),
        map: map.map(str::to_string),
        format,
        trusted: true,
    });
    write_adapters(store, &adapters)
}
/// Edit a registered scan adapter in place.
pub fn update_adapter(
    store: &Store,
    name: &str,
    command: Option<&str>,
    map: Option<&str>,
    format: Option<ScanFormat>,
) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("scan adapter name must not be empty");
    }
    if command.is_none() && map.is_none() && format.is_none() {
        bail!("nothing to update — pass --command, --map, and/or --format");
    }
    if let Some(command) = command {
        if command.trim().is_empty() {
            bail!("scan adapter command must not be empty");
        }
    }

    let mut adapters = list_adapters(store)?;
    let adapter = adapters
        .iter_mut()
        .find(|a| a.name == name)
        .ok_or_else(|| anyhow!("no scan adapter named '{name}'"))?;
    if let Some(command) = command {
        adapter.command = command.to_string();
        adapter.trusted = true;
    }
    if let Some(format) = format {
        adapter.format = format;
    }
    if let Some(map) = map {
        adapter.map = Some(map.to_string());
    }
    // The final combination must parse: a regex map under `lines`, a field map
    // under `json` — whichever of the pair changed.
    validate_map(adapter.format, adapter.map.as_deref())?;
    write_adapters(store, &adapters)
}

/// Validate an adapter's map for its format: a regex with named groups under
/// `lines`, a `field=path` list under `json`. A missing map is always valid
/// (both formats have defaults).
fn validate_map(format: ScanFormat, map: Option<&str>) -> Result<()> {
    match format {
        ScanFormat::Lines => match map {
            Some(map) => validate_map_regex(map),
            None => Ok(()),
        },
        ScanFormat::Json => json_field_map(map).map(|_| ()),
    }
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
/// Raw result of running one adapter's command — carries the exact config it ran
/// under, so the write phase reconciles against what actually executed even if
/// the registry changed underneath. No store access.
struct AdapterOutput {
    adapter: Adapter,
    output: String,
    exit_code: Option<i64>,
}

/// Run one adapter, or every configured adapter when `name` is `None`, holding
/// the passed store for the whole run. Kept for in-process callers and tests;
/// the CLI uses `run_unlocked` so external commands never execute under the
/// write lock.
pub fn run(store: &Store, root: &Path, name: Option<&str>) -> Result<ScanReport> {
    let adapters = select_adapters(store, name)?;
    let outputs = execute_adapters(root, adapters)?;
    write_results(store, root, &outputs)
}

/// The concurrency-friendly scan path. Reads adapter config under a SHARED lock,
/// runs the (possibly slow, 120s-timeout) external commands with NO lock held,
/// then reopens for a SHORT exclusive write to reconcile findings. During the
/// subprocess phase other agents can read and write the graph freely — this is
/// what stops one `loom scan` from freezing every other agent for minutes.
pub fn run_unlocked(root: &Path, name: Option<&str>) -> Result<ScanReport> {
    let adapters = {
        let store = Store::open_read(root)?;
        select_adapters(&store, name)?
    };
    let outputs = execute_adapters(root, adapters)?;
    let store = Store::open(root)?;
    write_results(&store, root, &outputs)
}

/// Phase 1 (no store): run each adapter's command and capture raw output. The
/// map (regex or JSON field list) is validated up front so a misconfigured
/// adapter fails before any subprocess work.
fn execute_adapters(root: &Path, adapters: Vec<Adapter>) -> Result<Vec<AdapterOutput>> {
    let mut outputs = Vec::with_capacity(adapters.len());
    for adapter in adapters {
        if !adapter.trusted {
            bail!(
                "scan adapter '{}' came from an import and its command is untrusted; review it, then run `loom scan update '{}' --command <reviewed-command>`",
                adapter.name,
                adapter.name
            );
        }
        validate_map(adapter.format, adapter.map.as_deref())?;
        let (output, exit_code) = run_adapter_command(root, &adapter.command, &adapter.name)?;
        outputs.push(AdapterOutput {
            adapter,
            output,
            exit_code,
        });
    }
    Ok(outputs)
}

/// Phase 2 (short write): parse captured output and reconcile findings. The
/// codefile registry and existing finding ids are re-read HERE, from the fresh
/// write store, so a change during the subprocess phase can never make the
/// stale-set reconciliation delete or restore the wrong derived findings.
fn write_results(store: &Store, root: &Path, outputs: &[AdapterOutput]) -> Result<ScanReport> {
    let codefiles = registered_codefiles(store, root)?;
    let mut report = ScanReport::default();
    // Re-read the registry from the fresh write store: an adapter whose config
    // changed or was removed during the subprocess phase must NOT be reconciled
    // against the config we ran under, or a concurrent `scan remove/update` would
    // make us upsert or resolve the wrong findings.
    let current = list_adapters(store)?;

    for out in outputs {
        let adapter = &out.adapter;
        if !current.iter().any(|a| a == adapter) {
            continue;
        }
        report.adapters_run += 1;
        let rule = ensure_adapter_rule(store, adapter)?;
        let existing = adapter_finding_ids(store, &adapter.name)?;
        // A command that could not run (127 not-found / 126 not-executable) would
        // otherwise parse to zero diagnostics and converge every prior finding to
        // resolved — silent data loss. Fail loudly instead (M-8). Linters that
        // merely FOUND issues exit non-zero WITH output, so they pass this gate.
        if matches!(out.exit_code, Some(126) | Some(127)) {
            bail!(
                "scan adapter '{}' failed to run (exit {}) — check its command; findings left untouched: {}",
                adapter.name,
                out.exit_code.unwrap_or(-1),
                out.output.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim()
            );
        }
        let (parsed, skipped) = match adapter.format {
            ScanFormat::Lines => {
                parse_output(&adapter_regex(adapter)?, &out.output, adapter.map.is_none())
            }
            ScanFormat::Json => {
                parse_json_output(&json_field_map(adapter.map.as_deref())?, &out.output)
            }
        };
        report.skipped_lines += skipped;
        // A non-zero exit that produced no recognizable diagnostics is a failed
        // run (a crash, a config error), not "every issue resolved" — a clean
        // pass exits 0. Converging then would wrongly resolve every prior finding
        // (M-8), so leave them untouched and let the next healthy run reconcile.
        let healthy_run = out.exit_code == Some(0) || !parsed.is_empty();
        let parsed_count = parsed.len();
        let mut attached = 0usize;
        let mut active = BTreeSet::new();

        for mut diagnostic in parsed {
            let Some(normalized) = normalize_path(root, &diagnostic.file) else {
                report.unattached += 1;
                continue;
            };
            diagnostic.file = normalized;
            let Some(codefile) = codefiles.get(&diagnostic.file) else {
                report.unattached += 1;
                continue;
            };

            attached += 1;
            let node = upsert_diagnostic(store, adapter, &rule, codefile, &diagnostic)?;
            if active.insert(node.id.clone()) && !existing.contains(&node.id) {
                report.new_findings += 1;
            }
            report.diagnostics += 1;
        }

        // Diagnostics parsed but NONE attached to a tracked CodeFile — a path
        // mapping failure (unregistered root, prefix mismatch), not "every issue
        // resolved". Converging here would wipe every prior finding just because
        // loom could not line up the paths, so leave them for a healthy run that
        // actually attaches (mirrors the M-8 exit-code guard).
        let attachment_failure = parsed_count > 0 && attached == 0;
        let stale: Vec<String> = existing.difference(&active).cloned().collect();
        if healthy_run && !attachment_failure && !stale.is_empty() {
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

/// Where diagnostic fields live inside a JSON finding object. Each entry is a
/// dotted path of object keys (`location.file`), pre-split for lookup. `items`
/// names the findings array inside an envelope object (qualirs-style
/// `{"summary":…, "smells":[…]}`); absent means the output IS the array.
#[derive(Debug, Clone, PartialEq, Eq)]
struct JsonFieldMap {
    items: Option<Vec<String>>,
    file: Vec<String>,
    line: Vec<String>,
    msg: Vec<String>,
    code: Vec<String>,
}

/// Parse a JSON-mode map spec: comma-separated `field=path` overrides of the
/// defaults (`file=file,line=line,msg=message,code=code`, no `items`). Paths
/// may be dotted for nested objects (`line=location.line_start`).
fn json_field_map(map: Option<&str>) -> Result<JsonFieldMap> {
    let split = |s: &str| s.split('.').map(str::to_string).collect::<Vec<_>>();
    let mut fields = JsonFieldMap {
        items: None,
        file: split("file"),
        line: split("line"),
        msg: split("message"),
        code: split("code"),
    };
    let Some(map) = map else {
        return Ok(fields);
    };
    for entry in map.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (field, path) = entry
            .split_once('=')
            .ok_or_else(|| anyhow!("scan adapter json map entry '{entry}' is not 'field=path'"))?;
        let path = path.trim();
        if path.is_empty() {
            bail!("scan adapter json map entry '{entry}' has an empty path");
        }
        match field.trim() {
            "items" => fields.items = Some(split(path)),
            "file" => fields.file = split(path),
            "line" => fields.line = split(path),
            "msg" => fields.msg = split(path),
            "code" => fields.code = split(path),
            other => {
                bail!(
                    "scan adapter json map field '{other}' is not one of items|file|line|msg|code"
                )
            }
        }
    }
    Ok(fields)
}

/// Parse JSON-mode adapter output: a top-level JSON array of finding objects,
/// or a JSONL stream (one object per line — non-JSON lines are skipped, so
/// build noise around the payload does not poison the run). A record without a
/// usable file or message is skipped; a missing/null line number means a
/// whole-file diagnostic and records as line 0.
fn parse_json_output(map: &JsonFieldMap, output: &str) -> (Vec<ParsedDiagnostic>, usize) {
    let mut diagnostics = Vec::new();
    let mut skipped = 0usize;
    for record in json_records(map, output, &mut skipped) {
        match json_diagnostic(map, &record) {
            Some(d) => diagnostics.push(d),
            None => skipped += 1,
        }
    }
    (diagnostics, skipped)
}

/// The finding objects of one output: a whole-string JSON document (tolerating
/// non-JSON noise before/after it), else per-line JSONL objects. With
/// `map.items` set, the document is an envelope object and the array lives at
/// that path; otherwise the document itself is the array.
fn json_records(map: &JsonFieldMap, output: &str, skipped: &mut usize) -> Vec<serde_json::Value> {
    if let Some(root) = json_root(output) {
        let found = match &map.items {
            Some(path) => json_lookup(&root, path).cloned(),
            None => Some(root),
        };
        if let Some(serde_json::Value::Array(items)) = found {
            return items;
        }
    }
    let mut items = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) if v.is_object() => items.push(v),
            _ => *skipped += 1,
        }
    }
    items
}

/// The one JSON document inside possibly noisy output: a clean whole-string
/// parse, else the first `[…]`/`{…}` value recovered by scanning each bracket
/// start. Splicing first-open to last-close lets a stray `[INFO]` banner poison
/// the span, so instead a streaming parse from each candidate offset returns the
/// first value that parses, tolerating trailing noise after it.
fn json_root(output: &str) -> Option<serde_json::Value> {
    if let Ok(v) = serde_json::from_str(output.trim()) {
        return Some(v);
    }
    for (idx, ch) in output.char_indices() {
        if ch == '[' || ch == '{' {
            let mut stream =
                serde_json::Deserializer::from_str(&output[idx..]).into_iter::<serde_json::Value>();
            if let Some(Ok(v)) = stream.next() {
                return Some(v);
            }
        }
    }
    None
}

/// One diagnostic from one JSON record, or None when the record lacks a file
/// or message at the mapped paths.
fn json_diagnostic(map: &JsonFieldMap, record: &serde_json::Value) -> Option<ParsedDiagnostic> {
    let file = json_lookup(record, &map.file)?.as_str()?.trim().to_string();
    if file.is_empty() {
        return None;
    }
    let msg = json_lookup(record, &map.msg)?.as_str()?.trim().to_string();
    if msg.is_empty() {
        return None;
    }
    // Numbers arrive as JSON numbers (pulse, qualirs) or numeric strings;
    // missing/null means a module/whole-file finding.
    let line = json_lookup(record, &map.line)
        .and_then(|v| match v {
            serde_json::Value::Number(n) => n.as_u64().or_else(|| {
                n.as_f64()
                    .filter(|f| f.is_finite() && *f >= 0.0)
                    .map(|f| f as u64)
            }),
            serde_json::Value::String(s) => s.trim().parse().ok(),
            _ => None,
        })
        .unwrap_or(0);
    let code = json_lookup(record, &map.code).and_then(|v| match v {
        serde_json::Value::String(s) => {
            let s = s.trim();
            (!s.is_empty()).then(|| s.to_string())
        }
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    });
    Some(ParsedDiagnostic {
        file,
        line,
        msg,
        code,
    })
}

/// Walk a dotted key path through nested JSON, descending object keys and, when
/// the current value is an array, a numeric segment indexes into it — so an
/// array envelope (ESLint's `[{…}]`) is reachable via e.g. `items=0.messages`.
fn json_lookup<'v>(
    record: &'v serde_json::Value,
    path: &[String],
) -> Option<&'v serde_json::Value> {
    let mut current = record;
    for key in path {
        current = match current {
            serde_json::Value::Array(items) => items.get(key.parse::<usize>().ok()?)?,
            _ => current.get(key)?,
        };
    }
    if current.is_null() {
        return None;
    }
    Some(current)
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
    // `exec 2>&1` merges stderr into stdout in emission order, so a diagnostic's
    // location and message lines stay adjacent for two-line pairing. The shared
    // runner bounds the capture and reaps the whole process group on timeout.
    let script = format!("exec 2>&1\n{command}");
    let Some(captured) =
        crate::subprocess::run(&script, root, Duration::from_secs(SCAN_TIMEOUT_SECS))
            .with_context(|| format!("running scan adapter '{adapter_name}'"))?
    else {
        bail!(
            "killed: scan adapter '{adapter_name}' exceeded scan_timeout_secs={SCAN_TIMEOUT_SECS}"
        );
    };
    Ok((
        String::from_utf8_lossy(&captured.stdout).into_owned(),
        captured.status.code().map(i64::from),
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
    fn default_map_parses_windows_drive_letter_path() -> Result<()> {
        let regex = Regex::new(DEFAULT_MAP)?;
        let diagnostic = parse_diagnostic(&regex, r"C:\src\a.rs:12: warning: boom")
            .ok_or_else(|| anyhow!("expected diagnostic"))?;
        assert_eq!(
            diagnostic.file, r"C:\src\a.rs",
            "the drive letter must stay part of the file path"
        );
        assert_eq!(diagnostic.line, 12);
        assert_eq!(diagnostic.msg, "warning: boom");
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
            ScanFormat::Lines,
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
        add_adapter(
            &store,
            "fake",
            "printf 'src/lib.rs:1: boom\\n'",
            None,
            ScanFormat::Lines,
        )?;

        let first = run(&store, root.path(), Some("fake"))?;
        assert_eq!(first.diagnostics, 1);
        assert_eq!(first.new_findings, 1);
        let findings = crate::signal::findings_view(&store)?;
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].state, "untriaged");
        assert!(findings[0].node.name.contains("src/lib.rs:1 boom"));

        remove_adapter(&store, "fake")?;
        add_adapter(&store, "fake", "printf ''", None, ScanFormat::Lines)?;
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

    // ---- JSON-format scan adapters ---------------------------------------

    // `json_field_map(None)` yields the documented defaults: file/line/message
    // paths, no items envelope.
    #[test]
    fn json_field_map_defaults_match_documented_paths() -> Result<()> {
        let m = json_field_map(None)?;
        assert_eq!(
            m.items, None,
            "items path must default to None (no envelope)"
        );
        assert_eq!(m.file, vec!["file"]);
        assert_eq!(m.line, vec!["line"]);
        assert_eq!(m.msg, vec!["message"]);
        assert_eq!(m.code, vec!["code"]);
        Ok(())
    }

    // An explicit `items=` plus a dotted path both parse into pre-split key
    // paths: the envelope path is captured and nested object lookup works.
    #[test]
    fn json_field_map_parses_items_and_dotted_paths() -> Result<()> {
        let m = json_field_map(Some("items=smells,file=location.file"))?;
        assert_eq!(m.items, Some(vec!["smells".to_string()]));
        assert_eq!(m.file, vec!["location", "file"], "dotted path must split");
        // Untouched fields keep their defaults.
        assert_eq!(m.line, vec!["line"]);
        assert_eq!(m.msg, vec!["message"]);
        assert_eq!(m.code, vec!["code"]);
        Ok(())
    }

    // Three malformed entries each reject at parse time so a misconfigured
    // adapter is rejected before any subprocess work: an empty path, an unknown
    // field name, and an entry with no `=` separator at all.
    #[test]
    fn json_field_map_rejects_empty_path_unknown_field_and_no_equals() -> Result<()> {
        assert!(
            json_field_map(Some("file=")).is_err(),
            "an empty path must error",
        );
        assert!(
            json_field_map(Some("notafield=x")).is_err(),
            "an unknown field name must error",
        );
        assert!(
            json_field_map(Some("garbage")).is_err(),
            "an entry without '=' must error",
        );
        Ok(())
    }

    // A top-level JSON array with a numeric `line` parses one diagnostic per
    // record. Numeric-string lines and missing/null lines map to u64 / 0.
    #[test]
    fn parse_json_output_array_line_coercions() -> Result<()> {
        let m = json_field_map(None)?;
        let out = r#"[
            {"file":"src/a.rs","line":12,"message":"boom"},
            {"file":"src/b.rs","line":"33","message":"warn"},
            {"file":"src/c.rs","message":"module-wide"}
        ]"#;
        let (diags, skipped) = parse_json_output(&m, out);
        assert_eq!(
            diags.len(),
            3,
            "every record has file+message, got {diags:?}"
        );
        assert_eq!(skipped, 0);
        assert_eq!(diags[0].file, "src/a.rs");
        assert_eq!(diags[0].line, 12, "numeric line must pass through");
        assert_eq!(diags[1].line, 33, "numeric-string line must parse");
        assert_eq!(
            diags[2].line, 0,
            "missing line must default to whole-file 0"
        );
        Ok(())
    }

    // A null line also falls to 0, and a record missing `file` or `message`
    // is dropped into the skipped count rather than emitted as a diagnostic.
    #[test]
    fn parse_json_output_null_line_and_missing_fields_skip_record() -> Result<()> {
        let m = json_field_map(None)?;
        let out = r#"[
            {"file":"src/a.rs","line":null,"message":"null line"},
            {"line":5,"message":"no file"},
            {"file":"src/b.rs","line":5}
        ]"#;
        let (diags, skipped) = parse_json_output(&m, out);
        assert_eq!(
            diags.len(),
            1,
            "only the null-line record is a real diagnostic"
        );
        assert_eq!(diags[0].line, 0, "null line must default to 0");
        assert_eq!(
            skipped, 2,
            "missing file and missing message records must each count as skipped",
        );
        Ok(())
    }

    // `code` accepts both a JSON string and a JSON number — pulse-style numeric
    // codes and qualirs-style string codes both surface verbatim.
    #[test]
    fn parse_json_output_code_accepts_string_and_number() -> Result<()> {
        let m = json_field_map(None)?;
        let out = r#"[
            {"file":"src/a.rs","line":1,"message":"x","code":"E42"},
            {"file":"src/b.rs","line":2,"message":"y","code":404}
        ]"#;
        let (diags, skipped) = parse_json_output(&m, out);
        assert_eq!(diags.len(), 2);
        assert_eq!(skipped, 0);
        assert_eq!(
            diags[0].code.as_deref(),
            Some("E42"),
            "string code passes through"
        );
        assert_eq!(
            diags[1].code.as_deref(),
            Some("404"),
            "numeric code is stringified"
        );
        Ok(())
    }

    // An envelope object `{"summary":…,"smells":[…]}` parses via `items=smells`:
    // the array is lifted out of the envelope and records become diagnostics.
    #[test]
    fn parse_json_output_envelope_via_items_path() -> Result<()> {
        let m = json_field_map(Some("items=smells"))?;
        let out = r#"{"summary":{"count":1},"smells":[{"file":"src/a.rs","line":7,"message":"smelly"}],"parse_errors":[]}"#;
        let (diags, skipped) = parse_json_output(&m, out);
        assert_eq!(
            diags.len(),
            1,
            "items=smells must lift the envelope array, got {diags:?}"
        );
        assert_eq!(diags[0].file, "src/a.rs");
        assert_eq!(diags[0].line, 7);
        assert_eq!(diags[0].msg, "smelly");
        assert_eq!(skipped, 0);
        Ok(())
    }

    // The SAME envelope object WITHOUT an `items` map yields zero diagnostics:
    // the document is an object (not an array), so json_records falls back to
    // JSONL parsing — each non-object top-level line is skipped. This pins the
    // "items= is required to read envelopes" contract.
    #[test]
    fn parse_json_output_envelope_without_items_yields_no_diagnostics() -> Result<()> {
        let m = json_field_map(None)?;
        let out =
            r#"{"summary":{"count":1},"smells":[{"file":"src/a.rs","line":7,"message":"smelly"}]}"#;
        let (diags, skipped) = parse_json_output(&m, out);
        assert!(
            diags.is_empty(),
            "an envelope object without items= must not produce diagnostics, got {diags:?}",
        );
        // The single envelope line is not a per-line object, so it counts skipped.
        assert!(
            skipped > 0,
            "the envelope line must be counted as skipped, got {skipped}"
        );
        Ok(())
    }

    // JSON embedded in leading/trailing noise: the outermost `[…]` span is
    // recovered and parsed, so build/banner noise around the payload does not
    // poison the run. This is the regression-prone parsing boundary.
    #[test]
    fn parse_json_output_recovers_array_embedded_in_noise() -> Result<()> {
        let m = json_field_map(None)?;
        let out = "Running linter...\n\n\
            [{\"file\":\"src/a.rs\",\"line\":1,\"message\":\"boom\"}]\n\
            Done. 1 warning.\n";
        let (diags, skipped) = parse_json_output(&m, out);
        assert_eq!(
            diags.len(),
            1,
            "embedded array must be recovered, got {diags:?}"
        );
        assert_eq!(diags[0].file, "src/a.rs");
        assert_eq!(diags[0].line, 1);
        assert_eq!(
            skipped, 0,
            "whole-string span recovery does not count lines skipped"
        );
        Ok(())
    }

    // JSON preceded by bracketed log-banner lines: a stray `[INFO]` bracket must
    // not poison the span. The scanning parse skips the banner and recovers the
    // real payload that follows.
    #[test]
    fn parse_json_output_recovers_array_after_bracketed_log_noise() -> Result<()> {
        let m = json_field_map(None)?;
        let out = "[INFO] starting linter\n\
            [WARN] one issue found\n\
            [{\"file\":\"src/a.rs\",\"line\":9,\"message\":\"boom\"}]\n";
        let (diags, skipped) = parse_json_output(&m, out);
        assert_eq!(
            diags.len(),
            1,
            "the payload after bracketed banners must be recovered, got {diags:?}"
        );
        assert_eq!(diags[0].file, "src/a.rs");
        assert_eq!(diags[0].line, 9);
        assert_eq!(skipped, 0);
        Ok(())
    }

    // ESLint's envelope is a top-level ARRAY whose first element carries the
    // `messages` array. A numeric path segment must index the array so
    // `items=0.messages` lifts the diagnostics out.
    #[test]
    fn parse_json_output_eslint_array_envelope_via_numeric_index() -> Result<()> {
        let m = json_field_map(Some("items=0.messages,line=line,msg=message"))?;
        let out = r#"[{"filePath":"src/a.rs","messages":[
            {"file":"src/a.rs","line":3,"message":"no-unused-vars"},
            {"file":"src/b.rs","line":7,"message":"eqeqeq"}
        ]}]"#;
        let (diags, skipped) = parse_json_output(&m, out);
        assert_eq!(
            diags.len(),
            2,
            "items=0.messages must index the array envelope, got {diags:?}"
        );
        assert_eq!(diags[0].line, 3);
        assert_eq!(diags[1].line, 7);
        assert_eq!(skipped, 0);
        Ok(())
    }

    // A JSON float line value coerces to a truncated u64 rather than silently
    // falling to whole-file 0 (which would churn duplicate findings).
    #[test]
    fn parse_json_output_float_line_coerces_to_integer() -> Result<()> {
        let m = json_field_map(None)?;
        let out = r#"[{"file":"src/a.rs","line":12.0,"message":"boom"}]"#;
        let (diags, skipped) = parse_json_output(&m, out);
        assert_eq!(diags.len(), 1);
        assert_eq!(
            diags[0].line, 12,
            "a float line must truncate to its integer, not fall to 0"
        );
        assert_eq!(skipped, 0);
        Ok(())
    }

    // Pure JSONL: one object per line. A non-JSON garbage line is skipped, the
    // valid object lines parse into diagnostics. Pins JSONL + skip counting.
    #[test]
    fn parse_json_output_jsonl_skips_garbage_line() -> Result<()> {
        let m = json_field_map(None)?;
        let out = "{\"file\":\"src/a.rs\",\"line\":1,\"message\":\"boom\"}\n\
            this is not json\n\
            {\"file\":\"src/b.rs\",\"line\":2,\"message\":\"warn\"}";
        let (diags, skipped) = parse_json_output(&m, out);
        assert_eq!(
            diags.len(),
            2,
            "the two valid JSONL objects must parse, got {diags:?}"
        );
        assert_eq!(diags[0].file, "src/a.rs");
        assert_eq!(diags[1].file, "src/b.rs");
        assert_eq!(
            skipped, 1,
            "the garbage line must count as exactly one skipped"
        );
        Ok(())
    }

    // End-to-end: a JSON adapter is registered against a `printf` command that
    // emits a small JSON array. The diagnostic must create a finding for the
    // registered codefile, a re-run must report zero new (det-key stability),
    // and removing the record from the output must resolve the finding.
    #[test]
    fn json_adapter_end_to_end_create_converge_resolve() -> Result<()> {
        let root = TestRoot::new("json_e2e")?;
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
            "pulse",
            r#"printf '[{"file":"src/lib.rs","start_line":3,"detail":"bad import"}]\n'"#,
            Some("line=start_line,msg=detail"),
            ScanFormat::Json,
        )?;

        let first = run(&store, root.path(), Some("pulse"))?;
        assert_eq!(first.adapters_run, 1);
        assert_eq!(
            first.diagnostics, 1,
            "the one JSON record must become one diagnostic"
        );
        assert_eq!(first.new_findings, 1);
        assert_eq!(first.resolved_findings, 0);
        assert_eq!(first.skipped_lines, 0);

        let findings = crate::signal::findings_view(&store)?;
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].node.status, "external_diagnostic");
        assert!(
            findings[0].node.name.contains("src/lib.rs:3"),
            "finding name must carry the JSON record's location, got {:?}",
            findings[0].node.name,
        );

        // Re-run with identical output: det-key stability means zero new and
        // zero resolved — the existing finding is reconciled, not re-created.
        let second = run(&store, root.path(), Some("pulse"))?;
        assert_eq!(second.diagnostics, 1);
        assert_eq!(
            second.new_findings, 0,
            "re-run must not duplicate the finding"
        );
        assert_eq!(second.resolved_findings, 0);
        assert_eq!(crate::signal::findings_view(&store)?.len(), 1);

        // Switch the command to emit an empty array: the prior finding has no
        // active diagnostic and must be resolved.
        update_adapter(&store, "pulse", Some(r#"printf '[]\n'"#), None, None)?;
        let third = run(&store, root.path(), Some("pulse"))?;
        assert_eq!(third.diagnostics, 0);
        assert_eq!(third.new_findings, 0);
        assert_eq!(
            third.resolved_findings, 1,
            "removing the record must resolve the finding"
        );
        assert!(crate::signal::findings_view(&store)?.is_empty());
        Ok(())
    }

    // A JSON adapter with a custom field map via `items=`+dotted paths works
    // end-to-end against a qualirs-shaped envelope emitted by `printf`.
    #[test]
    fn json_adapter_envelope_map_end_to_end() -> Result<()> {
        let root = TestRoot::new("json_env")?;
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
            "qualirs",
            r#"printf '{"summary":{},"smells":[{"file":"src/lib.rs","line":4,"detail":"missing"}]}\n'"#,
            Some("items=smells,msg=detail"),
            ScanFormat::Json,
        )?;
        let report = run(&store, root.path(), Some("qualirs"))?;
        assert_eq!(
            report.diagnostics, 1,
            "envelope items + msg=detail must yield one diagnostic"
        );
        let findings = crate::signal::findings_view(&store)?;
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].node.name.contains("missing"),
            "the detail field must surface as the message, got {:?}",
            findings[0].node.name,
        );
        Ok(())
    }

    // Registering a JSON adapter with a map that fails `json_field_map` (an
    // unknown field) must be rejected BEFORE the adapter is written to meta —
    // so the registry is left empty and a subsequent run finds no adapters.
    #[test]
    fn add_adapter_json_invalid_map_errors_before_registration() -> Result<()> {
        let root = TestRoot::new("json_bad")?;
        let store = Store::init(root.path(), Some("scan-test"), false)?;
        let err = add_adapter(
            &store,
            "bad",
            "printf '[]'",
            Some("notafield=x"),
            ScanFormat::Json,
        );
        assert!(err.is_err(), "an invalid json map must reject registration");
        assert!(
            list_adapters(&store)?.is_empty(),
            "a rejected registration must leave the registry empty",
        );
        Ok(())
    }

    // `update_adapter` switching format Lines→Json must validate the EXISTING
    // map against the new format: a regex map is not a valid JSON field list, so
    // the switch must error and the adapter must keep its lines format + regex
    // map intact (the update is atomic — no half-applied state).
    #[test]
    fn update_adapter_lines_to_json_validates_existing_map_against_new_format() -> Result<()> {
        let root = TestRoot::new("json_switch")?;
        let store = Store::init(root.path(), Some("scan-test"), false)?;
        // A lines adapter with a custom regex map (invalid as a json field list:
        // no '=' separator, and even if it had one the field name would be wrong).
        let map = r"^\[(?P<code>[A-Z]\d+)\] (?P<file>.+?)@(?P<line>\d+) (?P<msg>.+)$";
        add_adapter(&store, "sw", "printf ''", Some(map), ScanFormat::Lines)?;

        let err = update_adapter(&store, "sw", None, None, Some(ScanFormat::Json));
        assert!(
            err.is_err(),
            "switching to json with a regex map must error",
        );
        let adapter = list_adapters(&store)?
            .into_iter()
            .find(|a| a.name == "sw")
            .ok_or_else(|| anyhow!("adapter must still exist after a failed update"))?;
        assert_eq!(
            adapter.format,
            ScanFormat::Lines,
            "format must be unchanged on failure"
        );
        assert_eq!(
            adapter.map.as_deref(),
            Some(map),
            "the regex map must be unchanged on failure",
        );
        Ok(())
    }

    // Serde byte-stability: a Lines adapter serializes with NO `format` key
    // (pre-existing adapter configs stay byte-identical — INV-2 export
    // determinism), and a Json adapter serializes `"format":"json"`.
    #[test]
    fn adapter_serde_lines_omits_format_and_json_writes_it() -> Result<()> {
        let lines = Adapter {
            name: "lint".into(),
            command: "printf ''".into(),
            map: None,
            format: ScanFormat::Lines,
            trusted: true,
        };
        let serialized = serde_json::to_string(&lines)?;
        assert!(
            !serialized.contains("\"format\""),
            "a Lines adapter must not serialize a format key, got {serialized}",
        );
        // Round-trips back as Lines.
        let back: Adapter = serde_json::from_str(&serialized)?;
        assert_eq!(back, lines);

        let json = Adapter {
            name: "pulse".into(),
            command: "printf '[]'".into(),
            map: None,
            format: ScanFormat::Json,
            trusted: true,
        };
        let serialized_json = serde_json::to_string(&json)?;
        assert!(
            serialized_json.contains("\"format\":\"json\""),
            "a Json adapter must serialize format:\"json\", got {serialized_json}",
        );
        Ok(())
    }

    // Legacy compatibility: deserializing an adapter config that has NO
    // `format` key (the pre-JSON-format export shape) yields Lines — the
    // `#[serde(default)]` on the field keeps old configs readable.
    #[test]
    fn adapter_deserialize_legacy_without_format_defaults_to_lines() -> Result<()> {
        let legacy = r#"{"name":"lint","command":"printf ''"}"#;
        let adapter: Adapter = serde_json::from_str(legacy)?;
        assert_eq!(adapter.name, "lint");
        assert!(adapter.trusted, "legacy local adapters remain trusted");
        assert_eq!(
            adapter.format,
            ScanFormat::Lines,
            "missing format must default to Lines"
        );
        Ok(())
    }

    #[test]
    fn imported_untrusted_adapter_is_not_executed() {
        let adapter = Adapter {
            name: "imported".into(),
            command: "exit 0".into(),
            map: None,
            format: ScanFormat::Lines,
            trusted: false,
        };
        let error = execute_adapters(std::path::Path::new("."), vec![adapter])
            .err()
            .expect("untrusted adapter must be rejected");
        assert!(error.to_string().contains("command is untrusted"));
    }
}
