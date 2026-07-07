//! Shared per-symbol metric walk — complexity, nesting depth, argument count.
//!
//! Plane: structural (derived) — pure functions of the parse tree; no store access.
//!
//! One traversal grammar for every language: a [`MetricSpec`] names the node
//! kinds that branch, the kinds that begin a NESTED declaration (the walk stops
//! there — a nested callable gets its own `Symbol` and must not inflate its
//! parent), and the parameter-list shape. Pure functions of the parsed tree, so
//! the derived plane stays rebuildable (INV-2).
//!
//! Complexity is the cognitive proxy documented on `Symbol::complexity`:
//! 1 + branch/loop points plus one per `&&`/`||`; a `match`/`switch` counts
//! once, not per arm (wide-but-flat dispatch is not penalized), and `?` /
//! linear error propagation is not counted.

/// Per-language traversal vocabulary for [`measure`].
pub(super) struct MetricSpec {
    /// Kinds that add one branch point and one nesting level.
    pub branch_kinds: &'static [&'static str],
    /// Kinds whose subtree belongs to a DIFFERENT symbol (nested named
    /// callables/classes). Closures and lambdas are deliberately NOT
    /// boundaries — they carry the enclosing symbol's logic.
    pub boundary_kinds: &'static [&'static str],
    /// Kind of the parameter-list node (resolved via the `parameters` field).
    pub params_kind: &'static str,
    /// Named children of the parameter list counted as arguments. Receiver
    /// kinds (Rust `self_parameter`) are excluded by not being listed here.
    pub param_kinds: &'static [&'static str],
    /// A leading parameter whose first identifier matches one of these names
    /// is a receiver (`self`/`cls`/`this`) and is dropped from the count.
    pub receiver_names: &'static [&'static str],
}

/// The measured facts of one callable symbol.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SymbolMetrics {
    pub complexity: u32,
    pub max_nesting: u32,
    pub arg_count: u32,
}

pub(super) const RUST_METRICS: MetricSpec = MetricSpec {
    branch_kinds: &[
        "if_expression",
        "while_expression",
        "for_expression",
        "loop_expression",
        "match_expression",
    ],
    boundary_kinds: &["function_item"],
    params_kind: "parameters",
    param_kinds: &["parameter", "variadic_parameter"],
    receiver_names: &[],
};

pub(super) const PYTHON_METRICS: MetricSpec = MetricSpec {
    branch_kinds: &[
        "if_statement",
        "while_statement",
        "for_statement",
        "match_statement",
        "conditional_expression",
    ],
    boundary_kinds: &["function_definition", "class_definition"],
    params_kind: "parameters",
    param_kinds: &[
        "identifier",
        "typed_parameter",
        "default_parameter",
        "typed_default_parameter",
        "list_splat_pattern",
        "dictionary_splat_pattern",
    ],
    receiver_names: &["self", "cls"],
};

/// Go: one `parameter_declaration` may declare several names (`a, b int`);
/// counting declarations deliberately under-counts that shape — the detector
/// stays conservative rather than clever.
pub(super) const GO_METRICS: MetricSpec = MetricSpec {
    branch_kinds: &[
        "if_statement",
        "for_statement",
        "expression_switch_statement",
        "type_switch_statement",
        "select_statement",
    ],
    boundary_kinds: &["function_declaration", "method_declaration"],
    params_kind: "parameter_list",
    param_kinds: &["parameter_declaration", "variadic_parameter_declaration"],
    receiver_names: &[],
};

pub(super) const JS_METRICS: MetricSpec = MetricSpec {
    branch_kinds: &[
        "if_statement",
        "while_statement",
        "do_statement",
        "for_statement",
        "for_in_statement",
        "switch_statement",
        "ternary_expression",
    ],
    boundary_kinds: &[
        "function_declaration",
        "generator_function_declaration",
        "class_declaration",
        "method_definition",
    ],
    params_kind: "formal_parameters",
    param_kinds: &[
        "identifier",
        "assignment_pattern",
        "rest_pattern",
        "object_pattern",
        "array_pattern",
    ],
    receiver_names: &[],
};

