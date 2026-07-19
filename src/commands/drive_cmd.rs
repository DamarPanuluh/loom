//! Interactive, human-present drive sessions recorded as append-only exchanges.

use super::{open, pulse, require_human_presence};
use crate::cli::DriveCmd;
use crate::model::NodeType;
use crate::Result;
use anyhow::bail;
use std::io::{self, Write};
use std::path::Path;

pub(crate) fn dispatch(graph: Option<&Path>, cmd: Option<DriveCmd>, json: bool) -> Result<()> {
    match cmd {
        Some(DriveCmd::Freeze { name }) => freeze(graph, &name, json),
        None => drive(graph, json),
    }
}

/// A drive is intentionally an in-terminal human session. The selected intent
/// and the command's complete observed result are journaled together; this is
/// the replayable evidence chain consumed by `drive freeze`.
fn drive(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    require_human_presence("drive")?;
    let mut exchanges = Vec::new();
    loop {
        let Some(utterance) = prompt("utterance (blank to end): ")? else {
            break;
        };
        let hits = crate::commands::discover_cmd::keyword_hits(
            &store,
            &utterance,
            &[NodeType::Intent],
            5,
        )?;
        for (index, (score, _, name, id)) in hits.iter().enumerate() {
            println!("  {}. {} [{}] score={score}", index + 1, name, &id[..8]);
        }
        let picked = prompt("pick number, intent id, or m to mint: ")?
            .ok_or_else(|| anyhow::anyhow!("drive ended while choosing an intent"))?;
        let intent = if picked.eq_ignore_ascii_case("m") {
            let node = crate::commands::intent::create_intent(
                &store,
                &crate::commands::intent::IntentAddArgs {
                    name: utterance.clone(),
                    description: format!("operator drive utterance: {utterance}"),
                    level: "feature".into(),
                    lifecycle: "planned".into(),
                    visibility: Some("user_visible".into()),
                    layer: None,
                    aspect: None,
                    allow_symbol_name: false,
                },
            )?;
            // A solo operator's typed utterance is the birth ratification
            // evidence. LLM-lane mints remain explicitly unratified.
            if matches!(store.agent(), crate::store::Agent::Solo) {
                store.add_note(
                    &node.id,
                    "ratify",
                    &format!("born ratified by drive utterance: {utterance}"),
                )?;
            }
            node
        } else if let Ok(index) = picked.parse::<usize>() {
            let (_, _, _, id) = hits
                .get(index.saturating_sub(1))
                .ok_or_else(|| anyhow::anyhow!("no displayed match numbered {index}"))?;
            store.resolve_node(id, Some(NodeType::Intent))?
        } else {
            store.resolve_node(&picked, Some(NodeType::Intent))?
        };
        let command = prompt("confirmed command to execute (blank to skip): ")?.unwrap_or_default();
        let (exit, stdout, stderr) = if command.is_empty() {
            (None, String::new(), String::new())
        } else {
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(&command)
                .output()?;
            (
                output.status.code(),
                String::from_utf8_lossy(&output.stdout).into_owned(),
                String::from_utf8_lossy(&output.stderr).into_owned(),
            )
        };
        let entry = crate::journal::append(
            store.root(),
            "drive_exchange",
            "drive",
            serde_json::json!({
                "utterance": utterance,
                "intent": intent.name,
                "intent_id": intent.id,
                "command": command,
                "exit": exit,
                "stdout": stdout,
                "stderr": stderr,
            }),
        )?;
        store.add_note(
            &intent.id,
            "evidence",
            &format!("drive exchange {}", crate::journal::reference(&entry)),
        )?;
        exchanges.push(crate::journal::reference(&entry));
    }
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({ "exchanges": exchanges }),
        "loom drive freeze <name>",
        "recorded drive session",
    )
}

fn prompt(label: &str) -> Result<Option<String>> {
    print!("{label}");
    io::stdout().flush()?;
    let mut value = String::new();
    if io::stdin().read_line(&mut value)? == 0 {
        return Ok(None);
    }
    let value = value.trim().to_string();
    Ok((!value.is_empty()).then_some(value))
}

fn freeze(graph: Option<&Path>, name: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    // Normal interactive sessions are target `drive`; a named target remains
    // accepted so imported/synthetic journals can select a particular chain.
    let entries: Vec<_> = crate::journal::read(store.root())?
        .into_iter()
        .filter(|entry| {
            entry.event == "drive_exchange"
                && (entry.target_id == name || entry.target_id == "drive")
        })
        .collect();
    if entries.is_empty() {
        bail!("no journaled drive exchanges available for '{name}'");
    }
    let steps: Vec<_> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let command = entry.payload.get("command")?.as_str()?;
            (!command.trim().is_empty()).then(|| serde_json::json!({
                "name": format!("drive-{}", index + 1),
                "intent": entry.payload.get("intent").and_then(|v| v.as_str()).unwrap_or("drive exchange"),
                "run": command,
                "expect": { "exit_code": 0 },
            }))
        })
        .collect();
    let path = store.root().join("journeys").join(format!("{name}.yaml"));
    std::fs::create_dir_all(path.parent().expect("journey path has parent"))?;
    std::fs::write(
        &path,
        serde_norway::to_string(&serde_json::json!({ "journey": name, "steps": steps }))?,
    )?;
    // A frozen drive is immediately replayable: compile the observed command
    // chain, then freeze its first run as the journey baseline.
    let spec = crate::journey::parse(&path)?;
    let outcomes = crate::journey::execute_steps(&spec, Some(store.root()), false)?;
    let baseline = crate::journey::write_baseline(store.root(), name, &outcomes)?;
    let entry = crate::journal::append(
        store.root(),
        "drive_freeze",
        name,
        serde_json::json!({ "journey": path, "baseline": baseline, "exchanges": entries.len() }),
    )?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({ "journey": path, "baseline": baseline, "journal": crate::journal::reference(&entry) }),
        "loom journey run",
        format!("compiled and froze drive '{name}' into {}", path.display()),
    )
}
