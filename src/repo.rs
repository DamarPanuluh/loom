//! Filesystem introspection: a .gitignore-aware file walk and programmable
//! stack detection. Pure FS concerns (no DB) — shared by `loom coverage` and
//! `loom detect`, and the basis for guide mode-detection (greenfield vs
//! brownfield).

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::vec_utils::push_unique_nonempty;

/// Build/dependency/VCS directories that are never source — skipped
/// unconditionally so coverage is meaningful even in a repo with no `.gitignore`
/// (e.g. cargo's `target/` holds thousands of artifacts). `.gitignore` is still
/// honored on top of this.
const DEFAULT_SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "vendor",
    "__pycache__",
    "venv",
    ".venv",
    "dist",
    ".git",
    ".loom",
];

/// Relative file paths under `root`, respecting `.gitignore`/`.ignore`, skipping
/// hidden entries and well-known build/dependency dirs. Directories excluded.
pub fn walk_files(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    // require_git(false): honor .gitignore/.ignore even when this isn't a git
    // repo (or we're in a subdir), so coverage's denominator is meaningful.
    for result in WalkBuilder::new(root)
        .hidden(true)
        .require_git(false)
        .build()
    {
        let entry =
            result.with_context(|| format!("Failed while walking repo '{}'", root.display()))?;
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(root) {
            let Some(s) = rel.to_str().map(|s| s.replace('\\', "/")) else {
                continue;
            };
            if s.is_empty() {
                continue;
            }
            // Skip anything under a default build/dep dir.
            if s.split('/').any(|seg| DEFAULT_SKIP_DIRS.contains(&seg)) {
                continue;
            }
            files.push(s);
        }
    }
    Ok(files)
}

/// The file's modification time as an RFC3339 string, or `None` if it can't be
/// read. Used to stamp CodeFiles at registration so the first `sync` is a no-op.
pub fn mtime_rfc3339(path: &Path) -> Option<String> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let dt: chrono::DateTime<chrono::Utc> = modified.into();
    Some(dt.to_rfc3339())
}

/// FNV-1a 64-bit hash of raw bytes, as lowercase hex. `loom sync`'s change
/// detector: mtime alone false-flags after checkout/rebase (timestamps churn,
/// bytes don't), so "changed" is decided by content. Not cryptographic — just a
/// cheap, dependency-free fingerprint, which is all change detection needs.
pub fn content_hash(bytes: &[u8]) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = FNV_OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    format!("{h:016x}")
}

/// Pairwise git co-change counts among a set of known repo-relative paths, plus
/// how many of the scanned commits each path appears in. The evidence behind the
/// `cochange_coupling` smell: files that keep changing together are coupled even
/// when neither imports the other. Best-effort — returns empty on any failure
/// (no git, not a repo, no history), so callers degrade silently.
#[derive(Debug, Default)]
pub struct CoChange {
    /// (path_a, path_b) sorted → number of scanned commits that touched both.
    pub pairs: std::collections::HashMap<(String, String), usize>,
    /// path → number of scanned commits that touched it (the confidence denom).
    pub individual: std::collections::HashMap<String, usize>,
}

fn record_cochange_event(cc: &mut CoChange, files: &[String]) {
    for f in files {
        *cc.individual.entry(f.clone()).or_insert(0) += 1;
    }
    for i in 0..files.len() {
        for j in (i + 1)..files.len() {
            // files is sorted, so (i, j) is already the canonical order.
            *cc.pairs
                .entry((files[i].clone(), files[j].clone()))
                .or_insert(0) += 1;
        }
    }
}

fn nul_paths(output: &[u8]) -> impl Iterator<Item = String> + '_ {
    output
        .split(|b| *b == 0)
        .filter(|raw| !raw.is_empty())
        .filter_map(|raw| std::str::from_utf8(raw).ok())
        .map(|s| s.replace('\\', "/"))
}

fn git_known_pending_paths(root: &Path, paths: &HashSet<String>) -> Vec<String> {
    let mut changed: Vec<String> = Vec::new();
    for args in [
        &["diff", "--name-only", "-z", "HEAD", "--"][..],
        &["ls-files", "--others", "--exclude-standard", "-z"][..],
    ] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output();
        let Ok(output) = output else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        changed.extend(nul_paths(&output.stdout).filter(|p| paths.contains(p)));
    }
    changed.sort_unstable();
    changed.dedup();
    changed
}

/// Mine `git log` for evolutionary coupling among `paths`, over the last
/// `last_n` non-merge commits. Only files in `paths` (the graph's CodeFiles)
/// count, so the cost is bounded by history depth, not repo size. Pending
/// staged/worktree changes are counted as one synthetic newest commit, so a
/// clean smell report stays stable across the commit that records those paths.
pub fn git_cochange(root: &Path, paths: &HashSet<String>, last_n: usize) -> CoChange {
    let mut cc = CoChange::default();
    if paths.is_empty() {
        return cc;
    }
    let pending = git_known_pending_paths(root, paths);
    let log_limit = if pending.is_empty() {
        last_n
    } else {
        last_n.saturating_sub(1)
    };
    // `--format=%x00` prints only a NUL per commit, then --name-only lists its
    // files. Splitting stdout on NUL yields one chunk of file paths per commit.
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("log")
        .arg("--no-merges")
        .arg(format!("-n{log_limit}"))
        .arg("--name-only")
        .arg("--format=%x00")
        .output();
    let Ok(output) = output else {
        return cc;
    };
    if !output.status.success() {
        return cc;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for commit in text.split('\u{0}') {
        let mut files: Vec<String> = commit
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && paths.contains(*l))
            .map(ToString::to_string)
            .collect();
        files.sort_unstable();
        files.dedup();
        record_cochange_event(&mut cc, &files);
    }
    if !pending.is_empty() {
        record_cochange_event(&mut cc, &pending);
    }
    cc
}

