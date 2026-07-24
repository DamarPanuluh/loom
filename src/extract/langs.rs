//! Generic tree-sitter extraction for non-Rust languages (Python, Go, JS, TS).
//!
//! Plane: structural (derived) — a pure function of file content; no store
//! access, deterministic so the derived plane stays rebuildable (INV-2).

use super::metrics::{measure, MetricSpec, GO_METRICS, JS_METRICS, PYTHON_METRICS, TS_METRICS};
use super::{child_name, Language, Symbol};

struct LangSpec {
    symbols: &'static [(&'static str, &'static str)],
    imports: &'static [&'static str],
    /// Node kinds measured as callables (complexity / nesting / args).
    callables: &'static [&'static str],
    /// Whether `const x = …` bindings are declarations in this language. True
    /// for JS/TS, where the dominant function idiom is an arrow assigned to a
    /// const; false for Python/Go, where an assignment is just an assignment.
    declarators: bool,
    metrics: &'static MetricSpec,
}

const PYTHON_SPEC: LangSpec = LangSpec {
    symbols: &[
        ("function_definition", "function"),
        ("class_definition", "class"),
    ],
    imports: &["import_statement", "import_from_statement"],
    callables: &["function_definition"],
    declarators: false,
    metrics: &PYTHON_METRICS,
};

const GO_SPEC: LangSpec = LangSpec {
    symbols: &[
        ("function_declaration", "function"),
        ("method_declaration", "method"),
        ("type_spec", "type"),
    ],
    imports: &["import_declaration"],
    callables: &["function_declaration", "method_declaration"],
    declarators: false,
    metrics: &GO_METRICS,
};

const JS_SPEC: LangSpec = LangSpec {
    symbols: &[
        ("function_declaration", "function"),
        ("generator_function_declaration", "function"),
        ("class_declaration", "class"),
        ("method_definition", "method"),
    ],
    imports: &["import_statement"],
    callables: &[
        "function_declaration",
        "generator_function_declaration",
        "method_definition",
    ],
    declarators: true,
    metrics: &JS_METRICS,
};

const TS_SPEC: LangSpec = LangSpec {
    symbols: &[
        ("function_declaration", "function"),
        ("class_declaration", "class"),
        ("abstract_class_declaration", "class"),
        ("method_definition", "method"),
        ("interface_declaration", "interface"),
        ("type_alias_declaration", "type"),
        ("enum_declaration", "enum"),
    ],
    imports: &["import_statement"],
    callables: &["function_declaration", "method_definition"],
    declarators: true,
    metrics: &TS_METRICS,
};

