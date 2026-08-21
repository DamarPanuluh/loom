//! Discovery command family — find, explain, detect, schema.
//!
//! Plane: read-only search and repo detection. `keyword_hits` is shared with
//! the door landing menu in `capture_cmd`.

use super::*;
use crate::grammar::{
    ACTIVE_LIFECYCLES, ASPECTS, LEVELS, PLACEHOLDER_TOKENS, RATIFICATION_STATES, VISIBILITIES,
};

/// Keyword scoring shared by `loom find` and the door's landing menu: score
/// nodes of the given kinds against the query terms, best first, capped at
/// `limit`. Returns `(score, kind, name, id)` rows.
pub(crate) fn keyword_hits(
    store: &Store,
    query: &str,
    kinds: &[NodeType],
    limit: usize,
) -> Result<Vec<(usize, String, String, String)>> {
    let q = query_terms(query);
    let score = |hay: &str| -> usize {
        let h = hay.to_lowercase();
        q.iter().filter(|t| h.contains(t.as_str())).count()
    };
    let mut hits: Vec<(usize, String, String, String)> = Vec::new();
    for nt in kinds {
        for n in store.list_nodes(Some(*nt), usize::MAX)? {
            if n.status == "deprecated" {
                continue;
            }
            let s = score(&n.name) * 2 + score(&n.description);
            if s > 0 {
                hits.push((s, nt.as_str().to_string(), n.name.clone(), n.id.clone()));
            }
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(&b.2)));
    hits.truncate(limit);
    Ok(hits)
}

/// Allowed `--where` facet keys for `loom find` (minimal property allowlist).
pub(crate) const FIND_WHERE_KEYS: &[&str] =
    &["visibility", "level", "aspect", "origin", "ratification"];

pub(crate) fn find_cmd(
    graph: Option<&Path>,
    query: &str,
    limit: usize,
    exact: bool,
    tag: Option<&str>,
    where_facets: &[String],
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let kinds = [
        NodeType::Journey,
        NodeType::Intent,
        NodeType::CodeFile,
        NodeType::QualityRule,
    ];
    let filter_ids = resolve_find_filters(&store, tag, where_facets)?;
    let has_filters = tag.is_some() || !where_facets.is_empty();
    let filter_desc = {
        let mut parts: Vec<String> = Vec::new();
        if let Some(t) = tag {
            parts.push(format!("tag '{t}'"));
        }
        parts.extend(where_facets.iter().cloned());
        parts.join(" and ")
    };
    let q = query.trim();

    if exact {
        if q.is_empty() {
            bail!("--exact requires a non-empty query");
        }
        return find_exact(&store, q, &kinds, filter_ids.as_ref(), json);
    }

    let limited = if q.is_empty() {
        if !has_filters {
            bail!("pass a query and/or --tag / --where");
        }
        // Facet/tag-only: list matching Intent/CodeFile/QualityRule nodes.
        let mut rows = Vec::new();
        let Some(ids) = filter_ids else {
            bail!("internal: a tag/facet filter was requested but none resolved");
        };
        for id in ids {
            if let Some(n) = store.get_node(&id)? {
                if kinds.contains(&n.node_type) {
                    rows.push((100usize, n.node_type.as_str().to_string(), n.name, n.id));
                }
            }
            if rows.len() >= limit {
                break;
            }
        }
        rows
    } else {
        let mut hits = keyword_hits(&store, q, &kinds, limit.saturating_mul(4))?;
        if let Some(ids) = &filter_ids {
            hits.retain(|(_, _, _, id)| ids.contains(id));
        }
        hits.truncate(limit);
        hits
    };

    print_find_hits(&store, q, &limited, false, &filter_desc, json)
}

