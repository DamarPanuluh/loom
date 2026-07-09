//! Discovery command family — find, explain, detect, schema.
//!
//! Plane: read-only search and repo detection. `keyword_hits` is shared with
//! the door landing menu in `capture_cmd`.

use super::*;

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
pub(crate) const FIND_WHERE_KEYS: &[&str] = &["visibility", "level", "aspect"];

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
    let kinds = [NodeType::Intent, NodeType::CodeFile, NodeType::QualityRule];
    let filter_ids = resolve_find_filters(&store, tag, where_facets)?;
    let has_filters = tag.is_some() || !where_facets.is_empty();
    let q = query.trim();

    if exact {
        if q.is_empty() {
            bail!("--exact requires a non-empty query");
        }
        if filter_ids.is_none() {
            return find_exact(&store, q, &kinds, json);
        }
        return find_exact_filtered(&store, q, &kinds, filter_ids.as_ref(), json);
    }

    let limited = if q.is_empty() {
        if !has_filters {
            bail!("pass a query and/or --tag / --where");
        }
        // Facet/tag-only: list matching Intent/CodeFile/QualityRule nodes.
        let mut rows = Vec::new();
        let ids = filter_ids.expect("has_filters ⇒ Some");
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

    print_find_hits(&store, q, &limited, json)
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
        sets.push(store.nodes_where_facet(&key, &value)?.into_iter().collect());
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

fn find_exact_filtered(
    store: &Store,
    query: &str,
    kinds: &[NodeType],
    filter: Option<&std::collections::BTreeSet<String>>,
    json: bool,
) -> Result<()> {
    let mut limited = Vec::new();
    for kind in kinds {
        for n in store.list_nodes(Some(*kind), usize::MAX)? {
            if n.name.eq_ignore_ascii_case(query) {
                if filter.is_none_or(|ids| ids.contains(&n.id)) {
                    limited.push((100usize, kind.as_str().to_string(), n.name, n.id));
                }
            }
        }
    }
    print_find_hits(store, query, &limited, json)
}

fn print_find_hits(
    store: &Store,
    query: &str,
    limited: &[(usize, String, String, String)],
    json: bool,
) -> Result<()> {
    if json {
        let mut rows = Vec::new();
        for (s, kind, name, id) in limited {
            let mut groundings = Vec::new();
            if kind == "intent" {
                for e in store.edges_with(Some(EdgeKind::Implements), Some(id), None)? {
                    if store.edge_superseded(&e.id)? {
                        continue;
                    }
                    let path = store
                        .get_node(&e.to_id)?
                        .map(|n| n.name)
                        .unwrap_or_else(|| e.to_id.clone());
                    let locator = store
                        .get_facet(&e.id, TargetKind::Edge, "locator")?
                        .unwrap_or_default();
                    groundings.push(serde_json::json!({
                        "edge_id": e.id,
                        "path": path,
                        "locator": locator,
                        "role": store.grounding_role(&e.id)?.as_str(),
                        "status": e.status.as_str(),
                        "evidence": e.evidence,
                    }));
                }
            }
            rows.push(serde_json::json!({
                "score": s,
                "kind": kind,
                "name": name,
                "id": id,
                "groundings": groundings,
            }));
        }
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        if limited.is_empty() {
            println!(
                "no match for '{query}' — try `loom status` to see coverage, or it may not exist"
            );
        }
        let needle = query.trim();
        for (s, kind, name, id) in limited {
            let mark = if !needle.is_empty() && name.eq_ignore_ascii_case(needle) {
                " (exact)"
            } else {
                ""
            };
            println!("{:<10} {} [{}] (score {s}){mark}", kind, name, &id[..8]);
            if kind == "intent" {
                let grounds = store.edges_with(Some(EdgeKind::Implements), Some(id), None)?;
                if store.realizing_groundings(id)?.is_empty() {
                    println!("             ↳ (no realizing grounding yet)");
                }
                for e in grounds {
                    if store.edge_superseded(&e.id)? {
                        continue;
                    }
                    let role = store.grounding_role(&e.id)?;
                    let path = store
                        .get_node(&e.to_id)?
                        .map(|n| n.name)
                        .unwrap_or_else(|| e.to_id.clone());
                    let loc = store
                        .get_facet(&e.id, TargetKind::Edge, "locator")?
                        .unwrap_or_default();
                    let at = if loc.is_empty() {
                        String::new()
                    } else {
                        format!(" @ {loc}")
                    };
                    let ev = if e.evidence.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", e.evidence)
                    };
                    println!(
                        "             ↳ [{role}] {path}{at} [{}]{ev}",
                        e.status.as_str()
                    );
                }
            }
        }
    }
    Ok(())
}

