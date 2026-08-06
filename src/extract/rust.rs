//! Rust-specific extraction via tree-sitter-rust (symbols, imports, panic sites, complexity).
//!
//! Plane: structural (derived) — a pure function of file content; no store
//! access, deterministic so the derived plane stays rebuildable (INV-2).

use super::metrics::{measure, RUST_METRICS};
use super::{child_name, Symbol};

pub(super) fn rust_extract(
    root: tree_sitter::Node,
    content: &str,
) -> (Vec<Symbol>, Vec<String>, usize) {
    let bytes = content.as_bytes();
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut panic_sites = 0usize;

    // Stack carries (node, in_test_subtree, cfg_disabled_subtree). A parent
    // module's disabling `#[cfg(...)]` must suppress harness tests inside it.
    // Crate-level `#![cfg(...)]` on the root source file is the outermost gate.
    let root_cfg_disabled = crate_has_disabling_inner_cfg(&root, bytes);
    let mut stack: Vec<(tree_sitter::Node, bool, bool)> = vec![(root, false, root_cfg_disabled)];
    while let Some((node, in_test, cfg_disabled)) = stack.pop() {
        // Panic sites in a `#[cfg(test)]` module are test-only signal, never
        // production; count only outside a test subtree.
        if !in_test && is_panic_site(&node, bytes) {
            panic_sites += 1;
        }
        // A test module opens a test subtree for everything beneath it. We no
        // longer PRUNE that subtree: its function symbols must still be
        // extracted so call sites inside tests get a real enclosing symbol
        // (otherwise the call graph loses every test→production edge). Their
        // complexity/nesting/args stay zero, so test code adds no production
        // signal — only the call-attribution anchor.
        let child_in_test = in_test || (node.kind() == "mod_item" && is_cfg_test_mod(&node, bytes));
        let child_cfg_disabled =
            cfg_disabled || (node.kind() == "mod_item" && module_has_disabling_cfg(&node, bytes));
        match node.kind() {
            "function_item" => {
                if let Some(name) = child_name(&node, bytes) {
                    let m = if child_in_test {
                        super::metrics::SymbolMetrics::default()
                    } else {
                        measure(&node, bytes, &RUST_METRICS)
                    };
                    symbols.push(Symbol {
                        name,
                        kind: "function".into(),
                        is_test: !cfg_disabled
                            && !child_cfg_disabled
                            && is_harness_test(&node, bytes),
                        line_start: node.start_position().row + 1,
                        line_end: node.end_position().row + 1,
                        complexity: m.complexity,
                        max_nesting: m.max_nesting,
                        arg_count: m.arg_count,
                    });
                }
            }
            // A trait/extern signature has NO body, so there is nothing to
            // measure — cyclomatic base 1 would be a phantom branch. Extract it
            // as a zero-metric declaration (so sync still sees the symbol and
            // converges) rather than a callable.
            "function_signature_item" => {
                if let Some(name) = child_name(&node, bytes) {
                    symbols.push(Symbol {
                        name,
                        kind: "function".into(),
                        is_test: false,
                        line_start: node.start_position().row + 1,
                        line_end: node.end_position().row + 1,
                        complexity: 0,
                        max_nesting: 0,
                        arg_count: 0,
                    });
                }
            }
            "struct_item" | "enum_item" | "trait_item" | "type_item" | "const_item"
            | "static_item" | "union_item" | "macro_definition" => {
                if let Some(name) = child_name(&node, bytes) {
                    symbols.push(Symbol {
                        name,
                        kind: node.kind().trim_end_matches("_item").into(),
                        is_test: false,
                        line_start: node.start_position().row + 1,
                        line_end: node.end_position().row + 1,
                        complexity: 0,
                        max_nesting: 0,
                        arg_count: 0,
                    });
                }
            }
            "use_declaration" => {
                // Walk the use tree into atomic, normalized module paths:
                // visibility (`pub`/`pub(crate)`) is skipped, brace groups
                // (including nested) are expanded, `as` aliases and `*` globs are
                // dropped, `{self}` maps to the module itself, and a leading `::`
                // is trimmed — so the resolver sees `crate::a::b`, never
                // `pub use crate::{a, b}` or `a::b::*`.
                let arg = node.child_by_field_name("argument").or_else(|| {
                    let mut cur = node.walk();
                    let found = node
                        .named_children(&mut cur)
                        .find(|c| c.kind() != "visibility_modifier");
                    found
                });
                if let Some(arg) = arg {
                    collect_use_paths(&arg, bytes, "", &mut imports);
                }
            }
            _ => {}
        }
        for i in 0..node.child_count() as u32 {
            if let Some(c) = node.child(i) {
                stack.push((c, child_in_test, child_cfg_disabled));
            }
        }
    }
    symbols.sort_by(|a, b| a.line_start.cmp(&b.line_start).then(a.name.cmp(&b.name)));
    imports.sort();
    imports.dedup();
    (symbols, imports, panic_sites)
}

