//! `loom intent` command family.
//!
//! Plane: CLI surface over the judgment plane — the asserted intent lifecycle
//! (add/update/waive/reactivate/ratify); it writes assertions, never derived
//! truth.
//!
//! Contract (ratification): every intent carries `origin` (who minted it) and
//! `ratification` (whether the product authority wants it). Any lane may mint;
//! ONLY a human may decide ratification (INV-8). A solo human can write it
//! directly; an LLM lane can only record an explicit answer returned by the
//! host conversation. Redefining a ratified intent stales its ratification to
//! `needs_reconfirmation`, exactly as sync stales a verdict.

use super::{node_json, open, pulse, require_lane};
use crate::cli::{IntentCmd, IntentTagCmd};
use crate::grammar::{
    looks_like_symbol, ACTIVE_LIFECYCLES, ALL_LIFECYCLES, ASPECTS, LEVELS, VISIBILITIES,
};
use crate::model::{EdgeKind, Node, NodeType, TargetKind, TruthClass};
use crate::store::Store;
use crate::Result;
use anyhow::bail;
use std::path::Path;

pub fn dispatch(graph: Option<&Path>, cmd: IntentCmd, json: bool) -> Result<()> {
    match cmd {
        IntentCmd::Add {
            name,
            description,
            level,
            lifecycle,
            visibility,
            layer,
            aspect,
            allow_symbol_name,
        } => intent_add(
            graph,
            IntentAddArgs {
                name,
                description,
                level,
                lifecycle,
                visibility,
                layer,
                aspect,
                allow_symbol_name,
            },
            json,
        ),
        IntentCmd::Show { key } => intent_show(graph, key, json),
        IntentCmd::Waive { key, axis, reason } => intent_waive(graph, key, axis, reason, json),
        IntentCmd::Reactivate { key, reason } => intent_reactivate(graph, key, reason, json),
        IntentCmd::List { limit, offset } => intent_list(graph, limit, offset, json),
        IntentCmd::Update {
            key,
            description,
            name,
            level,
            visibility,
            aspect,
            lifecycle,
            rectify,
            reason,
            reword,
        } => intent_update(
            graph,
            IntentUpdateArgs {
                key,
                description,
                new_name: name,
                level,
                visibility,
                aspect,
                lifecycle,
                rectify,
                reason,
                reword,
            },
            json,
        ),
        IntentCmd::Remove { key, reason } => intent_remove(graph, key, reason, json),
        IntentCmd::Retire {
            key,
            reason,
            replaced_by,
        } => intent_retire(graph, key, reason, replaced_by, json),
        IntentCmd::Confirm { key } => intent_confirm(graph, key, json),
        IntentCmd::Impact {
            key,
            classification,
            evidence,
        } => intent_impact(graph, key, classification, evidence, json),
        IntentCmd::Dependents { key, depth } => intent_dependents(graph, &key, depth, json),
        IntentCmd::Ratify {
            key,
            all,
            evidence,
            human_decision,
        } => intent_ratify(
            graph,
            RatifyArgs {
                key,
                all,
                evidence,
                human_decision,
            },
            json,
        ),
        IntentCmd::Reject {
            key,
            reason,
            human_decision,
        } => intent_reject(graph, &key, &reason, human_decision, json),
        IntentCmd::Tag { cmd } => intent_tag(graph, cmd, json),
    }
}

/// Shape a ratification-state assertion. Demotions (`needs_reconfirmation`)
/// need no human authority — noticing that meaning drifted is not an act of
/// approval.
fn loom_assertion<'a>(id: &'a str, state: &'a str) -> crate::store::Assertion<'a> {
    crate::store::Assertion::new(
        crate::store::Subject::Node(id.to_string()),
        crate::model::Claim::Ratification,
        state,
        "sync",
    )
}