fn resolve_find_filters(
    store: &Store,
    tag: Option<&str>,
    where_facets: &[String],
) -> Result<Option<std::collections::BTreeSet<String>>> {
    if tag.is_none() && where_facets.is_empty() {
        return Ok(None);
    }
    let mut sets: Vec<std::collections::BTreeSet<String>> = Vec::new();
    if let Some(term) = tag {
        sets.push(store.nodes_with_tag(term)?.into_iter().collect());
    }
    for spec in where_facets {
        let (key, value) = parse_where_spec(spec)?;
        if !FIND_WHERE_KEYS.contains(&key.as_str()) {
            bail!(
                "unknown --where key '{key}' (allowed: {})",
                FIND_WHERE_KEYS.join(", ")
            );
        }
        let ids = if key == "ratification" {
            // Ratification is a FACT, not a facet — v3 moved it, and `set_facet`
            // refuses the key outright. Reading the facet table here meant every
            // state except the `unratified` special case matched nothing, and
            // silently: `--where ratification=ratified` returned an empty list
            // on a graph full of ratified intents.
            //
            // Absence still reads as unratified (INV-8), which is why that state
            // is answered from the shared predicate rather than the fact table.
            if value == "unratified" {
                crate::workitem::unratified_intents(store)?
                    .into_iter()
                    .map(|intent| intent.id)
                    .collect()
            } else {
                let mut ids = std::collections::BTreeSet::new();
                for intent in store.list_nodes(Some(crate::model::NodeType::Intent), usize::MAX)? {
                    if store.ratification(&intent.id)? == value {
                        ids.insert(intent.id);
                    }
                }
                ids
            }
        } else {
            store.nodes_where_facet(&key, &value)?.into_iter().collect()
        };
        sets.push(ids);
    }
    let mut iter = sets.into_iter();
    let mut acc = iter.next().unwrap_or_default();
    for s in iter {
        acc = acc.intersection(&s).cloned().collect();
    }
    Ok(Some(acc))
}

fn parse_where_spec(spec: &str) -> Result<(String, String)> {
    let (k, v) = spec
        .split_once('=')
        .ok_or_else(|| anyhow!("--where expects KEY=VALUE, got '{spec}'"))?;
    let key = k.trim().to_string();
    let value = v.trim().to_string();
    if key.is_empty() || value.is_empty() {
        bail!("--where expects non-empty KEY=VALUE, got '{spec}'");
    }
    Ok((key, value))
}

fn find_exact(
    store: &Store,
    query: &str,
    kinds: &[NodeType],
    filter: Option<&std::collections::BTreeSet<String>>,
    json: bool,
) -> Result<()> {
    // Behavior identity is Intent. A CodeFile, Journey, or QualityRule may
    // share a name without making two behaviors; only a second Intent is
    // ambiguous. Search Intent first, and only if none match fall back to
    // the other exact-addressable kinds.
    let mut limited = collect_exact_matches(store, query, &[NodeType::Intent], filter)?;
    if limited.is_empty() {
        limited = collect_exact_matches(store, query, kinds, filter)?;
    }
    if limited.len() > 1 {
        let candidates = limited
            .iter()
            .map(|(_, kind, name, id)| format!("{kind} [{}] {name}", crate::model::short(id)))
            .collect::<Vec<_>>()
            .join("; ");
        bail!(
            "ambiguous exact match for '{query}': {} active nodes are named exactly this \
             — narrow with --tag/--where: {candidates}",
            limited.len()
        );
    }
    print_find_hits(store, query, &limited, true, "", json)
}

fn collect_exact_matches(
    store: &Store,
    query: &str,
    kinds: &[NodeType],
    filter: Option<&std::collections::BTreeSet<String>>,
) -> Result<Vec<(usize, String, String, String)>> {
    let mut limited = Vec::new();
    for kind in kinds {
        for n in store.list_nodes(Some(*kind), usize::MAX)? {
            if n.status != "deprecated"
                && n.name.eq_ignore_ascii_case(query)
                && filter.is_none_or(|ids| ids.contains(&n.id))
            {
                limited.push((100usize, kind.as_str().to_string(), n.name, n.id));
            }
        }
    }
    Ok(limited)
}

