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
