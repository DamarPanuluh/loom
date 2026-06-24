//! `loom schema` — emit loom's data model so a cold LLM can introspect exactly
//! what is valid. The structural part (labels, edge types, properties) is
//! generated from the single source of truth in `db::schema`, so it cannot drift
//! from what the queries actually read/write.

use anyhow::Result;

use crate::db::schema::{
    prop_type, required_edge_props, required_node_props, EDGE_TYPES, NODE_LABELS, ROLES,
    SCHEMA_VERSION,
};
use crate::output::Printer;

/// One-line description of what each agent role is responsible for.
fn role_desc(role: &str) -> &'static str {
    match role {
        "builder" => "Constructs the graph: intents, hierarchy, codefiles, implements.",
        "analyzer" => "Grounds edges: criterion, evidence, confidence, inspection_status.",
        "fixer" => "Resolves failing edges + needs_change intents (transitions status).",
        "validator" => "Proves it works: runs validations, confirms intents, VALIDATES verdict.",
        "quality" => "The green gate: quality rules + GOVERNS verdicts.",
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
        "Ignore" => "A coverage exclusion pattern with a recorded reason — the honest escape hatch for generated, vendor, or out-of-scope files.",
        "Delegation" => "A subtree owned by another loom graph; coverage treats matching files as covered by the child graph's committed export.",
        "Hypothesis" => "An improvement proposal (the pre-decision plane): claim + proposal + predicted outcome. proposed → supported|refuted (proven by a DIFFERENT agent) → adopted → confirmed (outcome verified) or rejected. Invisible to coverage/completeness until adopted.",
        "VocabTerm" => "A registered tag term — the bounded vocabulary intents may carry in `tags` (max 3). A key, not a knowledge node: its value is forcing two descriptions of one responsibility to collide (`duplicated_responsibility`). Registry: `loom vocab list`.",
        "Persona" => "A named audience segment. SERVES edges verify which intents serve it; JOURNEYS edges bind saga proofs to its end-to-end path.",
        "InterfaceSurface" => "An externally callable surface such as an HTTP endpoint. Sagas CALL these surfaces; intents still describe the behavior being proven.",
        "InboxItem" => "A durable intake card for raw human/LLM language. Candidates only: normalize and route before creating graph truth.",
        _ => "",
    }
}

fn edge_desc(etype: &str) -> &'static str {
    match etype {
        "RELATES_TO" => "Intent ↔ Intent — any tracked relationship worth inspecting (the N×N grid).",
        "HIERARCHY" => "Intent → Intent — parent/child zoom (component rolls up feature). A TREE: each intent has at most one parent, no cycles (enforced).",
        "IMPLEMENTS" => "Intent → CodeFile — grounds a semantic intent in real code (carries a `locator`). A fresh grounding is LOCATED (the locator is verified present — `loom explain` shows `[located]`), a structural anchor; it becomes a full `passing` verdict only when an analyzer records a criterion. So IMPLEMENTS `passing` without a criterion means 'symbol present', not RELATES_TO's semantic 'criterion met'.",
        "GOVERNS" => "QualityRule → Intent — a norm that applies to an intent. A verdict at component/system altitude covers descendants ONLY when --covers-descendants is set (default: false — a direct verdict only).",
        "VALIDATES" => "Validation → Intent — a proof object attached to an intent.",
        "TARGETS" => "Hypothesis → Intent — which intents an improvement hypothesis would touch (full inspectable meta).",
        "SERVES" => "Persona → Intent — inspectable claim that the intent serves that audience segment.",
        "JOURNEYS" => "Persona → Validation — structural binding from an audience segment to a saga proof exercising its path.",
        "CALLS" => "Validation → InterfaceSurface — an ordered saga step calls a boundary surface; proof verdicts remain on VALIDATES and RELATES_TO.",
        _ => "",
    }
}

const STATES: &[(&str, &str)] = &[
    ("uninspected", "declared but never verified against actual code"),
    ("passing", "inspected, criterion met (on a criterion-less IMPLEMENTS = LOCATED: locator verified present, not yet criterion-judged)"),
    ("failing", "inspected, criterion violated"),
    ("independent", "inspected, confirmed no relationship (RELATES_TO: intents unrelated; GOVERNS: rule does not apply)"),
    ("partial", "inspected, bounded but not complete — some aspects comply but gaps remain (GOVERNS only)"),
    ("needs_reverification", "was passing/failing, adjacent code changed — stale"),
];

