//! Rust-specific extraction via tree-sitter-rust (symbols, imports, panic sites, complexity).
//!
//! Plane: structural (derived) — a pure function of file content; no store
//! access, deterministic so the derived plane stays rebuildable (INV-2).

use super::metrics::{measure, RUST_METRICS};
use super::{child_name, Symbol};

pub(super) fn rust_extract(content: &str) -> (Vec<Symbol>, Vec<String>, usize) {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .is_err()
    {
        return (Vec::new(), Vec::new(), 0);
    }
    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return (Vec::new(), Vec::new(), 0),
    };
    let bytes = content.as_bytes();
    let root = tree.root_node();
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut panic_sites = 0usize;

    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if is_panic_site(&node, bytes) {
            panic_sites += 1;
        }
        match node.kind() {
            "function_item" => {
                if let Some(name) = child_name(&node, bytes) {
                    let m = measure(&node, bytes, &RUST_METRICS);
                    symbols.push(Symbol {
                        name,
                        kind: "function".into(),
                        line_start: node.start_position().row + 1,
                        line_end: node.end_position().row + 1,
                        complexity: m.complexity,
                        max_nesting: m.max_nesting,
                        arg_count: m.arg_count,
                    });
                }
            }
            "struct_item" | "enum_item" | "trait_item" | "type_item" => {
                if let Some(name) = child_name(&node, bytes) {
                    symbols.push(Symbol {
                        name,
                        kind: node.kind().trim_end_matches("_item").into(),
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
            // test module: its symbols + panics are test-only — skip the subtree
            "mod_item" if is_cfg_test_mod(&node, bytes) => continue,
            _ => {}
        }
        for i in 0..node.child_count() as u32 {
            if let Some(c) = node.child(i) {
                stack.push(c);
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

/// Whether a `mod_item` is gated by `#[cfg(test)]`. Attributes may be a leading
/// child or a preceding sibling depending on grammar version, so both are
/// checked. Test modules are skipped during extraction — their unwrap/panic and
/// complexity are test-only, not production signal.
fn is_cfg_test_mod(node: &tree_sitter::Node, bytes: &[u8]) -> bool {
    let has_cfg_test = |n: &tree_sitter::Node| {
        n.kind() == "attribute_item"
            && n.utf8_text(bytes)
                .map(|t| t.replace(' ', "").contains("cfg(test)"))
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
