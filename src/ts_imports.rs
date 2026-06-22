//! Tree-sitter backed import-specifier extraction.
//!
//! This module deliberately returns syntax-level specifiers only. `repo.rs`
//! owns path resolution and the conservative "existing repo file only" filter.

use std::path::Path;

use tree_sitter::{Language, Node, Parser, Tree};

use crate::types::{StringLiteralFact, SymbolFact};
use crate::vec_utils::push_unique_nonempty;

#[derive(Debug, Default)]
pub struct PhysicalFacts {
    pub import_specifiers: Vec<String>,
    pub symbols: Vec<String>,
    pub symbol_facts: Vec<SymbolFact>,
}

pub fn extract_physical_facts(rel_path: &str, content: &str) -> Option<PhysicalFacts> {
    let ext = Path::new(rel_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let lang = match ext {
        "rs" => Lang::Rust,
        "ts" => Lang::TypeScript,
        "tsx" => Lang::Tsx,
        "js" | "jsx" | "mjs" => Lang::JavaScript,
        "py" => Lang::Python,
        _ => return None,
    };
    let tree = parse(lang.language(), content)?;
    let mut facts = PhysicalFacts::default();
    match lang {
        Lang::Rust => rust_specs(tree.root_node(), content, &mut facts.import_specifiers),
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => {
            js_ts_specs(tree.root_node(), content, &mut facts.import_specifiers)
        }
        Lang::Python => python_specs(tree.root_node(), content, &mut facts.import_specifiers),
    }
    collect_symbol_facts(
        tree.root_node(),
        lang,
        rel_path,
        content,
        &mut facts.symbol_facts,
    );
    facts.symbols = facts
        .symbol_facts
        .iter()
        .map(|fact| fact.label.clone())
        .collect();
    facts.import_specifiers.sort();
    facts.import_specifiers.dedup();
    facts.symbols.sort();
    facts.symbols.dedup();
    facts.symbol_facts.sort_by(|a, b| {
        a.label
            .cmp(&b.label)
            .then_with(|| a.line_start.cmp(&b.line_start))
            .then_with(|| a.line_end.cmp(&b.line_end))
    });
    facts.symbol_facts.dedup_by(|a, b| a.label == b.label);
    // Per-symbol body hash — the signal `loom sync` diffs to flip only the
    // edges whose symbol actually changed. Hash the symbol's source LINES;
    // sync matches symbols by label, so moving an unedited symbol (line shift)
    // keeps the same hash and correctly reads as unchanged.
    let lines: Vec<&str> = content.lines().collect();
    for fact in &mut facts.symbol_facts {
        let lo = fact.line_start.saturating_sub(1);
        let hi = fact.line_end.min(lines.len());
        let body = if lo < hi {
            lines[lo..hi].join("\n")
        } else {
            String::new()
        };
        fact.body_hash = crate::repo::content_hash(body.as_bytes());
    }
    Some(facts)
}

#[derive(Clone, Copy)]
enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
}

impl Lang {
    fn language(self) -> Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
        }
    }
}

fn parse(language: Language, content: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    parser.parse(content, None)
}

fn rust_specs(node: Node<'_>, content: &str, out: &mut Vec<String>) {
    match node.kind() {
        "use_declaration" => {
            if let Some(arg) = node.child_by_field_name("argument") {
                collect_rust_use(arg, content, &[], out);
            }
        }
        "mod_item" if node.child_by_field_name("body").is_none() => {
            if let Some(name) = node
                .child_by_field_name("name")
                .and_then(|n| text(n, content))
            {
                push_unique_nonempty(out, format!("mod:{name}"));
            }
        }
        _ => {}
    }
    visit_children(node, content, out, rust_specs);
}

fn collect_rust_use(node: Node<'_>, content: &str, prefix: &[String], out: &mut Vec<String>) {
    match node.kind() {
        "scoped_use_list" => {
            let mut scoped = prefix.to_vec();
            if let Some(path) = node.child_by_field_name("path") {
                scoped.extend(rust_path_segments(path, content));
            }
            if let Some(list) = node.child_by_field_name("list") {
                collect_rust_use(list, content, &scoped, out);
            }
        }
        "use_list" => {
            for_each_named_child(node, |child| collect_rust_use(child, content, prefix, out));
        }
        "use_as_clause" => {
            if let Some(path) = node.child_by_field_name("path") {
                let mut scoped = prefix.to_vec();
                scoped.extend(rust_path_segments(path, content));
                push_rust_path(out, scoped);
            }
        }
        "use_wildcard" => {
            let mut scoped = prefix.to_vec();
            for_each_named_child(node, |child| {
                scoped.extend(rust_path_segments(child, content))
            });
            push_rust_path(out, scoped);
        }
        "self" => push_rust_path(out, prefix.to_vec()),
        "crate" | "identifier" | "metavariable" | "super" | "scoped_identifier" => {
            let mut scoped = prefix.to_vec();
            scoped.extend(rust_path_segments(node, content));
            push_rust_path(out, scoped);
        }
        _ => {
            let mut scoped = prefix.to_vec();
            scoped.extend(rust_path_segments(node, content));
            push_rust_path(out, scoped);
        }
    }
}