/// Confine a registered path to the graph root: `.`/`..` fold lexically first,
/// symlinked components are resolved through the nearest existing ancestor, the
/// result must stay under `root`, and the returned form is root-relative with
/// `/` separators. `None` = the path escapes the root.
///
/// The graph is untrusted input (imports and hand edits travel in
/// loom.graph.json): a CodeFile path like `/etc/passwd`, `../../secret`, or a
/// root-local symlink to a file outside the repo must never reach `fs::read` —
/// sync hashes file bytes and probes locator substrings, which would otherwise
/// answer "is this string in that file?" for any readable file on the machine.
/// Absolute paths are accepted iff they resolve under `root` (and come back
/// relative, the stored convention). Resolving filesystem metadata reveals no
/// contents; bytes are never read here.
pub fn confine(root: &Path, path: &Path) -> Option<String> {
    use std::ffi::OsString;
    use std::path::{Component, Path, PathBuf};
    fn finish(rel: &Path) -> Option<String> {
        if rel.as_os_str().is_empty() {
            return None; // the root itself is not a file path
        }
        Some(rel.to_str()?.replace('\\', "/"))
    }
    fn canonicalize_with_missing_tail(path: &Path) -> Option<PathBuf> {
        let mut cursor = path;
        let mut tail: Vec<OsString> = Vec::new();
        loop {
            if let Ok(mut base) = cursor.canonicalize() {
                for component in tail.iter().rev() {
                    base.push(component);
                }
                return Some(base);
            }
            tail.push(cursor.file_name()?.to_os_string());
            cursor = cursor.parent()?;
        }
    }
    fn normalize_lexically(path: &Path) -> Option<PathBuf> {
        let mut resolved = PathBuf::new();
        for c in path.components() {
            match c {
                Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                    resolved.push(c)
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    if !resolved.pop() {
                        return None; // walked above the filesystem root
                    }
                }
            }
        }
        Some(resolved)
    }
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let resolved = normalize_lexically(&joined)?;
    match (
        root.canonicalize().ok(),
        canonicalize_with_missing_tail(&resolved),
    ) {
        (Some(croot), Some(cpath)) => finish(cpath.strip_prefix(&croot).ok()?),
        _ => {
            let lroot = normalize_lexically(root)?;
            finish(resolved.strip_prefix(&lroot).ok()?)
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LangCount {
    pub language: String,
    pub files: usize,
}

/// A quality-pack recommendation: which vantage point fits this repo, and the
/// disk evidence behind the suggestion. Seed with `loom rule seed <pack>`.
#[derive(Debug, Clone, Serialize)]
pub struct PackHint {
    pub pack: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Detection {
    /// Count of recognised source files (excludes docs/config/assets).
    pub source_files: usize,
    pub has_source: bool,
    /// Stacks inferred from manifests present at the root.
    pub stacks: Vec<String>,
    pub top_languages: Vec<LangCount>,
    /// Baseline mode from disk alone: "greenfield" if no source, else
    /// "brownfield". (Refactor is graph-dependent — the guide refines this.)
    pub suggested_mode: String,
    /// Quality packs that fit this repo's kind (`loom rule seed <pack>`) —
    /// the vantage points for 360° normative coverage, suggested by the
    /// binary so the agent doesn't have to remember them.
    pub recommended_packs: Vec<PackHint>,
}

/// Detect the repo's stack and whether there's existing source to map.
pub fn detect(root: &Path) -> Result<Detection> {
    let files = walk_files(root)?;

    let manifests: &[(&str, &str)] = &[
        ("Cargo.toml", "rust"),
        ("package.json", "node"),
        ("go.mod", "go"),
        ("pyproject.toml", "python"),
        ("requirements.txt", "python"),
        ("setup.py", "python"),
        ("pom.xml", "java"),
        ("build.gradle", "java/kotlin"),
        ("Gemfile", "ruby"),
        ("composer.json", "php"),
        ("CMakeLists.txt", "c/cpp"),
        ("Package.swift", "swift"),
        ("pubspec.yaml", "dart/flutter"),
        ("bun.lock", "bun"),
        ("bun.lockb", "bun"),
    ];
    let mut stacks: Vec<String> = Vec::new();
    for (file, name) in manifests {
        if root.join(file).exists() && !stacks.iter().any(|s| s == name) {
            stacks.push((*name).to_string());
        }
    }
    if ["svelte.config.js", "svelte.config.ts", "svelte.config.mjs"]
        .iter()
        .any(|file| root.join(file).exists())
        && !stacks.iter().any(|s| s == "svelte")
    {
        stacks.push("svelte".to_string());
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut source_files = 0usize;
    for f in &files {
        let lang = lang_of(f);
        if lang != "other" {
            source_files += 1;
            *counts.entry(lang.to_string()).or_insert(0) += 1;
        }
    }
    let mut top_languages: Vec<LangCount> = counts
        .into_iter()
        .map(|(language, files)| LangCount { language, files })
        .collect();
    top_languages.sort_by_key(|language| std::cmp::Reverse(language.files));
    top_languages.truncate(8);

    let has_source = source_files > 0;
    let recommended_packs = recommend_packs(root, &stacks, &files);
    Ok(Detection {
        source_files,
        has_source,
        stacks,
        top_languages,
        suggested_mode: if has_source {
            "brownfield".into()
        } else {
            "greenfield".into()
        },
        recommended_packs,
    })
}

/// Map disk evidence → quality packs (the repo-kind vantage points). Each hint
/// carries its evidence; the agent decides — seeding is never automatic.
fn recommend_packs(root: &Path, stacks: &[String], files: &[String]) -> Vec<PackHint> {
    let mut packs = vec![PackHint {
        pack: "iso5055".into(),
        reason: "baseline — reliability/security/performance/maintainability apply to any code"
            .into(),
    }];

    let ext_of = |f: &str| {
        Path::new(f)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
    };
    let has_ext = |exts: &[&str]| files.iter().any(|f| exts.contains(&ext_of(f).as_str()));
    let has_dir = |dirs: &[&str]| {
        files.iter().any(|f| {
            f.split('/')
                .next()
                .map(|d| dirs.contains(&d))
                .unwrap_or(false)
        })
    };

    if stacks
        .iter()
        .any(|s| s == "swift" || s == "java/kotlin" || s == "dart/flutter")
        || has_ext(&["swift", "kt", "kts", "dart"])
        || has_dir(&["ios", "android"])
        || files
            .iter()
            .any(|f| f == "pubspec.yaml" || f.ends_with("/pubspec.yaml"))
    {
        packs.push(PackHint {
            pack: "mobile".into(),
            reason: "mobile platform code detected (swift/kotlin/flutter or ios|android dirs) — lifecycle, offline, permissions, main thread".into(),
        });
    }
    if stacks.iter().any(|s| s == "svelte")
        || has_ext(&["tsx", "jsx", "vue", "svelte", "html", "css"])
    {
        packs.push(PackHint {
            pack: "web-ui".into(),
            reason: "frontend files detected (tsx/jsx/vue/svelte/html/css) — view states, accessibility, XSS, client-side trust".into(),
        });
    }
    if stacks.iter().any(|s| {
        matches!(
            s.as_str(),
            "rust" | "go" | "node" | "bun" | "python" | "java" | "java/kotlin" | "ruby" | "php"
        )
    }) || root.join("Dockerfile").exists()
        || root.join("docker-compose.yml").exists()
    {
        packs.push(PackHint {
            pack: "service".into(),
            reason: "backend-capable stack detected — applies where this code exposes or consumes service interfaces (contracts, idempotency, timeouts, sagas)".into(),
        });
        packs.push(PackHint {
            pack: "concurrency".into(),
            reason: "backend-capable stack detected — applies where this code shares state across threads/tasks or has hot paths worth a proven budget (sync discipline, lock hygiene, benchmarks)".into(),
        });
    }
    if has_ext(&["sql"])
        || files
            .iter()
            .any(|f| f.split('/').any(|seg| seg == "migrations"))
    {
        packs.push(PackHint {
            pack: "data".into(),
            reason: "SQL/migrations detected — migration safety, ingest validation, loss accounting, PII".into(),
        });
    }
    packs
}

// ---------------------------------------------------------------------------
// Grounding truth helpers: locator presence + static import extraction
// ---------------------------------------------------------------------------

/// True when an IMPLEMENTS locator can still be found in the file's content.
/// Empty locators are vacuously present (file-level grounding).
pub fn locator_present(content: &str, locator: &str) -> bool {
    let l = locator.trim();
    l.is_empty() || content.contains(l)
}

/// Best-effort static import extraction: repo-relative paths of files that
/// `rel_path` references. Heuristic, language-aware (rust / js-ts / python),
/// and conservative — a candidate is only returned if it exists under `root`.
/// This is the physical-plane evidence smells reconciles against the semantic
/// graph (undeclared coupling), so false negatives are fine; false positives
/// are not.
#[cfg(test)]
pub fn extract_imports(root: &Path, rel_path: &str, content: &str) -> Vec<String> {
    extract_physical_facts(root, rel_path, content).imports
}

/// Top-level canonical syntax symbols in `rel_path`. Empty in feature-light
/// builds or unsupported languages; this is diagnostic physical evidence only.
#[cfg(test)]
pub fn extract_symbols(rel_path: &str, content: &str) -> Vec<String> {
    #[cfg(feature = "treesitter")]
    if let Some(facts) = crate::ts_imports::extract_physical_facts(rel_path, content) {
        return facts.symbols;
    }
    let mut symbols: Vec<String> = extract_symbol_facts_heuristic(rel_path, content)
        .into_iter()
        .map(|fact| fact.label)
        .collect();
    symbols.sort();
    symbols.dedup();
    symbols
}

#[derive(Debug, Default)]
pub(crate) struct PhysicalFacts {
    pub imports: Vec<String>,
    pub symbols: Vec<String>,
    pub symbol_facts: Vec<crate::types::SymbolFact>,
}

pub(crate) fn extract_physical_facts(root: &Path, rel_path: &str, content: &str) -> PhysicalFacts {
    #[cfg(feature = "treesitter")]
    if let Some(facts) = crate::ts_imports::extract_physical_facts(rel_path, content) {
        return PhysicalFacts {
            imports: resolve_import_specifiers(root, rel_path, &facts.import_specifiers),
            symbols: facts.symbols,
            symbol_facts: facts.symbol_facts,
        };
    }
    let mut symbol_facts = extract_symbol_facts_heuristic(rel_path, content);
    let mut symbols: Vec<String> = symbol_facts.iter().map(|fact| fact.label.clone()).collect();
    symbols.sort();
    symbols.dedup();
    symbol_facts.sort_by(|a, b| {
        a.label
            .cmp(&b.label)
            .then_with(|| a.line_start.cmp(&b.line_start))
            .then_with(|| a.line_end.cmp(&b.line_end))
    });
    symbol_facts.dedup_by(|a, b| a.label == b.label);
    PhysicalFacts {
        imports: extract_imports_heuristic(root, rel_path, content),
        symbols,
        symbol_facts,
    }
}

/// The original dependency-free scanner. It stays compiled in every build as
/// the universal fallback for unsupported languages and `--no-default-features`.
pub(crate) fn extract_imports_heuristic(root: &Path, rel_path: &str, content: &str) -> Vec<String> {
    let mut specs: Vec<String> = Vec::new();

    let ext = Path::new(rel_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let dir = Path::new(rel_path)
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();

    match ext {
        "rs" => {
            for line in content.lines() {
                let t = line.trim().strip_prefix("pub ").unwrap_or(line.trim());
                // `mod x;` → sibling module file.
                if let Some(rest) = t.strip_prefix("mod ") {
                    if let Some(name) = rest.strip_suffix(';') {
                        let name = name.trim();
                        if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                            push_unique_nonempty(&mut specs, format!("mod:{name}"));
                        }
                    }
                }
                // `use crate::a::b::…` → src/a.rs | src/a/mod.rs | src/a/b.rs | …
                if let Some(rest) = t.strip_prefix("use crate::") {
                    let path_part: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
                        .collect();
                    if !path_part.is_empty() {
                        push_unique_nonempty(&mut specs, format!("crate::{path_part}"));
                    }
                }
            }
        }
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "svelte" => {
            for line in content.lines() {
                for marker in [
                    "from '",
                    "from \"",
                    "require('",
                    "require(\"",
                    "import('",
                    "import(\"",
                ] {
                    if let Some(idx) = line.find(marker) {
                        let rest = &line[idx + marker.len()..];
                        let spec: String = rest
                            .chars()
                            .take_while(|c| *c != '\'' && *c != '"')
                            .collect();
                        if !spec.starts_with('.') {
                            continue; // package import, not a repo file
                        }
                        push_unique_nonempty(&mut specs, spec);
                    }
                }
            }
        }
        "dart" => {
            for line in content.lines() {
                let t = line.trim();
                for prefix in ["import ", "export ", "part "] {
                    if let Some(rest) = t.strip_prefix(prefix) {
                        if let Some(spec) = first_quoted(rest) {
                            if spec.starts_with('.') || spec.starts_with("package:") {
                                push_unique_nonempty(&mut specs, spec);
                            }
                        }
                    }
                }
            }
        }
        "go" => {
            let mut in_block = false;
            for line in content.lines() {
                let t = line.trim();
                if t.starts_with("import (") {
                    in_block = true;
                    continue;
                }
                if in_block && t.starts_with(')') {
                    in_block = false;
                    continue;
                }
                if in_block {
                    if let Some(spec) = first_quoted(t) {
                        push_unique_nonempty(&mut specs, spec);
                    }
                } else if let Some(rest) = t.strip_prefix("import ") {
                    if let Some(spec) = first_quoted(rest) {
                        push_unique_nonempty(&mut specs, spec);
                    }
                }
            }
        }
        "kt" | "kts" | "swift" => {
            for line in content.lines() {
                let t = line.trim();
                if let Some(rest) = t.strip_prefix("import ") {
                    push_unique_nonempty(
                        &mut specs,
                        rest.split_whitespace()
                            .next()
                            .unwrap_or("")
                            .trim_end_matches(';')
                            .to_string(),
                    );
                }
            }
        }
        "py" => {
            for line in content.lines() {
                let t = line.trim();
                let module: Option<(String, bool)> = if let Some(rest) = t.strip_prefix("from ") {
                    rest.split_whitespace()
                        .next()
                        .map(|m| (m.to_string(), true))
                } else {
                    t.strip_prefix("import ").and_then(|rest| {
                        rest.split([' ', ','])
                            .next()
                            .map(|m| (m.to_string(), false))
                    })
                };
                if let Some((m, _)) = module {
                    let (base, name) = if let Some(stripped) = m.strip_prefix('.') {
                        // relative: each extra leading dot climbs a directory
                        let ups = stripped.chars().take_while(|c| *c == '.').count();
                        let name = stripped.trim_start_matches('.');
                        let mut d = dir.clone();
                        for _ in 0..ups {
                            d = Path::new(&d)
                                .parent()
                                .map(|p| p.to_string_lossy().into_owned())
                                .unwrap_or_default();
                        }
                        (d, name.to_string())
                    } else {
                        (String::new(), m)
                    };
                    let spec = if base.is_empty() {
                        name
                    } else if name.is_empty() {
                        base
                    } else if base == "." {
                        name
                    } else {
                        format!("{base}.{name}")
                    };
                    push_unique_nonempty(&mut specs, spec);
                }
            }
        }
        _ => {}
    }
    resolve_import_specifiers(root, rel_path, &specs)
}

fn resolve_import_specifiers(root: &Path, rel_path: &str, specs: &[String]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let ext = Path::new(rel_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let dir = Path::new(rel_path)
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();

    for spec in specs {
        match ext {
            "rs" => resolve_rust_spec(root, rel_path, &dir, spec, &mut found),
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "svelte" => {
                resolve_js_spec(root, rel_path, &dir, spec, &mut found)
            }
            "py" => resolve_python_spec(root, rel_path, &dir, spec, &mut found),
            "dart" => resolve_dart_spec(root, rel_path, &dir, spec, &mut found),
            "go" => resolve_go_spec(root, rel_path, spec, &mut found),
            "kt" | "kts" => resolve_kotlin_spec(root, rel_path, spec, &mut found),
            "swift" => resolve_swift_spec(root, rel_path, spec, &mut found),
            _ => {}
        }
    }
    found.sort();
    found
}

fn resolve_rust_spec(root: &Path, rel_path: &str, dir: &str, spec: &str, found: &mut Vec<String>) {
    if let Some(name) = spec.strip_prefix("mod:") {
        if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            push_if_file(root, rel_path, found, format!("{dir}/{name}.rs"));
            push_if_file(root, rel_path, found, format!("{dir}/{name}/mod.rs"));
        }
        return;
    }

    let Some(path_part) = spec.strip_prefix("crate::") else {
        return;
    };
    let segs: Vec<&str> = path_part.split("::").filter(|s| !s.is_empty()).collect();
    let mut acc = String::from("src");
    for seg in &segs {
        if !seg.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return;
        }
        acc = format!("{acc}/{seg}");
        push_if_file(root, rel_path, found, format!("{acc}.rs"));
        push_if_file(root, rel_path, found, format!("{acc}/mod.rs"));
    }
}

fn resolve_js_spec(root: &Path, rel_path: &str, dir: &str, spec: &str, found: &mut Vec<String>) {
    if !spec.starts_with('.') {
        return;
    }
    let base = format!("{dir}/{spec}");
    push_if_file(root, rel_path, found, base.clone());
    for e in [".ts", ".tsx", ".js", ".jsx", ".mjs", ".svelte"] {
        push_if_file(root, rel_path, found, format!("{base}{e}"));
    }
    for e in [
        "/index.ts",
        "/index.tsx",
        "/index.js",
        "/index.jsx",
        "/index.svelte",
    ] {
        push_if_file(root, rel_path, found, format!("{base}{e}"));
    }
}

fn resolve_python_spec(
    root: &Path,
    rel_path: &str,
    dir: &str,
    spec: &str,
    found: &mut Vec<String>,
) {
    let (mut base, name) = if let Some(stripped) = spec.strip_prefix('.') {
        // relative imports: one leading dot is the current package; each
        // additional dot climbs one directory.
        let ups = stripped.chars().take_while(|c| *c == '.').count();
        let name = stripped.trim_start_matches('.');
        let mut d = dir.to_string();
        for _ in 0..ups {
            d = Path::new(&d)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        (d, name.to_string())
    } else {
        (String::new(), spec.to_string())
    };
    if base.is_empty() {
        base = ".".into();
    }
    if name.is_empty() {
        return;
    }
    let as_path = name.replace('.', "/");
    push_if_file(root, rel_path, found, format!("{base}/{as_path}.py"));
    push_if_file(
        root,
        rel_path,
        found,
        format!("{base}/{as_path}/__init__.py"),
    );
}

fn resolve_dart_spec(root: &Path, rel_path: &str, dir: &str, spec: &str, found: &mut Vec<String>) {
    if let Some(path) = spec
        .strip_prefix("package:")
        .and_then(|s| s.split_once('/').map(|(_, rest)| rest))
    {
        push_if_file(root, rel_path, found, format!("lib/{path}"));
        return;
    }
    let base = format!("{dir}/{spec}");
    push_if_file(root, rel_path, found, base.clone());
    push_if_file(root, rel_path, found, format!("{base}.dart"));
}

fn resolve_go_spec(root: &Path, rel_path: &str, spec: &str, found: &mut Vec<String>) {
    let Some(module_path) = go_module_path(root) else {
        return;
    };
    let Some(rest) = spec.strip_prefix(&format!("{module_path}/")) else {
        return;
    };
    push_if_file(root, rel_path, found, format!("{rest}.go"));
    push_if_file(root, rel_path, found, format!("{rest}/mod.go"));
    push_package_files(root, rel_path, found, rest, "go");
}

fn go_module_path(root: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(root.join("go.mod")).ok()?;
    raw.lines().map(str::trim).find_map(|line| {
        line.strip_prefix("module ")
            .map(str::trim)
            .map(str::to_string)
    })
}

fn resolve_kotlin_spec(root: &Path, rel_path: &str, spec: &str, found: &mut Vec<String>) {
    let path = spec.replace('.', "/");
    for base in [
        "src/main/kotlin",
        "src/test/kotlin",
        "app/src/main/java",
        "app/src/main/kotlin",
        "",
    ] {
        push_if_file(root, rel_path, found, format!("{base}/{path}.kt"));
        push_if_file(root, rel_path, found, format!("{base}/{path}.kts"));
    }
}

fn resolve_swift_spec(root: &Path, rel_path: &str, spec: &str, found: &mut Vec<String>) {
    let name = spec.rsplit('.').next().unwrap_or(spec);
    let dir = Path::new(rel_path)
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    push_if_file(root, rel_path, found, format!("{dir}/{name}.swift"));
    for base in ["Sources", "Tests", ""] {
        push_if_file(root, rel_path, found, format!("{base}/{name}.swift"));
        push_package_files(root, rel_path, found, base, "swift");
    }
}

fn push_package_files(root: &Path, rel_path: &str, found: &mut Vec<String>, dir: &str, ext: &str) {
    let base = root.join(dir);
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            if let Ok(rel) = path.strip_prefix(root) {
                push_if_file(
                    root,
                    rel_path,
                    found,
                    rel.to_string_lossy().replace('\\', "/"),
                );
            }
        }
    }
}

fn first_quoted(s: &str) -> Option<String> {
    let start = s.find(['\'', '"'])?;
    let quote = s.as_bytes().get(start).copied()? as char;
    let rest = &s[start + 1..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn extract_symbol_facts_heuristic(rel_path: &str, content: &str) -> Vec<crate::types::SymbolFact> {
    let ext = Path::new(rel_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let mut facts = Vec::new();
    match ext {
        "dart" => collect_simple_symbols(
            rel_path,
            content,
            &mut facts,
            &["class ", "enum ", "mixin ", "extension "],
            &["void ", "Future<", "Stream<"],
        ),
        "go" => collect_go_symbols(rel_path, content, &mut facts),
        "kt" | "kts" => collect_simple_symbols(
            rel_path,
            content,
            &mut facts,
            &["class ", "object ", "interface ", "enum class "],
            &["fun "],
        ),
        "swift" => collect_simple_symbols(
            rel_path,
            content,
            &mut facts,
            &["class ", "struct ", "enum ", "protocol ", "extension "],
            &["func "],
        ),
        "svelte" => collect_svelte_symbols(rel_path, content, &mut facts),
        _ => {}
    }
    facts
}

fn collect_simple_symbols(
    rel_path: &str,
    content: &str,
    out: &mut Vec<crate::types::SymbolFact>,
    type_prefixes: &[&str],
    fn_prefixes: &[&str],
) {
    for (idx, line) in content.lines().enumerate() {
        let t = line.trim_start();
        for prefix in type_prefixes {
            if let Some(name) = symbol_name_after(t, prefix) {
                push_heuristic_symbol(out, rel_path, content, idx, prefix.trim(), &name);
            }
        }
        for prefix in fn_prefixes {
            if let Some(name) = symbol_name_after(t, prefix) {
                push_heuristic_symbol(out, rel_path, content, idx, "fn", &name);
            }
        }
    }
}

fn collect_go_symbols(rel_path: &str, content: &str, out: &mut Vec<crate::types::SymbolFact>) {
    for (idx, line) in content.lines().enumerate() {
        let t = line.trim_start();
        if let Some(name) = symbol_name_after(t, "func ") {
            push_heuristic_symbol(out, rel_path, content, idx, "func", &name);
        }
        if let Some(rest) = t.strip_prefix("type ") {
            if let Some(name) = rest.split_whitespace().next() {
                push_heuristic_symbol(out, rel_path, content, idx, "type", name);
            }
        }
    }
}

fn collect_svelte_symbols(rel_path: &str, content: &str, out: &mut Vec<crate::types::SymbolFact>) {
    for (idx, line) in content.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with("<script") {
            push_heuristic_symbol(out, rel_path, content, idx, "component", "script");
        }
        if let Some(name) = symbol_name_after(t, "export let ") {
            push_heuristic_symbol(out, rel_path, content, idx, "prop", &name);
        }
        if let Some(name) = symbol_name_after(t, "function ") {
            push_heuristic_symbol(out, rel_path, content, idx, "function", &name);
        }
    }
}

fn symbol_name_after(line: &str, prefix: &str) -> Option<String> {
    let rest = line.strip_prefix(prefix)?;
    let name: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect();
    (!name.is_empty()).then_some(name)
}

fn push_heuristic_symbol(
    out: &mut Vec<crate::types::SymbolFact>,
    rel_path: &str,
    content: &str,
    idx: usize,
    kind: &str,
    name: &str,
) {
    let label = format!("{kind} {name}");
    if out.iter().any(|fact| fact.label == label) {
        return;
    }
    let line = content.lines().nth(idx).unwrap_or_default();
    out.push(crate::types::SymbolFact {
        label,
        name: name.to_string(),
        kind: kind.to_string(),
        visibility: "public".to_string(),
        line_start: idx + 1,
        line_end: idx + 1,
        is_test: path_is_test_like(rel_path),
        string_literals: Vec::new(),
        panic_marker_count: 0,
        panic_markers: Vec::new(),
        body_hash: content_hash(line.as_bytes()),
        shape_hash: content_hash(format!("{kind} _").as_bytes()),
    });
}

fn path_is_test_like(rel_path: &str) -> bool {
    let p = rel_path.replace('\\', "/");
    p.contains("/test/")
        || p.contains("/tests/")
        || p.contains("/integration_test/")
        || p.starts_with("test/")
        || p.starts_with("tests/")
        || p.starts_with("integration_test/")
        || p.contains(".test.")
        || p.contains(".spec.")
        || p.ends_with("_test.dart")
        || p.ends_with("_test.go")
        || p.ends_with("Tests.swift")
}

fn push_if_file(root: &Path, rel_path: &str, found: &mut Vec<String>, cand: String) {
    let norm = normalize(&cand);
    if !norm.is_empty() && norm != rel_path && root.join(&norm).is_file() && !found.contains(&norm)
    {
        found.push(norm);
    }
}

/// Normalize `a/./b/../c` → `a/c` and strip leading `./`.
fn normalize(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if out.pop().is_none() {
                    return String::new(); // escapes the repo root
                }
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

fn lang_of(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" => "javascript",
        "py" => "python",
        "dart" => "dart",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp",
        "rb" => "ruby",
        "php" => "php",
        "sh" | "bash" => "shell",
        "sql" => "sql",
        "svelte" => "svelte",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn run_git(root: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn locator_presence() {
        assert!(locator_present("fn run() {}", "fn run"));
        assert!(locator_present("anything", "")); // file-level grounding
        assert!(!locator_present("fn walk() {}", "fn run"));
    }

    #[test]
    fn git_cochange_degrades_without_a_repo() {
        // A bare temp dir is not a git repo → `git log` fails → empty, no panic.
        let dir = std::env::temp_dir().join(format!("loom-nogit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut paths = std::collections::HashSet::new();
        paths.insert("src/a.rs".to_string());
        let cc = git_cochange(&dir, &paths, 100);
        assert!(
            cc.pairs.is_empty() && cc.individual.is_empty(),
            "no git repo must degrade to empty"
        );
        // Empty path set short-circuits regardless of environment.
        assert!(git_cochange(&dir, &std::collections::HashSet::new(), 100)
            .pairs
            .is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_cochange_counts_pending_paths_as_the_next_commit() {
        let dir = std::env::temp_dir().join(format!("loom-git-{}", uuid::Uuid::new_v4()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.rs"), "fn a() {}\n").unwrap();
        fs::write(dir.join("src/b.rs"), "fn b() {}\n").unwrap();
        assert!(run_git(&dir, &["init"]));
        assert!(run_git(
            &dir,
            &["config", "user.email", "loom@example.invalid"]
        ));
        assert!(run_git(&dir, &["config", "user.name", "Loom Test"]));
        assert!(run_git(&dir, &["add", "."]));
        assert!(run_git(&dir, &["commit", "-m", "initial"]));

        fs::write(dir.join("src/a.rs"), "fn a() { let _x = 1; }\n").unwrap();
        fs::write(dir.join("src/b.rs"), "fn b() { let _x = 1; }\n").unwrap();

        let paths = HashSet::from(["src/a.rs".to_string(), "src/b.rs".to_string()]);
        let before_commit = git_cochange(&dir, &paths, 10);
        assert_eq!(
            before_commit
                .pairs
                .get(&("src/a.rs".to_string(), "src/b.rs".to_string()))
                .copied(),
            Some(2),
            "pending changes should be counted as the newest cochange event"
        );
        assert_eq!(before_commit.individual.get("src/a.rs").copied(), Some(2));
        assert_eq!(before_commit.individual.get("src/b.rs").copied(), Some(2));

        assert!(run_git(&dir, &["add", "."]));
        assert!(run_git(&dir, &["commit", "-m", "second"]));
        let after_commit = git_cochange(&dir, &paths, 10);
        assert_eq!(
            before_commit.pairs, after_commit.pairs,
            "cochange evidence should be stable across the commit boundary"
        );
        assert_eq!(
            before_commit.individual, after_commit.individual,
            "individual churn evidence should be stable across the commit boundary"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn confine_keeps_root_relative_and_rejects_escapes() {
        let root = Path::new("/repo");
        // In-root forms normalize to the stored convention.
        assert_eq!(
            confine(root, Path::new("src/main.rs")).as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            confine(root, Path::new("./src/./main.rs")).as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            confine(root, Path::new("src/db/../gate.rs")).as_deref(),
            Some("src/gate.rs")
        );
        // Absolute is accepted iff it RESOLVES under root — and comes back relative.
        assert_eq!(
            confine(root, Path::new("/repo/sub/file.rs")).as_deref(),
            Some("sub/file.rs")
        );
        assert_eq!(
            confine(root, Path::new("/repo/a/../b.rs")).as_deref(),
            Some("b.rs")
        );
        // What matters is the resolved target, not the route taken to it.
        assert_eq!(
            confine(root, Path::new("../repo/src/x.rs")).as_deref(),
            Some("src/x.rs")
        );
        // Escapes: relative, `..`-smuggled, absolute, above-fs-root, root itself.
        assert_eq!(confine(root, Path::new("../outside.rs")), None);
        assert_eq!(confine(root, Path::new("src/../../etc/passwd")), None);
        assert_eq!(confine(root, Path::new("/etc/passwd")), None);
        assert_eq!(confine(root, Path::new("../../../../..")), None);
        assert_eq!(confine(root, Path::new("/repo")), None);
    }

    #[test]
    fn imports_rust_js_python() {
        let dir = std::env::temp_dir().join(format!("loom-imp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/db")).unwrap();
        fs::create_dir_all(dir.join("web")).unwrap();
        fs::create_dir_all(dir.join("pkg")).unwrap();
        fs::write(dir.join("src/db/mod.rs"), "").unwrap();
        fs::write(dir.join("src/db/schema.rs"), "").unwrap();
        fs::write(dir.join("src/gate.rs"), "").unwrap();
        fs::write(
            dir.join("src/main.rs"),
            "mod gate;\nuse crate::db::schema::esc;\n",
        )
        .unwrap();
        fs::write(dir.join("web/util.ts"), "").unwrap();
        fs::write(
            dir.join("web/app.ts"),
            "import {x} from './util';\nimport pkg from 'react';\n",
        )
        .unwrap();
        fs::write(dir.join("pkg/helper.py"), "").unwrap();
        fs::write(
            dir.join("pkg/main.py"),
            "from .helper import thing\nimport os\n",
        )
        .unwrap();

        let rs = extract_imports(
            &dir,
            "src/main.rs",
            &fs::read_to_string(dir.join("src/main.rs")).unwrap(),
        );
        assert!(rs.contains(&"src/gate.rs".to_string()), "{rs:?}");
        assert!(rs.contains(&"src/db/mod.rs".to_string()), "{rs:?}");
        assert!(rs.contains(&"src/db/schema.rs".to_string()), "{rs:?}");

        let ts = extract_imports(
            &dir,
            "web/app.ts",
            &fs::read_to_string(dir.join("web/app.ts")).unwrap(),
        );
        assert_eq!(
            ts,
            vec!["web/util.ts".to_string()],
            "package imports excluded"
        );

        let py = extract_imports(
            &dir,
            "pkg/main.py",
            &fs::read_to_string(dir.join("pkg/main.py")).unwrap(),
        );
        assert_eq!(
            py,
            vec!["pkg/helper.py".to_string()],
            "stdlib imports excluded: {py:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn heuristic_imports_rust_js_python() {
        let dir = std::env::temp_dir().join(format!("loom-imp-heur-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/db")).unwrap();
        fs::create_dir_all(dir.join("web")).unwrap();
        fs::create_dir_all(dir.join("pkg")).unwrap();
        fs::write(dir.join("src/db/mod.rs"), "").unwrap();
        fs::write(dir.join("src/db/schema.rs"), "").unwrap();
        fs::write(dir.join("src/gate.rs"), "").unwrap();
        fs::write(
            dir.join("src/main.rs"),
            "mod gate;\nuse crate::db::schema::esc;\n",
        )
        .unwrap();
        fs::write(dir.join("web/util.ts"), "").unwrap();
        fs::write(
            dir.join("web/app.ts"),
            "import {x} from './util';\nimport pkg from 'react';\n",
        )
        .unwrap();
        fs::write(dir.join("pkg/helper.py"), "").unwrap();
        fs::write(
            dir.join("pkg/main.py"),
            "from .helper import thing\nimport os\n",
        )
        .unwrap();

        let rs = extract_imports_heuristic(
            &dir,
            "src/main.rs",
            &fs::read_to_string(dir.join("src/main.rs")).unwrap(),
        );
        assert!(rs.contains(&"src/gate.rs".to_string()), "{rs:?}");
        assert!(rs.contains(&"src/db/mod.rs".to_string()), "{rs:?}");
        assert!(rs.contains(&"src/db/schema.rs".to_string()), "{rs:?}");

        let ts = extract_imports_heuristic(
            &dir,
            "web/app.ts",
            &fs::read_to_string(dir.join("web/app.ts")).unwrap(),
        );
        assert_eq!(ts, vec!["web/util.ts".to_string()]);

        let py = extract_imports_heuristic(
            &dir,
            "pkg/main.py",
            &fs::read_to_string(dir.join("pkg/main.py")).unwrap(),
        );
        assert_eq!(py, vec!["pkg/helper.py".to_string()]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn heuristic_imports_dart_go_kotlin_swift_and_svelte() {
        let dir = std::env::temp_dir().join(format!("loom-imp-mobile-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lib/src")).unwrap();
        fs::create_dir_all(dir.join("pkg/util")).unwrap();
        fs::create_dir_all(dir.join("src/main/kotlin/com/example")).unwrap();
        fs::create_dir_all(dir.join("Sources/App")).unwrap();
        fs::create_dir_all(dir.join("web/lib")).unwrap();
        fs::write(dir.join("go.mod"), "module example.com/app\n").unwrap();
        fs::write(dir.join("lib/src/helper.dart"), "class Helper {}\n").unwrap();
        fs::write(
            dir.join("lib/main.dart"),
            "import 'package:demo/src/helper.dart';\nclass App {}\n",
        )
        .unwrap();
        fs::write(
            dir.join("pkg/util/util.go"),
            "package util\nfunc Use() {}\n",
        )
        .unwrap();
        fs::write(
            dir.join("main.go"),
            "package main\nimport \"example.com/app/pkg/util\"\nfunc main() {}\n",
        )
        .unwrap();
        fs::write(
            dir.join("src/main/kotlin/com/example/Thing.kt"),
            "class Thing\n",
        )
        .unwrap();
        fs::write(
            dir.join("src/main/kotlin/App.kt"),
            "import com.example.Thing\nfun run() {}\n",
        )
        .unwrap();
        fs::write(dir.join("Sources/App/Widget.swift"), "struct Widget {}\n").unwrap();
        fs::write(
            dir.join("Sources/App/App.swift"),
            "import Widget\nfunc boot() {}\n",
        )
        .unwrap();
        fs::write(
            dir.join("web/lib/Widget.svelte"),
            "<script>export let name;</script>\n",
        )
        .unwrap();
        fs::write(
            dir.join("web/App.svelte"),
            "<script>import Widget from './lib/Widget'; function boot() {}</script>\n",
        )
        .unwrap();

        assert_eq!(
            extract_imports(
                &dir,
                "lib/main.dart",
                &fs::read_to_string(dir.join("lib/main.dart")).unwrap()
            ),
            vec!["lib/src/helper.dart".to_string()]
        );
        assert_eq!(
            extract_imports(
                &dir,
                "main.go",
                &fs::read_to_string(dir.join("main.go")).unwrap()
            ),
            vec!["pkg/util/util.go".to_string()]
        );
        assert_eq!(
            extract_imports(
                &dir,
                "src/main/kotlin/App.kt",
                &fs::read_to_string(dir.join("src/main/kotlin/App.kt")).unwrap()
            ),
            vec!["src/main/kotlin/com/example/Thing.kt".to_string()]
        );
        assert!(extract_imports(
            &dir,
            "Sources/App/App.swift",
            &fs::read_to_string(dir.join("Sources/App/App.swift")).unwrap()
        )
        .contains(&"Sources/App/Widget.swift".to_string()));
        assert_eq!(
            extract_imports(
                &dir,
                "web/App.svelte",
                &fs::read_to_string(dir.join("web/App.svelte")).unwrap()
            ),
            vec!["web/lib/Widget.svelte".to_string()]
        );

        let dart_symbols = extract_symbols(
            "lib/main.dart",
            &fs::read_to_string(dir.join("lib/main.dart")).unwrap(),
        );
        assert!(
            dart_symbols.contains(&"class App".to_string()),
            "{dart_symbols:?}"
        );
        let go_symbols =
            extract_symbols("main.go", &fs::read_to_string(dir.join("main.go")).unwrap());
        assert!(
            go_symbols.contains(&"func main".to_string()),
            "{go_symbols:?}"
        );
        let kotlin_symbols = extract_symbols(
            "src/main/kotlin/App.kt",
            &fs::read_to_string(dir.join("src/main/kotlin/App.kt")).unwrap(),
        );
        assert!(
            kotlin_symbols.contains(&"fn run".to_string()),
            "{kotlin_symbols:?}"
        );
        let swift_symbols = extract_symbols(
            "Sources/App/App.swift",
            &fs::read_to_string(dir.join("Sources/App/App.swift")).unwrap(),
        );
        assert!(
            swift_symbols.contains(&"fn boot".to_string()),
            "{swift_symbols:?}"
        );
        let svelte_symbols = extract_symbols(
            "web/App.svelte",
            &fs::read_to_string(dir.join("web/App.svelte")).unwrap(),
        );
        assert!(
            svelte_symbols.contains(&"component script".to_string()),
            "{svelte_symbols:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "treesitter")]
    #[test]
    fn tree_sitter_rust_extracts_grouped_and_reexport_imports() {
        let dir = std::env::temp_dir().join(format!("loom-imp-rs-ts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/db")).unwrap();
        fs::create_dir_all(dir.join("src/foo/bar")).unwrap();
        fs::write(dir.join("src/a.rs"), "").unwrap();
        fs::write(dir.join("src/b.rs"), "").unwrap();
        fs::write(dir.join("src/db/schema.rs"), "").unwrap();
        fs::write(dir.join("src/db/queries.rs"), "").unwrap();
        fs::write(dir.join("src/foo.rs"), "").unwrap();
        fs::write(dir.join("src/foo/bar.rs"), "").unwrap();
        fs::write(dir.join("src/reexport.rs"), "").unwrap();
        fs::write(
            dir.join("src/main.rs"),
            r#"
                use crate::db::{schema::esc, queries};
                pub use crate::{
                    a,
                    b as bee,
                    foo::{self, bar::Baz},
                };
                pub use crate::reexport::Thing as Alias;
            "#,
        )
        .unwrap();

        let imports = extract_imports(
            &dir,
            "src/main.rs",
            &fs::read_to_string(dir.join("src/main.rs")).unwrap(),
        );
        for expected in [
            "src/a.rs",
            "src/b.rs",
            "src/db/queries.rs",
            "src/db/schema.rs",
            "src/foo.rs",
            "src/foo/bar.rs",
            "src/reexport.rs",
        ] {
            assert!(imports.contains(&expected.to_string()), "{imports:?}");
        }

        let heuristic = extract_imports_heuristic(
            &dir,
            "src/main.rs",
            &fs::read_to_string(dir.join("src/main.rs")).unwrap(),
        );
        assert!(
            imports.len() > heuristic.len(),
            "tree-sitter should cover grouped imports missed by the line scanner: ts={imports:?} heuristic={heuristic:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "treesitter")]
    #[test]
    fn tree_sitter_js_ts_extracts_multiline_exports_and_dynamic_imports() {
        let dir = std::env::temp_dir().join(format!("loom-imp-js-ts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("web/lib")).unwrap();
        fs::write(dir.join("web/lib/util.ts"), "").unwrap();
        fs::write(dir.join("web/lib/barrel.ts"), "").unwrap();
        fs::write(dir.join("web/lib/lazy.ts"), "").unwrap();
        fs::write(dir.join("web/lib/legacy.js"), "").unwrap();
        fs::write(
            dir.join("web/app.ts"),
            r#"
                import {
                    helper,
                } from "./lib/util";
                export { helper as again } from "./lib/barrel";
                const legacy = require("./lib/legacy");
                const lazy = import("./lib/lazy");
                import react from "react";
            "#,
        )
        .unwrap();

        let imports = extract_imports(
            &dir,
            "web/app.ts",
            &fs::read_to_string(dir.join("web/app.ts")).unwrap(),
        );
        assert_eq!(
            imports,
            vec![
                "web/lib/barrel.ts".to_string(),
                "web/lib/lazy.ts".to_string(),
                "web/lib/legacy.js".to_string(),
                "web/lib/util.ts".to_string(),
            ]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "treesitter")]
    #[test]
    fn tree_sitter_python_extracts_parenthesized_and_relative_imports() {
        let dir = std::env::temp_dir().join(format!("loom-imp-py-ts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("pkg")).unwrap();
        fs::write(dir.join("pkg/helpers.py"), "").unwrap();
        fs::write(dir.join("pkg/sibling.py"), "").unwrap();
        fs::write(dir.join("pkg/core.py"), "").unwrap();
        fs::write(
            dir.join("pkg/main.py"),
            r#"
                from .helpers import (
                    one,
                    two,
                )
                from . import sibling
                import pkg.core as core
                from __future__ import annotations
            "#,
        )
        .unwrap();

        let imports = extract_imports(
            &dir,
            "pkg/main.py",
            &fs::read_to_string(dir.join("pkg/main.py")).unwrap(),
        );
        assert_eq!(
            imports,
            vec![
                "pkg/core.py".to_string(),
                "pkg/helpers.py".to_string(),
                "pkg/sibling.py".to_string(),
            ]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "treesitter")]
    #[test]
    fn tree_sitter_rust_extracts_top_level_symbols() {
        let content = r#"
                pub struct User { id: String }
                enum State { Ready }
                trait Render { fn render(&self); }
                type Id = String;
                const LIMIT: usize = 10;
                static NAME: &str = "loom";
                macro_rules! route { () => {} }
                impl Render for User {
                    fn render(&self) {}
                }
                fn build() {
                    fn local() {}
                }
                mod nested {
                    pub fn inside() {}
                }
                #[test]
                fn tests_it() {}
                #[cfg(test)]
                mod tests {
                    fn helper_uses_unwrap() {}
                }
            "#;
        let symbols = extract_symbols("src/lib.rs", content);
        for expected in [
            "const LIMIT",
            "enum State",
            "fn build",
            "impl Render for User",
            "macro route",
            "pub fn inside",
            "pub struct User",
            "static NAME",
            "trait Render",
            "type Id",
        ] {
            assert!(symbols.contains(&expected.to_string()), "{symbols:?}");
        }
        assert!(!symbols.contains(&"fn render".to_string()), "{symbols:?}");
        assert!(!symbols.contains(&"fn local".to_string()), "{symbols:?}");

        let facts = extract_physical_facts(Path::new("."), "src/lib.rs", content);
        let user = facts
            .symbol_facts
            .iter()
            .find(|fact| fact.name == "User")
            .unwrap();
        assert_eq!(user.visibility, "public");
        assert!(user.line_start > 0);
        let test = facts
            .symbol_facts
            .iter()
            .find(|fact| fact.name == "tests_it")
            .unwrap();
        assert!(test.is_test, "{test:?}");
        let helper = facts
            .symbol_facts
            .iter()
            .find(|fact| fact.name == "helper_uses_unwrap")
            .unwrap();
        assert!(
            helper.is_test,
            "symbols inside #[cfg(test)] modules should be test-only: {helper:?}"
        );
    }

    #[cfg(feature = "treesitter")]
    #[test]
    fn tree_sitter_shape_hash_survives_renames_and_comments() {
        let a = extract_physical_facts(
            Path::new("."),
            "src/a.rs",
            "fn alpha(input: usize) -> usize {\n    // comment\n    input + 1\n}\n",
        );
        let b = extract_physical_facts(
            Path::new("."),
            "src/b.rs",
            "fn beta(value: usize) -> usize {\n    value + 2\n}\n",
        );
        let alpha = a
            .symbol_facts
            .iter()
            .find(|fact| fact.name == "alpha")
            .unwrap();
        let beta = b
            .symbol_facts
            .iter()
            .find(|fact| fact.name == "beta")
            .unwrap();
        assert_ne!(
            alpha.body_hash, beta.body_hash,
            "raw body hashes stay exact for sync invalidation"
        );
        assert_eq!(
            alpha.shape_hash, beta.shape_hash,
            "normalized shape groups renamed/formatted clones"
        );
    }

    #[cfg(feature = "treesitter")]
    #[test]
    fn tree_sitter_js_ts_extracts_top_level_symbols() {
        let symbols = extract_symbols(
            "web/app.ts",
            r#"
                export interface User { id: string }
                export type Id = string;
                export enum Mode { On }
                export class Widget { render() {} }
                export function makeWidget() {}
                export const alpha = 1, beta = 2;
                let notConst = 3;
                function outer() {
                    const local = 1;
                }
            "#,
        );
        assert_eq!(
            symbols,
            vec![
                "export class Widget".to_string(),
                "export const alpha".to_string(),
                "export const beta".to_string(),
                "export enum Mode".to_string(),
                "export function makeWidget".to_string(),
                "export interface User".to_string(),
                "export type Id".to_string(),
                "function outer".to_string(),
            ]
        );
    }

    #[cfg(feature = "treesitter")]
    #[test]
    fn tree_sitter_python_extracts_top_level_symbols() {
        let symbols = extract_symbols(
            "pkg/app.py",
            r#"
                class User:
                    def method(self):
                        pass

                def build():
                    def local():
                        pass
                    return User()
            "#,
        );
        assert_eq!(
            symbols,
            vec!["class User".to_string(), "def build".to_string()]
        );
    }
}
