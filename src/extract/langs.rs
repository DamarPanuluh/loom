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
    /// The grammar node that WRAPS a definition together with its leading
    /// decorators (Python `decorated_definition`). When a symbol's parent is this
    /// kind, the symbol's span starts at the wrapper — so its line range, and the
    /// per-symbol fingerprint built from it, INCLUDE the decorators. Without this
    /// a route change like `@app.route("/old")` → `"/new"` sits above the `def`
    /// line the symbol otherwise starts at, so the fingerprint never moved and
    /// symbol-scoped staleness spared a proof that a decorator change broke.
    decorator_wrapper: Option<&'static str>,
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
    decorator_wrapper: Some("decorated_definition"),
    metrics: &PYTHON_METRICS,
};

const GO_SPEC: LangSpec = LangSpec {
    symbols: &[
        ("function_declaration", "function"),
        ("method_declaration", "method"),
        ("type_spec", "type"),
        ("type_alias", "type"),
    ],
    imports: &["import_declaration"],
    callables: &["function_declaration", "method_declaration"],
    declarators: false,
    decorator_wrapper: None,
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
    decorator_wrapper: None,
    metrics: &JS_METRICS,
};

const TS_SPEC: LangSpec = LangSpec {
    symbols: &[
        ("function_declaration", "function"),
        ("generator_function_declaration", "function"),
        ("class_declaration", "class"),
        ("abstract_class_declaration", "class"),
        ("method_definition", "method"),
        ("method_signature", "method"),
        ("function_signature", "function"),
        ("interface_declaration", "interface"),
        ("type_alias_declaration", "type"),
        ("enum_declaration", "enum"),
        ("internal_module", "namespace"),
    ],
    imports: &["import_statement"],
    callables: &[
        "function_declaration",
        "generator_function_declaration",
        "method_definition",
    ],
    declarators: true,
    decorator_wrapper: None,
    metrics: &TS_METRICS,
};

