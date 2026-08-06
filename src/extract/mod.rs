//! Code extraction — the derived structural facts of a file.
//!
//! Plane: derived. Everything here is a pure function of file content (plus its
//! path for language/role). `sync` calls `extract` and writes the results as
//! derived facets; nothing here touches the store. Deterministic by construction
//! so the derived plane is rebuildable (INV-2).

use std::path::Path;
mod calls;
mod langs;
mod metrics;
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

    /// The tree-sitter grammar for this language, or `None` for [`Language::Other`].
    /// The single place a grammar is chosen, so a file is parsed exactly once and
    /// the one tree is shared by symbol and call extraction.
    fn grammar(&self) -> Option<tree_sitter::Language> {
        Some(match self {
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
            Language::Python => tree_sitter_python::LANGUAGE.into(),
            Language::Go => tree_sitter_go::LANGUAGE.into(),
            Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Language::Other => return None,
        })
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
    /// Harness-executed test function (`#[test]`, or inside a `#[cfg(test)]`
    /// module). Only these symbols (plus symbols they reach) may serve as
    /// derived proof entry points — an uncalled helper must never look like an
    /// executed entry.
    pub is_test: bool,
    /// Complexity proxy (branch/loop points; a match counts once, not per arm).
    /// 0 for non-callable symbols.
    pub complexity: u32,
    /// Deepest branch-nesting level inside the body. 0 for non-callables.
    pub max_nesting: u32,
    /// Declared arguments, receiver (`self`/`cls`/`this`) excluded. 0 for
    /// non-callables.
    pub arg_count: u32,
}

/// One observed call: which symbol makes it, and the callee's written name.
///
/// The name is what the source says, not a resolved target — resolution needs
/// the whole repo and belongs in `callgraph`. Keeping extraction per-file and
/// pure is what lets the derived plane stay rebuildable (INV-2).
#[derive(Debug, Clone, PartialEq)]
pub struct CallSite {
    /// The enclosing symbol, or empty at file scope.
    pub from: String,
    /// The callee as written: `capture`, `Store::open`, `self.flush`.
    pub callee: String,
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
    /// Calls made in this file, sorted. The raw material of the call graph:
    /// "what breaks if I change this" is the one question an agent cannot
    /// cheaply rebuild per session, and the one loom is best placed to answer.
    pub calls: Vec<CallSite>,
}

/// Extract derived facts from a file's content.
pub fn extract(path: &str, content: &str) -> Extraction {
    let language = Language::detect(path);
    let role = Role::detect(path);
    let loc = content.lines().count();
    let content_hash = fnv1a(content);
    // Parse once and share the tree: symbol/import/panic extraction and call
    // extraction both walk the same root, instead of each re-parsing the file
    // (this runs per file on the sync hot path). Still a pure function of
    // content, so the derived plane stays rebuildable (INV-2).
    let tree = language.grammar().and_then(|g| parse(content, &g));
    let root = tree.as_ref().map(|t| t.root_node());
    let (symbols, imports, panic_sites) = match (language, root) {
        (Language::Rust, Some(r)) => rust::rust_extract(r, content),
        (Language::Other, _) | (_, None) => (Vec::new(), Vec::new(), 0),
        (other, Some(r)) => langs::extract(other, r, content),
    };
    // Calls are attributed to the enclosing symbol by line range, so the walk
    // stays per-node and the attribution stays one sorted pass.
    let calls = match root {
        Some(r) => calls::extract(language, r, content, &symbols),
        None => Vec::new(),
    };
    Extraction {
        language,
        role,
        loc,
        content_hash,
        symbols,
        imports,
        panic_sites,
        calls,
    }
}

/// Parse `content` with `grammar`, or `None` if the grammar cannot be set or the
/// parse fails.
fn parse(content: &str, grammar: &tree_sitter::Language) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(grammar).ok()?;
    parser.parse(content, None)
}