fn print_find_hits(
    store: &Store,
    query: &str,
    limited: &[(usize, String, String, String)],
    exact_only: bool,
    // What narrowed this search, when the query itself was empty.
    filter_desc: &str,
    json: bool,
) -> Result<()> {
    let rows = project_find_hits(store, limited)?;
    if json {
        let mut value = serde_json::to_value(&rows)?;
        if exact_only {
            for row in value
                .as_array_mut()
                .expect("FindHit serializes as an array")
            {
                row.as_object_mut()
                    .expect("FindHit serializes as an object")
                    .insert("exact".into(), serde_json::Value::Bool(true));
            }
        }
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        if rows.is_empty() {
            if exact_only {
                println!(
                    "no exact match for '{query}' — nothing named exactly this exists \
                     (drop --exact for fuzzy matches)"
                );
            } else if query.trim().is_empty() && !filter_desc.is_empty() {
                // Filter-only search. Reporting "no match for ''" reads as a
                // failed text search and sends the reader hunting for a typo in
                // a query they never typed; the filter is what came up empty.
                println!(
                    "nothing matches {filter_desc} — the filter is valid, nothing satisfies it"
                );
            } else {
                println!(
                    "no match for '{query}' — try `loom status` to see coverage, or it may not exist"
                );
            }
        }
        let needle = query.trim();
        for row in rows {
            let mark = if !needle.is_empty() && row.name.eq_ignore_ascii_case(needle) {
                " (exact)"
            } else {
                ""
            };
            println!(
                "{:<10} {} [{}] (score {}){mark}",
                row.kind,
                row.name,
                crate::model::short(&row.id),
                row.score
            );
            if row.kind == "intent" {
                if !row.groundings.iter().any(|g| g.role == "realizes") {
                    println!("             ↳ (no realizing grounding yet)");
                }
                for grounding in row.groundings {
                    let at = if grounding.locator.is_empty() {
                        String::new()
                    } else {
                        format!(" @ {}", grounding.locator)
                    };
                    let ev = if grounding.evidence.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", grounding.evidence)
                    };
                    println!(
                        "             ↳ [{}] {}{at} [{}]{ev}",
                        grounding.role, grounding.path, grounding.status
                    );
                }
            }
        }
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct FindHit {
    score: usize,
    kind: String,
    name: String,
    id: String,
    groundings: Vec<FindGrounding>,
}

#[derive(serde::Serialize)]
struct FindGrounding {
    edge_id: String,
    path: String,
    locator: String,
    role: String,
    status: String,
    evidence: String,
}

/// Build the shared semantic projection once; JSON and text are renderers of
/// the same rows, so neither can silently omit a grounding field or edge rule.
fn project_find_hits(
    store: &Store,
    limited: &[(usize, String, String, String)],
) -> Result<Vec<FindHit>> {
    let mut rows = Vec::with_capacity(limited.len());
    for (score, kind, name, id) in limited {
        let groundings = if kind == "intent" {
            project_groundings(store, id)?
        } else {
            Vec::new()
        };
        rows.push(FindHit {
            score: *score,
            kind: kind.clone(),
            name: name.clone(),
            id: id.clone(),
            groundings,
        });
    }
    Ok(rows)
}

fn project_groundings(store: &Store, intent_id: &str) -> Result<Vec<FindGrounding>> {
    let mut groundings = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Implements), Some(intent_id), None)? {
        if store.edge_superseded(&edge.id)? {
            continue;
        }
        groundings.push(FindGrounding {
            edge_id: edge.id.clone(),
            path: store
                .get_node(&edge.to_id)?
                .map(|n| n.name)
                .unwrap_or_else(|| edge.to_id.clone()),
            locator: store
                .get_facet(&edge.id, TargetKind::Edge, "locator")?
                .unwrap_or_default(),
            role: store.grounding_role(&edge.id)?.as_str().into(),
            status: edge.status.as_str().into(),
            evidence: store.verdict_prose(&edge.id)?,
        });
    }
    Ok(groundings)
}

