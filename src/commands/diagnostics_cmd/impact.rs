use super::*;

/// The full impact answer as JSON: callers (deduped, nearest-first), the intents
/// those callers put at risk with their proof strength, and the exact/heuristic
/// split. Shared verbatim by `loom impact` and the `loom_impact` MCP tool so the
/// two surfaces cannot report different numbers.
pub(crate) fn impact_report(
    store: &Store,
    target: &str,
    depth: usize,
) -> Result<serde_json::Value> {
    let cg = crate::callgraph::build(store)?;
    let anchor = if crate::locator::is_anchor_locator(target) {
        Some(crate::locator::resolve_anchor(store, target)?)
    } else {
        None
    };

    // A file target means "everything this file defines". An anchor means its
    // currently attached callable declaration; configuration entries still
    // return exact navigation and graph ownership without inventing a call.
    let symbols: Vec<String> = match &anchor {
        Some(anchor) => anchor.callable_symbol.iter().cloned().collect(),
        None => match store.codefiles()?.into_iter().find(|c| c.name == target) {
            Some(cf) => store
                .get_facet(
                    &cf.id,
                    TargetKind::Node,
                    crate::seed::SYMBOL_FINGERPRINTS_KEY,
                )?
                .and_then(|j| {
                    serde_json::from_str::<std::collections::BTreeMap<String, String>>(&j).ok()
                })
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default(),
            None => vec![target.to_string()],
        },
    };

    let impact = cg.impact_of(&symbols, depth);
    let callers = impact.callers;

    // Which intents own the reached files, and how well each is proven.
    let codefiles_by_name: std::collections::HashMap<String, crate::model::Node> = store
        .codefiles()?
        .into_iter()
        .map(|f| (f.name.clone(), f))
        .collect();
    let mut at_risk: Vec<serde_json::Value> = Vec::new();
    let mut seen_intents = std::collections::BTreeSet::new();
    let mut reached_files: std::collections::BTreeSet<&str> =
        callers.iter().map(|caller| caller.file.as_str()).collect();
    if let Some(anchor) = &anchor {
        reached_files.insert(anchor.file.as_str());
    }
    for file in reached_files {
        let Some(cf) = codefiles_by_name.get(file) else {
            continue;
        };
        for e in store.realizing_implementers(&cf.id)? {
            let Some(intent) = store.get_node(&e.from_id)? else {
                continue;
            };
            if !seen_intents.insert(intent.id.clone()) {
                continue;
            }
            let proofs = store.edges_with(Some(EdgeKind::Validates), None, Some(&intent.id))?;
            let passing = proofs
                .iter()
                .filter(|p| p.status == InspectionStatus::Passing)
                .count();
            at_risk.push(serde_json::json!({
                "intent": intent.name,
                "id": crate::model::short(&intent.id),
                "proofs": proofs.len(),
                "passing": passing,
            }));
        }
    }

    let exact = callers
        .iter()
        .filter(|c| c.resolution == crate::callgraph::Resolution::Exact)
        .count();
    Ok(serde_json::json!({
        "target": target,
        "anchor": anchor.as_ref().map(|anchor| serde_json::json!({
            "id": anchor.id,
            "locator": anchor.locator,
            "marker": anchor.marker,
            "codefile": anchor.file,
            "entry": {
                "kind": anchor.entry_kind,
                "name": anchor.entry_name,
                "line_start": anchor.line_start,
                "line_end": anchor.line_end,
                "callable_symbol": anchor.callable_symbol,
            }
        })),
        "depth": depth,
        "callers": callers,
        "intents_at_risk": at_risk,
        "resolution": { "exact": exact, "heuristic": callers.len() - exact },
        "unresolved_calls": impact.unresolved_calls,
    }))
}