/// FNV-1a 64-bit content hash as hex. The single implementation lives in
/// [`crate::artifact::fingerprint`] (the engine's generic change-fingerprint);
/// this alias keeps extraction's local vocabulary.
pub fn fnv1a(s: &str) -> String {
    crate::artifact::fingerprint(s)
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
    fn rust_import_normalization() {
        let src = r#"
use crate::{principal::{Principal}, delivery::deliver};
use crate::principal::{self, Principal as P2};
pub use crate::x::Y;
pub(crate) use crate::z::W;
use ::ext::api::C;
use crate::glob::*;

pub fn f() {}
"#;
        let ex = extract("src/a.rs", src);

        for expected in [
            "crate::principal::Principal",
            "crate::delivery::deliver",
            "crate::principal",
            "crate::x::Y",
            "crate::z::W",
            "ext::api::C",
            "crate::glob",
        ] {
            assert!(
                ex.imports.iter().any(|i| i == expected),
                "expected import {expected:?} in {:?}",
                ex.imports
            );
        }

        for i in &ex.imports {
            assert!(
                !i.contains("pub "),
                "import should not include visibility: {i:?}; imports: {:?}",
                ex.imports
            );
            assert!(
                !i.contains('{'),
                "import should not include raw groups: {i:?}; imports: {:?}",
                ex.imports
            );
            assert!(
                !i.contains('*'),
                "import should not include glob marker: {i:?}; imports: {:?}",
                ex.imports
            );
            assert!(
                !i.contains(" as "),
                "import should not include aliases: {i:?}; imports: {:?}",
                ex.imports
            );
            assert!(
                !i.starts_with(':'),
                "import should not start with a colon: {i:?}; imports: {:?}",
                ex.imports
            );
        }
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

    // ---- per-symbol extraction metrics (complexity / nesting / arg_count) -

    #[test]
    fn rust_arg_count_free_fn_excludes_nothing() {
        // A free function with no receiver: every declared parameter counts.
        let src = "fn seven(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32, g: i32) -> i32 { 0 }";
        let s = extract("src/a.rs", src).symbols[0].clone();
        assert_eq!(s.arg_count, 7, "7 declared params, no self -> arg_count 7");
        // A bare function carries no branches.
        assert_eq!(s.complexity, 1);
    }

    #[test]
    fn rust_arg_count_impl_method_excludes_self() {
        // `&self` is a `self_parameter`, which is not in the Rust param_kinds,
        // so the receiver is dropped by kind — only the two named params count.
        let src = "struct S;\nimpl S {\n    fn m(&self, a: i32, b: i32) -> i32 { 0 }\n}";
        let m = extract("src/a.rs", src)
            .symbols
            .into_iter()
            .find(|s| s.name == "m")
            .expect("method m extracted");
        assert_eq!(m.arg_count, 2, "&self dropped, two named params remain");
    }

    #[test]
    fn rust_nested_named_fn_is_a_complexity_boundary() {
        // The parent is flat except for a nested named `fn` whose body holds five
        // nested ifs. Because `function_item` is a boundary kind, the walk stops at
        // `inner` — the parent's complexity stays at its own base (1) and does not
        // absorb the nested body, while `inner` gets its own symbol with >1.
        let src = "\
fn outer() {
    let _ = 1;
    fn inner() {
        if a { if b { if c { if d { if e { } } } } }
    }
}
";
        let syms = extract("src/a.rs", src).symbols;
        let outer = syms.iter().find(|s| s.name == "outer").unwrap();
        let inner = syms.iter().find(|s| s.name == "inner").unwrap();
        // parent is flat (only a `let`); the nested fn does not leak into it.
        assert_eq!(
            outer.complexity, 1,
            "nested named fn must not inflate parent complexity"
        );
        assert_eq!(
            outer.max_nesting, 0,
            "nested named fn must not inflate parent nesting"
        );
        // the nested symbol carries its own five nested branches.
        assert!(
            inner.complexity > 1,
            "inner should have branchy complexity, got {}",
            inner.complexity
        );
        assert!(
            inner.max_nesting > 0,
            "inner should have nesting depth, got {}",
            inner.max_nesting
        );
    }

    #[test]
    fn rust_closure_body_counts_toward_parent() {
        // A closure is deliberately NOT a boundary — only `function_item` is. So
        // the closure's `if` is attributed to the enclosing symbol, lifting its
        // complexity above the bare base.
        let src = "fn with_closure(x: bool) -> i32 { let f = || { if x { 1 } }; 0 }";
        let s = extract("src/a.rs", src).symbols[0].clone();
        assert_eq!(s.name, "with_closure");
        assert_eq!(
            s.complexity, 2,
            "closure's single if must count toward the enclosing fn (base 1 + 1)"
        );
    }

    #[test]
    fn rust_max_nesting_counts_branch_depth_not_count() {
        // Three nested ifs form a depth-3 chain; three flat ifs at the same level
        // are each depth 1. This pins that max_nesting tracks nesting, not count.
        let nested = "fn n3(a: bool, b: bool, c: bool) { if a { if b { if c { } } } }";
        let flat = "fn nf(a: bool, b: bool, c: bool) { if a { } if b { } if c { } }";
        let n = extract("src/a.rs", nested).symbols[0].clone();
        let f = extract("src/a.rs", flat).symbols[0].clone();
        assert_eq!(n.max_nesting, 3, "three nested ifs -> depth 3");
        assert_eq!(f.max_nesting, 1, "flat ifs stay at depth 1");
        // Note: complexity counts branches, not nesting, so 3 nested ifs and
        // 3 flat ifs both add 3 branch points — complexity need not differ.
    }
    #[test]
    fn python_branchy_complexity_now_nonzero_and_self_excluded() {
        // Complexity used to be hardwired 0 for Python; now the metric walk runs,
        // so a branchy function scores above the base.
        let branchy =
            "def branchy(x):\n    if x:\n        if y:\n            return 1\n    return 0\n";
        let b = extract("svc/a.py", branchy).symbols[0].clone();
        assert_eq!(b.name, "branchy");
        assert!(
            b.complexity > 1,
            "Python branchy fn must now have complexity > 1, got {}",
            b.complexity
        );
        assert!(b.max_nesting > 0);

        // `self` is named in receiver_names and dropped from the count: only the
        // two real args remain.
        let src = "class C:\n    def m(self, a, b):\n        return 0\n";
        let m = extract("svc/a.py", src)
            .symbols
            .into_iter()
            .find(|s| s.name == "m")
            .unwrap();
        assert_eq!(m.arg_count, 2, "self dropped from a Python method's args");
    }

    #[test]
    fn python_nested_def_is_a_boundary() {
        // `function_definition` is a boundary kind, so a nested `def` does not
        // inflate its enclosing function — the outer stays at its own base.
        let src = "def outer():\n    def inner():\n        if a:\n            if b:\n                return 1\n    return 0\n";
        let syms = extract("svc/a.py", src).symbols;
        let outer = syms.iter().find(|s| s.name == "outer").unwrap();
        let inner = syms.iter().find(|s| s.name == "inner").unwrap();
        assert_eq!(
            outer.complexity, 1,
            "nested Python def must not inflate the enclosing fn"
        );
        assert!(inner.complexity > 1, "inner carries its own branches");
    }

    #[test]
    fn go_arg_count_counts_parameter_declarations_and_branches_score() {
        // `a, b int` is ONE `parameter_declaration` (conservative by design, see
        // GO_METRICS docs); `c string` is a second. arg_count counts declarations,
        // not names — so this is 2, not 3.
        let src = "package svc\n\nfunc F(a, b int, c string) int {\n    if a {\n        return 1\n    }\n    return 0\n}\n";
        let f = extract("svc/a.go", src).symbols[0].clone();
        assert_eq!(f.name, "F");
        assert_eq!(
            f.arg_count, 2,
            "Go counts parameter_declarations: `a, b int` is one decl"
        );
        assert!(f.complexity > 1, "branchy Go fn complexity > 1");
        assert!(f.max_nesting > 0, "branchy Go fn nesting > 0");

        // Two single-name declarations -> arg_count 2.
        let g = extract(
            "svc/a.go",
            "package svc\n\nfunc G(x int, y int) int { return 0 }\n",
        )
        .symbols[0]
            .clone();
        assert_eq!(g.arg_count, 2);
    }

    #[test]
    fn typescript_arg_count_and_branch_complexity() {
        // Three `required_parameter` nodes -> arg_count 3; branchy body scores.
        let src = "export function verify(a: string, b: number, c: boolean): boolean {\n    if (a) { return true; }\n    return false;\n}\n";
        let v = extract("svc/a.ts", src).symbols[0].clone();
        assert_eq!(v.name, "verify");
        assert_eq!(v.arg_count, 3);
        assert!(v.complexity > 1, "branchy TS fn complexity > 1");
        assert!(v.max_nesting > 0);
    }

    #[test]
    fn typescript_method_excludes_this_receiver() {
        // `this` is named in TS receiver_names and dropped: only a, b remain.
        let src = "class C {\n    m(this: C, a: number, b: number): number { return 0; }\n}\n";
        let m = extract("svc/a.ts", src)
            .symbols
            .into_iter()
            .find(|s| s.name == "m")
            .unwrap();
        assert_eq!(m.arg_count, 2, "this dropped from a TS method's args");
    }

    #[test]
    fn non_callable_symbols_keep_zero_metrics() {
        // Structs/enums/traits (and their peers in other languages) are not in
        // the callables list, so they get the default-zero metrics.
        let rust = extract("src/a.rs", "struct S;\nenum E { V }\ntrait T { fn f(); }");
        for s in &rust.symbols {
            assert_eq!(s.complexity, 0, "{} should be 0-complexity", s.name);
            assert_eq!(s.max_nesting, 0, "{} should be 0-nesting", s.name);
            assert_eq!(s.arg_count, 0, "{} should be 0-args", s.name);
        }

        // A TS class and interface are non-callable too.
        let ts = extract(
            "svc/a.ts",
            "export interface Token { value: string; }\nclass TokenStore { x: number = 0; }\n",
        );
        for s in &ts.symbols {
            assert_eq!(s.complexity, 0, "{} should be 0-complexity", s.name);
            assert_eq!(s.max_nesting, 0, "{} should be 0-nesting", s.name);
            assert_eq!(s.arg_count, 0, "{} should be 0-args", s.name);
        }
    }
}