/// Expand a Rust `use` tree into atomic module paths. Recurses brace groups,
/// distributes the path prefix, drops `as` aliases and `*` globs, and maps a
/// `{self}` member to the module itself. Emitted paths have a leading `::`
/// trimmed, so a global path resolves like a plain one.
fn collect_use_paths(node: &tree_sitter::Node, bytes: &[u8], prefix: &str, out: &mut Vec<String>) {
    match node.kind() {
        "use_list" => {
            let mut cur = node.walk();
            for child in node.named_children(&mut cur) {
                collect_use_paths(&child, bytes, prefix, out);
            }
        }
        "scoped_use_list" => {
            let path = node
                .child_by_field_name("path")
                .and_then(|p| p.utf8_text(bytes).ok())
                .unwrap_or("");
            let next = join_mod(prefix, path);
            if let Some(list) = node.child_by_field_name("list") {
                collect_use_paths(&list, bytes, &next, out);
            }
        }
        "use_as_clause" => {
            if let Some(p) = node
                .child_by_field_name("path")
                .and_then(|p| p.utf8_text(bytes).ok())
            {
                push_mod(prefix, p, out);
            }
        }
        "use_wildcard" => {
            let mut cur = node.walk();
            let path = node
                .named_children(&mut cur)
                .next()
                .and_then(|p| p.utf8_text(bytes).ok());
            match path {
                Some(p) => push_mod(prefix, p, out),
                None if !prefix.is_empty() => push_mod(prefix, "", out),
                None => {}
            }
        }
        // A bare `self` inside a group (`{self, …}`) means the module itself.
        "self" if !prefix.is_empty() => push_mod(prefix, "", out),
        "identifier" | "scoped_identifier" | "crate" | "super" | "self" | "metavariable" => {
            if let Ok(t) = node.utf8_text(bytes) {
                push_mod(prefix, t, out);
            }
        }
        _ => {}
    }
}

/// Join a module prefix and a segment with `::`, tolerating an empty side.
fn join_mod(prefix: &str, seg: &str) -> String {
    match (prefix.is_empty(), seg.is_empty()) {
        (true, _) => seg.to_string(),
        (_, true) => prefix.to_string(),
        _ => format!("{prefix}::{seg}"),
    }
}

/// Push a joined path, trimming a leading `::` (global-path root) and skipping empties.
fn push_mod(prefix: &str, seg: &str, out: &mut Vec<String>) {
    let path = join_mod(prefix, seg);
    let path = path.strip_prefix("::").unwrap_or(&path);
    if !path.is_empty() {
        out.push(path.to_string());
    }
}

/// A production unwrap()/panic! site: `expr.unwrap()` or `panic!(…)`. AST-based,
/// so `.unwrap()`/`panic!(` text inside strings or comments never counts.
fn is_panic_site(node: &tree_sitter::Node, bytes: &[u8]) -> bool {
    match node.kind() {
        "call_expression" => {
            node.child_by_field_name("function")
                .filter(|f| f.kind() == "field_expression")
                .and_then(|f| f.child_by_field_name("field"))
                .and_then(|n| n.utf8_text(bytes).ok())
                == Some("unwrap")
        }
        "macro_invocation" => {
            node.child_by_field_name("macro")
                .and_then(|n| n.utf8_text(bytes).ok())
                == Some("panic")
        }
        _ => false,
    }
}

