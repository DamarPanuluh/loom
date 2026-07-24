//! Call-site extraction — the raw material of the call graph.
//!
//! Plane: structural (derived). Pure function of file content, deterministic
//! and sorted, so the derived plane stays rebuildable (INV-2).
//!
//! Contract: this records what the source SAYS, never what it means. `capture`,
//! `Store::open`, `self.flush` are written names; deciding which definition each
//! refers to needs the whole repo and belongs in `callgraph`. Splitting it here
//! keeps extraction per-file and pure — and keeps the guessing in one place
//! where it can be labelled as guessing.

use super::{CallSite, Language, Symbol};
use tree_sitter::Node;

/// Call-expression node kinds per language, and the field holding the callee.
fn spec(language: Language) -> Option<(tree_sitter::Language, &'static [&'static str])> {
    Some(match language {
        Language::Rust => (
            tree_sitter_rust::LANGUAGE.into(),
            &["call_expression", "macro_invocation"] as &[&str],
        ),
        Language::Python => (tree_sitter_python::LANGUAGE.into(), &["call"]),
        Language::Go => (tree_sitter_go::LANGUAGE.into(), &["call_expression"]),
        Language::JavaScript => (
            tree_sitter_javascript::LANGUAGE.into(),
            &["call_expression"],
        ),
        Language::TypeScript => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            &["call_expression"],
        ),
        Language::Other => return None,
    })
}

/// The callee as written. A dotted/scoped path keeps its last two segments
/// (`Store::open`, `self.flush`) so resolution can try the qualified name first
/// and the bare one second — `open` alone is far more ambiguous than
/// `Store::open`.
fn callee_name(node: &Node, bytes: &[u8]) -> Option<String> {
    let target = node
        .child_by_field_name("function")
        .or_else(|| node.child_by_field_name("macro"))?;
    let text = target.utf8_text(bytes).ok()?.trim();
    if text.is_empty() || text.len() > 200 {
        return None;
    }
    // Normalize whitespace and generics: `foo :: < T >` → `foo`.
    let cleaned: String = text
        .chars()
        .take_while(|c| *c != '<' && *c != '(')
        .filter(|c| !c.is_whitespace())
        .collect();
    let parts: Vec<&str> = cleaned
        .replace("::", ".")
        .split('.')
        .filter(|p| !p.is_empty())
        .map(|p| {
            // SAFETY of the borrow: re-find each part in the original so the
            // slices outlive the temporary `replace` allocation.
            let start = cleaned.find(p).unwrap_or(0);
            &cleaned[start..start + p.len()]
        })
        .collect();
    match parts.len() {
        0 => None,
        1 => Some(parts[0].to_string()),
        n => Some(format!("{}::{}", parts[n - 2], parts[n - 1])),
    }
}

/// Which declared symbol encloses this line. Symbols are sorted by start line,
/// so the innermost enclosing range wins.
fn enclosing(symbols: &[Symbol], line: usize) -> &str {
    symbols
        .iter()
        .filter(|s| s.line_start <= line && line <= s.line_end)
        .min_by_key(|s| s.line_end - s.line_start)
        .map(|s| s.name.as_str())
        .unwrap_or("")
}

pub(super) fn extract(language: Language, content: &str, symbols: &[Symbol]) -> Vec<CallSite> {
    let Some((lang, kinds)) = spec(language) else {
        return Vec::new();
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&lang).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(content, None) else {
        return Vec::new();
    };
    let bytes = content.as_bytes();
    let mut out: Vec<CallSite> = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if kinds.contains(&node.kind()) {
            if let Some(callee) = callee_name(&node, bytes) {
                let line = node.start_position().row + 1;
                out.push(CallSite {
                    from: enclosing(symbols, line).to_string(),
                    callee,
                });
            }
        }
        for i in 0..node.child_count() as u32 {
            if let Some(c) = node.child(i) {
                stack.push(c);
            }
        }
    }
    out.sort_by(|a, b| a.from.cmp(&b.from).then(a.callee.cmp(&b.callee)));
    out.dedup();
    out
}

#[cfg(test)]
mod tests {

    #[test]
    fn rust_calls_are_attributed_to_their_enclosing_function() {
        let src = "fn capture(o: u64) -> u64 {\n    let s = Store::open();\n    validate(o)\n}\n";
        let ex = crate::extract::extract("src/pay.rs", src);
        let calls: Vec<(&str, &str)> = ex
            .calls
            .iter()
            .map(|c| (c.from.as_str(), c.callee.as_str()))
            .collect();
        assert!(calls.contains(&("capture", "validate")), "{calls:?}");
        // A qualified path keeps its last two segments: `open` alone would be
        // hopelessly ambiguous across a real repo.
        assert!(calls.contains(&("capture", "Store::open")), "{calls:?}");
    }

    #[test]
    fn typescript_calls_inside_an_arrow_const_are_attributed() {
        // The idiom the extraction fix recovered: without `variable_declarator`
        // as a symbol, `handler` would not exist and every call inside it would
        // be attributed to file scope — the call graph would have a hole exactly
        // where most TS logic lives.
        let src = "export const handler = async (id: string) => {\n  return fetchUser(id);\n};\n";
        let ex = crate::extract::extract("src/api.ts", src);
        let calls: Vec<(&str, &str)> = ex
            .calls
            .iter()
            .map(|c| (c.from.as_str(), c.callee.as_str()))
            .collect();
        assert!(calls.contains(&("handler", "fetchUser")), "{calls:?}");
    }

    #[test]
    fn extraction_is_deterministic() {
        let src = "fn a() { z(); b(); }\nfn b() { c(); }\n";
        let first = crate::extract::extract("src/x.rs", src).calls;
        let second = crate::extract::extract("src/x.rs", src).calls;
        assert_eq!(first, second, "the derived plane must rebuild identically");
        // Sorted by (caller, callee) — callees order WITHIN each caller, not
        // globally, so `a` calling b and z precedes `b` calling c.
        let order: Vec<(&str, &str)> = first
            .iter()
            .map(|c| (c.from.as_str(), c.callee.as_str()))
            .collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted, "sorted output, not walk order");
    }
}
