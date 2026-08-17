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
///
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

/// Is this intent ratified? Absence = unratified (fail closed: wantedness
/// is never presumed).
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