/// Walk a parsed tree and pull named declarations (via the `name` field) and
/// import statements. Callable declarations additionally get the shared metric
/// walk (complexity / nesting / args). Deterministic (sorted) like
/// `rust_extract`, so the derived plane stays rebuildable (INV-2).
/// panic_sites = 0 here (Rust-only signal).
fn generic_extract(
    root: tree_sitter::Node,
    content: &str,
    spec: &LangSpec,
) -> (Vec<Symbol>, Vec<String>, usize) {
    let bytes = content.as_bytes();
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        if let Some((_, sym_kind)) = spec.symbols.iter().find(|(nk, _)| *nk == kind) {
            if let Some(name) = child_name(&node, bytes) {
                let m = if spec.callables.contains(&kind) {
                    measure(&node, bytes, spec.metrics)
                } else {
                    Default::default()
                };
                // Start the span at the decorator wrapper when there is one, so
                // the symbol's line range (and its fingerprint) cover the
                // decorators sitting above the `def`/`class` line.
                let start_row = decorated_start(&node, spec);
                symbols.push(Symbol {
                    name,
                    kind: (*sym_kind).into(),
                    is_test: false,
                    line_start: start_row + 1,
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
        if spec.declarators
            && matches!(
                kind,
                "variable_declarator" | "field_definition" | "public_field_definition"
            )
        {
            // The bound name lives on the `name` field (variable_declarator, TS
            // `public_field_definition`) or `property` (JS `field_definition`).
            // Skip destructuring patterns: `const { a, b } = …` puts an
            // object/array pattern here, and recording "{ a, b }" as one symbol
            // is a phantom that fuses several bindings into one garbage name.
            let name_node = node
                .child_by_field_name("name")
                .or_else(|| node.child_by_field_name("property"));
            let is_plain_name = name_node.is_some_and(|n| {
                matches!(
                    n.kind(),
                    "identifier"
                        | "property_identifier"
                        | "shorthand_property_identifier"
                        | "private_property_identifier"
                )
            });
            if let (true, Some(name_node)) = (is_plain_name, name_node) {
                if let Ok(name) = name_node.utf8_text(bytes) {
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
                        name: name.to_string(),
                        kind: if is_fn { "function" } else { "binding" }.into(),
                        is_test: false,
                        line_start: node.start_position().row + 1,
                        line_end: node.end_position().row + 1,
                        complexity: m.complexity,
                        max_nesting: m.max_nesting,
                        arg_count: m.arg_count,
                    });
                }
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

/// The row a symbol's span should START on: the decorator wrapper's row when the
/// node is a decorated definition, else the node's own row. Returned 0-based (the
/// caller adds 1). Only extends UP to a direct decorator wrapper parent, so an
/// undecorated definition is unaffected.
fn decorated_start(node: &tree_sitter::Node, spec: &LangSpec) -> usize {
    if let Some(wrapper) = spec.decorator_wrapper {
        if let Some(parent) = node.parent() {
            if parent.kind() == wrapper {
                return parent.start_position().row;
            }
        }
    }
    node.start_position().row
}

/// Dispatch a non-Rust language to its spec, walking the already-parsed `root`.
/// Rust/Other yield empty.
pub(super) fn extract(
    language: Language,
    root: tree_sitter::Node,
    content: &str,
) -> (Vec<Symbol>, Vec<String>, usize) {
    let spec: &LangSpec = match language {
        Language::Python => &PYTHON_SPEC,
        Language::Go => &GO_SPEC,
        Language::JavaScript => &JS_SPEC,
        Language::TypeScript => &TS_SPEC,
        _ => return (Vec::new(), Vec::new(), 0),
    };
    generic_extract(root, content, spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test shim mirroring the old content-parsing signature: parse once here,
    /// then delegate to the root-taking `super::extract`.
    fn extract(language: Language, content: &str) -> (Vec<Symbol>, Vec<String>, usize) {
        let Some(tree) = language
            .grammar()
            .and_then(|g| crate::extract::parse(content, &g))
        else {
            return (Vec::new(), Vec::new(), 0);
        };
        super::extract(language, tree.root_node(), content)
    }

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

    /// `elif` and `except` are decision paths a reader must hold, so they count
    /// toward complexity. Counting the `if` alone read a branchy Python function
    /// as nearly linear. `elif` is flat (its body sits at the if's depth), so the
    /// chain must not inflate nesting the way a real nested `if` would.
    #[test]
    fn python_elif_and_except_count_toward_complexity() {
        let src = "\
def classify(x):
    if x == 1:
        return 'a'
    elif x == 2:
        return 'b'
    elif x == 3:
        return 'c'
    else:
        return 'd'
";
        let (symbols, _, _) = extract(Language::Python, src);
        let f = symbols.iter().find(|s| s.name == "classify").unwrap();
        // 1 (base) + if + elif + elif = 4.
        assert_eq!(f.complexity, 4, "elif rungs each count once");
        // A flat if/elif chain is one level deep, not three.
        assert_eq!(f.max_nesting, 1, "elif bodies are not deeper nesting");

        let with_try = "\
def load(p):
    try:
        return read(p)
    except IOError:
        return None
    except ValueError:
        return None
";
        let (syms, _, _) = extract(Language::Python, with_try);
        let g = syms.iter().find(|s| s.name == "load").unwrap();
        // 1 (base) + except + except = 3; `try` itself is not a branch.
        assert_eq!(g.complexity, 3, "each except handler is a path");
    }

    /// A decorated definition's span must include its decorators, so a decorator
    /// change (a route path, an auth guard) moves the symbol's fingerprint and
    /// re-opens the proofs grounded in it. Before this the span began at `def`.
    #[test]
    fn python_decorated_span_covers_the_decorators() {
        let src = "\
@app.route(\"/health\")
@requires_auth
def health():
    return \"ok\"
";
        let (symbols, _, _) = extract(Language::Python, src);
        let f = symbols.iter().find(|s| s.name == "health").unwrap();
        // Line 1 is the first decorator; the span must start there, not at `def`.
        assert_eq!(f.line_start, 1, "span starts at the first decorator");
    }

    /// A single bare-parameter arrow (`id => …`, no parens) still declares one
    /// argument; the grammar exposes it on a `parameter` field, not a
    /// parenthesized list, so it used to read as zero args.
    #[test]
    fn typescript_bare_parameter_arrow_counts_one_arg() {
        let src = "export const idf = id => id;\n";
        let (symbols, _, _) = extract(Language::TypeScript, src);
        let f = symbols.iter().find(|s| s.name == "idf").expect("idf");
        assert_eq!(f.kind, "function");
        assert_eq!(f.arg_count, 1, "one bare parameter is one arg");
    }

    /// A destructuring binding names several things at once; its `name` node is
    /// an object/array pattern, not an identifier. Recording the pattern text
    /// ("{ a, b }") as one symbol is a phantom — such bindings are skipped.
    #[test]
    fn typescript_destructuring_binding_is_not_a_phantom_symbol() {
        let src = "const { createClient, other } = require('x');\n";
        let (symbols, _, _) = extract(Language::TypeScript, src);
        assert!(
            symbols.iter().all(|s| !s.name.contains('{')),
            "no symbol should be named after a destructuring pattern: {:?}",
            symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }
}
