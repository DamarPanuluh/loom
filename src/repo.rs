//! Filesystem introspection: a .gitignore-aware file walk and programmable
//! stack detection. Pure FS concerns (no DB) — shared by `loom coverage` and
//! `loom detect`, and the basis for guide mode-detection (greenfield vs
//! brownfield).

use ignore::WalkBuilder;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

/// Build/dependency/VCS directories that are never source — skipped
/// unconditionally so coverage is meaningful even in a repo with no `.gitignore`
/// (e.g. cargo's `target/` holds thousands of artifacts). `.gitignore` is still
/// honored on top of this.
const DEFAULT_SKIP_DIRS: &[&str] = &[
    "target", "node_modules", "vendor", "__pycache__", "venv", ".venv", "dist",
    ".git", ".loom",
];

/// Relative file paths under `root`, respecting `.gitignore`/`.ignore`, skipping
/// hidden entries and well-known build/dependency dirs. Directories excluded.
pub fn walk_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    // require_git(false): honor .gitignore/.ignore even when this isn't a git
    // repo (or we're in a subdir), so coverage's denominator is meaningful.
    for result in WalkBuilder::new(root).hidden(true).require_git(false).build() {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(root) {
            let s = rel.to_string_lossy().replace('\\', "/");
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
    files
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

#[derive(Debug, Clone, Serialize)]
pub struct LangCount {
    pub language: String,
    pub files: usize,
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
}

/// Detect the repo's stack and whether there's existing source to map.
pub fn detect(root: &Path) -> Detection {
    let files = walk_files(root);

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
    top_languages.sort_by(|a, b| b.files.cmp(&a.files));
    top_languages.truncate(8);

    let has_source = source_files > 0;
    Detection {
        source_files,
        has_source,
        stacks,
        top_languages,
        suggested_mode: if has_source { "brownfield".into() } else { "greenfield".into() },
    }
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
pub fn extract_imports(root: &Path, rel_path: &str, content: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let push_if_file = |cand: String, found: &mut Vec<String>| {
        let norm = normalize(&cand);
        if !norm.is_empty() && norm != rel_path && root.join(&norm).is_file()
            && !found.contains(&norm)
        {
            found.push(norm);
        }
    };

    let ext = Path::new(rel_path).extension().and_then(|e| e.to_str()).unwrap_or("");
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
                            push_if_file(format!("{dir}/{name}.rs"), &mut found);
                            push_if_file(format!("{dir}/{name}/mod.rs"), &mut found);
                        }
                    }
                }
                // `use crate::a::b::…` → src/a.rs | src/a/mod.rs | src/a/b.rs | …
                if let Some(rest) = t.strip_prefix("use crate::") {
                    let path_part: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
                        .collect();
                    let segs: Vec<&str> = path_part.split("::").filter(|s| !s.is_empty()).collect();
                    let mut acc = String::from("src");
                    for seg in &segs {
                        acc = format!("{acc}/{seg}");
                        push_if_file(format!("{acc}.rs"), &mut found);
                        push_if_file(format!("{acc}/mod.rs"), &mut found);
                    }
                }
            }
        }
        "ts" | "tsx" | "js" | "jsx" | "mjs" => {
            for line in content.lines() {
                for marker in ["from '", "from \"", "require('", "require(\"", "import('", "import(\""] {
                    if let Some(idx) = line.find(marker) {
                        let rest = &line[idx + marker.len()..];
                        let spec: String = rest
                            .chars()
                            .take_while(|c| *c != '\'' && *c != '"')
                            .collect();
                        if !spec.starts_with('.') {
                            continue; // package import, not a repo file
                        }
                        let base = format!("{dir}/{spec}");
                        push_if_file(base.clone(), &mut found);
                        for e in [".ts", ".tsx", ".js", ".jsx", ".mjs"] {
                            push_if_file(format!("{base}{e}"), &mut found);
                        }
                        for e in ["/index.ts", "/index.tsx", "/index.js", "/index.jsx"] {
                            push_if_file(format!("{base}{e}"), &mut found);
                        }
                    }
                }
            }
        }
        "py" => {
            for line in content.lines() {
                let t = line.trim();
                let module: Option<(String, bool)> = if let Some(rest) = t.strip_prefix("from ") {
                    rest.split_whitespace().next().map(|m| (m.to_string(), true))
                } else {
                    t.strip_prefix("import ")
                        .and_then(|rest| rest.split([' ', ',']).next().map(|m| (m.to_string(), false)))
                };
                if let Some((m, _)) = module {
                    let (mut base, name) = if let Some(stripped) = m.strip_prefix('.') {
                        // relative: each extra leading dot climbs a directory
                        let ups = stripped.chars().take_while(|c| *c == '.').count();
                        let name = stripped.trim_start_matches('.');
                        let mut d = dir.clone();
                        for _ in 0..ups {
                            d = Path::new(&d).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                        }
                        (d, name.to_string())
                    } else {
                        (String::new(), m)
                    };
                    if base.is_empty() {
                        base = ".".into();
                    }
                    let as_path = name.replace('.', "/");
                    push_if_file(format!("{base}/{as_path}.py"), &mut found);
                    push_if_file(format!("{base}/{as_path}/__init__.py"), &mut found);
                }
            }
        }
        _ => {}
    }
    found.sort();
    found
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
    fn imports_rust_js_python() {
        let dir = std::env::temp_dir().join(format!("loom-imp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/db")).unwrap();
        fs::create_dir_all(dir.join("web")).unwrap();
        fs::create_dir_all(dir.join("pkg")).unwrap();
        fs::write(dir.join("src/db/mod.rs"), "").unwrap();
        fs::write(dir.join("src/db/schema.rs"), "").unwrap();
        fs::write(dir.join("src/gate.rs"), "").unwrap();
        fs::write(dir.join("src/main.rs"), "mod gate;\nuse crate::db::schema::esc;\n").unwrap();
        fs::write(dir.join("web/util.ts"), "").unwrap();
        fs::write(dir.join("web/app.ts"), "import {x} from './util';\nimport pkg from 'react';\n").unwrap();
        fs::write(dir.join("pkg/helper.py"), "").unwrap();
        fs::write(dir.join("pkg/main.py"), "from .helper import thing\nimport os\n").unwrap();

        let rs = extract_imports(&dir, "src/main.rs", &fs::read_to_string(dir.join("src/main.rs")).unwrap());
        assert!(rs.contains(&"src/gate.rs".to_string()), "{rs:?}");
        assert!(rs.contains(&"src/db/mod.rs".to_string()), "{rs:?}");
        assert!(rs.contains(&"src/db/schema.rs".to_string()), "{rs:?}");

        let ts = extract_imports(&dir, "web/app.ts", &fs::read_to_string(dir.join("web/app.ts")).unwrap());
        assert_eq!(ts, vec!["web/util.ts".to_string()], "package imports excluded");

        let py = extract_imports(&dir, "pkg/main.py", &fs::read_to_string(dir.join("pkg/main.py")).unwrap());
        assert_eq!(py, vec!["pkg/helper.py".to_string()], "stdlib imports excluded: {py:?}");

        let _ = fs::remove_dir_all(&dir);
    }
}

fn lang_of(path: &str) -> &'static str {
    let ext = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("");
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