/// Is this intent ratified? Absence = unratified (fail closed: wantedness
/// is never presumed).
/// Say a behavior is not wanted.
///
/// Deliberately cheaper than ratifying: presence is required, but no typed
/// challenge. Writing a substantive reason IS the deliberate act, and making
/// refusal expensive is how you get a graph nobody ever refuses anything in.
///
/// A rejection is not a delete. Every place the code still performs the
/// behavior becomes a finding, so removing it enters triage as ordinary work
/// with the evidence already attached — and until it is gone, the intent is a
/// `ZombieBehavior` the ladder blocks on.
fn intent_reject(
    graph: Option<&Path>,
    key: &str,
    reason: &str,
    human_decision: Option<String>,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    if crate::model::is_placeholder(reason) {
        bail!("--reason must say why this is not wanted, substantively");
    }
    let decision = match human_decision {
        Some(response) => super::mediated_decision(response)?,
        None if super::human_present() => crate::ratification::HumanDecision::direct("tty")?,
        None => bail!(
            "INV-8: only a human may judge whether a behavior is wanted — ask the human, then pass their exact answer with --human-decision"
        ),
    };
    let intent = store.resolve_node(key, Some(NodeType::Intent))?;
    let minted = reject_intent_core(&store, &intent, reason, &decision)?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "rejected": { "id": intent.id, "name": intent.name },
            "reason": reason,
            "removal_work": minted,
        }),
        "loom next --mode triage",
        format!(
            "rejected '{}' — {} place(s) still perform it",
            intent.name,
            minted.len()
        ),
    )?;
    Ok(())
}

/// The consequence of a human's reject decision: record it, mint removal
/// work for every place still performing the behavior, and retire the
/// intent. Shared by `intent reject` and `judgment confirm` — the inbox is
/// another door into the SAME gated write, never a second semantics.
pub(crate) fn reject_intent_core(
    store: &Store,
    intent: &Node,
    reason: &str,
    decision: &crate::ratification::HumanDecision,
) -> Result<Vec<serde_json::Value>> {
    store.reject_intent_from_human(&intent.id, reason, decision)?;

    // Every realizing grounding becomes removal work, with the reason attached.
    let mut minted = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Implements), Some(&intent.id), None)? {
        if store.edge_superseded(&e.id)?
            || store.grounding_role(&e.id)? != crate::model::GroundingRole::Realizes
        {
            continue;
        }
        let Some(cf) = store.get_node(&e.to_id)? else {
            continue;
        };
        let finding = store.add_node(
            NodeType::Finding,
            &format!("unwanted behavior in {}", cf.name),
            &format!("'{}' was rejected: {reason}", intent.name),
            "unwanted_behavior",
            serde_json::json!({
                "kind": "unwanted_behavior",
                "intent": intent.id,
                "codefile": cf.name,
            }),
        )?;
        store.ensure_edge(EdgeKind::Flags, &finding.id, &cf.id)?;
        minted.push(serde_json::json!({ "id": finding.id, "file": cf.name }));
    }
    // loom-stability-exempt: retires an intent
    store.set_node_status(&intent.id, "deprecated")?;
    Ok(minted)
}

pub(crate) fn is_ratified(store: &Store, intent_id: &str) -> Result<bool> {
    Ok(store.ratification(intent_id)? == "ratified")
}

pub(crate) struct IntentAddArgs {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) level: String,
    pub(crate) lifecycle: String,
    pub(crate) visibility: Option<String>,
    pub(crate) layer: Option<String>,
    pub(crate) aspect: Option<String>,
    pub(crate) allow_symbol_name: bool,
}

/// Validate a scenario aspect label.
fn check_aspect(aspect: &str) -> Result<()> {
    if !ASPECTS.contains(&aspect) {
        bail!("unknown aspect '{aspect}' (use {})", ASPECTS.join("|"));
    }
    Ok(())
}

fn check_level(level: &str) -> Result<()> {
    if !LEVELS.contains(&level) {
        bail!("unknown level '{level}' (use {})", LEVELS.join("|"));
    }
    Ok(())
}

fn check_lifecycle(lifecycle: &str, allow_deprecated: bool) -> Result<()> {
    let lifecycles = if allow_deprecated {
        ALL_LIFECYCLES
    } else {
        ACTIVE_LIFECYCLES
    };
    if !lifecycles.contains(&lifecycle) {
        bail!(
            "unknown lifecycle '{lifecycle}' (use {})",
            lifecycles.join("|")
        );
    }
    Ok(())
}

fn check_visibility(visibility: &str) -> Result<()> {
    if !VISIBILITIES.contains(&visibility) {
        bail!(
            "unknown visibility '{visibility}' (use {})",
            VISIBILITIES.join("|")
        );
    }
    Ok(())
}

