//! `loom bootstrap` — cold-start assist that drafts a Proposal of planned
//! pillar intents from derived signals (registered codefiles, tests, README).
//!
//! Plane: judgment-plane capture only. Never writes verdicts, never sets
//! `lifecycle=implemented`, never creates `implements`/`governs`/`validates`
//! edges. The operator adopts items via `loom proposal item adopt --as intent`.

use super::{open, pulse};
use crate::cli::BootstrapCmd;
use crate::model::NodeType;
use crate::Result;
use anyhow::bail;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

pub fn dispatch(graph: Option<&Path>, cmd: BootstrapCmd, json: bool) -> Result<()> {
    match cmd {
        BootstrapCmd::Suggest => suggest(graph, json),
    }
}

fn suggest(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let root = store.root().to_path_buf();
    let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?;
    if !intents.is_empty() {
        bail!(
            "bootstrap suggest refuses a non-empty intent graph ({} intent(s) already exist) — \
             use loom door / loom intent add for incremental capture",
            intents.len()
        );
    }
    let codefiles = store.list_nodes(Some(NodeType::CodeFile), usize::MAX)?;
    if codefiles.is_empty() {
        bail!(
            "bootstrap suggest needs registered codefiles first — \
             run: loom codefile add '<glob>' && loom sync"
        );
    }

    let candidates = collect_candidates(&root, &codefiles);
    if candidates.is_empty() {
        bail!("bootstrap suggest found no pillar candidates from codefiles/tests/README");
    }

    let mut raw_lines = vec![
        "Auto-drafted by loom bootstrap suggest from derived signals.".to_string(),
        "Adopt with: loom proposal item adopt <id> <n> --as intent --name '…' --description '…'"
            .to_string(),
        "Never treat these as proven — they are planned drafts only.".to_string(),
        String::new(),
    ];
    for (i, c) in candidates.iter().enumerate() {
        raw_lines.push(format!(
            "{}. {} — {} (suggested visibility={}, level={})",
            i + 1,
            c.name,
            c.description,
            c.visibility,
            c.level
        ));
    }
    let raw = raw_lines.join("\n");

    let mut items = Vec::new();
    for (i, c) in candidates.iter().enumerate() {
        items.push(json!({
            "number": i + 1,
            "text": format!("{} — {}", c.name, c.description),
            "kind": "intent",
            "status": "open",
            "suggested_name": c.name,
            "suggested_description": c.description,
            "suggested_level": c.level,
            "suggested_visibility": c.visibility,
            "signal": c.signal,
        }));
    }

    let body = json!({
        "raw": raw,
        "source": "bootstrap_suggest",
        "source_path": Value::Null,
        "items": items,
    });
    let title = format!(
        "bootstrap pillars for {}",
        root.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("repo")
    );
    let description = format!(
        "{} candidate pillar intent(s) from codefiles/tests/README — adopt to planned intents",
        candidates.len()
    );
    let node = store.add_node(NodeType::Proposal, &title, &description, "captured", body)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "proposal": {
                    "id": node.id,
                    "name": node.name,
                    "status": node.status,
                    "description": node.description,
                    "body": node.body,
                },
                "candidates": candidates.len(),
                "next": format!(
                    "loom proposal item adopt {} <n> --as intent --name '<pillar>' --description '<criterion>'",
                    &node.id[..8]
                ),
            }))?
        );
    } else {
        pulse::emit_line(
            &store,
            false,
            json!({
                "proposal_id": node.id,
                "candidates": candidates.len(),
            }),
            &format!(
                "loom proposal item adopt {} 1 --as intent --name '<pillar>' --description '<criterion>'",
                &node.id[..8]
            ),
            format!(
                "bootstrap suggest: proposal '{}' [{}] with {} candidate pillar(s)",
                node.name,
                &node.id[..8],
                candidates.len()
            ),
        )?;
        println!("  adopt each: loom proposal item adopt {} <n> --as intent", &node.id[..8]);
        println!("  never auto-implements or marks implemented — planned only");
    }
    Ok(())
}

#[derive(Debug)]
struct Candidate {
    name: String,
    description: String,
    level: &'static str,
    visibility: &'static str,
    signal: String,
}

fn collect_candidates(root: &Path, codefiles: &[crate::model::Node]) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();

    // Top-level module directories under src/ (or first path segment of codefiles).
    let mut modules = BTreeSet::new();
    for cf in codefiles {
        let path = cf.name.replace('\\', "/");
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 2 {
            // src/foo.rs or src/foo/mod.rs → foo
            if parts[0] == "src" {
                let mod_name = parts[1].trim_end_matches(".rs");
                if !mod_name.is_empty() && mod_name != "main" && mod_name != "lib" {
                    modules.insert(mod_name.to_string());
                }
            }
        }
    }
    for m in modules.into_iter().take(8) {
        let name = format!("{m} behavior is mapped and grounded");
        if seen.insert(name.clone()) {
            out.push(Candidate {
                description: format!(
                    "the {m} module's primary behaviors are named as intents and grounded to symbols"
                ),
                name,
                level: "component",
                visibility: "internal",
                signal: format!("codefile_module:{m}"),
            });
        }
    }

    // Test binaries / integration tests.
    let tests_dir = root.join("tests");
    if tests_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&tests_dir) {
            for entry in entries.flatten().take(6) {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("rs") {
                    continue;
                }
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("tests")
                    .to_string();
                let name = format!("{stem} suite proves its claimed behaviors");
                if seen.insert(name.clone()) {
                    out.push(Candidate {
                        description: format!(
                            "integration/unit coverage in tests/{stem}.rs is attached as validations to the intents it exercises"
                        ),
                        name,
                        level: "feature",
                        visibility: "internal",
                        signal: format!("test_file:tests/{stem}.rs"),
                    });
                }
            }
        }
    }

    // README H2 headings as user-visible product pillars.
    for heading in readme_h2(root).into_iter().take(4) {
        let name = heading.clone();
        if seen.insert(name.clone()) {
            out.push(Candidate {
                description: format!(
                    "user-facing capability described under README heading '{heading}' holds as falsifiable behavior"
                ),
                name,
                level: "feature",
                visibility: "user_visible",
                signal: format!("readme_h2:{heading}"),
            });
        }
    }

    // Always offer one system-level product intent if nothing else.
    if out.is_empty() {
        let repo = root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("this codebase");
        out.push(Candidate {
            name: format!("{repo} delivers its primary user-visible behaviors"),
            description: format!(
                "the core product behaviors of {repo} are named, grounded, and proven"
            ),
            level: "system",
            visibility: "user_visible",
            signal: "fallback_system".into(),
        });
    }

    out
}

fn readme_h2(root: &Path) -> Vec<String> {
    let candidates = ["README.md", "Readme.md", "readme.md"];
    let mut path: Option<PathBuf> = None;
    for name in candidates {
        let p = root.join(name);
        if p.is_file() {
            path = Some(p);
            break;
        }
    }
    let Some(path) = path else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("## ") {
                let h = rest.trim();
                if h.is_empty()
                    || h.eq_ignore_ascii_case("table of contents")
                    || h.eq_ignore_ascii_case("license")
                    || h.eq_ignore_ascii_case("changelog")
                {
                    return None;
                }
                Some(h.to_string())
            } else {
                None
            }
        })
        .collect()
}
