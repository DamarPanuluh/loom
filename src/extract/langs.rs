//! Generic tree-sitter extraction for non-Rust languages (Python, Go, JS, TS).

use super::metrics::{measure, MetricSpec, GO_METRICS, JS_METRICS, PYTHON_METRICS, TS_METRICS};
use super::{child_name, Language, Symbol};

struct LangSpec {
    symbols: &'static [(&'static str, &'static str)],
    imports: &'static [&'static str],
    /// Node kinds measured as callables (complexity / nesting / args).
    callables: &'static [&'static str],
    metrics: &'static MetricSpec,
}

const PYTHON_SPEC: LangSpec = LangSpec {
    symbols: &[
        ("function_definition", "function"),
        ("class_definition", "class"),
    ],
    imports: &["import_statement", "import_from_statement"],
    callables: &["function_definition"],
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
        if spec.imports.contains(&kind) {
            if let Ok(text) = node.utf8_text(bytes) {
                let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if !t.is_empty() {
                    imports.push(t);
                }
            }
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
