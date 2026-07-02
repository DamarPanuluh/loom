use super::*;

pub(crate) fn dispatch(graph: Option<&Path>, cmd: CodefileCmd, json: bool) -> Result<()> {
    match cmd {
        CodefileCmd::Add { path } => {
            let root = resolve_root(graph)?;
            let store = Store::open(&root)?;
            // Expand globs against the graph root; register each new file.
            let matched = crate::fsglob::expand(&root, &path)?;
            let targets: Vec<String> = if matched.is_empty() {
                // No glob match: treat as a literal path (may be a not-yet-existing file).
                vec![path.replace('\\', "/")]
            } else {
                matched
            };
            let existing: std::collections::HashSet<String> =
                store.codefiles()?.into_iter().map(|n| n.name).collect();
            let mut added = 0usize;
            for t in &targets {
                if existing.contains(t) {
                    continue;
                }
                store.add_node(NodeType::CodeFile, t, "", "", serde_json::json!({}))?;
                added += 1;
            }
            // Remember the glob so `codefile rescan` can pick up files that
            // appear later (e.g. a new endpoint in a vendored upstream).
            if path.contains('*') || path.contains('?') {
                remember_glob(&store, &path)?;
            }
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "registered": added,
                    "matched": targets.len(),
                    "already_present": targets.len() - added,
                    "pattern": path,
                    "targets": targets,
                }),
                "loom sync",
                format!(
                    "registered {added} codefile(s) ({} matched, {} already present)",
                    targets.len(),
                    targets.len() - added
                ),
            )?;
            Ok(())
        }
        CodefileCmd::Rescan => codefile_rescan(graph, json),
        CodefileCmd::Remove { key } => {
            let store = open(graph)?;
            let n = store.resolve_node(&key, Some(NodeType::CodeFile))?;
            store.delete_node(&n.id)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "removed": true,
                    "codefile": node_json(&n),
                }),
                "loom status",
                format!("removed codefile '{}' (and its groundings)", n.name),
            )?;
            Ok(())
        }
        CodefileCmd::Show { key } => codefile_show(graph, &key, json),
        CodefileCmd::List { limit } => {
            let store = open(graph)?;
            let files = store.list_nodes(Some(NodeType::CodeFile), limit)?;
            if json {
                let rows: Vec<_> = files
                    .iter()
                    .map(|n| {
                        serde_json::json!({
                            "id": n.id,
                            "path": n.name,
                            "status": n.status,
                            "created_at": n.created_at,
                            "updated_at": n.updated_at,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                if files.is_empty() {
                    println!("no codefiles");
                }
                for n in &files {
                    println!("{} [{}]", n.name, &n.id[..8]);
                }
            }
            Ok(())
        }
    }
}
fn registered_globs(store: &Store) -> Result<Vec<String>> {
    Ok(store
        .get_meta("codefile_globs")?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}
fn remember_glob(store: &Store, pattern: &str) -> Result<()> {
    let mut globs = registered_globs(store)?;
    if !globs.iter().any(|g| g == pattern) {
        globs.push(pattern.to_string());
        store.set_meta("codefile_globs", &serde_json::to_string(&globs)?)?;
    }
    Ok(())
}
fn codefile_rescan(graph: Option<&Path>, json: bool) -> Result<()> {
    let root = resolve_root(graph)?;
    let store = Store::open(&root)?;
    let globs = registered_globs(&store)?;
    if globs.is_empty() {
        return pulse::emit_line(
            &store,
            json,
            serde_json::json!({
                "rescanned": false,
                "globs": 0,
                "new_files": [],
            }),
            "loom codefile add '<glob>'",
            "no globs remembered — register files with `loom codefile add '<glob>'` first",
        );
    }
    let existing: std::collections::HashSet<String> =
        store.codefiles()?.into_iter().map(|n| n.name).collect();
    let mut new_files: Vec<String> = Vec::new();
    for g in &globs {
        for t in crate::fsglob::expand(&root, g)? {
            if existing.contains(&t) || new_files.contains(&t) {
                continue;
            }
            store.add_node(NodeType::CodeFile, &t, "", "", serde_json::json!({}))?;
            new_files.push(t);
        }
    }
    let next_step = if new_files.is_empty() {
        "loom status"
    } else {
        "loom sync"
    };
    pulse::emit(
        &store,
        json,
        serde_json::json!({
            "rescanned": true,
            "globs": globs.len(),
            "new_files": new_files,
        }),
        next_step,
        || {
            println!(
                "rescanned {} glob(s): {} new file(s) registered",
                globs.len(),
                new_files.len()
            );
            for f in new_files.iter().take(10) {
                println!("    + {f}");
            }
            if !new_files.is_empty() {
                println!("  run `loom sync` to extract them");
            }
            Ok(())
        },
    )?;
    Ok(())
}
fn codefile_show(graph: Option<&Path>, key: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(key, Some(NodeType::CodeFile))?;
    let facet = |k: &str| -> Result<String> {
        Ok(store
            .get_facet(&n.id, TargetKind::Node, k)?
            .unwrap_or_default())
    };
    let (language, role, loc, symbols) = (
        facet("language")?,
        facet("role")?,
        facet("loc")?,
        facet("symbol_count")?,
    );

    // Owning intents: implements edges INTO this file, each with its locator and
    // recorded verdict.
    let mut owners = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Implements), None, Some(&n.id))? {
        let name = store
            .get_node(&e.from_id)?
            .map(|x| x.name)
            .unwrap_or_else(|| e.from_id.clone());
        let locator = store
            .get_facet(&e.id, TargetKind::Edge, "locator")?
            .unwrap_or_default();
        owners.push((name, locator, e.status.as_str().to_string(), e.evidence));
    }
    owners.sort_by(|a, b| a.0.cmp(&b.0));

    // Findings flagging this file, joined with their adjudication state.
    let flagged: std::collections::HashSet<String> = store
        .edges_with(Some(EdgeKind::Flags), None, Some(&n.id))?
        .into_iter()
        .map(|e| e.from_id)
        .collect();
    let findings: Vec<crate::signal::FindingView> = crate::signal::findings_view(&store)?
        .into_iter()
        .filter(|fv| flagged.contains(&fv.node.id))
        .collect();

    if json {
        let out = serde_json::json!({
            "name": n.name,
            "id": n.id,
            "language": language,
            "role": role,
            "loc": loc.parse::<u64>().ok(),
            "symbol_count": symbols.parse::<u64>().ok(),
            "owners": owners.iter().map(|(name, loc, verdict, ev)| serde_json::json!({
                "intent": name, "locator": loc, "verdict": verdict, "evidence": ev,
            })).collect::<Vec<_>>(),
            "findings": findings.iter().map(|fv| serde_json::json!({
                "state": fv.state, "stale": fv.stale, "title": fv.node.name, "reason": fv.reason,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("{} [{}]", n.name, &n.id[..8.min(n.id.len())]);
    let mut facets = Vec::new();
    if !language.is_empty() {
        facets.push(format!("language={language}"));
    }
    if !role.is_empty() {
        facets.push(format!("role={role}"));
    }
    if !loc.is_empty() {
        facets.push(format!("loc={loc}"));
    }
    if !symbols.is_empty() {
        facets.push(format!("symbols={symbols}"));
    }
    if !facets.is_empty() {
        println!("  {}", facets.join("  "));
    }
    println!("  owned by {} intent(s):", owners.len());
    if owners.is_empty() {
        println!("    (unowned — no implements edge; coverage gap or non-behavioral)");
    }
    for (name, locator, verdict, ev) in &owners {
        let at = if locator.is_empty() {
            String::new()
        } else {
            format!(" @ {locator}")
        };
        let ev = if ev.is_empty() {
            String::new()
        } else {
            format!(" — {ev}")
        };
        println!("    ↳ {name}{at} [{verdict}]{ev}");
    }
    if !findings.is_empty() {
        println!("  findings:");
        for fv in &findings {
            let stale = if fv.stale { "·STALE" } else { "" };
            let why = if fv.state == "untriaged" {
                String::new()
            } else {
                format!(" — {}", fv.reason)
            };
            println!("    [{}{}] {}{}", fv.state, stale, fv.node.description, why);
        }
    }
    Ok(())
}
