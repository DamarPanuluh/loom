//! Filesystem introspection: a .gitignore-aware file walk and programmable
//! stack detection. Pure FS concerns (no DB) — shared by `loom coverage` and
//! `loom detect`, and the basis for guide mode-detection (greenfield vs
//! brownfield).

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

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

/// Confine a registered path to the graph root: `.`/`..` fold lexically (the
/// file may not exist yet, and a hostile path must never be probed), the
/// result must stay under `root`, and the returned form is root-relative with
/// `/` separators. `None` = the path escapes the root.
///
/// The graph is untrusted input (imports and hand edits travel in
/// loom.graph.json): a CodeFile path like `/etc/passwd` or `../../secret`
/// must never reach `fs::read` — sync hashes file bytes and probes locator
/// substrings, which would otherwise answer "is this string in that file?"
/// for any readable file on the machine. Absolute paths are accepted iff
/// they resolve under `root` (and come back relative, the stored convention).
/// When the lexical check fails for an absolute path, both sides resolve
/// through `canonicalize` once — an absolute path may spell the root through
/// a symlinked prefix (e.g. `/var` vs `/private/var` on macOS); resolving a
/// path reveals nothing, contents are never read.
pub fn confine(root: &Path, path: &Path) -> Option<String> {
    use std::path::{Component, Path, PathBuf};
    fn finish(rel: &Path) -> Option<String> {
        if rel.as_os_str().is_empty() {
            return None; // the root itself is not a file path
        }
        Some(rel.to_str()?.replace('\\', "/"))
    }
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let mut resolved = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => resolved.push(c),
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() {
                    return None; // walked above the filesystem root
                }
            }
        }
    }
    if let Ok(rel) = resolved.strip_prefix(root) {
        return finish(rel);
    }
    if path.is_absolute() {
        let croot = root.canonicalize().ok()?;
        let cpath = resolved.canonicalize().ok()?;
        return finish(cpath.strip_prefix(&croot).ok()?);
    }
    None
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
    ];
    let mut stacks: Vec<String> = Vec::new();
    for (file, name) in manifests {
        if root.join(file).exists() && !stacks.iter().any(|s| s == name) {
            stacks.push((*name).to_string());
        }
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

    if stacks.iter().any(|s| s == "swift" || s == "java/kotlin")
        || has_ext(&["swift", "kt"])
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
    if has_ext(&["tsx", "jsx", "vue", "svelte", "html", "css"]) {
        packs.push(PackHint {
            pack: "web-ui".into(),
            reason: "frontend files detected (tsx/jsx/vue/svelte/html/css) — view states, accessibility, XSS, client-side trust".into(),
        });
    }
    if stacks.iter().any(|s| {
        matches!(
            s.as_str(),
            "rust" | "go" | "node" | "python" | "java" | "java/kotlin" | "ruby" | "php"
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
#[allow(dead_code)]
pub fn extract_imports(root: &Path, rel_path: &str, content: &str) -> Vec<String> {
    extract_physical_facts(root, rel_path, content).imports
}

/// Top-level canonical syntax symbols in `rel_path`. Empty in feature-light
/// builds or unsupported languages; this is diagnostic physical evidence only.
#[allow(dead_code)]
pub fn extract_symbols(rel_path: &str, content: &str) -> Vec<String> {
    #[cfg(feature = "treesitter")]
    if let Some(facts) = crate::ts_imports::extract_physical_facts(rel_path, content) {
        return facts.symbols;
    }
    let _ = rel_path;
    let _ = content;
    Vec::new()
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
    PhysicalFacts {
        imports: extract_imports_heuristic(root, rel_path, content),
        symbols: Vec::new(),
        symbol_facts: Vec::new(),
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
                            push_unique(&mut specs, format!("mod:{name}"));
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
                        push_unique(&mut specs, format!("crate::{path_part}"));
                    }
                }
            }
        }
        "ts" | "tsx" | "js" | "jsx" | "mjs" => {
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
                        push_unique(&mut specs, spec);
                    }
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
                    push_unique(&mut specs, spec);
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
            "ts" | "tsx" | "js" | "jsx" | "mjs" => {
                resolve_js_spec(root, rel_path, &dir, spec, &mut found)
            }
            "py" => resolve_python_spec(root, rel_path, &dir, spec, &mut found),
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
    for e in [".ts", ".tsx", ".js", ".jsx", ".mjs"] {
        push_if_file(root, rel_path, found, format!("{base}{e}"));
    }
    for e in ["/index.ts", "/index.tsx", "/index.js", "/index.jsx"] {
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

fn push_if_file(root: &Path, rel_path: &str, found: &mut Vec<String>, cand: String) {
    let norm = normalize(&cand);
    if !norm.is_empty() && norm != rel_path && root.join(&norm).is_file() && !found.contains(&norm)
    {
        found.push(norm);
    }
}

fn push_unique(out: &mut Vec<String>, item: String) {
    if !item.is_empty() && !out.contains(&item) {
        out.push(item);
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
        "go" => "go",
        "java" => "java",
        "kt" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp",
        "rb" => "ruby",
        "php" => "php",
        "sh" | "bash" => "shell",
        "sql" => "sql",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn locator_presence() {
        assert!(locator_present("fn run() {}", "fn run"));
        assert!(locator_present("anything", "")); // file-level grounding
        assert!(!locator_present("fn walk() {}", "fn run"));
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