/// Create an intent node with all its asserted facets, enforcing the
/// command-layer gates (symbol-name rejection, level/lifecycle/visibility/aspect
/// validation). The single source of these rules: both `loom intent add` and the
/// `loom apply` batch call it, so neither can drift from the other. Store-only —
/// no output — so it composes inside a batch transaction.
pub(crate) fn create_intent(store: &Store, args: &IntentAddArgs) -> Result<Node> {
    // INV-ATOM: symbols are locators, not intents.
    if looks_like_symbol(&args.name) {
        if !args.allow_symbol_name {
            bail!(
                "intent name '{}' looks like a code symbol. Intents are behaviors, \
                 not functions. Use a behavioral name, or pass --allow-symbol-name with \
                 a behavioral --description if this is a deliberate symbol-level intent.",
                args.name
            );
        }
        if args.description.trim().is_empty() {
            bail!(
                "--allow-symbol-name requires a non-empty --description carrying a \
                 behavioral criterion"
            );
        }
    }
    check_level(&args.level)?;
    check_lifecycle(&args.lifecycle, false)?;
    if let Some(v) = &args.visibility {
        check_visibility(v)?;
    }
    if let Some(a) = &args.aspect {
        check_aspect(a)?;
    }
    // Scenario aspects (sad/fallback/edge_case) surround a happy path — they are
    // not independent product surfaces. Default them to internal unless the
    // caller explicitly set visibility (keeps elaborate/quality/journey on the
    // happy-path spine for any repo).
    let visibility = match (&args.visibility, &args.aspect) {
        (Some(v), _) => Some(v.clone()),
        (None, Some(a)) if matches!(a.as_str(), "sad" | "fallback" | "edge_case") => {
            Some("internal".into())
        }
        (None, _) => None,
    };
    let node = store.add_node(
        NodeType::Intent,
        &args.name,
        &args.description,
        &args.lifecycle,
        serde_json::json!({ "level": args.level }),
    )?;
    if args.allow_symbol_name && looks_like_symbol(&args.name) {
        store.set_facet(
            &node.id,
            TargetKind::Node,
            "symbol_name_override",
            "true",
            TruthClass::Asserted,
        )?;
    }
    store.set_facet(
        &node.id,
        TargetKind::Node,
        "level",
        &args.level,
        TruthClass::Asserted,
    )?;
    if let Some(v) = &visibility {
        store.set_facet(
            &node.id,
            TargetKind::Node,
            "visibility",
            v,
            TruthClass::Asserted,
        )?;
    }
    if let Some(l) = &args.layer {
        store.set_facet(&node.id, TargetKind::Node, "layer", l, TruthClass::Asserted)?;
    }
    if let Some(a) = &args.aspect {
        store.set_facet(
            &node.id,
            TargetKind::Node,
            "aspect",
            a,
            TruthClass::Asserted,
        )?;
    }
    // Provenance + ratification (INV-8): anyone may mint, only a human may
    // ratify. A solo (human-at-the-keyboard) mint is itself a ratification act —
    // the minting utterance is the evidence. A declared `llm:*` lane mints an
    // unratified intent that stays honestly failing until a human ratifies it.
    // A PERSON at a terminal, not merely an unset agent. `Agent::Solo` is the
    // default whenever `LOOM_AGENT` is absent, so treating it as human meant
    // `loom intent add` in CI minted ratified intents — wantedness asserted by
    // a script.
    let solo = super::human_present();
    let origin = if solo { "human" } else { "llm" };
    store.set_facet(
        &node.id,
        TargetKind::Node,
        "origin",
        origin,
        TruthClass::Asserted,
    )?;
    if solo {
        // A born-ratified intent now goes through the SAME boundary as an
        // explicit `loom intent ratify`: the evidence gate, INV-8, and the
        // journal entry. It used to be a raw `set_facet`, which is precisely how
        // a ratification could exist with no evidence and no journal behind it —
        // the shape 39 of this graph's own ratifications had.
        store.ratify_intent(
            &node.id,
            &format!("minted at a terminal: {}", args.description.trim()),
            "mint",
        )?;
    }
    // An unratified intent needs no facet: ABSENCE reads as unratified
    // everywhere. Wantedness is never presumed, so there is nothing to write.
    Ok(node)
}

/// Ratify an intent (or every unratified intent with `--all`): the human
/// authority's evidence-bearing "yes, this is wanted". A lane may record an
/// explicit host-mediated human answer, but its direct write remains denied
/// (INV-8).
/// One `loom intent ratify` invocation's parameters, bundled so the handler
/// stays under the excess-args gate as the surface grows.
struct RatifyArgs {
    key: Option<String>,
    all: bool,
    evidence: Option<String>,
    human_decision: Option<String>,
}

