//! Tree-sitter backed import-specifier extraction.
//!
//! This module deliberately returns syntax-level specifiers only. `repo.rs`
//! owns path resolution and the conservative "existing repo file only" filter.

use std::path::Path;

use tree_sitter::{Language, Node, Parser, Tree};

use crate::types::SymbolFact;

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
        "mod_item" => {
            if node.child_by_field_name("body").is_none() {
                if let Some(name) = node
                    .child_by_field_name("name")
                    .and_then(|n| text(n, content))
                {
                    push_unique(out, format!("mod:{name}"));
                }
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
        push_unique(out, segs.join("::"));
    }
}

fn js_ts_specs(node: Node<'_>, content: &str, out: &mut Vec<String>) {
    match node.kind() {
        "import_statement" | "export_statement" => {
            if let Some(source) = node
                .child_by_field_name("source")
                .and_then(|n| string_value(n, content))
            {
                push_unique(out, source);
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
                        push_unique(out, source);
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
                push_unique(out, module.to_string());
                if module.chars().all(|c| c == '.') {
                    let mut names = Vec::new();
                    collect_descendant_text(node, content, "dotted_name", &mut names);
                    for name in names {
                        push_unique(out, format!("{module}{name}"));
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
    for_each_named_child(root, |child| {
        collect_top_level_symbol(child, lang, rel_path, content, false, out)
    });
}

fn collect_top_level_symbol(
    node: Node<'_>,
    lang: Lang,
    rel_path: &str,
    content: &str,
    exported: bool,
    out: &mut Vec<SymbolFact>,
) {
    match node.kind() {
        "declaration_list" => collect_symbol_facts(node, lang, rel_path, content, out),
        "mod_item" if matches!(lang, Lang::Rust) => {
            if let Some(body) = node.child_by_field_name("body") {
                collect_symbol_facts(body, lang, rel_path, content, out);
            }
        }
        "export_statement" if matches!(lang, Lang::TypeScript | Lang::Tsx | Lang::JavaScript) => {
            for_each_named_child(node, |child| {
                collect_top_level_symbol(child, lang, rel_path, content, true, out)
            });
        }
        "lexical_declaration"
            if matches!(lang, Lang::TypeScript | Lang::Tsx | Lang::JavaScript) =>
        {
            collect_js_ts_const_facts(node, rel_path, content, exported, out);
        }
        _ => {
            if let Some(symbol) = symbol_fact(node, lang, rel_path, content, exported) {
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
) -> Option<SymbolFact> {
    match lang {
        Lang::Rust => rust_symbol_fact(node, rel_path, content),
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => {
            js_ts_symbol_fact(node, rel_path, content, exported)
        }
        Lang::Python => python_symbol_fact(node, rel_path, content),
    }
}

fn rust_symbol_fact(node: Node<'_>, rel_path: &str, content: &str) -> Option<SymbolFact> {
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
        is_test: path_is_test(rel_path) || rust_symbol_is_test(node, content),
        body_hash: String::new(),
    })
}

fn js_ts_symbol_fact(
    node: Node<'_>,
    rel_path: &str,
    content: &str,
    exported: bool,
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
        is_test: path_is_test(rel_path) || js_ts_name_is_test(text(node, content).unwrap_or("")),
        body_hash: String::new(),
    })
}

fn collect_js_ts_const_facts(
    node: Node<'_>,
    rel_path: &str,
    content: &str,
    exported: bool,
    out: &mut Vec<SymbolFact>,
) {
    let Some(raw) = text(node, content).map(str::trim_start) else {
        return;
    };
    if !raw.starts_with("const ") {
        return;
    }
    for_each_named_child(node, |child| {
        if child.kind() == "variable_declarator" {
            if let Some(name_node) = child.child_by_field_name("name") {
                if name_node.kind() == "identifier" {
                    if let Some(name) = text(name_node, content) {
                        let base_label = format!("const {name}");
                        push_unique_fact(
                            out,
                            SymbolFact {
                                label: if exported {
                                    format!("export {base_label}")
                                } else {
                                    base_label
                                },
                                name: name.into(),
                                kind: "const".into(),
                                visibility: if exported {
                                    "public".into()
                                } else {
                                    "private".into()
                                },
                                line_start: child.start_position().row + 1,
                                line_end: child.end_position().row + 1,
                                is_test: path_is_test(rel_path) || name.starts_with("test"),
                                body_hash: String::new(),
                            },
                        );
                    }
                }
            }
        }
    });
}

fn python_symbol_fact(node: Node<'_>, rel_path: &str, content: &str) -> Option<SymbolFact> {
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
        is_test: path_is_test(rel_path) || name.starts_with("test_"),
        line_start: node.start_position().row + 1,
        line_end: node.end_position().row + 1,
        kind: kind.into(),
        name,
        body_hash: String::new(),
    })
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
    text(node, content).is_some_and(|raw| raw.contains("#[test") || raw.contains("::test]"))
        || preceding_attribute_lines(content, node.start_position().row)
            .iter()
            .any(|line| line.contains("#[test") || line.contains("::test]"))
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

fn js_ts_name_is_test(raw: &str) -> bool {
    raw.contains("describe(") || raw.contains("it(") || raw.contains("test(")
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
            push_unique(out, s.to_string());
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
    Some(raw[1..raw.len() - 1].to_string())
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn push_unique(out: &mut Vec<String>, item: String) {
    if !item.is_empty() && !out.contains(&item) {
        out.push(item);
    }
}

fn push_unique_fact(out: &mut Vec<SymbolFact>, item: SymbolFact) {
    if !item.label.is_empty() && !out.iter().any(|existing| existing.label == item.label) {
        out.push(item);
    }
}
