//! Code extraction — the derived structural facts of a file.
//!
//! Plane: derived. Everything here is a pure function of file content (plus its
//! path for language/role). `sync` calls `extract` and writes the results as
//! derived facets; nothing here touches the store. Deterministic by construction
//! so the derived plane is rebuildable (INV-2).

use std::path::Path;
mod langs;
mod rust;

/// Source language, detected from extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    Go,
    JavaScript,
    TypeScript,
    Other,
}

impl Language {
    pub fn as_str(&self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Python => "python",
            Language::Go => "go",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
            Language::Other => "other",
        }
    }

    pub fn detect(path: &str) -> Language {
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        match ext {
            "rs" => Language::Rust,
            "py" => Language::Python,
            "go" => Language::Go,
            "ts" | "tsx" => Language::TypeScript,
            "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
            _ => Language::Other,
        }
    }
}

/// The role a file plays — drives which rules apply (e.g. panic markers are
/// tolerated in tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Source,
    Test,
    Generated,
    Vendor,
    Config,
    Migration,
    Other,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Source => "source",
            Role::Test => "test",
            Role::Generated => "generated",
            Role::Vendor => "vendor",
            Role::Config => "config",
            Role::Migration => "migration",
            Role::Other => "other",
        }
    }

    pub fn detect(path: &str) -> Role {
        let p = path.replace('\\', "/");
        let lower = p.to_lowercase();
        let file = Path::new(&p)
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("");
        let has = |dir: &str| {
            lower.starts_with(&format!("{dir}/")) || lower.contains(&format!("/{dir}/"))
        };
        if has("vendor") || has("third_party") {
            return Role::Vendor;
        }
        if has("target")
            || has("node_modules")
            || has("dist")
            || has("build")
            || lower.contains(".min.")
        {
            return Role::Generated;
        }
        if has("migrations") {
            return Role::Migration;
        }
        if has("tests")
            || file.ends_with("_test.rs")
            || file.ends_with("_test.go")
            || file.contains(".test.")
            || file.contains(".spec.")
            || file.starts_with("test_")
        {
            return Role::Test;
        }
        let ext = Path::new(&p)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if matches!(
            ext,
            "toml" | "yaml" | "yml" | "json" | "ini" | "cfg" | "lock"
        ) || matches!(
            file,
            "Cargo.toml" | "package.json" | "go.mod" | "Dockerfile"
        ) {
            return Role::Config;
        }
        Role::Source
    }
}

/// A symbol declaration with a complexity proxy.
#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub line_start: usize,
    pub line_end: usize,
    /// Complexity proxy (branch/loop points; a match counts once, not per arm).
    /// 0 for non-callable symbols.
    pub complexity: u32,
}

/// The derived facts of one file.
#[derive(Debug, Clone, PartialEq)]
pub struct Extraction {
    pub language: Language,
    pub role: Role,
    pub loc: usize,
    pub content_hash: String,
    pub symbols: Vec<Symbol>,
    pub imports: Vec<String>,
    /// Production unwrap()/panic! sites (AST-counted, excludes test modules and
    /// string/comment text). 0 for non-Rust.
    pub panic_sites: usize,
}

/// Extract derived facts from a file's content.
pub fn extract(path: &str, content: &str) -> Extraction {
    let language = Language::detect(path);
    let role = Role::detect(path);
    let loc = content.lines().count();
    let content_hash = fnv1a(content);
    let (symbols, imports, panic_sites) = match language {
        Language::Rust => rust::rust_extract(content),
        Language::Other => (Vec::new(), Vec::new(), 0),
        other => langs::extract(other, content),
    };
    Extraction {
        language,
        role,
        loc,
        content_hash,
        symbols,
        imports,
        panic_sites,
    }
}