fn intent_ratify(graph: Option<&Path>, args: RatifyArgs, json: bool) -> Result<()> {
    let store = open(graph)?;
    let evidence = args.evidence.ok_or_else(|| {
        anyhow::anyhow!("--evidence is required: say why this behavior is wanted")
    })?;
    let targets: Vec<Node> = match (&args.key, args.all) {
        (Some(_), true) => bail!("pass a key or --all, not both"),
        (None, false) => bail!("pass an intent key, or --all to ratify every unratified intent"),
        (Some(k), false) => vec![store.resolve_node(k, Some(NodeType::Intent))?],
        (None, true) => {
            let mut v = Vec::new();
            for n in store.list_nodes(Some(NodeType::Intent), usize::MAX)? {
                if n.status == "deprecated" {
                    continue;
                }
                if !is_ratified(&store, &n.id)? {
                    v.push(n);
                }
            }
            v
        }
    };
    if targets.is_empty() {
        pulse::emit_line(
            &store,
            json,
            serde_json::json!({ "ratified": [] }),
            "loom status",
            "nothing to ratify — every active intent is already ratified",
        )?;
        return Ok(());
    }
    // ONE human decision authorizes this invocation, whether direct or host
    // mediated, not one prompt per intent. Asking 51 times is not 51 times the
    // assurance — it is how a worker facing 51 prompts ends up forging the
    // records instead, which is exactly what happened to 39 of this graph's own
    // ratifications.
    //
    let subject = match targets.as_slice() {
        [one] => one.name.clone(),
        many => format!("ratify {}", many.len()),
    };
    let decision = super::ratification_decision(&subject, args.human_decision)?;
    let batch_id = if targets.len() > 1 {
        let subjects: Vec<String> = targets.iter().map(|n| n.id.clone()).collect();
        let executor = std::env::var("LOOM_AGENT").unwrap_or_else(|_| "solo".into());
        // Contemporaneous set record before the per-intent writes.
        let digest = crate::batch_auth::subject_digest(&subjects);
        let pre = crate::journal::append(
            store.root(),
            "batch_intent",
            &digest,
            serde_json::json!({
                "operation": "ratify",
                "subjects": subjects,
                "human_decision": decision,
                "evidence": evidence,
            }),
        )?;
        let now = crate::journal::now_iso();
        let envelope = crate::batch_auth::BatchAuthorization::seal(
            crate::batch_auth::BatchClaim::Ratification,
            "ratify",
            subjects,
            "human",
            &executor,
            &evidence,
            vec![format!("journal:{}", pre.id)],
        )?
        .with_command_id(format!("intent-ratify-all:{}", targets.len()))
        .with_time_bounds(&now, &now)
        .with_human_decision(decision.clone());
        let entry = crate::batch_auth::append_envelope(store.root(), &envelope)?;
        Some(entry.id)
    } else {
        None
    };
    let mut ratified = Vec::new();
    for n in &targets {
        match &batch_id {
            Some(bid) => store.ratify_intent_from_human_batch(&n.id, &evidence, &decision, bid)?,
            None => store.ratify_intent_from_human(&n.id, &evidence, &decision)?,
        }
        ratified.push(serde_json::json!({ "id": n.id, "name": n.name }));
    }
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({ "ratified": ratified, "evidence": evidence }),
        "loom status",
        if targets.len() == 1 {
            format!("ratified '{}'", targets[0].name)
        } else {
            format!("ratified {} intent(s)", targets.len())
        },
    )?;
    Ok(())
}

fn intent_add(graph: Option<&Path>, args: IntentAddArgs, json: bool) -> Result<()> {
    let store = open(graph)?;
    let node = create_intent(&store, &args)?;
    let visibility = store.get_facet(&node.id, TargetKind::Node, "visibility")?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": node_json(&node),
            "level": args.level,
            "visibility": visibility,
            "layer": args.layer,
            "aspect": args.aspect,
            "allow_symbol_name": args.allow_symbol_name,
        }),
        "loom status",
        format!(
            "added intent '{}' [{}]",
            node.name,
            crate::model::short(&node.id)
        ),
    )?;
    Ok(())
}