/// Read-only neighborhood brief for an intent — not a `loom next` lane.
pub(crate) fn explain_cmd(graph: Option<&Path>, intent_key: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    let intent = store.resolve_node(intent_key, Some(NodeType::Intent))?;
    let visibility = store.get_facet(&intent.id, TargetKind::Node, "visibility")?;
    let level = store.get_facet(&intent.id, TargetKind::Node, "level")?;
    let aspect = store.get_facet(&intent.id, TargetKind::Node, "aspect")?;
    let tags = store.tags_of(&intent.id, TargetKind::Node)?;

    let grounding_rows = project_groundings(&store, &intent.id)?;
    let groundings: Vec<_> = grounding_rows
        .iter()
        .map(|grounding| {
            serde_json::json!({
                "edge_id": grounding.edge_id,
                "path": grounding.path,
                "locator": grounding.locator,
                "role": grounding.role,
                "status": grounding.status,
            })
        })
        .collect();

    let mut related = Vec::new();
    for kind in [
        EdgeKind::Relates,
        EdgeKind::Requires,
        EdgeKind::Hierarchy,
        EdgeKind::ScenarioOf,
        EdgeKind::Triggers,
        EdgeKind::Sequence,
    ] {
        for e in store.edges_with(Some(kind), Some(&intent.id), None)? {
            let other = store.get_node(&e.to_id)?;
            related.push(serde_json::json!({
                "kind": kind.as_str(),
                "direction": "from",
                "status": e.status.as_str(),
                "peer": other.map(|n| serde_json::json!({"id": n.id, "name": n.name, "status": n.status})),
            }));
        }
        for e in store.edges_with(Some(kind), None, Some(&intent.id))? {
            let other = store.get_node(&e.from_id)?;
            related.push(serde_json::json!({
                "kind": kind.as_str(),
                "direction": "to",
                "status": e.status.as_str(),
                "peer": other.map(|n| serde_json::json!({"id": n.id, "name": n.name, "status": n.status})),
            }));
        }
    }

    let mut validations = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Validates), None, Some(&intent.id))? {
        if let Some(v) = store.get_node(&e.from_id)? {
            validations.push(serde_json::json!({
                "id": v.id,
                "name": v.name,
                "status": v.status,
                "edge_status": e.status.as_str(),
            }));
        }
    }

    let scorecard = crate::completeness::scorecard(&store, &intent)?;
    let mut open_questions = Vec::new();
    for question in store.list_nodes(Some(NodeType::Question), usize::MAX)? {
        if question.status != "open"
            || store
                .edges_with(
                    Some(EdgeKind::Questions),
                    Some(&question.id),
                    Some(&intent.id),
                )?
                .is_empty()
        {
            continue;
        }
        open_questions.push(serde_json::json!({
            "id": question.id,
            "text": question.description,
        }));
    }

    let brief = serde_json::json!({
        "intent": {
            "id": intent.id,
            "name": intent.name,
            "description": intent.description,
            "lifecycle": intent.status,
            "visibility": visibility,
            "level": level,
            "aspect": aspect,
            "tags": tags,
        },
        "groundings": groundings,
        "related": related,
        "validations": validations,
        "completeness": scorecard,
        "open_questions": open_questions,
    });

    if json {
        println!("{}", serde_json::to_string_pretty(&brief)?);
    } else {
        println!("{} [{}]", intent.name, crate::model::short(&intent.id));
        println!("  lifecycle: {}", intent.status);
        if let Some(v) = &visibility {
            println!("  visibility: {v}");
        }
        if let Some(l) = &level {
            println!("  level: {l}");
        }
        if !intent.description.is_empty() {
            println!("  description: {}", intent.description);
        }
        if !tags.is_empty() {
            println!("  tags: {}", tags.join(", "));
        }
        println!("  groundings:");
        if groundings.is_empty() {
            println!("    (none)");
        } else {
            for g in &groundings {
                println!(
                    "    [{}] {} @ {} [{}]",
                    g["role"].as_str().unwrap_or(""),
                    g["path"].as_str().unwrap_or(""),
                    g["locator"].as_str().unwrap_or("-"),
                    g["status"].as_str().unwrap_or("")
                );
            }
        }
        println!("  related (1 hop): {}", related.len());
        for r in related.iter().take(12) {
            let peer = r["peer"]["name"].as_str().unwrap_or("?");
            println!(
                "    {} ({}) {} — {}",
                r["kind"].as_str().unwrap_or(""),
                r["direction"].as_str().unwrap_or(""),
                peer,
                r["status"].as_str().unwrap_or("")
            );
        }
        println!("  validations: {}", validations.len());
        for v in &validations {
            println!(
                "    {} [{}] proof={}",
                v["name"].as_str().unwrap_or(""),
                crate::model::short(v["id"].as_str().unwrap_or("")),
                v["status"].as_str().unwrap_or("")
            );
        }
        println!("  completeness open axes: {}", scorecard.open);
        if !open_questions.is_empty() {
            println!("  open questions: {}", open_questions.len());
        }
    }
    Ok(())
}

