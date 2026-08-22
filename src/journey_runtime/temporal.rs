use crate::journey::{JourneySpec, RuntimeSource, SurfaceFileAction};
use crate::Result;
use anyhow::{anyhow, bail, Context};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use super::types::FileTransitionReport;
use super::values::{runtime_scalar_text, source_value};

pub(crate) struct RuntimeTemplateSources<'a> {
    pub(crate) spec: &'a JourneySpec,
    pub(crate) inputs: &'a BTreeMap<String, Value>,
    pub(crate) captures: &'a BTreeMap<String, Value>,
    pub(crate) redacted_captures: &'a BTreeSet<String>,
    pub(crate) run_id: &'a str,
}

pub(crate) struct TemporalOutcome {
    pub(crate) report: FileTransitionReport,
    pub(crate) detail: Option<String>,
}

pub(crate) fn apply_temporal_file_action(
    live_root: &Path,
    snapshot_root: &Path,
    step_id: &str,
    action: &SurfaceFileAction,
    sources: &RuntimeTemplateSources<'_>,
) -> Result<TemporalOutcome> {
    let live_root = live_root
        .canonicalize()
        .with_context(|| format!("canonicalizing live repository {}", live_root.display()))?;
    let snapshot_root = snapshot_root.canonicalize().with_context(|| {
        format!(
            "canonicalizing Journey snapshot {}",
            snapshot_root.display()
        )
    })?;
    if snapshot_root == live_root {
        bail!("temporal file actions refuse the live repository");
    }
    let snapshot = crate::store::Store::open_read(&snapshot_root)?;
    let path = action.resolve_for_store(&snapshot)?;
    drop(snapshot);
    let live_path = live_root.join(&action.path);
    if live_path
        .canonicalize()
        .ok()
        .is_some_and(|live_path| path.canonicalize().ok().as_ref() == Some(&live_path))
    {
        bail!(
            "temporal file action '{}' resolved to the live repository",
            action.path
        );
    }

    let before = std::fs::read_to_string(&path)
        .with_context(|| format!("reading temporal file '{}'", action.path))?;
    let observed_before_hash = crate::artifact::fingerprint(&before);
    if observed_before_hash != action.expected_hash {
        let report = FileTransitionReport {
            step_id: step_id.to_string(),
            path: action.path.clone(),
            expected_hash: action.expected_hash.clone(),
            observed_before_hash: observed_before_hash.clone(),
            observed_after_hash: observed_before_hash.clone(),
            changed: false,
            applied: false,
        };
        return Ok(TemporalOutcome {
            report,
            detail: Some(format!(
                "before_steps.{step_id} path '{}' expected prior hash '{}' but observed '{}'",
                action.path, action.expected_hash, observed_before_hash
            )),
        });
    }

    let replacement = match (&action.content, &action.template) {
        (Some(content), None) => content.clone(),
        (None, Some(template)) => render_temporal_template(template, sources)?,
        _ => {
            action.validate()?;
            unreachable!("SurfaceFileAction::validate accepts exactly one replacement")
        }
    };
    atomic_replace_temporal_file(&path, replacement.as_bytes())?;
    let observed_after = std::fs::read_to_string(&path)
        .with_context(|| format!("reading replaced temporal file '{}'", action.path))?;
    let observed_after_hash = crate::artifact::fingerprint(&observed_after);
    let expected_after_hash = crate::artifact::fingerprint(&replacement);
    if observed_after_hash != expected_after_hash {
        bail!(
            "temporal file action '{}' did not install the exact replacement bytes",
            action.path
        );
    }
    Ok(TemporalOutcome {
        report: FileTransitionReport {
            step_id: step_id.to_string(),
            path: action.path.clone(),
            expected_hash: action.expected_hash.clone(),
            observed_before_hash: observed_before_hash.clone(),
            observed_after_hash: observed_after_hash.clone(),
            changed: observed_before_hash != observed_after_hash,
            applied: true,
        },
        detail: None,
    })
}

fn render_temporal_template(
    template: &str,
    sources: &RuntimeTemplateSources<'_>,
) -> Result<String> {
    crate::journey::template_references(template)?;
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        rendered.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or_else(|| anyhow!("temporal template has an unterminated reference"))?;
        let source = after[..end].trim();
        match crate::journey::parse_runtime_source(source)? {
            RuntimeSource::Input(id) if sources.spec.inputs.get(id).is_some_and(|v| v.secret) => {
                bail!("secret input '{id}' cannot enter temporal file content")
            }
            RuntimeSource::StepOutput { .. } if sources.redacted_captures.contains(source) => {
                bail!("redacted output '{source}' cannot enter temporal file content")
            }
            _ => {}
        }
        let value = source_value(source, sources.inputs, sources.captures, sources.run_id)
            .ok_or_else(|| anyhow!("temporal template source '{source}' is unavailable"))?;
        let value = runtime_scalar_text(value.as_ref())
            .ok_or_else(|| anyhow!("temporal template source '{source}' is not scalar"))?;
        if value.contains('\0') {
            bail!("temporal template source '{source}' resolved a NUL byte");
        }
        rendered.push_str(&value);
        rest = &after[end + 2..];
    }
    rendered.push_str(rest);
    Ok(rendered)
}

fn atomic_replace_temporal_file(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("temporal file '{}' has no parent", path.display()))?;
    let permissions = std::fs::symlink_metadata(path)?.permissions();
    for sequence in 0..1000_u32 {
        let temporary = parent.join(format!(
            ".{}.loom-temporal-{}-{sequence}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("file"),
            std::process::id()
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        let result = (|| -> Result<()> {
            file.write_all(content)?;
            file.sync_all()?;
            std::fs::set_permissions(&temporary, permissions.clone())?;
            drop(file);
            std::fs::rename(&temporary, path)
                .with_context(|| format!("installing temporal file {}", path.display()))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        return result;
    }
    bail!(
        "could not allocate a temporal sibling for '{}'",
        path.display()
    )
}
