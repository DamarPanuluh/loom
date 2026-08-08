//! `loom codefile` command family — registering files and globs with the graph.
//!
//! Plane: CLI surface over asserted file registration (the roster the
//! structural plane extracts from). Owns glob expansion, the ignore gate
//! (explicit literal paths always override ignore rules), and the remembered
//! globs `rescan`/`sync` replay for future arrivals. Registration only —
//! content extraction and derived facets belong to `sync`, never here.

use super::*;
use anyhow::Context;

pub(crate) fn dispatch(graph: Option<&Path>, cmd: CodefileCmd, json: bool) -> Result<()> {
    match cmd {
        CodefileCmd::Add { path, observed } => {
            let root = resolve_root(graph)?;
            let store = Store::open(&root)?;
            let is_glob = path.contains('*') || path.contains('?');
            // Expand globs against the graph root; register each new file.
            let matched = crate::fsglob::expand(&root, &path)?;
            let targets: Vec<String> = if is_glob {
                // A glob that matched nothing is not a literal path — it just
                // means no files exist yet. `remember_glob` below still records
                // it so `rescan`/`sync` will pick up future arrivals. But
                // silently registering nothing is loom's worst first
                // impression, so when the tree HAS source files this glob did
                // not reach, say which globs would have.
                if matched.is_empty() {
                    let want = path.rsplit('.').next().filter(|e| !e.contains('/'));
                    let suggestions = crate::fsglob::suggest(&root, want);
                    if !suggestions.is_empty() && !json {
                        eprintln!("'{path}' matched no files. This tree has:");
                        for (glob, n) in &suggestions {
                            eprintln!("  loom codefile add '{glob}'   ({n} file(s))");
                        }
                    }
                }
                matched
            } else if matched.is_empty() {
                // No glob metacharacters and no on-disk hit: treat as a literal
                // path (may be a not-yet-existing file).
                vec![path.replace('\\', "/")]
            } else {
                matched
            };
            let existing: std::collections::HashMap<String, Node> = store
                .codefiles()?
                .into_iter()
                .map(|n| (n.name.clone(), n))
                .collect();
            // Ignore gate: only new files discovered via glob expansion are
            // checked — explicit literal paths and re-adds of existing files
            // always go through (explicit intent overrides ignore).
            let ignore = if is_glob {
                Some(crate::fsglob::matcher(store.ignore_globs()?)?)
            } else {
                None
            };
            let mut added = 0usize;
            let mut ignored_count = 0usize;
            let mut marked_observed = 0usize;
            for t in &targets {
                match existing.get(t) {
                    Some(n) => {
                        // Re-adding with --observed marks an already-registered
                        // file observed (the flag never silently clears).
                        if observed && !codefile_observed(n) {
                            let mut body = n.body.clone();
                            body["observed"] = serde_json::Value::Bool(true);
                            store.set_node_body(&n.id, &body)?;
                            marked_observed += 1;
                        }
                    }
                    None => {
                        if let Some(ign) = &ignore {
                            if ign.is_match(t.as_str()) {
                                ignored_count += 1;
                                continue;
                            }
                        }
                        store.add_node(NodeType::CodeFile, t, "", "", codefile_body(observed))?;
                        added += 1;
                    }
                }
            }
            // Remember the glob so `codefile rescan` / `sync` can pick up
            // files that appear later (e.g. a new endpoint in a vendored upstream).
            if is_glob {
                remember_glob(&store, &path, observed)?;
            }
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "registered": added,
                    "matched": targets.len(),
                    "already_present": targets.len() - added - ignored_count,
                    "ignored": ignored_count,
                    "observed": observed,
                    "marked_observed": marked_observed,
                    "pattern": path,
                    "targets": targets,
                }),
                "loom sync",
                format!(
                    "registered {added}{} codefile(s) ({} matched, {} already present{}{})",
                    if observed { " observed" } else { "" },
                    targets.len(),
                    targets.len() - added - ignored_count,
                    if marked_observed > 0 {
                        format!(", {marked_observed} marked observed")
                    } else {
                        String::new()
                    },
                    if ignored_count > 0 {
                        format!(", {ignored_count} ignored")
                    } else {
                        String::new()
                    }
                ),
            )?;
            Ok(())
        }
        CodefileCmd::Rescan => codefile_rescan(graph, json),
        CodefileCmd::Remove { key, successor } => {
            let store = open(graph)?;
            let n = store.resolve_node(&key, Some(NodeType::CodeFile))?;
            // The graph must survive refactors of its own subjects (P10):
            // removing a codefile with live asserted edges is either refused
            // with every blocker named, or — with --successor — expressed as
            // ONE recorded operation: each edge retargeted in place (verdict
            // history intact) before the node goes. Ghost registrations warn
            // forever; a rename/split should not manufacture them.
            let live = |e: &crate::model::Edge| -> Result<bool> {
                Ok(e.truth_class == TruthClass::Asserted && !store.edge_superseded(&e.id)?)
            };
            let mut to_incident = Vec::new();
            for e in store.edges_with(None, None, Some(&n.id))? {
                if live(&e)? {
                    to_incident.push(e);
                }
            }
            let mut from_incident = Vec::new();
            for e in store.edges_with(None, Some(&n.id), None)? {
                if live(&e)? {
                    from_incident.push(e);
                }
            }
            let describe = |e: &crate::model::Edge| -> Result<String> {
                let from = store
                    .get_node(&e.from_id)?
                    .map(|x| x.name)
                    .unwrap_or_else(|| e.from_id.clone());
                Ok(format!(
                    "  [{}] {} '{}' → {} [{}] — loom edge retarget {} --to <successor> --reason '…'",
                    crate::model::short(&e.id),
                    e.kind,
                    from,
                    n.name,
                    e.status.as_str(),
                    crate::model::short(&e.id)
                ))
            };
            // From-incident live edges can never be auto-cascaded (which
            // successor would own the outgoing claim is ambiguous) — they are
            // blockers with or without --successor.
            if !from_incident.is_empty() {
                let mut lines = Vec::new();
                for e in &from_incident {
                    lines.push(describe(e)?);
                }
                anyhow::bail!(
                    "cannot remove codefile '{}': {} live edge(s) originate FROM it and cannot \
                     be cascaded to a successor — retarget or remove them first:\n{}",
                    n.name,
                    from_incident.len(),
                    lines.join("\n")
                );
            }
            let succ = match &successor {
                None => {
                    if !to_incident.is_empty() {
                        let mut lines = Vec::new();
                        for e in &to_incident {
                            lines.push(describe(e)?);
                        }
                        anyhow::bail!(
                            "cannot remove codefile '{}': {} live edge(s) still target it — \
                             removing it now would orphan their claims. Either cascade them in \
                             one operation (`loom codefile remove {0} --successor <file>` after \
                             registering the successor) or retarget each:\n{}",
                            n.name,
                            to_incident.len(),
                            lines.join("\n")
                        );
                    }
                    None
                }
                Some(key) => {
                    let s = store.resolve_node(key, Some(NodeType::CodeFile))?;
                    if s.id == n.id {
                        anyhow::bail!(
                            "successor is the file being removed — nothing to cascade to"
                        );
                    }
                    Some(s)
                }
            };
            let mut retargeted = Vec::new();
            if let Some(s) = &succ {
                for e in &to_incident {
                    let reason = format!(
                        "codefile '{}' removed; behavior carried by successor '{}'",
                        n.name, s.name
                    );
                    store.retarget_edge(&e.id, &s.id, &reason)?;
                    store.append_journal(
                        "edge_retargeted",
                        &e.id,
                        serde_json::json!({
                            "kind": e.kind,
                            "from": { "id": e.from_id },
                            "old_to": { "id": n.id, "name": n.name },
                            "new_to": { "id": s.id, "name": s.name },
                            "reason": reason,
                            "via": "codefile remove --successor",
                        }),
                    )?;
                    retargeted.push(crate::model::short(&e.id).to_string());
                }
            }
            store.delete_node(&n.id)?;
            store.append_journal(
                "node_removed",
                &n.id,
                serde_json::json!({
                    "kind": "codefile",
                    "name": n.name,
                    "successor": succ.as_ref().map(|s| s.name.clone()),
                    "retargeted_edges": retargeted,
                }),
            )?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "removed": true,
                    "codefile": node_json(&n),
                    "successor": succ.as_ref().map(node_json),
                    "retargeted_edges": retargeted,
                }),
                "loom sync",
                match &succ {
                    Some(s) => format!(
                        "removed codefile '{}': {} edge(s) retargeted to '{}' in place \
                         (verdict history kept; `loom sync` re-verifies evidence at the new location)",
                        n.name,
                        retargeted.len(),
                        s.name
                    ),
                    None => format!("removed codefile '{}'", n.name),
                },
            )?;
            Ok(())
        }
        CodefileCmd::Show { key } => codefile_show(graph, &key, json),
        CodefileCmd::List { limit, offset } => {
            let store = open(graph)?;
            let files = store.list_nodes_page(Some(NodeType::CodeFile), limit, offset)?;
            let total = store.count_nodes(Some(NodeType::CodeFile))?;
            if json {
                let rows: Vec<_> = files
                    .iter()
                    .map(|n| {
                        serde_json::json!({
                            "id": n.id,
                            "path": n.name,
                            "status": n.status,
                            "observed": codefile_observed(n),
                            "created_at": n.created_at,
                            "updated_at": n.updated_at,
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&super::pagination_envelope(
                        &rows, offset, limit, total
                    ))?
                );
            } else {
                if files.is_empty() && offset == 0 {
                    println!("no codefiles");
                }
                for n in &files {
                    println!(
                        "{} [{}]{}",
                        n.name,
                        crate::model::short(&n.id),
                        if codefile_observed(n) {
                            "  (observed)"
                        } else {
                            ""
                        }
                    );
                }
                if let Some(footer) = super::page_footer(files.len(), offset, total) {
                    println!("{footer}");
                }
            }
            Ok(())
        }
    }
}
fn codefile_body(observed: bool) -> serde_json::Value {
    if observed {
        serde_json::json!({"observed": true})
    } else {
        serde_json::json!({})
    }
}
fn registered_globs(store: &Store) -> Result<Vec<String>> {
    read_globs(store, "codefile_globs")
}
fn observed_globs(store: &Store) -> Result<Vec<String>> {
    read_globs(store, "observed_globs")
}
fn read_globs(store: &Store, key: &str) -> Result<Vec<String>> {
    super::read_json_meta(store, key)
}
fn remember_glob(store: &Store, pattern: &str, observed: bool) -> Result<()> {
    let mut globs = registered_globs(store)?;
    if !globs.iter().any(|g| g == pattern) {
        globs.push(pattern.to_string());
        store.set_meta("codefile_globs", &serde_json::to_string(&globs)?)?;
    }
    // An observed glob is remembered separately so `rescan` registers files
    // that appear under it later as observed too.
    if observed {
        let mut obs = observed_globs(store)?;
        if !obs.iter().any(|g| g == pattern) {
            obs.push(pattern.to_string());
            store.set_meta("observed_globs", &serde_json::to_string(&obs)?)?;
        }
    }
    Ok(())
}
fn codefile_rescan(graph: Option<&Path>, json: bool) -> Result<()> {
    let root = resolve_root(graph)?;
    let store = Store::open(&root)?;
    let outcome = rescan_globs(&store, &root)?;
    if outcome.globs == 0 {
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
    let next_step = if outcome.new_files.is_empty() {
        "loom status"
    } else {
        "loom sync"
    };
    pulse::emit(
        &store,
        json,
        serde_json::json!({
            "rescanned": true,
            "globs": outcome.globs,
            "new_files": outcome.new_files,
            "new_observed": outcome.new_observed,
        }),
        next_step,
        || {
            println!(
                "rescanned {} glob(s): {} new file(s) registered",
                outcome.globs,
                outcome.new_files.len()
            );
            for f in outcome.new_files.iter().take(10) {
                println!("    + {f}");
            }
            if !outcome.new_files.is_empty() {
                println!("  run `loom sync` to extract them");
            }
            Ok(())
        },
    )?;
    Ok(())
}

/// Outcome of a glob-based rescan — used by both `codefile rescan` and `sync`.
pub(crate) struct RescanOutcome {
    pub globs: usize,
    pub new_files: Vec<String>,
    pub new_observed: usize,
}

/// Expand remembered globs, register any new files, skip ignored paths.
/// Owned globs expand first so a file matched by both registers as owned.
/// Files already registered are never touched (rescan never deletes nodes).
pub(crate) fn rescan_globs(store: &Store, root: &Path) -> Result<RescanOutcome> {
    let globs = registered_globs(store)?;
    if globs.is_empty() {
        return Ok(RescanOutcome {
            globs: 0,
            new_files: Vec::new(),
            new_observed: 0,
        });
    }
    let existing: std::collections::HashSet<String> =
        store.codefiles()?.into_iter().map(|n| n.name).collect();
    let ignore = crate::fsglob::matcher(store.ignore_globs()?)?;
    let observed = observed_globs(store)?;
    let (owned_globs, obs_globs): (Vec<&String>, Vec<&String>) =
        globs.iter().partition(|g| !observed.contains(g));
    let mut new_files: Vec<String> = Vec::new();
    let mut new_observed = 0usize;
    for (pass_globs, as_observed) in [(owned_globs, false), (obs_globs, true)] {
        for g in pass_globs {
            for t in crate::fsglob::expand(root, g)? {
                if existing.contains(&t) || new_files.contains(&t) {
                    continue;
                }
                if ignore.is_match(&t) {
                    continue;
                }
                store.add_node(NodeType::CodeFile, &t, "", "", codefile_body(as_observed))?;
                if as_observed {
                    new_observed += 1;
                }
                new_files.push(t);
            }
        }
    }
    new_files.sort();
    Ok(RescanOutcome {
        globs: globs.len(),
        new_files,
        new_observed,
    })
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
        if store.edge_superseded(&e.id)? {
            continue; // rehomed away — history, not an owner
        }
        let name = store
            .get_node(&e.from_id)?
            .map(|x| x.name)
            .unwrap_or_else(|| e.from_id.clone());
        let locator = store
            .get_facet(&e.id, TargetKind::Edge, "locator")?
            .unwrap_or_default();
        let grole = store.grounding_role(&e.id)?.as_str().to_string();
        owners.push((
            name,
            locator,
            e.status.as_str().to_string(),
            store.verdict_prose(&e.id)?,
            grole,
        ));
    }
    // realizing owners first (they own coverage), then by name.
    owners.sort_by(|a, b| {
        (a.4 != "realizes")
            .cmp(&(b.4 != "realizes"))
            .then(a.0.cmp(&b.0))
    });

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

    let observed = codefile_observed(&n);
    if json {
        let loc = if loc.is_empty() {
            None
        } else {
            Some(
                loc.parse::<u64>()
                    .with_context(|| format!("invalid loc facet on '{}'", n.name))?,
            )
        };
        let symbol_count = if symbols.is_empty() {
            None
        } else {
            Some(
                symbols
                    .parse::<u64>()
                    .with_context(|| format!("invalid symbol_count facet on '{}'", n.name))?,
            )
        };
        let out = serde_json::json!({
            "name": n.name,
            "id": n.id,
            "observed": observed,
            "language": language,
            "role": role,
            "loc": loc,
            "symbol_count": symbol_count,
            "owners": owners.iter().map(|(name, loc, verdict, ev, role)| serde_json::json!({
                "intent": name, "locator": loc, "verdict": verdict, "evidence": ev, "role": role,
            })).collect::<Vec<_>>(),
            "findings": findings.iter().map(|fv| serde_json::json!({
                "state": fv.state, "stale": fv.stale, "title": fv.node.name, "reason": fv.reason,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!(
        "{} [{}]{}",
        n.name,
        crate::model::short(&n.id),
        if observed { "  (observed)" } else { "" }
    );
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
    let realizing = owners.iter().filter(|o| o.4 == "realizes").count();
    println!(
        "  grounded by {} edge(s) ({realizing} realizing):",
        owners.len()
    );
    if realizing == 0 {
        if observed {
            println!("    (observed — monitored upstream; no coverage obligation)");
        } else {
            println!(
                "    (no realizing owner — coverage gap; consumes/configures/verifies do not own)"
            );
        }
    }
    for (name, locator, verdict, ev, role) in &owners {
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
        println!("    ↳ [{role}] {name}{at} [{verdict}]{ev}");
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