pub(crate) fn detect_cmd(graph: Option<&Path>, json: bool) -> Result<()> {
    let root = resolve_root(graph).or_else(|_| std::env::current_dir())?;
    let mut langs: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    let mut markers: Vec<&str> = Vec::new();
    for (marker, label) in [
        ("Cargo.toml", "rust"),
        ("package.json", "node"),
        ("go.mod", "go"),
        ("pyproject.toml", "python"),
        ("Dockerfile", "docker"),
    ] {
        if root.join(marker).exists() {
            markers.push(label);
        }
    }
    count_exts(&root, &mut langs, 0);
    // Recommend only packs that actually exist (crate::packs::PACKS), from
    // honest signals: a recommendation the seeder rejects is a dead end. The
    // same function feeds the compass when the quality rung is unseeded.
    let recommended = crate::packs::recommended_packs(&root);
    debug_assert!(
        recommended.iter().all(|p| crate::packs::PACKS.contains(p)),
        "detect recommended a pack that cannot be seeded"
    );
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "languages": langs,
                "project_markers": markers,
                "recommended_quality_packs": recommended,
                "available_packs": crate::packs::PACKS,
            }))?
        );
        return Ok(());
    }
    println!("detected languages:");
    for (ext, n) in &langs {
        println!("  {ext}: {n} file(s)");
    }
    println!(
        "project markers: {}",
        if markers.is_empty() {
            "none".into()
        } else {
            markers.join(", ")
        }
    );
    println!("recommended quality packs: {}", recommended.join(", "));
    println!(
        "  seed with: loom rule seed <pack>   (available: {})",
        crate::packs::PACKS.join(", ")
    );
    Ok(())
}
fn count_exts(
    dir: &Path,
    langs: &mut std::collections::BTreeMap<&'static str, usize>,
    depth: usize,
) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries {
        // Advisory language census: an unreadable entry must not abort the
        // scan, but a silent drop would undercount without a trace.
        let e = match e {
            Ok(e) => e,
            Err(error) => {
                eprintln!(
                    "warning: skipping unreadable entry in {}: {error}",
                    dir.display()
                );
                continue;
            }
        };
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            count_exts(&p, langs, depth + 1);
        } else if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
            let label = match ext {
                "rs" => "rust",
                "py" => "python",
                "go" => "go",
                "ts" | "tsx" => "typescript",
                "js" | "jsx" => "javascript",
                "sql" => "sql",
                _ => continue,
            };
            *langs.entry(label).or_insert(0) += 1;
        }
    }
}
/// The statement grammar: the well-formedness rules every graph write must
/// satisfy, served by the tool itself so an LLM driver reads them from
/// `loom schema` each session instead of from drifting prose. Lexicon
/// (vocabulary) lives in the registries; grammar (sentence rules) lives here;
/// pragmatics (how statements are used) is `loom guide` / the prompt contracts.
fn grammar_json() -> serde_json::Value {
    serde_json::json!({
        "intent_name": {
            "rule": "a behavioral phrase, falsifiable at a meaningful altitude — never a code symbol",
            "rejected": "snake_case / camelCase / Path::symbol / fn() names, unless --allow-symbol-name AND a behavioral --description are both given (override is recorded for audit)",
            "examples_good": ["payment can be captured", "an operator captures a topic through door"],
            "examples_bad": ["capture_payment", "runWithSqlite", "Store::open"],
        },
        "intent_facets": {
            "level": LEVELS,
            "lifecycle_at_add": ACTIVE_LIFECYCLES,
            "visibility": VISIBILITIES,
            "aspect": ASPECTS,
            "origin": ["human", "llm"],
            "ratification": RATIFICATION_STATES,
        },
        "authorship": {
            "mint": "any agent (solo or any llm:* lane) may add intents, edges, findings, notes",
            "ratify": "human-authorized (INV-8): an LLM may present options, recommend, wait, and record an explicit --human-decision; without that answer every llm:* direct write is rejected",
            "verdicts": "asserted verdicts need non-placeholder criterion AND evidence (INV-6); confidence < 0.7 routes to review",
            "derived": "sync alone writes derived facts; no agent re-judges a machine fact (INV-5)",
        },
        "evidence": {
            "rule": "criterion/evidence/reason fields must be substantive — whole-field placeholders are rejected at the write boundary",
            "rejected_placeholders": PLACEHOLDER_TOKENS,
            "prefer": "file:line spans, command output excerpts, utterances, source-doc refs",
        },
        "ratification_lifecycle": {
            "born_ratified": "intent minted by the human (solo agent) — the minting act is the evidence",
            "born_unratified": "intent minted by an llm:* lane — first-class in the graph, but the `wanted` rung stays unmet until a human ratifies",
            "staled": "redefining a ratified intent's description moves it to needs_reconfirmation — wantedness rots with meaning",
        },
    })
}