/// Whether a `function_item` is marked `#[test]` (or another harness-executed
/// attribute such as `#[tokio::test]`). The attribute is a leading child or a
/// preceding sibling depending on grammar version, so both are checked. Only
/// these symbols are executed directly by `cargo test`; an uncalled helper in
/// the same file is not.
fn is_harness_test(node: &tree_sitter::Node, bytes: &[u8]) -> bool {
    let attr_text = |n: &tree_sitter::Node| -> Option<String> {
        if n.kind() != "attribute_item" {
            return None;
        }
        n.utf8_text(bytes)
            .ok()
            .map(|t| t.chars().filter(|c| !c.is_whitespace()).collect::<String>())
    };
    // Audited harness macros only. Arbitrary `#[noop::test]` is not evidence
    // the cargo harness will execute the function.
    let is_harness_macro = |path: &str| -> bool {
        matches!(
            path,
            "test"
                | "tokio::test"
                | "async_std::test"
                | "actix_rt::test"
                | "actix_web::test"
                | "rstest"
                | "rstest::rstest"
        )
    };
    let parse_attr_path = |t: &str| -> Option<String> {
        let bare = t.trim_end_matches(']');
        let bare = bare.strip_prefix("#[")?;
        // Drop invocation args: `test(worker_threads = 2)` / `ignore = "msg"`.
        let path = bare
            .split('(')
            .next()?
            .split('=')
            .next()?
            .trim_end_matches(',');
        Some(path.to_string())
    };
    let is_test_attr = |n: &tree_sitter::Node| {
        attr_text(n)
            .and_then(|t| parse_attr_path(&t))
            .is_some_and(|path| is_harness_macro(&path))
    };
    let is_ignore_attr = |n: &tree_sitter::Node| {
        attr_text(n).is_some_and(|t| {
            // Direct ignore, or cfg_attr that injects ignore under any predicate
            // we do not evaluate (fail closed: treat as potentially skipped).
            if let Some(path) = parse_attr_path(&t) {
                if path == "ignore" {
                    return true;
                }
            }
            let bare = t.trim_end_matches(']');
            bare.starts_with("#[cfg_attr(") && bare.contains(",ignore")
        })
    };
    // `#[cfg(...)]` / `#[cfg_attr(...)]` that is not pure enablement for test
    // may disable default-run execution under ordinary `cargo test`.
    let is_disabling_cfg = |n: &tree_sitter::Node| {
        attr_text(n).is_some_and(|t| {
            let bare = t.trim_end_matches(']');
            if bare.starts_with("#[cfg_attr(") {
                // Unevaluated cfg_attr may inject ignore or disable the item.
                return true;
            }
            if !bare.starts_with("#[cfg") {
                return false;
            }
            bare != "#[cfg(test)"
        })
    };
    let mut has_test = false;
    let mut has_ignore = false;
    let mut has_disabling_cfg = false;
    let mut consider = |n: &tree_sitter::Node| {
        if is_test_attr(n) {
            has_test = true;
        }
        if is_ignore_attr(n) {
            has_ignore = true;
        }
        if is_disabling_cfg(n) {
            has_disabling_cfg = true;
        }
    };
    consider(node);
    for i in 0..node.child_count() as u32 {
        if let Some(c) = node.child(i) {
            if c.kind() == "function_item" || c.kind() == "function_definition" {
                break;
            }
            consider(&c);
        }
    }
    let mut prev = node.prev_sibling();
    while let Some(sib) = prev {
        if matches!(
            sib.kind(),
            "function_item"
                | "function_signature_item"
                | "struct_item"
                | "enum_item"
                | "impl_item"
                | "mod_item"
                | "const_item"
                | "static_item"
                | "type_item"
                | "trait_item"
                | "macro_definition"
        ) {
            break;
        }
        consider(&sib);
        prev = sib.prev_sibling();
    }
    has_test && !has_ignore && !has_disabling_cfg
}

fn crate_has_disabling_inner_cfg(root: &tree_sitter::Node, bytes: &[u8]) -> bool {
    for i in 0..root.child_count() as u32 {
        if let Some(c) = root.child(i) {
            if c.kind() == "inner_attribute_item" || c.kind() == "attribute_item" {
                if let Ok(text) = c.utf8_text(bytes) {
                    let t = text
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect::<String>();
                    if t.starts_with("#![cfg") {
                        let bare = t.trim_end_matches(']');
                        if bare != "#![cfg(test)" {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

/// True when a module item carries a `#[cfg(...)]` other than pure `#[cfg(test)]`.
/// Tests nested under such a module are not default-run by ordinary `cargo test`.
fn module_has_disabling_cfg(node: &tree_sitter::Node, bytes: &[u8]) -> bool {
    let mut prev = node.prev_sibling();
    while let Some(sib) = prev {
        if sib.kind() == "attribute_item" {
            if let Ok(text) = sib.utf8_text(bytes) {
                let t = text
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>();
                if t.starts_with("#[cfg") {
                    let bare = t.trim_end_matches(']');
                    if bare != "#[cfg(test)" {
                        return true;
                    }
                }
            }
        } else if matches!(
            sib.kind(),
            "function_item"
                | "mod_item"
                | "struct_item"
                | "enum_item"
                | "impl_item"
                | "const_item"
                | "static_item"
                | "type_item"
                | "trait_item"
                | "macro_definition"
        ) {
            break;
        }
        prev = sib.prev_sibling();
    }
    for i in 0..node.child_count() as u32 {
        if let Some(c) = node.child(i) {
            if c.kind() == "attribute_item" {
                if let Ok(text) = c.utf8_text(bytes) {
                    let t = text
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect::<String>();
                    if t.starts_with("#[cfg") {
                        let bare = t.trim_end_matches(']');
                        if bare != "#[cfg(test)" {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

fn is_cfg_test_mod(node: &tree_sitter::Node, bytes: &[u8]) -> bool {
    let has_cfg_test = |n: &tree_sitter::Node| {
        n.kind() == "attribute_item"
            && n.utf8_text(bytes)
                .map(|t| {
                    t.chars()
                        .filter(|c| !c.is_whitespace())
                        .collect::<String>()
                        .contains("cfg(test)")
                })
                .unwrap_or(false)
    };
    let mut sib = node.prev_sibling();
    while let Some(s) = sib {
        if has_cfg_test(&s) {
            return true;
        }
        if !matches!(s.kind(), "line_comment" | "block_comment") {
            break;
        }
        sib = s.prev_sibling();
    }
    for i in 0..node.child_count() as u32 {
        if let Some(c) = node.child(i) {
            if has_cfg_test(&c) {
                return true;
            }
            if c.kind() == "mod" {
                break;
            }
        }
    }
    false
}
