use super::{open, pulse, require_challenge};
use crate::cli::{PatternCmd, PatternExemplarCmd};
use crate::model::{Claim, EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use crate::pattern::{Applicability, PatternBody, PatternGuidance, PatternView};
use crate::store::{Assertion, Store, Subject};
use crate::Result;
use anyhow::bail;
use std::path::Path;

/// One command's result in both renderings.
///
/// `next` distinguishes the two contracts in this dispatcher: a mutation
/// carries the driver's next move and goes out through the shared pulse, while
/// a read (`None`) renders its own view and stops there.
struct Emission {
    value: serde_json::Value,
    human: String,
    next: Option<String>,
}

impl Emission {
    fn wrote(value: serde_json::Value, human: impl Into<String>, next: impl Into<String>) -> Self {
        Self {
            value,
            human: human.into(),
            next: Some(next.into()),
        }
    }

    fn read(value: serde_json::Value, human: impl Into<String>) -> Self {
        Self {
            value,
            human: human.into(),
            next: None,
        }
    }
}

pub fn dispatch(graph: Option<&Path>, cmd: PatternCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    let out = match cmd {
        PatternCmd::Add {
            name,
            rationale,
            when_to_use,
            when_not_to_use,
            paths,
            intent_tags,
        } => {
            let body = body(rationale, when_to_use, when_not_to_use, paths, intent_tags)?;
            let node = store.add_node(
                NodeType::Pattern,
                &name,
                "",
                "draft",
                serde_json::to_value(body)?,
            )?;
            // A fresh pattern is a draft: unratified and exemplar-less, so it
            // cannot route yet. Point at ratification, the human-only gate.
            Emission::wrote(
                serde_json::to_value(&node)?,
                format!("pattern '{}' added as a draft", node.name),
                format!(
                    "loom pattern ratify {} --evidence \"<why this is the house way>\"",
                    node.id
                ),
            )
        }
        PatternCmd::Update {
            key,
            name,
            rationale,
            when_to_use,
            when_not_to_use,
            paths,
            intent_tags,
            clear_paths,
            clear_intent_tags,
            reason,
        } => {
            let node = store.resolve_node(&key, Some(NodeType::Pattern))?;
            let normative = rationale.is_some()
                || when_to_use.is_some()
                || when_not_to_use.is_some()
                || !paths.is_empty()
                || !intent_tags.is_empty()
                || clear_paths
                || clear_intent_tags;
            if normative {
                let mut old = PatternBody::parse(&node.body)?;
                if let Some(v) = rationale {
                    old.rationale = v;
                }
                if let Some(v) = when_to_use {
                    old.when_to_use = v;
                }
                if let Some(v) = when_not_to_use {
                    old.when_not_to_use = v;
                }
                if clear_paths {
                    old.applicability.path_globs.clear();
                } else if !paths.is_empty() {
                    old.applicability.path_globs = paths;
                }
                if clear_intent_tags {
                    old.applicability.intent_tags.clear();
                } else if !intent_tags.is_empty() {
                    old.applicability.intent_tags = intent_tags;
                }
                PatternBody::parse(&serde_json::to_value(&old)?)?;
                store.set_node_body(&node.id, &serde_json::to_value(old)?)?;
            }
            if let Some(name) = name {
                store.update_node(&node.id, Some(&name), None, None)?;
            }
            store.add_note(&node.id, "decision", &format!("pattern updated: {reason}"))?;
            let updated = store
                .get_node(&node.id)?
                .ok_or_else(|| anyhow::anyhow!("Pattern vanished after update"))?;
            // A normative edit changes what the pattern asks for, so the
            // standing human ratification no longer covers the current text.
            let human = if normative {
                format!(
                    "pattern '{}' updated; its ratification no longer covers this text",
                    updated.name
                )
            } else {
                format!("pattern '{}' updated", updated.name)
            };
            Emission::wrote(
                serde_json::to_value(&updated)?,
                human,
                format!("loom pattern show {}", updated.id),
            )
        }
        PatternCmd::Show { key } => {
            let node = store.resolve_node(&key, Some(NodeType::Pattern))?;
            let view = crate::pattern::inspect(&store, &node)?;
            Emission::read(
                show_value(&store, &node, &view)?,
                human_show(&store, &view)?,
            )
        }
        PatternCmd::List => {
            let nodes = store.list_nodes(Some(NodeType::Pattern), usize::MAX)?;
            let mut values = Vec::with_capacity(nodes.len());
            let mut lines = Vec::with_capacity(nodes.len());
            for n in &nodes {
                let view = crate::pattern::inspect(&store, n)?;
                values.push(show_value(&store, n, &view)?);
                lines.push(format!(
                    "{:<10} {:<4} {}",
                    view.health,
                    view.exemplars.len(),
                    n.name
                ));
            }
            let human = if lines.is_empty() {
                "no patterns declared".to_string()
            } else {
                format!(
                    "{:<10} {:<4} {}\n{}",
                    "HEALTH",
                    "EX",
                    "PATTERN",
                    lines.join("\n")
                )
            };
            Emission::read(serde_json::Value::Array(values), human)
        }
        PatternCmd::Lookup {
            paths,
            intent_tags,
            offset,
        } => {
            if paths.is_empty() && intent_tags.is_empty() {
                bail!("pattern lookup requires --path and/or --intent-tag; selectorless patterns are manual-only");
            }
            let page = crate::pattern::guidance_page(&store, &paths, &intent_tags, offset)?;
            let human = human_lookup(&page);
            Emission::read(serde_json::to_value(&page)?, human)
        }
        PatternCmd::Ratify { key, evidence } => {
            let n = store.resolve_node(&key, Some(NodeType::Pattern))?;
            let presence = require_challenge(&n.name)?;
            store.ratify_pattern(&n.id, &evidence, presence)?;
            let view = crate::pattern::inspect(&store, &n)?;
            Emission::wrote(
                show_value(&store, &n, &view)?,
                format!("pattern '{}' ratified; health is {}", n.name, view.health),
                format!("loom pattern show {}", n.id),
            )
        }
        PatternCmd::Retire { key, reason } => {
            let n = store.resolve_node(&key, Some(NodeType::Pattern))?;
            store.add_note(&n.id, "decision", &format!("retired: {reason}"))?;
            // loom-stability-exempt: retires a pattern
            store.set_node_status(&n.id, "deprecated")?;
            let view = crate::pattern::inspect(&store, &n)?;
            Emission::wrote(
                show_value(&store, &n, &view)?,
                format!("pattern '{}' retired; it no longer routes", n.name),
                "loom pattern list",
            )
        }
        PatternCmd::Remove { key, reason } => {
            let n = store.resolve_node(&key, Some(NodeType::Pattern))?;
            if !store
                .edges_with(Some(EdgeKind::Exemplar), Some(&n.id), None)?
                .is_empty()
            {
                bail!("remove Pattern exemplars first");
            }
            store.add_note(&n.id, "decision", &format!("removed: {reason}"))?;
            store.delete_node(&n.id)?;
            Emission::wrote(
                serde_json::json!({"removed":n.id}),
                format!("pattern '{}' removed", n.name),
                "loom pattern list",
            )
        }
        PatternCmd::Exemplar { cmd } => exemplar(&store, cmd)?,
    };

    match out.next {
        Some(next) => pulse::emit_line(&store, json, out.value, &next, out.human),
        None => {
            if json {
                println!("{}", serde_json::to_string_pretty(&out.value)?);
            } else {
                println!("{}", out.human);
            }
            Ok(())
        }
    }
}

fn body(
    rationale: String,
    when_to_use: String,
    when_not_to_use: String,
    paths: Vec<String>,
    intent_tags: Vec<String>,
) -> Result<PatternBody> {
    let v = PatternBody {
        rationale,
        when_to_use,
        when_not_to_use,
        applicability: Applicability {
            path_globs: paths,
            intent_tags,
        },
    };
    PatternBody::parse(&serde_json::to_value(&v)?)?;
    Ok(v)
}

fn exemplar(store: &Store, cmd: PatternExemplarCmd) -> Result<Emission> {
    match cmd {
        PatternExemplarCmd::Add {
            pattern,
            codefile,
            locator,
        } => {
            if locator.trim().is_empty() {
                bail!("Exemplar requires a nonempty --locator");
            }
            let p = store.resolve_node(&pattern, Some(NodeType::Pattern))?;
            let f = store.resolve_node(&codefile, Some(NodeType::CodeFile))?;
            if crate::runner::unique_locator_probe(store.root(), &f.name, &locator).is_none() {
                bail!(
                    "Exemplar locator must resolve exactly one live symbol in '{}'",
                    f.name
                );
            }
            let e = store.add_edge(EdgeKind::Exemplar, &p.id, &f.id, TruthClass::Asserted)?;
            store.set_facet(
                &e.id,
                TargetKind::Edge,
                "locator",
                &locator,
                TruthClass::Asserted,
            )?;
            // An exemplar is a claim about the code until an analyzer grounds
            // it, so the next move is the verdict, not another exemplar.
            Ok(Emission::wrote(
                serde_json::to_value(&e)?,
                format!(
                    "exemplar '{}' in {} attached to pattern '{}'",
                    locator, f.name, p.name
                ),
                format!(
                    "loom pattern exemplar verdict --edge {} --verdict ground --criterion \"<what you checked>\" --evidence \"{}\"",
                    e.id, f.name
                ),
            ))
        }
        PatternExemplarCmd::Verdict {
            edge,
            verdict,
            criterion,
            evidence,
            confidence,
        } => {
            let e = store.resolve_edge(&edge)?;
            if e.kind != EdgeKind::Exemplar {
                bail!("edge is not an Exemplar");
            }
            let state = match verdict.as_str() {
                "ground" => InspectionStatus::Passing,
                "issue" => InspectionStatus::Failing,
                "independent" => InspectionStatus::Independent,
                _ => bail!("verdict must be ground|issue|independent"),
            };
            let cited = crate::evidence::cite(store.root(), &evidence)?;
            store.assert_fact(
                Assertion::new(
                    Subject::Edge(e.id.clone()),
                    Claim::Verdict,
                    state.as_str(),
                    "analyzer",
                )
                .criterion(&criterion)
                .confidence(confidence)
                .cited(cited),
            )?;
            let edge = store
                .get_edge(&e.id)?
                .ok_or_else(|| anyhow::anyhow!("Exemplar edge vanished after verdict"))?;
            let pattern = store.get_node(&edge.from_id)?;
            let pattern_name = pattern
                .map(|n| n.name)
                .unwrap_or_else(|| edge.from_id.clone());
            Ok(Emission::wrote(
                serde_json::to_value(&edge)?,
                format!(
                    "exemplar verdict recorded: {verdict} ({state})",
                    state = state.as_str()
                ),
                format!("loom pattern show {pattern_name}"),
            ))
        }
        PatternExemplarCmd::Remove { edge, reason } => {
            let e = store.resolve_edge(&edge)?;
            if e.kind != EdgeKind::Exemplar {
                bail!("edge is not an Exemplar");
            }
            // Check ownership before recording the decision note: a denied
            // builder attempt must leave no partial history behind.
            store.require_edge_owner(&e.id)?;
            store.add_note(
                &e.from_id,
                "decision",
                &format!("exemplar removed: {reason}"),
            )?;
            store.delete_edge(&e.id)?;
            Ok(Emission::wrote(
                serde_json::json!({"removed":e.id}),
                format!("exemplar {} removed", e.id),
                format!("loom pattern show {}", e.from_id),
            ))
        }
    }
}

fn show_value(
    store: &Store,
    n: &crate::model::Node,
    view: &PatternView,
) -> Result<serde_json::Value> {
    Ok(
        serde_json::json!({"pattern":n,"ratification":store.ratification(&n.id)?,"health":view.health,"health_reason":view.health_reason,"exemplars":view.exemplars}),
    )
}

fn human_show(store: &Store, view: &PatternView) -> Result<String> {
    let n = &view.node;
    let body = PatternBody::parse(&n.body)?;
    let mut out = vec![
        format!("{} [{}]", n.name, n.status),
        format!("  health:       {} — {}", view.health, view.health_reason),
        format!("  ratification: {}", store.ratification(&n.id)?),
        format!("  rationale:    {}", body.rationale),
        format!("  use when:     {}", body.when_to_use),
        format!("  not when:     {}", body.when_not_to_use),
    ];
    let globs = &body.applicability.path_globs;
    let tags = &body.applicability.intent_tags;
    if globs.is_empty() && tags.is_empty() {
        // Selectorless patterns never match a lookup; say so rather than
        // printing two empty fields the reader has to interpret.
        out.push("  applies to:   nothing automatically (manual-only)".into());
    } else {
        if !globs.is_empty() {
            out.push(format!("  paths:        {}", globs.join(", ")));
        }
        if !tags.is_empty() {
            out.push(format!("  intent tags:  {}", tags.join(", ")));
        }
    }
    if view.exemplars.is_empty() {
        out.push("  exemplars:    none".into());
    } else {
        out.push(format!("  exemplars:    {}", view.exemplars.len()));
        for e in &view.exemplars {
            out.push(format!("    - {}:{}", e.path, e.locator));
        }
    }
    Ok(out.join("\n"))
}

fn human_lookup(page: &PatternGuidance) -> String {
    if page.matched == 0 {
        return "no routable pattern matches those selectors".to_string();
    }
    let mut out = vec![format!(
        "{} exemplar(s) matched, {} shown, {} omitted",
        page.matched, page.included, page.omitted
    )];
    for item in &page.items {
        out.push(String::new());
        out.push(format!("{} — {}", item.name, item.rationale));
        out.push(format!("  use when: {}", item.when_to_use));
        out.push(format!("  not when: {}", item.when_not_to_use));
        out.push(format!("  example:  {}:{}", item.path, item.locator));
    }
    if page.omitted > 0 {
        out.push(String::new());
        out.push(format!("next page: {}", page.lookup_command));
    }
    out.join("\n")
}