pub fn run(printer: &Printer) -> Result<()> {
    let field_json = |(name, owner): &(&str, &str)| {
        serde_json::json!({
            "name": name, "populated_by": owner, "type": prop_type(name),
        })
    };
    let nodes: Vec<serde_json::Value> = NODE_LABELS
        .iter()
        .map(|&l| {
            serde_json::json!({
                "label": l, "description": node_desc(l),
                "properties": required_node_props(l).iter().map(field_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    let edges: Vec<serde_json::Value> = EDGE_TYPES
        .iter()
        .map(|&e| {
            serde_json::json!({
                "type": e, "description": edge_desc(e),
                "properties": required_edge_props(e).iter().map(field_json).collect::<Vec<_>>(),
            })
        })
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
        "abstraction_level": {
            "values": ["feature", "component", "system", "cross_cutting"],
            "granularity": "system: 1–3 per repo (the product's purpose) · component: 5–15 (cohesive subsystems) · feature: many, ATOMIC — independently verifiable · cross_cutting: spans everything. Test: one falsifiable criterion per intent; a description needing 'and' is several intents.",
        },
        "lifecycle": ["planned", "implemented", "needs_change", "deferred"],
        "lifecycle_model": {
            "active_states": {
                "planned": "designed promise, not expected to be grounded in current code yet",
                "implemented": "current code is meant to realize this intent",
                "needs_change": "known issue or refactor target; work remains",
            },
            "transitions": [
                "new behavior -> planned via intent add, saga spawn, or hypothesis adoption",
                "planned -> implemented via build, codefile add, edge implement, and intent mark",
                "implemented -> needs_change -> implemented for admitted repairs",
                "superseded active intent -> status=deprecated via intent retire",
                "import --as-planned resets incoming implemented work to planned design",
            ],
            "distinct_from": {
                "intent.status": "proposed/confirmed/deprecated says whether the meaning itself is accepted or retired",
                "inspection_status": "edge evidence freshness/currentness",
                "validation_result": "proof freshness/currentness",
                "hypothesis_status": "pre-decision idea state before lifecycle work exists",
            },
        },
        "visibility": ["user_visible", "internal"],
        "boundary": {"values": ["inbound", "outbound"], "meaning": "inbound: exposes a surface the outside world calls (a provider contract) · outbound: calls an external system (a consumer dependency) · unset: internal, no boundary crossing. Surfaced in work items so traversal knows a change here is contract-affecting."},
        "inspection_status": STATES.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
        "note_kind": ["justification", "commentary", "idea", "question", "decision", "todo", "transition", "confirm"],
        "inbox_kind": crate::commands::inbox::INBOX_KINDS,
        "severity": ["warning", "error"],
        "validation_type": ["test", "assertion", "benchmark", "manual_check", "saga"],
        "relation_kind": crate::types::RelationKind::ALL.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
        "governs_kind": crate::types::GovernsKind::ALL.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
        "validation_result": ["passed", "failed", "not_run", "blocked"],
        "hypothesis_status": ["proposed", "supported", "refuted", "adopted", "confirmed", "rejected"],
        "aspect": {"open": true, "suggested": ["happy", "sad", "fallback", "edge_case", "lifecycle", "security", "performance"]},
    });

    if printer.json {
        printer.print_json(&serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "edge_identity": "Derived, never stored: <prefix>:<from-id>:<to-id> with prefixes rt (RELATES_TO), hy (HIERARCHY), imp (IMPLEMENTS), gov (GOVERNS), val (VALIDATES), tgt (TARGETS), srv (SERVES), jrn (JOURNEYS), call (CALLS). Stable across export/import; CALLS uses the step index in its command-facing id.",
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
            .map(|(n, o)| match prop_type(n) {
                "string" => format!("{n} [{o}]"),
                ty => format!("{n}:{ty} [{o}]"),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };

    println!(
        "── loom schema (v{}) ─────────────────────────────────────────────────",
        SCHEMA_VERSION
    );
    println!();
    println!("Fields are shown as  name [owning-role]  — the role responsible for filling it");
    println!("(non-string fields carry a :type — list fields read and write as real arrays).");
    println!();
    println!("Edge identity is DERIVED, never stored: <prefix>:<from-id>:<to-id> —");
    println!(
        "  rt=RELATES_TO  hy=HIERARCHY  imp=IMPLEMENTS  gov=GOVERNS  val=VALIDATES  tgt=TARGETS"
    );
    println!("  srv=SERVES     jrn=JOURNEYS  call=CALLS.");
    println!("  Stable across export/import; it is the id `loom edge show` and notes reference.");
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
    println!("                     system: 1–3 per repo · component: 5–15 · feature: many, ATOMIC");
    println!("                     (one falsifiable criterion each; an 'and' in the description = split it)");
    println!("  lifecycle:         planned | implemented | needs_change | deferred | to_be_removed");
    println!("                     to_be_removed = cleanup as a tracked verb (criterion falsifiable by ABSENCE — gates green only once the code is gone); deferred PARKS valid-but-not-now work (out of the build queue, never blocks a roll-up). Both are distinct from retire (status=deprecated via `loom intent retire`), which is for SUPERSEDED/out-of-scope design — a dead meaning kept for history.");
    println!("                     porting with `import --as-planned` resets incoming work to planned design");
    println!("  visibility:        user_visible | internal | (unset = untriaged — the align interview triages it; internal leaves the interview until redefined)");
    println!("  boundary:          inbound (exposes a surface the outside world calls — provider contract) | outbound (calls an external system — consumer dependency) | (unset = internal, no crossing)");
    println!("  note_kind:         justification | commentary | idea | question | decision | todo | transition (auto: verdict history) | confirm (auto: `loom intent confirm` stamp)");
    println!("  severity:          warning | error");
    println!("  validation_type:   test | assertion | benchmark | manual_check | saga (consumer-plane chain, `loom saga`)");
    println!("  validation_result: passed | failed | not_run | blocked (recorded \"can't run yet\" + reason)");
    println!(
        "  relation_kind:     {} (RELATES_TO multiset — how two intents are coupled)",
        crate::types::RelationKind::ALL
            .iter()
            .map(|k| k.as_str())
            .collect::<Vec<_>>()
            .join(" | ")
    );
    println!(
        "  governs_kind:      {} (QualityRule norm category)",
        crate::types::GovernsKind::ALL
            .iter()
            .map(|k| k.as_str())
            .collect::<Vec<_>>()
            .join(" | ")
    );
    println!("  aspect (open):     happy | sad | fallback | edge_case | lifecycle | security | performance | …");
    println!("  tags (bounded):    ≤3 registered VocabTerm names per intent — `loom vocab list` is the menu; unknown terms error with it inlined");
    Ok(())
}