/// Walk a parsed tree and pull named declarations (via the `name` field) and
/// import statements. Callable declarations additionally get the shared metric
/// walk (complexity / nesting / args). Deterministic (sorted) like
/// `rust_extract`, so the derived plane stays rebuildable (INV-2).
/// panic_sites = 0 here (Rust-only signal).
fn generic_extract(
    content: &str,
    language: &tree_sitter::Language,
    spec: &LangSpec,
) -> (Vec<Symbol>, Vec<String>, usize) {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(language).is_err() {
        return (Vec::new(), Vec::new(), 0);
    }
    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return (Vec::new(), Vec::new(), 0),
    };
    let bytes = content.as_bytes();
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        if let Some((_, sym_kind)) = spec.symbols.iter().find(|(nk, _)| *nk == kind) {
            if let Some(name) = child_name(&node, bytes) {
                let m = if spec.callables.contains(&kind) {
                    measure(&node, bytes, spec.metrics)
                } else {
                    Default::default()
                };
                symbols.push(Symbol {
                    name,
                    kind: (*sym_kind).into(),
                    line_start: node.start_position().row + 1,
                    line_end: node.end_position().row + 1,
                    complexity: m.complexity,
                    max_nesting: m.max_nesting,
                    arg_count: m.arg_count,
                });
            }
        }
        // `variable_declarator` — the JS/TS idiom the declaration-kind list
        // cannot see. Found on a real polyglot repo: loom extracted 7 of 10
        // declarations from a TypeScript file, and all three it missed were
        // `export const …`. That matters far beyond a count, because
        // `export const handler = async () => {}` is how MOST functions in
        // idiomatic TS/JS are written — so locators could not point at them,
        // coverage could not see them, and a call graph would find neither
        // their callers nor their callees.
        //
        // The initializer decides the kind: an arrow/function expression is a
        // function (and gets the metric walk); anything else is a binding.
        if spec.declarators && kind == "variable_declarator" {
            if let Some(name) = child_name(&node, bytes) {
                let init = node.child_by_field_name("value");
                let is_fn = init.is_some_and(|v| {
                    matches!(
                        v.kind(),
                        "arrow_function" | "function_expression" | "function"
                    )
                });
                let m = match (is_fn, init) {
                    (true, Some(v)) => measure(&v, bytes, spec.metrics),
                    _ => Default::default(),
                };
                symbols.push(Symbol {
                    name,
                    kind: if is_fn { "function" } else { "binding" }.into(),
                    line_start: node.start_position().row + 1,
                    line_end: node.end_position().row + 1,
                    complexity: m.complexity,
                    max_nesting: m.max_nesting,
                    arg_count: m.arg_count,
                });
            }
        }
        if spec.imports.contains(&kind) {
            if let Ok(text) = node.utf8_text(bytes) {
                let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if !t.is_empty() {
                    imports.push(t);
                }
            }
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
    (symbols, imports, 0)
}

/// Dispatch a non-Rust language to its grammar + spec. Rust/Other yield empty.
pub(super) fn extract(language: Language, content: &str) -> (Vec<Symbol>, Vec<String>, usize) {
    let (lang, spec): (tree_sitter::Language, &LangSpec) = match language {
        Language::Python => (tree_sitter_python::LANGUAGE.into(), &PYTHON_SPEC),
        Language::Go => (tree_sitter_go::LANGUAGE.into(), &GO_SPEC),
        Language::JavaScript => (tree_sitter_javascript::LANGUAGE.into(), &JS_SPEC),
        Language::TypeScript => (tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), &TS_SPEC),
        _ => return (Vec::new(), Vec::new(), 0),
    };
    generic_extract(content, &lang, spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression from the first foreign repo loom was ever pointed at: it
    /// extracted 7 of 10 declarations from a real TypeScript file, and all
    /// three misses were `export const …`. The count is the small part — the
    /// arrow-const is how most functions in idiomatic TS/JS are written, so
    /// missing it blinds locators, coverage, and any call graph built on top.
    #[test]
    fn typescript_sees_const_bindings_and_arrow_functions() {
        let src = r#"
export interface Channel { id: string }
export type Member = { kind: 'human' };
export const AGENTS: Record<string, number> = {};
export const handler = async (id: string) => { return id; };
export function getChannel(id: string) { return id; }
"#;
        let (symbols, _, _) = extract(Language::TypeScript, src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        for expected in ["Channel", "Member", "AGENTS", "handler", "getChannel"] {
            assert!(names.contains(&expected), "missing {expected}: {names:?}");
        }
        // The initializer decides the kind: an arrow is a function, a record is
        // a binding. Conflating them would let a detector treat data as code.
        let kind = |n: &str| {
            symbols
                .iter()
                .find(|s| s.name == n)
                .map(|s| s.kind.as_str())
                .unwrap_or("")
        };
        assert_eq!(kind("handler"), "function");
        assert_eq!(kind("AGENTS"), "binding");
    }

    /// A language where `x = …` is an assignment, not a declaration, must not
    /// grow phantom symbols from the same walk.
    #[test]
    fn python_assignments_are_not_declarations() {
        let src = "AGENTS = {}\n\ndef get_channel(cid):\n    local = cid\n    return local\n";
        let (symbols, _, _) = extract(Language::Python, src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["get_channel"], "got {names:?}");
    }
}