/// FNV-1a 64-bit content hash as hex. Deterministic, no external crate.
pub fn fnv1a(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The `name` child identifier of a declaration node, as text.
pub(super) fn child_name(node: &tree_sitter::Node, bytes: &[u8]) -> Option<String> {
    let name = node.child_by_field_name("name")?;
    name.utf8_text(bytes).ok().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_and_role_detection() {
        assert_eq!(Language::detect("src/payment.rs"), Language::Rust);
        assert_eq!(Language::detect("app/main.py"), Language::Python);
        assert_eq!(Role::detect("tests/ring1.rs"), Role::Test);
        assert_eq!(Role::detect("src/store.rs"), Role::Source);
        assert_eq!(Role::detect("target/debug/x.rs"), Role::Generated);
        assert_eq!(Role::detect("Cargo.toml"), Role::Config);
    }

    #[test]
    fn fnv_is_deterministic_and_sensitive() {
        assert_eq!(fnv1a("hello"), fnv1a("hello"));
        assert_ne!(fnv1a("hello"), fnv1a("hellp"));
    }

    #[test]
    fn rust_symbols_and_complexity() {
        let src = r#"
use std::fmt;
use crate::model::Node;

pub fn simple() -> i32 { 1 }

pub fn branchy(x: i32) -> i32 {
    if x > 0 {
        for _ in 0..x {
            if x % 2 == 0 && x > 4 { return 1; }
        }
    }
    match x {
        0 => 0,
        _ => 2,
    }
}

struct Thing;
"#;
        let ex = extract("src/demo.rs", src);
        assert_eq!(ex.language, Language::Rust);
        let names: Vec<_> = ex.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"simple"));
        assert!(names.contains(&"branchy"));
        assert!(names.contains(&"Thing"));
        let simple = ex.symbols.iter().find(|s| s.name == "simple").unwrap();
        let branchy = ex.symbols.iter().find(|s| s.name == "branchy").unwrap();
        assert_eq!(simple.complexity, 1);
        assert!(branchy.complexity > simple.complexity);
        assert!(ex.imports.iter().any(|i| i.contains("std::fmt")));
    }

    #[test]
    fn panic_sites_count_production_only() {
        let src = r#"
fn prod() { let x: Option<i32> = None; x.unwrap(); }
fn shout() { panic!("boom"); }
fn in_string() { let s = "text with .unwrap() inside"; let _ = s; }
fn in_comment() { /* x.unwrap() here */ let _ = 1; }
#[cfg(test)]
mod tests {
    fn t() { let y: Option<i32> = None; y.unwrap(); }
}
fn after_tests() { let z: Option<i32> = None; z.unwrap(); }
"#;
        let ex = extract("src/demo.rs", src);
        // prod unwrap + panic! + after_tests unwrap = 3; string, comment and the
        // #[cfg(test)] module are all excluded — and production AFTER a test
        // module is still counted (the text heuristic could not do this).
        assert_eq!(ex.panic_sites, 3);
    }

    #[test]
    fn wide_flat_match_is_not_penalized_like_nested_logic() {
        let flat = r#"
fn dispatch(x: i32) -> i32 {
    match x {
        0 => 0, 1 => 1, 2 => 2, 3 => 3, 4 => 4,
        5 => 5, 6 => 6, 7 => 7, _ => 8,
    }
}
"#;
        let nested = r#"
fn tangled(x: i32) -> i32 {
    if x > 0 {
        if x > 1 {
            for _ in 0..x {
                if x % 2 == 0 { return 1; }
            }
        }
    }
    0
}
"#;
        let flat_c = extract("src/a.rs", flat).symbols[0].complexity;
        let nested_c = extract("src/b.rs", nested).symbols[0].complexity;
        // a 9-arm flat match is counted once, so it stays tiny (base + 1 match) —
        // a regression to per-arm counting would push this to ~10 and fail here.
        assert!(flat_c <= 2, "flat dispatch over-penalized: {flat_c}");
        // genuinely nested branching still scores higher than the wide flat match
        assert!(
            nested_c > flat_c,
            "nested {nested_c} should exceed flat {flat_c}"
        );
    }

    #[test]
    fn linear_try_chain_is_not_complex() {
        let src = r#"
fn pipeline() -> Result<(), ()> {
    a()?;
    b()?;
    c()?;
    d()?;
    e()?;
    Ok(())
}
"#;
        // `?` is linear error propagation, not a cognitive branch — a straight
        // chain of fallible calls has complexity 1 (counting it would inflate to ~6).
        assert_eq!(extract("src/a.rs", src).symbols[0].complexity, 1);
    }

    // ---- non-Rust extraction (generic tree-sitter) -------------------------

    #[test]
    fn python_symbols_and_imports() {
        let src = r#"
import os
from hashlib import sha256

def verify(token):
    return len(token) > 0

class TokenStore:
    def get(self, key):
        return os.environ.get(key)
"#;
        let ex = extract("svc/auth.py", src);
        assert_eq!(ex.language, Language::Python);
        let names: Vec<_> = ex.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"verify"), "got {names:?}");
        assert!(names.contains(&"TokenStore"), "got {names:?}");
        assert!(names.contains(&"get"), "got {names:?}");
        assert!(ex.imports.iter().any(|i| i.contains("os")));
        assert!(ex.imports.iter().any(|i| i.contains("sha256")));
    }

    #[test]
    fn go_symbols_and_imports() {
        let src = r#"
package svc

import "fmt"

type Token struct { Value string }

func Verify(t string) bool { return len(t) > 0 }

func (s *Store) Get(k string) string { return "" }
"#;
        let ex = extract("svc/auth.go", src);
        assert_eq!(ex.language, Language::Go);
        let names: Vec<_> = ex.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Verify"), "got {names:?}");
        assert!(names.contains(&"Get"), "got {names:?}");
        assert!(names.contains(&"Token"), "got {names:?}");
        assert!(ex.imports.iter().any(|i| i.contains("fmt")));
    }

    #[test]
    fn javascript_symbols_and_imports() {
        let src = r#"
import { sha256 } from "crypto";

export function verify(token) {
    return token.length > 0;
}

class TokenStore {
    get(key) { return key; }
}
"#;
        let ex = extract("svc/auth.js", src);
        assert_eq!(ex.language, Language::JavaScript);
        let names: Vec<_> = ex.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"verify"), "got {names:?}");
        assert!(names.contains(&"TokenStore"), "got {names:?}");
        assert!(names.contains(&"get"), "got {names:?}");
        assert!(ex.imports.iter().any(|i| i.contains("crypto")));
    }

    #[test]
    fn typescript_symbols_and_imports() {
        let src = r#"
import { Hash } from "crypto";

export interface Token { value: string; }

export type Id = string;

export function verify(token: string): boolean {
    return token.length > 0;
}

class TokenStore {
    get(key: string): string { return key; }
}
"#;
        let ex = extract("svc/auth.ts", src);
        assert_eq!(ex.language, Language::TypeScript);
        let names: Vec<_> = ex.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"verify"), "got {names:?}");
        assert!(names.contains(&"TokenStore"), "got {names:?}");
        assert!(names.contains(&"Token"), "got {names:?}");
        assert!(names.contains(&"Id"), "got {names:?}");
        assert!(ex.imports.iter().any(|i| i.contains("crypto")));
    }
}