pub(crate) fn schema_cmd(json: bool) -> Result<()> {
    use crate::model::*;
    if json {
        let edge_kinds: Vec<_> = crate::registry::REGISTRY
            .iter()
            .map(|s| {
                serde_json::json!({
                    "kind": s.kind.as_str(),
                    "from": s.from.as_str(),
                    "to": s.to.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
                    "truth_classes": s.truth_classes.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
                    "owner": s.owner.as_str(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "node_types": NodeType::ALL.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
                "edge_kinds": edge_kinds,
                "inspection_statuses": InspectionStatus::ALL.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                "intent_lifecycle": IntentLifecycle::ALL.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
                "truth_classes": TruthClass::ALL.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
                "finding_verdicts": ["needed", "justified", "rejected", "deferred", "blocked", "duplicate", "resolved"],
                "find_where_keys": FIND_WHERE_KEYS,
                "grammar": grammar_json(),
                "apply_batch": crate::commands::batch_schema(),
            }))?
        );
        return Ok(());
    }
    println!("node types:");
    for t in NodeType::ALL {
        println!("  {}", t.as_str());
    }
    println!("edge kinds (from registry):");
    for s in crate::registry::REGISTRY {
        let tcs: Vec<&str> = s.truth_classes.iter().map(|t| t.as_str()).collect();
        println!(
            "  {:<12} {} → {}  [{}] owner={}",
            s.kind.as_str(),
            s.from.as_str(),
            s.to_display(),
            tcs.join("|"),
            s.owner.as_str()
        );
    }
    println!("inspection statuses:");
    for s in InspectionStatus::ALL {
        print!(" {}", s.as_str());
    }
    println!();
    println!(
        "intent lifecycle: {}",
        IntentLifecycle::ALL
            .iter()
            .map(|l| l.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!(
        "truth classes (stored edges): {}",
        TruthClass::ALL
            .iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!("finding verdicts:");
    println!("  needed | justified | rejected | deferred | blocked | duplicate | resolved");
    println!("  stored as asserted adjudication facets on stable Finding ids");
    println!("  verdicts go stale when the flagged codefile content hash changes");
    println!("find --where keys: {}", FIND_WHERE_KEYS.join(" "));
    println!("grammar (write-boundary rules):");
    println!("  intent name: behavioral phrase, never a code symbol (override: --allow-symbol-name + behavioral --description, audited)");
    println!(
        "  ratification: {} — human-only decision (INV-8): a host LLM may record the human's explicit answer with --human-decision",
        RATIFICATION_STATES.join(" | ")
    );
    println!("  evidence: criterion/evidence/reason must be substantive; whole-field placeholders rejected (INV-6)");
    println!("  redefinition: changing a ratified intent's description stales it to needs_reconfirmation");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::grammar_json;
    use crate::grammar::{
        ACTIVE_LIFECYCLES, ASPECTS, LEVELS, PLACEHOLDER_TOKENS, RATIFICATION_STATES, VISIBILITIES,
    };

    #[test]
    fn empty_json_validation_id_formats_without_panicking() {
        let validation = serde_json::json!({});
        assert_eq!(
            crate::model::short(validation["id"].as_str().unwrap_or("")),
            ""
        );
    }

    #[test]
    fn schema_json_grammar_uses_the_write_gate_tables() {
        let grammar = grammar_json();
        let facets = &grammar["intent_facets"];
        assert_eq!(facets["level"], serde_json::json!(LEVELS));
        assert_eq!(
            facets["lifecycle_at_add"],
            serde_json::json!(ACTIVE_LIFECYCLES)
        );
        assert_eq!(facets["visibility"], serde_json::json!(VISIBILITIES));
        assert_eq!(facets["aspect"], serde_json::json!(ASPECTS));
        assert_eq!(
            facets["ratification"],
            serde_json::json!(RATIFICATION_STATES)
        );
        assert_eq!(
            grammar["evidence"]["rejected_placeholders"],
            serde_json::json!(PLACEHOLDER_TOKENS)
        );
    }
}