fn intent_show(graph: Option<&Path>, key: String, json: bool) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    let level = store.get_facet(&n.id, TargetKind::Node, "level")?;
    let visibility = store.get_facet(&n.id, TargetKind::Node, "visibility")?;
    let layer = store.get_facet(&n.id, TargetKind::Node, "layer")?;
    let aspect = store.get_facet(&n.id, TargetKind::Node, "aspect")?;
    let origin = store.get_facet(&n.id, TargetKind::Node, "origin")?;
    let ratification = store
        .ratification(&n.id)
        .map(Some)?
        .unwrap_or_else(|| "unratified".into());
    let ratified_by = store.get_facet(&n.id, TargetKind::Node, "ratified_by")?;
    let ratified_at = store.get_facet(&n.id, TargetKind::Node, "ratified_at")?;
    let tags = store.tags_of(&n.id, TargetKind::Node)?;

    if json {
        let mut intent = node_json(&n);
        intent["level"] = serde_json::json!(level);
        intent["visibility"] = serde_json::json!(visibility);
        intent["layer"] = serde_json::json!(layer);
        intent["aspect"] = serde_json::json!(aspect);
        intent["origin"] = serde_json::json!(origin);
        intent["ratification"] = serde_json::json!(ratification);
        intent["ratified_by"] = serde_json::json!(ratified_by);
        intent["ratified_at"] = serde_json::json!(ratified_at);
        intent["tags"] = serde_json::json!(tags);
        println!("{}", serde_json::to_string_pretty(&intent)?);
        return Ok(());
    }

    println!("{} [{}]", n.name, n.id);
    println!("  lifecycle: {}", n.status);
    if !n.description.is_empty() {
        println!("  description: {}", n.description);
    }
    if let Some(level) = level {
        println!("  level: {level}");
    }
    if let Some(vis) = visibility {
        println!("  visibility: {vis}");
    }
    if let Some(layer) = layer {
        println!("  layer: {layer}");
    }
    if let Some(aspect) = aspect {
        println!("  aspect: {aspect}");
    }
    println!("  origin: {}", origin.unwrap_or_else(|| "unknown".into()));
    println!("  ratification: {ratification}");
    if let Some(by) = ratified_by {
        println!("  ratified_by: {by}");
    }
    if let Some(at) = ratified_at {
        println!("  ratified_at: {at}");
    }
    if !tags.is_empty() {
        println!("  tags: {}", tags.join(", "));
    }
    Ok(())
}

/// Deliberately close a completeness axis for this intent. The waiver is an
/// asserted facet (`waiver:<axis>` = reason) plus a decision note, and it
/// re-opens automatically when the intent is redefined — a waiver outliving
/// the meaning it waived would be a silent lie.
fn intent_waive(
    graph: Option<&Path>,
    key: String,
    axis: String,
    reason: String,
    json: bool,
) -> Result<()> {
    crate::completeness::check_axis(&axis)?;
    if axis == "questions" {
        bail!(
            "the questions axis is never waivable: answer the question or withdraw it \
             (loom inbox mark <id> rejected --reason '…')"
        );
    }
    if reason.trim().is_empty() {
        bail!("a waiver needs a substantive --reason");
    }
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    store.set_facet(
        &n.id,
        TargetKind::Node,
        &format!("waiver:{axis}"),
        &reason,
        TruthClass::Asserted,
    )?;
    store.add_note(&n.id, "decision", &format!("waived {axis}: {reason}"))?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": node_json(&n),
            "waived_axis": axis,
            "reason": reason,
        }),
        "loom status",
        format!("waived {axis} for '{}'", n.name),
    )?;
    Ok(())
}

fn intent_reactivate(graph: Option<&Path>, key: String, reason: String, json: bool) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    if n.status != "deprecated" {
        bail!(
            "intent '{}' is not retired (status: {}) — nothing to reactivate",
            n.name,
            n.status
        );
    }
    // loom-stability-exempt: reactivates a retired intent
    store.set_node_status(&n.id, "planned")?;
    store.add_note(&n.id, "transition", &format!("reactivated: {reason}"))?;
    let mut intent = node_json(&n);
    intent["status"] = serde_json::json!("planned");
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": intent,
            "reason": reason,
        }),
        "loom status",
        format!("reactivated intent '{}' → planned", n.name),
    )?;
    Ok(())
}

