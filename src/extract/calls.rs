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

/// Call-expression node kinds per language. `None` for languages with no
/// grammar; the caller parses the tree, so no grammar is returned here.
fn call_kinds(language: Language) -> Option<&'static [&'static str]> {
    Some(match language {
        Language::Rust => &["call_expression", "macro_invocation"] as &[&str],
        Language::Python => &["call"],
        Language::Go => &["call_expression"],
        Language::JavaScript => &["call_expression"],
        Language::TypeScript => &["call_expression"],
        Language::Other => return None,
    })
}

/// Calls hiding inside a macro's token tree.
///
/// Only `ident (` counts — an identifier whose very next token opens a call.
/// Deliberately narrow: a token tree has no grammar to lean on, so anything
/// cleverer would start inventing edges. Paths keep their last two segments the
/// same way `callee_name` does, so `Store::init` reads alike from both places.
fn identifiers_called_in(tree: &Node, bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![*tree];
    while let Some(node) = stack.pop() {
        for i in 0..node.child_count() as u32 {
            let Some(child) = node.child(i) else { continue };
            stack.push(child);
            if child.kind() != "identifier" && child.kind() != "scoped_identifier" {
                continue;
            }
            // The next sibling must open the argument list, with nothing
            // between: `foo (` is a call, `foo, (` is two tokens.
            let Some(next) = child.next_sibling() else {
                continue;
            };
            if next.kind() != "(" && next.kind() != "token_tree" {
                continue;
            }
            if next.start_byte() != child.end_byte() {
                continue;
            }
            let Ok(text) = child.utf8_text(bytes) else {
                continue;
            };
            let cleaned = text.trim();
            if cleaned.is_empty() || cleaned.len() > 200 {
                continue;
            }
            // Macro-only shapes that are never functions.
            if matches!(cleaned, "if" | "match" | "while" | "for" | "return") {
                continue;
            }
            out.push(cleaned.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The callee as written. A dotted/scoped path keeps its last two segments
/// (`Store::open`, `self.flush`) so resolution can try the qualified name first
/// and the bare one second — `open` alone is far more ambiguous than
/// `Store::open`.
///
/// Structural, not textual: the callee is read by walking the `function` field's
/// AST and collecting its trailing name segments. A textual scan that truncated
/// at the first `(` discarded the OUTER call of a chain (`Store::open(p).flush()`
/// recorded only `Store::open`) and dropped qualified/parenthesized callees
/// (`(a.b)()`, `<T as Tr>::m()`) entirely.
fn callee_name(node: &Node, bytes: &[u8]) -> Option<String> {
    let target = node
        .child_by_field_name("function")
        .or_else(|| node.child_by_field_name("macro"))?;
    let mut segments: Vec<String> = Vec::new();
    collect_callee_segments(target, bytes, &mut segments);
    if segments.is_empty() {
        // Best-effort fallback for a shape the structural walk did not
        // recognize: strip generics/args from the raw text and split, as the
        // old path did. Strictly a superset of the walk, never a regression.
        return callee_from_text(&target, bytes);
    }
    match segments.len() {
        1 => Some(segments.pop().unwrap()),
        n => Some(format!("{}::{}", segments[n - 2], segments[n - 1])),
    }
}

/// Collect the callee's name segments from the AST, outermost-receiver first,
/// method/name last. Argument lists and generic arguments are structurally
/// skipped (they are separate fields), so a chain reduces to its written path.
fn collect_callee_segments(node: Node, bytes: &[u8], out: &mut Vec<String>) {
    match node.kind() {
        "identifier"
        | "field_identifier"
        | "property_identifier"
        | "type_identifier"
        | "shorthand_property_identifier"
        | "package_identifier" => {
            if let Ok(t) = node.utf8_text(bytes) {
                let t = t.trim();
                if !t.is_empty() && t.len() <= 200 {
                    out.push(t.to_string());
                }
            }
        }
        // Rust `A::B` / `A::B::C`.
        "scoped_identifier" | "scoped_type_identifier" => {
            if let Some(p) = node.child_by_field_name("path") {
                collect_callee_segments(p, bytes, out);
            }
            if let Some(n) = node.child_by_field_name("name") {
                collect_callee_segments(n, bytes, out);
            }
        }
        // Rust `x.y` — receiver then field.
        "field_expression" => {
            if let Some(v) = node.child_by_field_name("value") {
                collect_callee_segments(v, bytes, out);
            }
            if let Some(f) = node.child_by_field_name("field") {
                collect_callee_segments(f, bytes, out);
            }
        }
        // JS/TS `x.y` — object then property.
        "member_expression" => {
            if let Some(o) = node.child_by_field_name("object") {
                collect_callee_segments(o, bytes, out);
            }
            if let Some(p) = node.child_by_field_name("property") {
                collect_callee_segments(p, bytes, out);
            }
        }
        // Go `pkg.Func` — operand then field.
        "selector_expression" => {
            if let Some(o) = node.child_by_field_name("operand") {
                collect_callee_segments(o, bytes, out);
            }
            if let Some(f) = node.child_by_field_name("field") {
                collect_callee_segments(f, bytes, out);
            }
        }
        // A chained call's receiver (`a().b()`): recurse into the inner call's
        // callee so the OUTER method is not lost and the args are skipped.
        "call_expression" | "call" => {
            if let Some(f) = node
                .child_by_field_name("function")
                .or_else(|| node.child_by_field_name("macro"))
            {
                collect_callee_segments(f, bytes, out);
            }
        }
        // Turbofish / generic application: the callee is the wrapped function.
        "generic_function" | "generic_type" => {
            if let Some(f) = node.child_by_field_name("function") {
                collect_callee_segments(f, bytes, out);
            } else if let Some(inner) = node.named_child(0) {
                collect_callee_segments(inner, bytes, out);
            }
        }
        // Grouping parens (`(a.b)()`): descend to the wrapped expression.
        "parenthesized_expression" => {
            if let Some(inner) = node.named_child(0) {
                collect_callee_segments(inner, bytes, out);
            }
        }
        _ => {}
    }
}

/// Textual fallback: strip whitespace and everything from the first `<`/`(` on,
/// then keep the last two `.`/`::` segments. Used only when the structural walk
/// finds no segments.
fn callee_from_text(target: &Node, bytes: &[u8]) -> Option<String> {
    let text = target.utf8_text(bytes).ok()?.trim();
    if text.is_empty() || text.len() > 200 {
        return None;
    }
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

pub(super) fn extract(
    language: Language,
    root: tree_sitter::Node,
    content: &str,
    symbols: &[Symbol],
) -> Vec<CallSite> {
    let Some(kinds) = call_kinds(language) else {
        return Vec::new();
    };
    let bytes = content.as_bytes();
    let mut out: Vec<CallSite> = Vec::new();
    let mut stack = vec![root];
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
        // A macro's arguments are an unstructured `token_tree`, so nothing
        // inside one is parsed as a call. In Rust that hides almost every call
        // a test makes — `assert_eq!(effective(...), ...)` — which is exactly
        // the evidence the S3 call witness needs. Scan the tokens for the one
        // unambiguous shape: an identifier immediately followed by `(`.
        if node.kind() == "token_tree" {
            // Only the OUTERMOST token tree: `identifiers_called_in` already
            // recurses into nested token trees, so also scanning a nested one
            // (now that whole-row dedup is gone) would double-count its calls.
            let nested = node
                .parent()
                .map(|p| p.kind() == "token_tree")
                .unwrap_or(false);
            if !nested {
                let line = node.start_position().row + 1;
                let from = enclosing(symbols, line).to_string();
                for callee in identifiers_called_in(&node, bytes) {
                    out.push(CallSite {
                        from: from.clone(),
                        callee,
                    });
                }
            }
        }
        for i in 0..node.child_count() as u32 {
            if let Some(c) = node.child(i) {
                stack.push(c);
            }
        }
    }
    out.sort_by(|a, b| a.from.cmp(&b.from).then(a.callee.cmp(&b.callee)));
    // Sorted, but NOT deduped: two calls to the same callee from one function
    // are two call sites, and collapsing them to one erased call multiplicity
    // that a weight- or frequency-aware witness would need. Equal rows sit
    // adjacent, so the output stays deterministic (INV-2).
    out
}

#[cfg(test)]
mod chain_tests {
    use super::*;

    /// Test shim mirroring the old content-parsing signature: parse once here,
    /// then delegate to the root-taking `super::extract`.
    fn extract(language: Language, content: &str, symbols: &[Symbol]) -> Vec<CallSite> {
        let Some(tree) = language
            .grammar()
            .and_then(|g| crate::extract::parse(content, &g))
        else {
            return Vec::new();
        };
        super::extract(language, tree.root_node(), content, symbols)
    }

    #[test]
    fn a_chained_call_records_the_outer_method_not_just_the_inner() {
        // `Store::open(p).flush()` is TWO calls: the inner `Store::open` and the
        // outer `.flush`. Truncating the callee text at the first `(` dropped the
        // outer method entirely; the structural walk keeps both.
        let src = "fn go(p: u64) {\n    Store::open(p).flush();\n}\n";
        let calls: Vec<String> = extract(Language::Rust, src, &[])
            .into_iter()
            .map(|c| c.callee)
            .collect();
        assert!(calls.contains(&"Store::open".to_string()), "{calls:?}");
        // The outer method is reachable now (bare name `flush`), where before it
        // was silently discarded.
        assert!(
            calls.iter().any(|c| c.rsplit("::").next() == Some("flush")),
            "outer .flush must be recorded: {calls:?}"
        );
    }

    #[test]
    fn repeated_calls_keep_their_multiplicity() {
        // Two calls to the same callee are two call sites; whole-row dedup used
        // to collapse them to one, erasing multiplicity.
        let src = "fn f() {\n    g();\n    g();\n}\n";
        let calls: Vec<String> = extract(Language::Rust, src, &[])
            .into_iter()
            .filter(|c| c.callee == "g")
            .map(|c| c.callee)
            .collect();
        assert_eq!(calls.len(), 2, "both call sites must survive: {calls:?}");
    }
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

#[cfg(test)]
mod macro_call_tests {
    use super::*;

    /// Test shim mirroring the old content-parsing signature: parse once here,
    /// then delegate to the root-taking `super::extract`.
    fn extract(language: Language, content: &str, symbols: &[Symbol]) -> Vec<CallSite> {
        let Some(tree) = language
            .grammar()
            .and_then(|g| crate::extract::parse(content, &g))
        else {
            return Vec::new();
        };
        super::extract(language, tree.root_node(), content, symbols)
    }

    /// Calls inside a macro are still calls.
    ///
    /// A macro's arguments parse as an unstructured `token_tree`, so nothing
    /// inside one was seen. In Rust that hides almost everything a test does —
    /// `assert_eq!(effective(..), ..)` — and the S3 call witness reads exactly
    /// those calls to decide whether a proof reaches the code it proves. Every
    /// Rust suite in the graph was invisible to it.
    #[test]
    fn a_call_inside_a_macro_is_extracted() {
        let src = "fn t() {\n    assert_eq!(effective(a, b), c);\n}\n";
        let calls: Vec<String> = extract(Language::Rust, src, &[])
            .into_iter()
            .map(|c| c.callee)
            .collect();
        assert!(calls.contains(&"effective".to_string()), "{calls:?}");
        assert!(calls.contains(&"assert_eq".to_string()), "{calls:?}");
    }

    /// The scan stays narrow: only an identifier whose very next token opens a
    /// call. A token tree has no grammar to lean on, so anything cleverer would
    /// start inventing edges that are not there.
    #[test]
    fn bare_identifiers_in_a_macro_are_not_calls() {
        let src = "fn t() {\n    assert!(flag, \"msg\", other);\n}\n";
        let calls: Vec<String> = extract(Language::Rust, src, &[])
            .into_iter()
            .map(|c| c.callee)
            .collect();
        assert!(!calls.contains(&"flag".to_string()), "{calls:?}");
        assert!(!calls.contains(&"other".to_string()), "{calls:?}");
    }
}
