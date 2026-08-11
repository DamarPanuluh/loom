//! Interactive, human-present drive sessions recorded as append-only exchanges.

use super::{open, pulse, require_challenge};
use crate::cli::DriveCmd;
use crate::model::NodeType;
use crate::Result;
use anyhow::{bail, Context};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;

pub(crate) fn dispatch(graph: Option<&Path>, cmd: Option<DriveCmd>, json: bool) -> Result<()> {
    match cmd {
        Some(DriveCmd::Freeze { name }) => freeze(graph, &name, json),
        None => drive(graph, json),
    }
}

/// A drive is intentionally an in-terminal human session. The selected intent
/// and the command's complete observed result are journaled together; `drive
/// freeze` extracts only their semantic user actions into an authored Journey.
fn drive(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    require_challenge("drive")?;
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
            println!(
                "  {}. {} [{}] score={score}",
                index + 1,
                name,
                crate::model::short(id)
            );
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
        let entry = store.append_journal(
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
    crate::journey::validate_stable_id("drive Journey", name)?;
    let mut steps = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let utterance = entry
            .payload
            .get("utterance")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let intent = entry
            .payload
            .get("intent")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let action = utterance.or(intent).ok_or_else(|| {
            anyhow::anyhow!(
                "drive exchange {} has no semantic utterance or intent; cannot register a semantic Journey",
                index + 1
            )
        })?;
        let step_id = format!("drive-{}", index + 1);
        let expectation = intent
            .map(|value| format!("{value} is true"))
            .unwrap_or_else(|| format!("the requested outcome '{action}' is observable"));
        steps.push(crate::journey::JourneyStep {
            id: step_id,
            name: format!("Drive exchange {}", index + 1),
            action: action.into(),
            expects: vec![expectation],
            produces: BTreeMap::new(),
        });
    }
    let spec = crate::journey::JourneySpec {
        schema: crate::journey::JOURNEY_SCHEMA.into(),
        id: name.into(),
        name: format!("Drive: {name}"),
        actor: "operator".into(),
        goal: format!(
            "Semantic Journey captured from {} journaled drive exchange(s)",
            entries.len()
        ),
        description: None,
        inputs: BTreeMap::new(),
        preconditions: Vec::new(),
        steps,
        profiles: crate::journey::proof_profiles(),
    };
    spec.validate()?;

    let journeys = store.root().join("journeys");
    std::fs::create_dir_all(&journeys)
        .with_context(|| format!("creating {}", journeys.display()))?;
    let root = store
        .root()
        .canonicalize()
        .with_context(|| format!("resolving graph root {}", store.root().display()))?;
    let journeys = journeys
        .canonicalize()
        .with_context(|| format!("resolving {}", journeys.display()))?;
    if !journeys.starts_with(&root) {
        bail!(
            "Journey directory '{}' escapes graph root {}",
            journeys.display(),
            store.root().display()
        );
    }
    let path = journeys.join(format!("{name}.yaml"));
    if std::fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        bail!(
            "refusing to replace symlinked Journey artifact {}",
            path.display()
        );
    }
    std::fs::write(&path, serde_norway::to_string(&spec)?)
        .with_context(|| format!("writing semantic Journey {}", path.display()))?;

    // Registration owns hashing, update invalidation, and command output. The
    // drive's executable commands and observed streams remain only in its
    // journal evidence; they never cross into the authored Journey artifact.
    drop(store);
    super::journey::journey_add(graph, path, json)
}