fn intent_list(graph: Option<&Path>, limit: usize, offset: usize, json: bool) -> Result<()> {
    let store = open(graph)?;
    let intents = store.list_nodes_page(Some(NodeType::Intent), limit, offset)?;
    let total = store.count_nodes(Some(NodeType::Intent))?;
    if json {
        let rows: Vec<_> = intents
            .iter()
            .map(|n| {
                serde_json::json!({
                    "id": n.id,
                    "name": n.name,
                    "status": n.status,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&super::pagination_envelope(&rows, offset, limit, total))?
        );
        return Ok(());
    }
    if intents.is_empty() && offset == 0 {
        println!("no intents");
    }
    for n in &intents {
        println!(
            "{:<12} {} [{}]",
            n.status,
            n.name,
            crate::model::short(&n.id)
        );
    }
    if let Some(footer) = super::page_footer(intents.len(), offset, total) {
        println!("{footer}");
    }
    Ok(())
}

fn intent_update(graph: Option<&Path>, args: IntentUpdateArgs, json: bool) -> Result<()> {
    let IntentUpdateArgs {
        key,
        description,
        new_name,
        level,
        visibility,
        aspect,
        lifecycle,
        rectify,
        reason,
        reword,
    } = args;
    if description.is_none()
        && new_name.is_none()
        && level.is_none()
        && visibility.is_none()
        && aspect.is_none()
        && lifecycle.is_none()
        && rectify.is_none()
    {
        bail!(
            "nothing to update — pass --description, --name, --level, --visibility, \
             --aspect, --lifecycle and/or --rectify"
        );
    }
    if reason.trim().is_empty() {
        bail!("intent update needs substantive --reason");
    }
    if let Some(l) = &level {
        check_level(l)?;
    }
    if let Some(v) = &visibility {
        check_visibility(v)?;
    }
    if let Some(a) = &aspect {
        check_aspect(a)?;
    }
    if let Some(lc) = &lifecycle {
        check_lifecycle(lc, false)?;
    }
    if let Some(r) = &rectify {
        if r != "escalated" && r != "clear" {
            bail!("unknown --rectify '{r}' (use escalated|clear)");
        }
    }
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    require_lane(&store, crate::registry::OwnerRole::Builder)?;
    let mut parts: Vec<String> = Vec::new();
    // Rename first: a label change, never a ripple — the description stays
    // the behavioral criterion.
    if let Some(name) = &new_name {
        if crate::commands::looks_like_symbol(name) {
            bail!(
                "new name '{name}' looks like a code symbol — intents are behaviors; \
                 symbols belong on implements-edge locators"
            );
        }
        store.update_node(&n.id, Some(name), None, None)?;
        store.add_note(
            &n.id,
            "decision",
            &format!("renamed from '{}': {reason}", n.name),
        )?;
        parts.push(format!("renamed from '{}'", n.name));
    }
    // Attribute corrections: asserted facets, never a ripple.
    if let Some(l) = &level {
        store.set_facet(&n.id, TargetKind::Node, "level", l, TruthClass::Asserted)?;
        parts.push(format!("level={l}"));
    }
    if let Some(v) = &visibility {
        store.set_facet(
            &n.id,
            TargetKind::Node,
            "visibility",
            v,
            TruthClass::Asserted,
        )?;
        parts.push(format!("visibility={v}"));
    }
    if let Some(a) = &aspect {
        store.set_facet(&n.id, TargetKind::Node, "aspect", a, TruthClass::Asserted)?;
        parts.push(format!("aspect={a}"));
    }
    // Lifecycle: the prescriptive state moves; recorded, never a ripple.
    if let Some(lc) = &lifecycle {
        store.update_node(&n.id, None, None, Some(lc))?;
        store.add_note(&n.id, "decision", &format!("lifecycle {lc}: {reason}"))?;
        parts.push(format!("lifecycle={lc}"));
    }
    if let Some(r) = &rectify {
        match r.as_str() {
            "escalated" => {
                store.set_facet(
                    &n.id,
                    TargetKind::Node,
                    crate::divergence::RECTIFY_FACET,
                    crate::divergence::RECTIFY_ESCALATED,
                    TruthClass::Asserted,
                )?;
                store.add_note(
                    &n.id,
                    "decision",
                    &format!("rectify escalated to human ratify: {reason}"),
                )?;
                parts.push("rectify=escalated".into());
            }
            "clear" => {
                let duplicate_pairs =
                    crate::divergence::clear_duplicate_pairs(&store, &n.id, &reason)?;
                if duplicate_pairs > 0 {
                    parts.push(format!(
                        "rectify=clear ({duplicate_pairs} duplicate pair decision{})",
                        if duplicate_pairs == 1 { "" } else { "s" }
                    ));
                } else {
                    store.clear_facet(&n.id, TargetKind::Node, crate::divergence::RECTIFY_FACET)?;
                    store.add_note(
                        &n.id,
                        "decision",
                        &format!("rectify escalation cleared: {reason}"),
                    )?;
                    parts.push("rectify=clear".into());
                }
            }
            _ => unreachable!("validated above"),
        }
    }
    // Description last: the ONLY ripple source. Redefinition re-opens settled
    // dependents; --reword keeps the concept and ripples nothing.
    let mut reopened = 0usize;
    if let Some(description) = &description {
        if reword {
            store.update_node(&n.id, None, Some(description), None)?;
            store.add_note(&n.id, "decision", &format!("reworded: {reason}"))?;
            parts.push("reworded (no ripple)".into());
        } else {
            // redefine_intent also stales a ratified intent's ratification to
            // needs_reconfirmation — wantedness rots with meaning.
            reopened = store.redefine_intent(&n.id, description)?;
            store.add_note(&n.id, "decision", &format!("redefined: {reason}"))?;
            parts.push(format!("redefined — {reopened} edge(s) re-opened"));
        }
    }
    let display_name = new_name.as_deref().unwrap_or(n.name.as_str());
    let next_step = if lifecycle.as_deref() == Some("implemented") {
        "loom sync"
    } else {
        "loom status"
    };
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": {
                "id": n.id,
                "name": display_name,
                "previous_name": n.name,
                "description": description,
                "level": level,
                "visibility": visibility,
                "aspect": aspect,
                "lifecycle": lifecycle,
                "status": lifecycle.as_deref().unwrap_or(&n.status),
            },
            "reword": reword,
            "reopened_edges": reopened,
            "reason": reason,
        }),
        next_step,
        format!("updated '{display_name}': {}", parts.join(", ")),
    )?;
    Ok(())
}