/// Read-only neighborhood brief for an intent — not a `loom next` lane.
pub(crate) fn explain_cmd(graph: Option<&Path>, intent_key: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    let intent = store.resolve_node(intent_key, Some(NodeType::Intent))?;
    let visibility = store.get_facet(&intent.id, TargetKind::Node, "visibility")?;
    let level = store.get_facet(&intent.id, TargetKind::Node, "level")?;
    let aspect = store.get_facet(&intent.id, TargetKind::Node, "aspect")?;
    let tags = store.tags_of(&intent.id, TargetKind::Node)?;

    let mut groundings = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Implements), Some(&intent.id), None)? {
        if store.edge_superseded(&e.id)? {
            continue;
        }
        let path = store
            .get_node(&e.to_id)?
            .map(|n| n.name)
            .unwrap_or_else(|| e.to_id.clone());
        let locator = store.get_facet(&e.id, TargetKind::Edge, "locator")?;
        groundings.push(serde_json::json!({
            "edge_id": e.id,
            "path": path,
            "locator": locator,
            "role": store.grounding_role(&e.id)?.as_str(),
            "status": e.status.as_str(),
        }));
    }

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
    let open_questions: Vec<_> = store
        .list_nodes(Some(NodeType::Question), usize::MAX)?
        .into_iter()
        .filter(|q| q.status == "open")
        .filter(|q| {
            store
                .edges_with(Some(EdgeKind::Questions), Some(&q.id), Some(&intent.id))
                .ok()
                .map(|es| !es.is_empty())
                .unwrap_or(false)
        })
        .map(|q| {
            serde_json::json!({
                "id": q.id,
                "text": q.description,
            })
        })
        .collect();

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
        println!("{} [{}]", intent.name, &intent.id[..8]);
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
                &v["id"].as_str().unwrap_or("")[..8.min(v["id"].as_str().unwrap_or("").len())],
                v["status"].as_str().unwrap_or("")
            );
        }
        println!(
            "  completeness open axes: {}",
            scorecard.open
        );
        if !open_questions.is_empty() {
            println!("  open questions: {}", open_questions.len());
        }
    }
    Ok(())
}

/// `loom find --exact`: whole-name (case-insensitive) matches only, no scoring.
/// Fuzzy `find` ranks by substring, so a partial hit can read as a match that
/// isn't there — the false positive that seeded a bad dedup. This answers
/// "does a node named exactly this exist?" deterministically, and lists every
/// colliding id when duplicates share the name.
fn find_exact(store: &Store, query: &str, kinds: &[NodeType], json: bool) -> Result<()> {
    let needle = query.trim();
    let mut hits: Vec<(String, String, String)> = Vec::new();
    for nt in kinds {
        for n in store.list_nodes(Some(*nt), usize::MAX)? {
            if n.status == "deprecated" {
                continue;
            }
            if n.name.eq_ignore_ascii_case(needle) {
                hits.push((nt.as_str().to_string(), n.name.clone(), n.id.clone()));
            }
        }
    }
    hits.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));
    if json {
        let rows: Vec<_> = hits
            .iter()
            .map(|(kind, name, id)| {
                serde_json::json!({ "kind": kind, "name": name, "id": id, "exact": true })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else if hits.is_empty() {
        println!(
            "no exact match for '{query}' — nothing named exactly this exists \
             (drop --exact for fuzzy matches)"
        );
    } else {
        for (kind, name, id) in &hits {
            println!("{:<10} {} [{}] (exact)", kind, name, &id[..8.min(id.len())]);
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
    // honest signals: a recommendation the seeder rejects is a dead end.
    let mut recommended: Vec<&str> = vec!["iso5055"];
    if markers.contains(&"docker") {
        recommended.push("docker");
    }
    if markers.contains(&"node") {
        recommended.push("web-ui");
        recommended.push("service");
    }
    if markers.contains(&"rust") || markers.contains(&"go") {
        recommended.push("concurrency");
    }
    if root.join("migrations").is_dir() || langs.contains_key("sql") {
        recommended.push("data");
    }
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
    for e in entries.flatten() {
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
pub(crate) fn schema_cmd(json: bool) -> Result<()> {
    use crate::model::*;
    if json {
        let edge_kinds: Vec<_> = crate::registry::REGISTRY
            .iter()
            .map(|s| {
                serde_json::json!({
                    "kind": s.kind.as_str(),
                    "from": s.from.as_str(),
                    "to": s.to.as_str(),
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
                "finding_verdicts": ["needed", "justified", "rejected", "deferred", "blocked", "duplicate"],
                "find_where_keys": FIND_WHERE_KEYS,
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
            s.to.as_str(),
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
    println!("  needed | justified | rejected | deferred | blocked | duplicate");
    println!("  stored as asserted adjudication facets on stable Finding ids");
    println!("  verdicts go stale when the flagged codefile content hash changes");
    println!(
        "find --where keys: {}",
        FIND_WHERE_KEYS.join(" ")
    );
    Ok(())
}