pub(super) const TS_METRICS: MetricSpec = MetricSpec {
    branch_kinds: &[
        "if_statement",
        "while_statement",
        "do_statement",
        "for_statement",
        "for_in_statement",
        "switch_statement",
        "ternary_expression",
    ],
    boundary_kinds: &[
        "function_declaration",
        "generator_function_declaration",
        "class_declaration",
        "abstract_class_declaration",
        "interface_declaration",
        "method_definition",
    ],
    params_kind: "formal_parameters",
    param_kinds: &["required_parameter", "optional_parameter"],
    receiver_names: &["this"],
};

/// Measure one callable symbol node. The walk skips `boundary_kinds` subtrees
/// (other than the root itself) so nested declarations never leak into the
/// parent's numbers.
pub(super) fn measure(node: &tree_sitter::Node, bytes: &[u8], spec: &MetricSpec) -> SymbolMetrics {
    let mut m = SymbolMetrics {
        complexity: 1,
        max_nesting: 0,
        arg_count: arg_count(node, bytes, spec),
    };
    let mut stack: Vec<(tree_sitter::Node, u32)> = vec![(*node, 0)];
    while let Some((n, depth)) = stack.pop() {
        let mut child_depth = depth;
        if n.id() != node.id() {
            let kind = n.kind();
            if spec.boundary_kinds.contains(&kind) {
                continue;
            }
            if spec.branch_kinds.contains(&kind) {
                m.complexity += 1;
                child_depth = depth + 1;
                m.max_nesting = m.max_nesting.max(child_depth);
            } else {
                m.complexity += bool_ops(&n);
            }
        }
        for i in 0..n.child_count() as u32 {
            if let Some(c) = n.child(i) {
                stack.push((c, child_depth));
            }
        }
    }
    m
}

/// `&&`/`||` branch points on this node: one per operator token of a
/// `binary_expression` (Rust/Go/JS/TS), or one for a Python `boolean_operator`.
fn bool_ops(n: &tree_sitter::Node) -> u32 {
    match n.kind() {
        "binary_expression" => {
            let mut count = 0;
            let mut c = n.walk();
            for child in n.children(&mut c) {
                if matches!(child.kind(), "&&" | "||") {
                    count += 1;
                }
            }
            count
        }
        "boolean_operator" => 1,
        _ => 0,
    }
}

/// Declared argument count: named children of the `parameters` field (falling
/// back to the first child of the spec's params kind) that are parameter
/// nodes, minus a leading receiver.
fn arg_count(node: &tree_sitter::Node, bytes: &[u8], spec: &MetricSpec) -> u32 {
    let params = node
        .child_by_field_name("parameters")
        .or_else(|| named_child_of_kind(node, spec.params_kind));
    let Some(params) = params else { return 0 };
    let mut count = 0u32;
    let mut first = true;
    let mut c = params.walk();
    for child in params.named_children(&mut c) {
        if !spec.param_kinds.contains(&child.kind()) {
            continue;
        }
        let is_receiver = first && is_receiver_param(&child, bytes, spec.receiver_names);
        first = false;
        if !is_receiver {
            count += 1;
        }
    }
    count
}

/// First named child of the given kind, with the tree's lifetime (a plain loop
/// so the returned node never borrows the local cursor).
fn named_child_of_kind<'t>(
    node: &tree_sitter::Node<'t>,
    kind: &str,
) -> Option<tree_sitter::Node<'t>> {
    for i in 0..node.named_child_count() as u32 {
        if let Some(c) = node.named_child(i) {
            if c.kind() == kind {
                return Some(c);
            }
        }
    }
    None
}

/// Whether a parameter's first identifier names a receiver (`self`, `cls`,
/// `this`). Text-based so one rule covers `self`, `self: Type`, and
/// `this: Foo` across grammars.
fn is_receiver_param(node: &tree_sitter::Node, bytes: &[u8], receivers: &[&str]) -> bool {
    if receivers.is_empty() {
        return false;
    }
    let Ok(text) = node.utf8_text(bytes) else {
        return false;
    };
    let head: String = text
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    receivers.contains(&head.as_str())
}
