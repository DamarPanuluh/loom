use super::{open, require_challenge};
use crate::cli::{PatternCmd, PatternExemplarCmd};
use crate::model::{Claim, EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use crate::pattern::{Applicability, PatternBody};
use crate::store::{Assertion, Store, Subject};
use crate::Result;
use anyhow::bail;
use std::path::Path;

pub fn dispatch(graph: Option<&Path>, cmd: PatternCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    let value = match cmd {
        PatternCmd::Add {
            name,
            rationale,
            when_to_use,
            when_not_to_use,
            paths,
            intent_tags,
        } => {
            let body = body(rationale, when_to_use, when_not_to_use, paths, intent_tags)?;
            serde_json::to_value(store.add_node(
                NodeType::Pattern,
                &name,
                "",
                "draft",
                serde_json::to_value(body)?,
            )?)?
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
            serde_json::to_value(
                store
                    .get_node(&node.id)?
                    .ok_or_else(|| anyhow::anyhow!("Pattern vanished after update"))?,
            )?
        }
        PatternCmd::Show { key } => {
            show(&store, &store.resolve_node(&key, Some(NodeType::Pattern))?)?
        }
        PatternCmd::List => serde_json::Value::Array(
            store
                .list_nodes(Some(NodeType::Pattern), usize::MAX)?
                .iter()
                .map(|n| show(&store, n))
                .collect::<Result<Vec<_>>>()?,
        ),
        PatternCmd::Lookup {
            paths,
            intent_tags,
            offset,
        } => {
            if paths.is_empty() && intent_tags.is_empty() {
                bail!("pattern lookup requires --path and/or --intent-tag; selectorless patterns are manual-only");
            }
            serde_json::to_value(crate::pattern::guidance_page(
                &store,
                &paths,
                &intent_tags,
                offset,
            )?)?
        }
        PatternCmd::Ratify { key, evidence } => {
            let n = store.resolve_node(&key, Some(NodeType::Pattern))?;
            let presence = require_challenge(&n.name)?;
            store.ratify_pattern(&n.id, &evidence, presence)?;
            show(&store, &n)?
        }
        PatternCmd::Retire { key, reason } => {
            let n = store.resolve_node(&key, Some(NodeType::Pattern))?;
            store.add_note(&n.id, "decision", &format!("retired: {reason}"))?;
            store.set_node_status(&n.id, "deprecated")?;
            show(&store, &n)?
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
            serde_json::json!({"removed":n.id})
        }
        PatternCmd::Exemplar { cmd } => exemplar(&store, cmd)?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&value)?);
    }
    Ok(())
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

fn exemplar(store: &Store, cmd: PatternExemplarCmd) -> Result<serde_json::Value> {
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
            Ok(serde_json::to_value(e)?)
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
            Ok(serde_json::to_value(store.get_edge(&e.id)?.ok_or_else(
                || anyhow::anyhow!("Exemplar edge vanished after verdict"),
            )?)?)
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
            Ok(serde_json::json!({"removed":e.id}))
        }
    }
}

fn show(store: &Store, n: &crate::model::Node) -> Result<serde_json::Value> {
    let view = crate::pattern::inspect(store, n)?;
    let health = view.health;
    let health_reason = view.health_reason;
    let exemplars = view.exemplars;
    Ok(
        serde_json::json!({"pattern":n,"ratification":store.ratification(&n.id)?,"health":health,"health_reason":health_reason,"exemplars":exemplars}),
    )
}
