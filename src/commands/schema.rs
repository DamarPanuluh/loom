//! `loom schema` — emit loom's data model so a cold LLM can introspect exactly
//! what is valid. The structural part (labels, edge types, properties) is
//! generated from the single source of truth in `db::schema`, so it cannot drift
//! from what the queries actually read/write.

use anyhow::Result;

use crate::db::schema::{required_edge_props, required_node_props, EDGE_TYPES, NODE_LABELS, ROLES, SCHEMA_VERSION};
use crate::output::Printer;

/// One-line description of what each agent role is responsible for.
fn role_desc(role: &str) -> &'static str {
    match role {
        "builder"   => "Constructs the graph: intents, hierarchy, codefiles, implements.",
        "analyzer"  => "Grounds edges: criterion, evidence, confidence, inspection_status.",
        "fixer"     => "Resolves failing edges + needs_change intents (transitions status).",
        "validator" => "Proves it works: runs validations, confirms intents, VALIDATES verdict.",
        "quality"   => "The green gate: quality rules + GOVERNS verdicts.",
        _ => "",
    }
}

fn node_desc(label: &str) -> &'static str {
    match label {
        "Intent" => "What a piece of code is supposed to do (the semantic plane).",
        "CodeFile" => "A physical file on disk (the physical plane).",
        "QualityRule" => "A named anti-pattern / norm (the normative plane).",
        "Validation" => "A runnable proof that an intent is fulfilled.",
        "Note" => "Append-only free-text memory (justification, idea, question, …).",
        _ => "",
    }
}

fn edge_desc(etype: &str) -> &'static str {
    match etype {
        "RELATES_TO" => "Intent ↔ Intent — any tracked relationship worth inspecting (the N×N grid).",
        "HIERARCHY" => "Intent → Intent — parent/child zoom (component rolls up feature). A TREE: each intent has at most one parent, no cycles (enforced).",
        "IMPLEMENTS" => "Intent → CodeFile — grounds a semantic intent in real code (carries a `locator`).",
        "GOVERNS" => "QualityRule → Intent — a norm that applies to an intent.",
        "VALIDATES" => "Validation → Intent — a proof object attached to an intent.",
        _ => "",
    }
}

const STATES: &[(&str, &str)] = &[
    ("uninspected", "declared but never verified against actual code"),
    ("passing", "inspected, criterion met"),
    ("failing", "inspected, criterion violated"),
    ("independent", "inspected, confirmed no relationship (RELATES_TO only)"),
    ("needs_reverification", "was passing/failing, adjacent code changed — stale"),
];

pub fn run(printer: &Printer) -> Result<()> {
    let field_json = |(name, owner): &(&str, &str)| serde_json::json!({
        "name": name, "populated_by": owner,
    });
    let nodes: Vec<serde_json::Value> = NODE_LABELS
        .iter()
        .map(|&l| serde_json::json!({
            "label": l, "description": node_desc(l),
            "properties": required_node_props(l).iter().map(field_json).collect::<Vec<_>>(),
        }))
        .collect();
    let edges: Vec<serde_json::Value> = EDGE_TYPES
        .iter()
        .map(|&e| serde_json::json!({
            "type": e, "description": edge_desc(e),
            "properties": required_edge_props(e).iter().map(field_json).collect::<Vec<_>>(),
        }))
        .collect();
    let roles: Vec<serde_json::Value> = ROLES
        .iter()
        .map(|&r| serde_json::json!({"role": r, "responsibility": role_desc(r)}))
        .collect();
    let states: Vec<serde_json::Value> = STATES
        .iter()
        .map(|(s, m)| serde_json::json!({"state": s, "meaning": m}))
        .collect();

    let vocab = serde_json::json!({
        "abstraction_level": ["feature", "component", "system", "cross_cutting"],
        "lifecycle": ["planned", "implemented", "needs_change"],
        "inspection_status": STATES.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
        "note_kind": ["justification", "commentary", "idea", "question", "decision", "todo"],
        "severity": ["warning", "error"],
        "validation_type": ["test", "assertion", "benchmark", "manual_check"],
        "aspect": {"open": true, "suggested": ["happy", "sad", "fallback", "edge_case", "lifecycle", "security", "performance"]},
    });

    if printer.json {
        printer.print_json(&serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "node_labels": nodes,
            "edge_types": edges,
            "inspection_states": states,
            "agent_roles": roles,
            "vocabularies": vocab,
        }));
        return Ok(());
    }

    let fmt_fields = |specs: &[(&str, &str)]| {
        specs
            .iter()
            .map(|(n, o)| format!("{n} [{o}]"))
            .collect::<Vec<_>>()
            .join(", ")
    };

    println!("── loom schema (v{}) ─────────────────────────────────────────────────", SCHEMA_VERSION);
    println!();
    println!("Fields are shown as  name [owning-role]  — the role responsible for filling it.");
    println!();
    println!("Nodes:");
    for &l in NODE_LABELS {
        println!("  {}", l);
        println!("    {}", node_desc(l));
        println!("    props: {}", fmt_fields(required_node_props(l)));
    }
    println!();
    println!("Edges:");
    for &e in EDGE_TYPES {
        println!("  {}", e);
        println!("    {}", edge_desc(e));
        println!("    props: {}", fmt_fields(required_edge_props(e)));
    }
    println!();
    println!("Agent roles (who populates what):");
    for &r in ROLES {
        println!("  {:<10} {}", r, role_desc(r));
    }
    println!();
    println!("Inspection states (the heartbeat — on every edge):");
    for (s, m) in STATES {
        println!("  {:<22} {}", s, m);
    }
    println!();
    println!("Vocabularies:");
    println!("  abstraction_level: feature | component | system | cross_cutting");
    println!("  lifecycle:         planned | implemented | needs_change");
    println!("  note_kind:         justification | commentary | idea | question | decision | todo");
    println!("  severity:          warning | error");
    println!("  validation_type:   test | assertion | benchmark | manual_check");
    println!("  aspect (open):     happy | sad | fallback | edge_case | lifecycle | security | performance | …");
    Ok(())
}
