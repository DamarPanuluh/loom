//! Rust-specific extraction via tree-sitter-rust (symbols, imports, panic sites, complexity).

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
                    let complexity = complexity_of(&node);
                    symbols.push(Symbol {
                        name,
                        kind: "function".into(),
                        line_start: node.start_position().row + 1,
                        line_end: node.end_position().row + 1,
                        complexity,
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
                    });
                }
            }
            "use_declaration" => {
                if let Ok(text) = node.utf8_text(bytes) {
                    let t = text
                        .trim_start_matches("use ")
                        .trim_end_matches(';')
                        .trim()
                        .to_string();
                    if !t.is_empty() {
                        imports.push(t);
                    }
                }
            }
            // test module: its symbols + panics are test-only — skip the subtree
            "mod_item" if is_cfg_test_mod(&node, bytes) => continue,
            _ => {}
        }
        for i in 0..node.child_count() {
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
    for i in 0..node.child_count() {
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

/// Cognitive-complexity proxy: 1 + nesting/branching points (if / while / for /
/// loop / && / ||) plus one per `match` (counted once, not per arm). `?` is
/// linear error propagation, not cognitive load, so it is NOT counted; and
/// wide-but-flat dispatch tables are not penalized like genuinely nested logic.
fn complexity_of(node: &tree_sitter::Node) -> u32 {
    let mut count: u32 = 1;
    let mut stack = vec![*node];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "if_expression" | "while_expression" | "for_expression" | "loop_expression"
            | "match_expression" => count += 1,
            "binary_expression" => {
                // && and || add a branch
                let mut c = n.walk();
                for child in n.children(&mut c) {
                    if matches!(child.kind(), "&&" | "||") {
                        count += 1;
                    }
                }
            }
            _ => {}
        }
        for i in 0..n.child_count() {
            if let Some(c) = n.child(i) {
                stack.push(c);
            }
        }
    }
    count
}