struct IntentUpdateArgs {
    key: String,
    description: Option<String>,
    new_name: Option<String>,
    level: Option<String>,
    visibility: Option<String>,
    aspect: Option<String>,
    lifecycle: Option<String>,
    rectify: Option<String>,
    reason: String,
    reword: bool,
}
fn intent_remove(graph: Option<&Path>, key: String, reason: String, json: bool) -> Result<()> {
    if reason.trim().is_empty() {
        bail!("intent remove needs substantive --reason");
    }
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    require_lane(&store, crate::registry::OwnerRole::Builder)?;
    let children = store.edges_with(Some(EdgeKind::Hierarchy), Some(&n.id), None)?;
    if !children.is_empty() {
        bail!(
            "intent '{}' has {} hierarchy child edge(s); retire it or re-parent/remove the children first",
            n.name,
            children.len()
        );
    }
    store.delete_node(&n.id)?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "removed": true,
            "intent": node_json(&n),
            "reason": reason,
        }),
        "loom status",
        format!("removed mistaken intent '{}'", n.name),
    )?;
    Ok(())
}

fn intent_retire(
    graph: Option<&Path>,
    key: String,
    reason: String,
    replaced_by: Option<String>,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    let rb = match replaced_by.as_deref() {
        Some(r) => Some(store.resolve_node(r, Some(NodeType::Intent))?.id),
        None => None,
    };
    store.retire_intent(&n.id, &reason, rb.as_deref())?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": {
                "id": n.id,
                "name": n.name,
                "status": "deprecated",
            },
            "reason": reason,
            "replaced_by": rb,
        }),
        "loom status",
        format!("retired '{}'", n.name),
    )?;
    Ok(())
}

fn intent_confirm(graph: Option<&Path>, key: String, json: bool) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    store.add_note(&n.id, "confirm", "meaning re-affirmed")?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": node_json(&n),
            "confirmed": true,
        }),
        "loom status",
        format!("confirmed '{}'", n.name),
    )?;
    Ok(())
}