/// `loom impact <symbol|file>` — what a change here could reach.
///
/// The one question an agent cannot cheaply reconstruct per session, and the
/// one loom is best placed to answer: it already holds the symbols, the calls,
/// and which intents own the code. The answer names the intents at risk and the
/// weakest proof standing behind them, because "42 callers" is trivia unless it
/// tells you what could silently break.
pub(crate) fn impact_cmd(
    graph: Option<&Path>,
    target: &str,
    depth: usize,
    json: bool,
) -> Result<()> {
    let store = open_read(graph)?;
    let report = impact_report(&store, target, depth)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let callers = report["callers"].as_array().cloned().unwrap_or_default();
    let at_risk = report["intents_at_risk"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let exact = report["resolution"]["exact"].as_u64().unwrap_or(0) as usize;
    let heuristic = report["resolution"]["heuristic"].as_u64().unwrap_or(0) as usize;
    let unresolved = report["unresolved_calls"].as_u64().unwrap_or(0);

    println!("impact of {target} (depth {depth}):");
    if let Some(anchor) = report["anchor"].as_object() {
        let entry = &anchor["entry"];
        println!(
            "  anchor resolves to {}:{}-{} ({} {})",
            anchor["codefile"].as_str().unwrap_or(""),
            entry["line_start"].as_u64().unwrap_or(0),
            entry["line_end"].as_u64().unwrap_or(0),
            entry["kind"].as_str().unwrap_or("entry"),
            entry["name"].as_str().unwrap_or("")
        );
    }
    if callers.is_empty() {
        println!("  nothing in this graph calls it — a leaf, or a seam loom cannot see");
    }
    for c in callers.iter().take(15) {
        let mark = if c["resolution"].as_str() == Some("heuristic") {
            "  [heuristic]"
        } else {
            ""
        };
        println!(
            "  {}x  {}::{}{}",
            c["hops"].as_u64().unwrap_or(0),
            c["file"].as_str().unwrap_or(""),
            c["symbol"].as_str().unwrap_or(""),
            mark
        );
    }
    if callers.len() > 15 {
        println!("  … and {} more", callers.len() - 15);
    }
    if !at_risk.is_empty() {
        println!("  intents at risk:");
        for i in &at_risk {
            println!(
                "    {} [{}] — {} proof(s), {} passing",
                i["intent"].as_str().unwrap_or(""),
                i["id"].as_str().unwrap_or(""),
                i["proofs"],
                i["passing"]
            );
        }
    }
    // Exact and heuristic are never blended: a blast radius that mixes them
    // tells you nothing you can act on.
    println!(
        "  resolution: {exact} exact, {heuristic} heuristic, {unresolved} call(s) unresolved (std/third-party)"
    );
    Ok(())
}

/// Run the self-fabrication detector over this graph's own record.
///
/// Exits non-zero when anything is found, like `doctor` — an audit whose
/// findings are advisory is a scoreboard, and the whole point is that loom is
/// willing to fail its own check.
pub(crate) fn audit_cmd(graph: Option<&Path>, efficacy: bool, json: bool) -> Result<()> {
    let store = open_read(graph)?;
    if efficacy {
        let e = crate::audit::efficacy(&store)?;
        if json {
            println!("{}", serde_json::to_string_pretty(&e)?);
        } else if e.served == 0 {
            println!("no packets served yet — efficacy is unmeasured, not zero");
        } else {
            println!(
                "{} of {} served packets were followed by re-checkable work ({:.0}%)",
                e.converted,
                e.served,
                e.ratio * 100.0
            );
            for (kind, (served, converted)) in &e.by_kind {
                println!("  {kind}: {converted}/{served}");
            }
            // A ratio off a handful of packets is a coincidence with a
            // percent sign. Say so rather than letting it be quoted.
            if e.served < crate::audit::EFFICACY_MIN_SAMPLE {
                println!(
                    "  too few packets to mean anything yet ({} of {} needed)",
                    e.served,
                    crate::audit::EFFICACY_MIN_SAMPLE
                );
            }
            println!("  statistical — reported, never gating (INV-3)");
        }
        return Ok(());
    }
    let findings = crate::audit::run(&store)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else if findings.is_empty() {
        println!("audit clean — every settled claim is anchored and every judgment is journaled");
    } else {
        for f in &findings {
            println!("[{}] {}", f.kind, f.detail);
            println!("  → {}", f.remedy);
        }
        println!("\n{} audit finding(s)", findings.len());
    }
    if findings.is_empty() {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

/// Rank what is worth strengthening next.
pub(crate) fn deepen_cmd(graph: Option<&Path>, limit: usize, json: bool) -> Result<()> {
    let store = open_read(graph)?;
    let ranked = crate::risk::rank(&store)?;
    let shown: Vec<_> = ranked.iter().take(limit).collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&shown)?);
        return Ok(());
    }
    if shown.is_empty() {
        println!("nothing to deepen yet — ground and prove some behavior first");
        return Ok(());
    }
    println!("what to strengthen next ({} candidate(s)):\n", ranked.len());
    for (i, c) in shown.iter().enumerate() {
        println!("{}. {} [{}]", i + 1, c.intent_name, c.proof_strength);
        println!("   {}", c.why);
        println!(
            "   score {:.3} = blast {:.2} x proof gap x age {}d",
            c.score, c.blast_radius, c.evidence_age_days
        );
        println!("   next: {}", c.next_move.as_str());
    }
    Ok(())
}

/// Observe the working tree and propose what the graph is missing.
pub(crate) fn absorb_cmd(graph: Option<&Path>, confirm: bool, json: bool) -> Result<()> {
    let store = open(graph)?;
    let root = store.root().to_path_buf();
    let items = crate::absorb::observe(&store, &root)?;
    if items.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "items": [],
                    "proposal": null,
                    "persisted_proposal": null,
                })
            );
        } else {
            println!("nothing to absorb — the graph already reflects the tree");
        }
        return Ok(());
    }
    let proposal = crate::absorb::record(&store, &items)?;
    let persisted_proposal = node_json(&proposal);

    // `--confirm` adopts only what needs nothing from a human. The behavioral
    // criterion is the one thing loom cannot derive, so an item that wants one
    // is always left for a person.
    let ready: Vec<&crate::absorb::Item> = items.iter().filter(|i| i.needs.is_empty()).collect();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "proposal": proposal.id.clone(),
                "persisted_proposal": persisted_proposal,
                "items": items,
                "ready": ready.len(),
                "needs_you": items.len() - ready.len(),
            }))?
        );
        return Ok(());
    }
    println!(
        "absorbed {} observation(s) into proposal {} ({} ready, {} need you)",
        items.len(),
        crate::model::short(&proposal.id),
        ready.len(),
        items.len() - ready.len()
    );
    for (i, item) in items.iter().enumerate() {
        println!("  {}. [{}] {}", i + 1, item.kind.as_str(), item.text);
        for need in &item.needs {
            println!("     needs: {need}");
        }
    }
    if confirm {
        println!(
            "\nadopt with: loom proposal item adopt {} <n>",
            crate::model::short(&proposal.id)
        );
    }
    Ok(())
}