fn rust_path_segments(node: Node<'_>, content: &str) -> Vec<String> {
    match node.kind() {
        "crate" | "identifier" | "metavariable" | "self" | "super" => text(node, content)
            .map(|s| vec![s.to_string()])
            .unwrap_or_default(),
        "scoped_identifier" => {
            let mut segs = node
                .child_by_field_name("path")
                .map(|n| rust_path_segments(n, content))
                .unwrap_or_default();
            if let Some(name) = node.child_by_field_name("name") {
                segs.extend(rust_path_segments(name, content));
            }
            segs
        }
        "use_as_clause" => node
            .child_by_field_name("path")
            .map(|n| rust_path_segments(n, content))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn push_rust_path(out: &mut Vec<String>, segs: Vec<String>) {
    if !segs.is_empty() {
        push_unique_nonempty(out, segs.join("::"));
    }
}

fn js_ts_specs(node: Node<'_>, content: &str, out: &mut Vec<String>) {
    match node.kind() {
        "import_statement" | "export_statement" => {
            if let Some(source) = node
                .child_by_field_name("source")
                .and_then(|n| string_value(n, content))
            {
                push_unique_nonempty(out, source);
            }
        }
        "call_expression" => {
            let function = node
                .child_by_field_name("function")
                .and_then(|n| text(n, content))
                .unwrap_or("");
            if matches!(function, "require" | "import") {
                if let Some(args) = node.child_by_field_name("arguments") {
                    if let Some(source) = first_string_descendant(args, content) {
                        push_unique_nonempty(out, source);
                    }
                }
            }
        }
        _ => {}
    }
    visit_children(node, content, out, js_ts_specs);
}

fn python_specs(node: Node<'_>, content: &str, out: &mut Vec<String>) {
    match node.kind() {
        "import_statement" => {
            collect_descendant_text(node, content, "dotted_name", out);
        }
        "import_from_statement" => {
            if let Some(module) = node
                .child_by_field_name("module_name")
                .and_then(|n| text(n, content))
            {
                push_unique_nonempty(out, module.to_string());
                if module.chars().all(|c| c == '.') {
                    let mut names = Vec::new();
                    collect_descendant_text(node, content, "dotted_name", &mut names);
                    for name in names {
                        push_unique_nonempty(out, format!("{module}{name}"));
                    }
                }
            }
        }
        _ => {}
    }
    visit_children(node, content, out, python_specs);
}

fn collect_symbol_facts(
    root: Node<'_>,
    lang: Lang,
    rel_path: &str,
    content: &str,
    out: &mut Vec<SymbolFact>,
) {
    collect_symbol_facts_in(root, lang, rel_path, content, false, out);
}

fn collect_symbol_facts_in(
    root: Node<'_>,
    lang: Lang,
    rel_path: &str,
    content: &str,
    in_test_context: bool,
    out: &mut Vec<SymbolFact>,
) {
    for_each_named_child(root, |child| {
        collect_top_level_symbol(child, lang, rel_path, content, false, in_test_context, out)
    });
}

fn collect_top_level_symbol(
    node: Node<'_>,
    lang: Lang,
    rel_path: &str,
    content: &str,
    exported: bool,
    in_test_context: bool,
    out: &mut Vec<SymbolFact>,
) {
    match node.kind() {
        "declaration_list" => {
            collect_symbol_facts_in(node, lang, rel_path, content, in_test_context, out)
        }
        "mod_item" if matches!(lang, Lang::Rust) => {
            if let Some(body) = node.child_by_field_name("body") {
                let child_test_context = in_test_context || rust_symbol_is_test(node, content);
                collect_symbol_facts_in(body, lang, rel_path, content, child_test_context, out);
            }
        }
        "export_statement" if matches!(lang, Lang::TypeScript | Lang::Tsx | Lang::JavaScript) => {
            for_each_named_child(node, |child| {
                collect_top_level_symbol(child, lang, rel_path, content, true, in_test_context, out)
            });
        }
        "lexical_declaration" | "variable_declaration"
            if matches!(lang, Lang::TypeScript | Lang::Tsx | Lang::JavaScript) =>
        {
            collect_js_ts_binding_facts(node, rel_path, content, exported, in_test_context, out);
        }
        "impl_item" if matches!(lang, Lang::Rust) => {
            // The impl is one fact spanning the whole block, but its methods
            // need their OWN facts: a method-level locator (`file:method`) must
            // resolve to a symbol whose body hash moves when the method body
            // changes. Without per-method facts, a method edit only flipped the
            // IMPL's hash — whose changed name is the *type*, not the method —
            // so `loom sync` silently false-greened the method's grounding.
            let impl_test = in_test_context || rust_symbol_is_test(node, content);
            if let Some(mut symbol) =
                symbol_fact(node, lang, rel_path, content, exported, in_test_context)
            {
                let qualifier = symbol.name.clone();
                // Re-attribute the impl container fact's OWN physical facts: a
                // string literal or panic marker inside a method body belongs to
                // that method's fact, not the impl. `symbol_fact` collected over
                // the whole impl span (methods included), so counting both
                // double-fires `string_contract_duplicate` / `panic_marker_risk`
                // on a literal/marker that occurs exactly once in source. Recompute
                // excluding method subtrees — impl-direct facts (associated consts,
                // etc.) are kept.
                symbol.string_literals = string_literal_facts_excluding_methods(node, content);
                let impl_hits = panic_marker_hits_excluding_methods(node, content);
                symbol.panic_marker_count = impl_hits.len();
                symbol.panic_markers = panic_markers_from_hits(&impl_hits);
                push_unique_fact(out, symbol);
                if let Some(body) = node.child_by_field_name("body") {
                    for_each_named_child(body, |m| {
                        if m.kind() == "function_item" {
                            if let Some(fact) =
                                rust_method_fact(m, rel_path, content, &qualifier, impl_test)
                            {
                                push_unique_fact(out, fact);
                            }
                        }
                    });
                }
            }
        }
        _ => {
            if let Some(symbol) =
                symbol_fact(node, lang, rel_path, content, exported, in_test_context)
            {
                push_unique_fact(out, symbol);
            }
        }
    }
}

fn symbol_fact(
    node: Node<'_>,
    lang: Lang,
    rel_path: &str,
    content: &str,
    exported: bool,
    in_test_context: bool,
) -> Option<SymbolFact> {
    match lang {
        Lang::Rust => rust_symbol_fact(node, rel_path, content, in_test_context),
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => {
            js_ts_symbol_fact(node, rel_path, content, exported, in_test_context)
        }
        Lang::Python => python_symbol_fact(node, rel_path, content, in_test_context),
    }
}

fn rust_symbol_fact(
    node: Node<'_>,
    rel_path: &str,
    content: &str,
    in_test_context: bool,
) -> Option<SymbolFact> {
    let name = || {
        node.child_by_field_name("name")
            .and_then(|n| text(n, content))
    };
    let (kind, name, base_label) = match node.kind() {
        "function_item" => {
            let name = name()?.to_string();
            ("fn", name.clone(), format!("fn {name}"))
        }
        "struct_item" => {
            let name = name()?.to_string();
            ("struct", name.clone(), format!("struct {name}"))
        }
        "enum_item" => {
            let name = name()?.to_string();
            ("enum", name.clone(), format!("enum {name}"))
        }
        "trait_item" => {
            let name = name()?.to_string();
            ("trait", name.clone(), format!("trait {name}"))
        }
        "type_item" => {
            let name = name()?.to_string();
            ("type", name.clone(), format!("type {name}"))
        }
        "const_item" => {
            let name = name()?.to_string();
            ("const", name.clone(), format!("const {name}"))
        }
        "static_item" => {
            let name = name()?.to_string();
            ("static", name.clone(), format!("static {name}"))
        }
        "macro_definition" => {
            let name = name()?.to_string();
            ("macro", name.clone(), format!("macro {name}"))
        }
        "impl_item" => {
            let typ = normalize_ws(text(node.child_by_field_name("type")?, content)?);
            let name = if let Some(trait_node) = node.child_by_field_name("trait") {
                let trait_name = normalize_ws(text(trait_node, content)?);
                format!("{trait_name} for {typ}")
            } else {
                typ
            };
            ("impl", name.clone(), format!("impl {name}"))
        }
        _ => return None,
    };
    let visibility_prefix = rust_visibility_prefix(node, content);
    let label = visibility_prefix
        .as_ref()
        .map(|v| format!("{v} {base_label}"))
        .unwrap_or(base_label);
    Some(SymbolFact {
        label,
        name,
        kind: kind.into(),
        visibility: if visibility_prefix.is_some() {
            "public".into()
        } else {
            "private".into()
        },
        line_start: node.start_position().row + 1,
        line_end: node.end_position().row + 1,
        is_test: path_is_test(rel_path) || in_test_context || rust_symbol_is_test(node, content),
        string_literals: string_literal_facts(node, content),
        panic_marker_count: panic_marker_count(node, content),
        panic_markers: panic_markers(node, content),
        body_hash: String::new(),
        shape_hash: shape_hash(node, content),
    })
}

/// A method inside an `impl` block. The label is qualified by the impl's
/// (trait-aware) type name so two impls' same-named methods (`Display::fmt`
/// vs `Debug::fmt`) get distinct, non-colliding facts; `name` stays the bare
/// method name so a `file:method` locator still resolves by identifier word.
fn rust_method_fact(
    node: Node<'_>,
    rel_path: &str,
    content: &str,
    qualifier: &str,
    in_test_context: bool,
) -> Option<SymbolFact> {
    let name = node
        .child_by_field_name("name")
        .and_then(|n| text(n, content))?
        .to_string();
    let visibility_prefix = rust_visibility_prefix(node, content);
    let base_label = format!("fn {qualifier}::{name}");
    let label = visibility_prefix
        .as_ref()
        .map(|v| format!("{v} {base_label}"))
        .unwrap_or(base_label);
    Some(SymbolFact {
        label,
        name,
        kind: "fn".into(),
        visibility: if visibility_prefix.is_some() {
            "public".into()
        } else {
            "private".into()
        },
        line_start: node.start_position().row + 1,
        line_end: node.end_position().row + 1,
        is_test: path_is_test(rel_path) || in_test_context || rust_symbol_is_test(node, content),
        string_literals: string_literal_facts(node, content),
        panic_marker_count: panic_marker_count(node, content),
        panic_markers: panic_markers(node, content),
        body_hash: String::new(),
        shape_hash: shape_hash(node, content),
    })
}

fn js_ts_symbol_fact(
    node: Node<'_>,
    rel_path: &str,
    content: &str,
    exported: bool,
    in_test_context: bool,
) -> Option<SymbolFact> {
    let name = || {
        node.child_by_field_name("name")
            .and_then(|n| text(n, content))
    };
    let kind = match node.kind() {
        "function_declaration" => "function",
        "class_declaration" => "class",
        "interface_declaration" => "interface",
        "type_alias_declaration" => "type",
        "enum_declaration" => "enum",
        _ => return None,
    };
    let name = name()?.to_string();
    let base_label = format!("{kind} {name}");
    let is_test = path_is_test(rel_path) || in_test_context || js_ts_name_is_test(&name);
    Some(SymbolFact {
        label: if exported {
            format!("export {base_label}")
        } else {
            base_label
        },
        name,
        kind: kind.into(),
        visibility: if exported {
            "public".into()
        } else {
            "private".into()
        },
        line_start: node.start_position().row + 1,
        line_end: node.end_position().row + 1,
        is_test,
        string_literals: string_literal_facts(node, content),
        panic_marker_count: panic_marker_count(node, content),
        panic_markers: panic_markers(node, content),
        body_hash: String::new(),
        shape_hash: shape_hash(node, content),
    })
}

fn collect_js_ts_binding_facts(
    node: Node<'_>,
    rel_path: &str,
    content: &str,
    exported: bool,
    in_test_context: bool,
    out: &mut Vec<SymbolFact>,
) {
    let Some(raw) = text(node, content).map(str::trim_start) else {
        return;
    };
    // `const`/`let` (lexical_declaration) and `var` (variable_declaration) all
    // bind a top-level name; an arrow or function expression assigned to any of
    // them is a real grounding target. Emitting only `const` left a `let`/`var`
    // export as an invisible symbol — a method-level locator on it could never
    // resolve, so `loom sync` silently false-greened when its body changed.
    let keyword = match raw.split_whitespace().next() {
        Some(kw @ ("const" | "let" | "var")) => kw,
        _ => return,
    };
    for_each_named_child(node, |child| {
        if child.kind() == "variable_declarator" {
            if let Some(name_node) = child.child_by_field_name("name") {
                if name_node.kind() == "identifier" {
                    if let Some(name) = text(name_node, content) {
                        let base_label = format!("{keyword} {name}");
                        push_unique_fact(
                            out,
                            SymbolFact {
                                label: if exported {
                                    format!("export {base_label}")
                                } else {
                                    base_label
                                },
                                name: name.into(),
                                kind: keyword.into(),
                                visibility: if exported {
                                    "public".into()
                                } else {
                                    "private".into()
                                },
                                line_start: child.start_position().row + 1,
                                line_end: child.end_position().row + 1,
                                is_test: path_is_test(rel_path)
                                    || in_test_context
                                    || name == "test"
                                    || name.starts_with("test_"),
                                string_literals: string_literal_facts(child, content),
                                panic_marker_count: panic_marker_count(child, content),
                                panic_markers: panic_markers(child, content),
                                body_hash: String::new(),
                                shape_hash: shape_hash(child, content),
                            },
                        );
                    }
                }
            }
        }
    });
}

fn python_symbol_fact(
    node: Node<'_>,
    rel_path: &str,
    content: &str,
    in_test_context: bool,
) -> Option<SymbolFact> {
    let name = || {
        node.child_by_field_name("name")
            .and_then(|n| text(n, content))
    };
    let kind = match node.kind() {
        "function_definition" => "def",
        "class_definition" => "class",
        _ => return None,
    };
    let name = name()?.to_string();
    Some(SymbolFact {
        label: format!("{kind} {name}"),
        visibility: if name.starts_with('_') {
            "private".into()
        } else {
            "public".into()
        },
        is_test: path_is_test(rel_path) || in_test_context || name.starts_with("test_"),
        line_start: node.start_position().row + 1,
        line_end: node.end_position().row + 1,
        kind: kind.into(),
        name,
        string_literals: string_literal_facts(node, content),
        panic_marker_count: panic_marker_count(node, content),
        panic_markers: panic_markers(node, content),
        body_hash: String::new(),
        shape_hash: shape_hash(node, content),
    })
}

fn string_literal_facts(node: Node<'_>, content: &str) -> Vec<StringLiteralFact> {
    let mut out = Vec::new();
    collect_string_literal_facts(node, content, false, &mut out);
    out
}

/// Like `string_literal_facts` but does NOT descend into nested `function_item`
/// subtrees — used for a Rust `impl` container fact so a method body's literals
/// are owned ONLY by that method's own fact. Without this, a literal inside a
/// method is recorded on both the impl fact and the method fact, double-firing
/// `string_contract_duplicate` on a string that occurs exactly once in source.
fn string_literal_facts_excluding_methods(node: Node<'_>, content: &str) -> Vec<StringLiteralFact> {
    let mut out = Vec::new();
    collect_string_literal_facts(node, content, true, &mut out);
    out
}

fn collect_string_literal_facts(
    node: Node<'_>,
    content: &str,
    skip_methods: bool,
    out: &mut Vec<StringLiteralFact>,
) {
    if is_source_string_literal_kind(node.kind()) {
        if let Some(value) = source_string_literal_value(node, content) {
            out.push(StringLiteralFact {
                value,
                line: node.start_position().row + 1,
            });
        }
        return;
    }
    for idx in 0..node.child_count() {
        if let Some(child) = node.child(idx as u32) {
            if skip_methods && child.kind() == "function_item" {
                continue;
            }
            collect_string_literal_facts(child, content, skip_methods, out);
        }
    }
}

fn is_source_string_literal_kind(kind: &str) -> bool {
    matches!(
        kind,
        "raw_string_literal" | "string" | "string_literal" | "template_string"
    )
}

fn source_string_literal_value(node: Node<'_>, content: &str) -> Option<String> {
    let raw = text(node, content)?.trim();
    let quote_idx = raw.find(['\'', '"', '`'])?;
    let quote = raw.as_bytes().get(quote_idx).copied()? as char;
    let prefix = &raw[..quote_idx];
    if quote == '`' && raw.contains("${") {
        return None;
    }
    if prefix.chars().any(|c| matches!(c, 'f' | 'F')) {
        return None;
    }
    if quote == '"' && prefix.starts_with('r') && prefix[1..].chars().all(|c| c == '#') {
        let hashes = prefix.len() - 1;
        let suffix = "#".repeat(hashes);
        if raw.ends_with(&suffix) {
            let end = raw.len().checked_sub(hashes + 1)?;
            if end > quote_idx + 1 {
                return Some(raw[quote_idx + 1..end].to_string());
            }
        }
    }
    if raw[quote_idx..].starts_with(&quote.to_string().repeat(3))
        && raw.ends_with(&quote.to_string().repeat(3))
        && raw.len() >= quote_idx + 6
    {
        return Some(raw[quote_idx + 3..raw.len() - 3].to_string());
    }
    if raw.as_bytes().last().copied()? as char != quote || raw.len() <= quote_idx + 1 {
        return None;
    }
    Some(raw[quote_idx + 1..raw.len() - 1].to_string())
}

fn panic_marker_count(node: Node<'_>, content: &str) -> usize {
    panic_marker_hits(node, content).len()
}

fn panic_markers(node: Node<'_>, content: &str) -> Vec<String> {
    panic_markers_from_hits(&panic_marker_hits(node, content))
}

fn panic_markers_from_hits(hits: &[&'static str]) -> Vec<String> {
    let mut out = Vec::new();
    for marker in ["panic", "unwrap", "expect", "todo", "unimplemented"] {
        if hits.contains(&marker) {
            out.push(marker.to_string());
        }
    }
    out
}

fn panic_marker_hits(node: Node<'_>, content: &str) -> Vec<&'static str> {
    panic_marker_hits_inner(node, content, false)
}

/// Like `panic_marker_hits` but skips nested `function_item` subtrees — used for
/// a Rust `impl` container fact so a method's `unwrap`/`expect`/`panic` is
/// counted ONLY on that method's fact, not double-counted on the impl too
/// (which would emit a second, phantom `panic_marker_risk` finding).
fn panic_marker_hits_excluding_methods(node: Node<'_>, content: &str) -> Vec<&'static str> {
    panic_marker_hits_inner(node, content, true)
}

fn panic_marker_hits_inner(node: Node<'_>, content: &str, skip_methods: bool) -> Vec<&'static str> {
    let mut tokens = Vec::new();
    source_tokens_without_literals(node, content, skip_methods, &mut tokens);
    let mut hits = Vec::new();
    for (idx, tok) in tokens.iter().enumerate() {
        let Some(marker) = panic_marker(tok.as_str()) else {
            continue;
        };
        if previous_token_is_definition(&tokens, idx) {
            continue;
        }
        if next_token_is_call_or_macro(&tokens, idx) {
            hits.push(marker);
        }
    }
    hits
}

fn source_tokens_without_literals(
    node: Node<'_>,
    content: &str,
    skip_methods: bool,
    out: &mut Vec<String>,
) {
    let kind = node.kind();
    if is_comment_kind(kind) || is_literal_kind(kind) {
        return;
    }
    if node.child_count() == 0 {
        if let Some(raw) = text(node, content).map(str::trim).filter(|s| !s.is_empty()) {
            out.push(raw.to_string());
        }
        return;
    }
    for idx in 0..node.child_count() {
        if let Some(child) = node.child(idx as u32) {
            if skip_methods && child.kind() == "function_item" {
                continue;
            }
            source_tokens_without_literals(child, content, skip_methods, out);
        }
    }
}

fn panic_marker(tok: &str) -> Option<&'static str> {
    match tok {
        "panic" => Some("panic"),
        "unwrap" => Some("unwrap"),
        "expect" => Some("expect"),
        "todo" => Some("todo"),
        "unimplemented" => Some("unimplemented"),
        _ => None,
    }
}

fn previous_token_is_definition(tokens: &[String], idx: usize) -> bool {
    idx > 0 && matches!(tokens[idx - 1].as_str(), "fn" | "def" | "function")
}

fn next_token_is_call_or_macro(tokens: &[String], idx: usize) -> bool {
    matches!(
        tokens.get(idx + 1).map(String::as_str),
        Some("(") | Some("!")
    )
}

fn shape_hash(node: Node<'_>, content: &str) -> String {
    let mut tokens = Vec::new();
    normalized_shape_tokens(node, content, &mut tokens);
    if tokens.is_empty() {
        String::new()
    } else {
        crate::repo::content_hash(tokens.join(" ").as_bytes())
    }
}

fn normalized_shape_tokens(node: Node<'_>, content: &str, out: &mut Vec<&'static str>) {
    let kind = node.kind();
    if is_comment_kind(kind) {
        return;
    }
    if is_identifier_kind(kind) {
        out.push("ID");
        return;
    }
    if is_literal_kind(kind) {
        out.push("LIT");
        return;
    }
    if node.child_count() == 0 {
        if text(node, content).map(str::trim).unwrap_or("").is_empty() {
            return;
        }
        out.push(kind);
        return;
    }
    for idx in 0..node.child_count() {
        if let Some(child) = node.child(idx as u32) {
            normalized_shape_tokens(child, content, out);
        }
    }
}

fn is_comment_kind(kind: &str) -> bool {
    kind.contains("comment")
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "field_identifier"
            | "property_identifier"
            | "shorthand_property_identifier"
            | "shorthand_property_identifier_pattern"
            | "type_identifier"
            | "scoped_identifier"
            | "namespace_identifier"
            | "statement_identifier"
            | "module_identifier"
    ) || kind.ends_with("_identifier")
}

fn is_literal_kind(kind: &str) -> bool {
    matches!(
        kind,
        "char_literal"
            | "false"
            | "float_literal"
            | "integer_literal"
            | "negative_literal"
            | "none"
            | "null"
            | "raw_string_literal"
            | "string"
            | "string_content"
            | "string_literal"
            | "template_string"
            | "true"
    ) || kind.ends_with("_literal")
}

fn rust_visibility_prefix(node: Node<'_>, content: &str) -> Option<String> {
    for idx in 0..node.child_count() {
        let Some(child) = node.child(idx as u32) else {
            continue;
        };
        if child.kind() == "visibility_modifier" {
            return text(child, content).map(str::to_string);
        }
    }
    let raw = text(node, content)?.trim_start();
    raw.strip_prefix("pub ")
        .map(|_| "pub".to_string())
        .or_else(|| {
            raw.strip_prefix("pub(").map(|_| {
                raw.split_whitespace()
                    .next()
                    .unwrap_or("pub")
                    .trim_end_matches('{')
                    .to_string()
            })
        })
}

fn rust_symbol_is_test(node: Node<'_>, content: &str) -> bool {
    // Scope test-classification to the item's OWN preceding attributes, never
    // its subtree text. The previous first clause scanned `text(node, content)`
    // — the ENTIRE subtree — for `#[cfg(test`/`#[test`, so any production impl
    // containing one nested `#[cfg(test)]` helper was tagged test and made
    // invisible to the size/panic detectors. That hid loom's largest behavioral
    // unit: the 4137-line `impl SqliteGraphStore` is exempt because its body
    // contains `#[cfg(test)] pub fn in_memory()`.
    //
    // A test item still classifies itself: a `#[test] fn`/`#[cfg(test)] mod`
    // carries its attribute on the line(s) immediately above the keyword, which
    // `preceding_attribute_lines` collects. Individual test fns are never lost
    // (each owns its `#[test]`); only the false container-tagging goes away.
    preceding_attribute_lines(content, node.start_position().row)
        .iter()
        .any(|line| rust_attr_text_marks_test(line))
}

fn rust_attr_text_marks_test(raw: &str) -> bool {
    raw.contains("#[test")
        || raw.contains("::test]")
        || raw.contains("#[cfg(test")
        || raw.contains("#[cfg_attr(test")
}

fn preceding_attribute_lines(content: &str, row: usize) -> Vec<&str> {
    let lines: Vec<&str> = content.lines().collect();
    let mut out = Vec::new();
    let mut idx = row;
    while idx > 0 {
        idx -= 1;
        let line = lines.get(idx).copied().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("#[") {
            out.push(line);
            continue;
        }
        break;
    }
    out
}

fn path_is_test(rel_path: &str) -> bool {
    let p = rel_path.replace('\\', "/");
    p.contains("/tests/")
        || p.starts_with("tests/")
        || p.contains(".test.")
        || p.contains(".spec.")
        || p.ends_with("_test.py")
}

fn js_ts_name_is_test(name: &str) -> bool {
    // Test declarations are name-anchored, not body-substring: `it(` inside
    // a function body (commit(, init(, exit(, …) must not classify the
    // enclosing function as a test.
    name == "it"
        || name == "test"
        || name == "describe"
        || name.starts_with("it_")
        || name.starts_with("test_")
        || name.starts_with("describe_")
}

fn visit_children(
    node: Node<'_>,
    content: &str,
    out: &mut Vec<String>,
    visit: fn(Node<'_>, &str, &mut Vec<String>),
) {
    for_each_named_child(node, |child| visit(child, content, out));
}

fn for_each_named_child(node: Node<'_>, mut f: impl FnMut(Node<'_>)) {
    for idx in 0..node.named_child_count() {
        if let Some(child) = node.named_child(idx as u32) {
            f(child);
        }
    }
}

fn collect_descendant_text(node: Node<'_>, content: &str, kind: &str, out: &mut Vec<String>) {
    if node.kind() == kind {
        if let Some(s) = text(node, content) {
            push_unique_nonempty(out, s.to_string());
        }
    }
    for_each_named_child(node, |child| {
        collect_descendant_text(child, content, kind, out)
    });
}

fn first_string_descendant(node: Node<'_>, content: &str) -> Option<String> {
    if matches!(node.kind(), "string" | "template_string") {
        return string_value(node, content);
    }
    for idx in 0..node.named_child_count() {
        let child = node.named_child(idx as u32)?;
        if let Some(value) = first_string_descendant(child, content) {
            return Some(value);
        }
    }
    None
}

fn text<'a>(node: Node<'_>, content: &'a str) -> Option<&'a str> {
    node.utf8_text(content.as_bytes()).ok()
}

fn string_value(node: Node<'_>, content: &str) -> Option<String> {
    let raw = text(node, content)?.trim();
    let first = raw.as_bytes().first().copied()? as char;
    let last = raw.as_bytes().last().copied()? as char;
    if !matches!(first, '\'' | '"' | '`') || first != last {
        return None;
    }
    if first == '`' && raw.contains("${") {
        return None;
    }
    if raw.len() < 2 {
        return None;
    }
    Some(raw[1..raw.len() - 1].to_string())
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn push_unique_fact(out: &mut Vec<SymbolFact>, item: SymbolFact) {
    if !item.label.is_empty() && !out.iter().any(|existing| existing.label == item.label) {
        out.push(item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_value_single_quote_returns_none() {
        let content = "\"";
        let tree = parse(Lang::JavaScript.language(), content).unwrap();

        assert_eq!(string_value(tree.root_node(), content), None);
    }

    #[test]
    fn impl_methods_get_their_own_facts() {
        let content = "\
struct Repo;
impl Repo {
    pub fn save(&self) -> i32 {
        let x = 1;
        x + 1
    }
}
impl std::fmt::Display for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, \"r\")
    }
}
impl std::fmt::Debug for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, \"d\")
    }
}
";
        let facts = extract_physical_facts("src/repo.rs", content).unwrap();
        let labels: Vec<&str> = facts
            .symbol_facts
            .iter()
            .map(|f| f.label.as_str())
            .collect();
        // The method is its own fact, qualified by the impl type.
        assert!(labels.contains(&"pub fn Repo::save"), "{labels:?}");
        // Two impls' same-named `fmt` must NOT collide into one fact — distinct
        // qualifiers keep both, so a change to one is attributable.
        assert!(
            labels.iter().any(|l| l.contains("Display for Repo::fmt")),
            "{labels:?}"
        );
        assert!(
            labels.iter().any(|l| l.contains("Debug for Repo::fmt")),
            "{labels:?}"
        );
        // The bare method name is preserved so a `file:save` locator resolves.
        let save = facts
            .symbol_facts
            .iter()
            .find(|f| f.label == "pub fn Repo::save")
            .unwrap();
        assert_eq!(save.name, "save");
    }

    #[test]
    fn top_level_let_and_var_bindings_are_extracted() {
        // Only `const` was emitted before, so a `let`/`var` arrow export was an
        // invisible (un-syncable) grounding target.
        let content = "export const a = () => 1;\n\
                       export let b = () => 2;\n\
                       var c = function () { return 3; };\n";
        let facts = extract_physical_facts("app.js", content).unwrap();
        let labels: Vec<&str> = facts
            .symbol_facts
            .iter()
            .map(|f| f.label.as_str())
            .collect();
        assert!(labels.contains(&"export const a"), "{labels:?}");
        assert!(labels.contains(&"export let b"), "{labels:?}");
        assert!(labels.contains(&"var c"), "{labels:?}");
    }

    // FALSE-GREEN [is-test-subtree-substring-misclassification]: a production
    // `impl` whose body contains one `#[cfg(test)]` helper must NOT be tagged
    // test — that hid loom's largest behavioral unit (`impl SqliteGraphStore`,
    // 4137 lines) from the large_behavioral_symbol detector. is_test is scoped
    // to the item's OWN preceding attributes; the nested helper still tags
    // itself (it carries `#[cfg(test)]` on its own preceding line).
    fn first_node_of_kind<'a>(root: Node<'a>, kind: &str) -> Option<Node<'a>> {
        let mut q = std::collections::VecDeque::new();
        q.push_back(root);
        while let Some(n) = q.pop_front() {
            if n.kind() == kind {
                return Some(n);
            }
            for i in 0..n.child_count() {
                if let Some(c) = n.child(i as u32) {
                    q.push_back(c);
                }
            }
        }
        None
    }

    fn fn_node_named<'a>(root: Node<'a>, content: &'a str, name: &str) -> Option<Node<'a>> {
        let mut q = std::collections::VecDeque::new();
        q.push_back(root);
        while let Some(n) = q.pop_front() {
            if n.kind() == "function_item" {
                let nm = n
                    .child_by_field_name("name")
                    .and_then(|c| text(c, content))
                    .unwrap_or("");
                if nm == name {
                    return Some(n);
                }
            }
            for i in 0..n.child_count() {
                if let Some(c) = n.child(i as u32) {
                    q.push_back(c);
                }
            }
        }
        None
    }

    #[test]
    fn rust_symbol_is_test_scoped_to_own_attributes_not_subtree() {
        // A production impl with a nested #[cfg(test)] helper, plus a real
        // #[test] fn and a plain production fn. Mirrors src/db/sqlite.rs.
        let src = "\
struct Foo;

impl Foo {
    pub fn production_method(&self) -> i32 {
        42
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self
    }
}

#[test]
fn real_test() {
    assert!(true);
}
";
        let tree = parse(Lang::Rust.language(), src).unwrap();
        let root = tree.root_node();

        // The bug: the whole `impl Foo` was tagged test because its subtree text
        // contains `#[cfg(test]`. It must read as production (false).
        let impl_node = first_node_of_kind(root, "impl_item").expect("impl_item present");
        assert!(
            !rust_symbol_is_test(impl_node, src),
            "a production impl with a nested #[cfg(test)] helper must NOT be tagged test"
        );

        // The nested helper still classifies itself (own preceding #[cfg(test)]).
        let in_memory = fn_node_named(root, src, "in_memory").expect("in_memory present");
        assert!(
            rust_symbol_is_test(in_memory, src),
            "the nested #[cfg(test)] helper must still tag itself test"
        );

        // A real #[test] fn is still detected via its own preceding attribute.
        let real_test = fn_node_named(root, src, "real_test").expect("real_test present");
        assert!(
            rust_symbol_is_test(real_test, src),
            "a #[test] fn is detected via its own preceding #[test] attribute"
        );

        // A plain production fn is not test.
        let prod =
            fn_node_named(root, src, "production_method").expect("production_method present");
        assert!(
            !rust_symbol_is_test(prod, src),
            "a plain production fn must not be tagged test"
        );
    }

    #[test]
    fn impl_container_fact_excludes_method_literals_and_markers() {
        // A string literal and a panic marker live ONCE in source, inside a
        // method of an impl. The method fact owns them; the impl CONTAINER fact
        // must NOT also claim them. Before this fix, `symbol_fact` collected over
        // the whole impl span, so the single occurrence was recorded on BOTH the
        // impl and the method — double-firing `string_contract_duplicate` and
        // `panic_marker_risk` on code that occurs exactly once.
        let src = "\
struct Foo;

impl Foo {
    pub fn method(&self) -> u8 {
        let _msg = \"a distinctive contract sentence that occurs exactly once here\";
        Some(1u8).unwrap()
    }
}
";
        let facts = extract_physical_facts("src/foo.rs", src).expect("rust facts extracted");
        let impl_fact = facts
            .symbol_facts
            .iter()
            .find(|f| f.kind == "impl")
            .expect("impl fact present");
        let method_fact = facts
            .symbol_facts
            .iter()
            .find(|f| f.kind == "fn" && f.name == "method")
            .expect("method fact present");

        // The method keeps its OWN literal and marker.
        assert!(
            method_fact
                .string_literals
                .iter()
                .any(|l| l.value.contains("distinctive contract sentence")),
            "the method fact must keep its own string literal"
        );
        assert_eq!(
            method_fact.panic_marker_count, 1,
            "the method fact must keep its own unwrap marker"
        );

        // The impl container must NOT double-count the method's literal/marker.
        assert!(
            !impl_fact
                .string_literals
                .iter()
                .any(|l| l.value.contains("distinctive contract sentence")),
            "the impl container must NOT re-claim the method's literal (double-count): {:?}",
            impl_fact.string_literals
        );
        assert_eq!(
            impl_fact.panic_marker_count, 0,
            "the impl container must NOT re-count the method's unwrap marker"
        );
    }

    #[test]
    fn impl_container_fact_keeps_its_own_direct_literals() {
        // A literal directly in the impl (an associated const), NOT inside a
        // method, must still be attributed to the impl fact — the method-exclusion
        // must not throw away impl-direct facts.
        let src = "\
struct Bar;

impl Bar {
    const LABEL: &'static str = \"an impl-direct associated const contract string\";

    pub fn method(&self) -> u8 {
        7
    }
}
";
        let facts = extract_physical_facts("src/bar.rs", src).expect("rust facts extracted");
        let impl_fact = facts
            .symbol_facts
            .iter()
            .find(|f| f.kind == "impl")
            .expect("impl fact present");
        assert!(
            impl_fact
                .string_literals
                .iter()
                .any(|l| l.value.contains("impl-direct associated const")),
            "an impl-direct (non-method) literal must remain on the impl fact: {:?}",
            impl_fact.string_literals
        );
    }
}