/// Record the builder's post-change semantic assessment. This is deliberately
/// not ratification: a changed criterion only stales wantedness and hands the
/// decision back to the terminal-gated human ratify queue (INV-8).
fn intent_impact(
    graph: Option<&Path>,
    key: String,
    classification: String,
    evidence: String,
    json: bool,
) -> Result<()> {
    if !matches!(
        classification.as_str(),
        "preserved" | "changed_within_intent" | "criterion_changed"
    ) {
        bail!(
            "impact classification must be preserved, changed_within_intent, or criterion_changed"
        );
    }
    if crate::model::is_placeholder(&evidence) {
        bail!("intent impact requires substantive --evidence");
    }
    let store = open(graph)?;
    require_lane(&store, crate::registry::OwnerRole::Builder)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    store.set_facet(
        &n.id,
        TargetKind::Node,
        "semantic_impact",
        &classification,
        TruthClass::Asserted,
    )?;
    store.set_facet(
        &n.id,
        TargetKind::Node,
        "semantic_impact_evidence",
        &evidence,
        TruthClass::Asserted,
    )?;
    store.add_note(
        &n.id,
        "decision",
        &format!("semantic impact {classification}: {evidence}"),
    )?;
    let mut reconfirmation_required = false;
    if classification == "criterion_changed"
        && store.ratification(&n.id).map(Some)?.as_deref() == Some("ratified")
    {
        store.assert_fact(
            loom_assertion(&n.id, "needs_reconfirmation")
                .criterion("criterion changed since ratification")
                .cited(vec![crate::evidence::CitedEvidence::Claim(
                    evidence.trim().to_string(),
                )]),
        )?;
        store.add_note(
            &n.id,
            "ratify",
            "ratification staled by semantic impact assessment",
        )?;
        reconfirmation_required = true;
    }
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": node_json(&n),
            "classification": classification,
            "evidence": evidence,
            "reconfirmation_required": reconfirmation_required,
        }),
        if reconfirmation_required {
            "loom next --mode ratify"
        } else {
            "loom status"
        },
        format!(
            "semantic impact for '{}' recorded as {classification}",
            n.name
        ),
    )?;
    Ok(())
}

/// Tag an intent with a registered vocab term, through the one gate the CLI
/// enforces — the term must already be in the vocab registry. Returns the
/// resolved intent. Shared by `loom intent tag add` and the `loom apply` tags
/// batch so the batch can never accept what the per-verb command rejects.
pub(crate) fn tag_intent(store: &Store, key: &str, term: &str) -> Result<crate::model::Node> {
    let n = store.resolve_node(key, Some(NodeType::Intent))?;
    if !store.vocab_has(term)? {
        bail!("'{term}' is not a registered vocab term; add it with `loom vocab add`");
    }
    store.set_tag(&n.id, TargetKind::Node, term)?;
    Ok(n)
}

fn intent_tag(graph: Option<&Path>, cmd: IntentTagCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        IntentTagCmd::Add { key, term } => {
            let n = tag_intent(&store, &key, &term)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "intent": node_json(&n),
                    "action": "add",
                    "term": term,
                }),
                "loom status",
                format!("tagged '{}' with '{term}'", n.name),
            )?;
        }
        IntentTagCmd::Remove { key, term } => {
            let n = store.resolve_node(&key, Some(NodeType::Intent))?;
            store.remove_tag(&n.id, TargetKind::Node, &term)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "intent": node_json(&n),
                    "action": "remove",
                    "term": term,
                }),
                "loom status",
                format!("untagged '{}' '{term}'", n.name),
            )?;
        }
    }
    Ok(())
}

/// Render what stands on a behavior.
///
/// The unproven ones are the point: a dependent with no passing proof is where
/// a change to the queried behavior would break something silently, so they are
/// called out rather than left for the reader to spot in a list.
fn intent_dependents(graph: Option<&Path>, key: &str, depth: usize, json: bool) -> Result<()> {
    let store = crate::commands::open_read(graph)?;
    let target = store.resolve_node(key, Some(NodeType::Intent))?;
    let found = store.dependents(&target.id, depth)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "intent": { "id": target.id, "name": target.name },
                "depth": depth,
                "dependents": found,
                "unproven": found.iter().filter(|d| !d.proven).count(),
            }))?
        );
        return Ok(());
    }

    if found.is_empty() {
        println!(
            "nothing stands on '{}' within {depth} hop(s) — changing it reaches no other behavior",
            target.name
        );
        return Ok(());
    }
    println!("{} behavior(s) stand on '{}':", found.len(), target.name);
    for d in &found {
        println!(
            "  {:>2} hop{}  {:<9} {}",
            d.hops,
            if d.hops == 1 { " " } else { "s" },
            if d.proven { "proven" } else { "UNPROVEN" },
            d.intent.name
        );
    }
    let unproven = found.iter().filter(|d| !d.proven).count();
    if unproven > 0 {
        println!(
            "\n{unproven} of them have no passing proof — a change here would not be caught there."
        );
    }
    Ok(())
}
