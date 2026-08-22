use super::spec::template_references;
use crate::Result;
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

/// Journey-specific preparation for an accepted reusable surface. The manifest
/// exposes only the graph source and ordered operation ids; cloning, confinement,
/// execution, and evidence accounting remain runtime implementation details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceSetup {
    pub graph: SetupGraph,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<SurfaceGitSetup>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub before_steps: BTreeMap<String, Vec<SurfaceFileAction>>,
    pub operations: Vec<String>,
}

/// One exact file transition applied only inside the trusted local snapshot,
/// immediately before the keyed authored step. Literal content and templates
/// are separate so source files containing `{{ ... }}` remain representable
/// without accidentally becoming runtime interpolation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceFileAction {
    pub path: String,
    pub expected_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupGraph {
    LocalSnapshot,
}

/// Optional Git state materialized only inside the runtime's trusted local
/// snapshot. The manifest names evidence paths; repository initialization,
/// history construction, confinement, and teardown remain runtime details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceGitSetup {
    pub mode: SurfaceGitMode,
    pub dirty_paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceGitMode {
    IsolatedSnapshot,
}

impl SurfaceGitSetup {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.dirty_paths.is_empty() {
            bail!("surface setup git.dirty_paths must not be empty");
        }
        let mut seen = BTreeSet::new();
        for path in &self.dirty_paths {
            validate_surface_git_path(path)?;
            if !seen.insert(path.as_str()) {
                bail!("surface setup git.dirty_paths repeats path '{path}'");
            }
        }
        Ok(())
    }

    pub(crate) fn validate_for_store(&self, store: &crate::store::Store) -> Result<()> {
        self.validate()?;
        let root = store
            .root()
            .canonicalize()
            .with_context(|| format!("canonicalizing graph root {}", store.root().display()))?;
        let registered: BTreeSet<String> = store
            .list_nodes(Some(crate::model::NodeType::CodeFile), usize::MAX)?
            .into_iter()
            .map(|node| node.name)
            .collect();
        for path in &self.dirty_paths {
            if !registered.contains(path) {
                bail!("surface setup git dirty path '{path}' is not a registered CodeFile");
            }
            let file = store.root().join(path);
            let metadata = std::fs::symlink_metadata(&file)
                .with_context(|| format!("reading surface setup git dirty path '{path}'"))?;
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                bail!("surface setup git dirty path '{path}' is not a file");
            }
            if !file
                .canonicalize()
                .with_context(|| format!("canonicalizing surface setup git dirty path '{path}'"))?
                .starts_with(&root)
            {
                bail!("surface setup git dirty path '{path}' escapes the graph root");
            }
        }
        Ok(())
    }
}

impl SurfaceFileAction {
    pub(crate) fn validate(&self) -> Result<()> {
        validate_surface_temporal_path(&self.path)?;
        if self.expected_hash.len() != 16
            || !self
                .expected_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            bail!(
                "surface setup temporal path '{}' expected_hash must be a lowercase 16-digit content fingerprint",
                self.path
            );
        }
        match (&self.content, &self.template) {
            (Some(content), None) => {
                if content.contains("${{") {
                    bail!(
                        "surface setup temporal path '{}' content must not contain '${{{{' syntax",
                        self.path
                    );
                }
                Ok(())
            }
            (None, Some(template)) => {
                if template.contains("${{") {
                    bail!(
                        "surface setup temporal path '{}' template must not contain '${{{{' syntax",
                        self.path
                    );
                }
                template_references(template).map(|_| ())
            }
            (Some(_), Some(_)) => bail!(
                "surface setup temporal path '{}' must declare exactly one of content or template",
                self.path
            ),
            (None, None) => bail!(
                "surface setup temporal path '{}' must declare content or template",
                self.path
            ),
        }
    }

    pub(crate) fn resolve_for_store(&self, store: &crate::store::Store) -> Result<PathBuf> {
        self.validate()?;
        let registered = store
            .list_nodes(Some(crate::model::NodeType::CodeFile), usize::MAX)?
            .into_iter()
            .any(|node| node.name == self.path);
        if !registered {
            bail!(
                "surface setup temporal path '{}' is not a registered CodeFile",
                self.path
            );
        }
        confined_regular_file(store.root(), &self.path, "surface setup temporal path")
    }
}

impl SurfaceSetup {
    pub(super) fn has_temporal_actions(&self) -> bool {
        self.before_steps
            .values()
            .any(|actions| !actions.is_empty())
    }

    pub(crate) fn validate_for_store(&self, store: &crate::store::Store) -> Result<()> {
        if let Some(git) = &self.git {
            git.validate_for_store(store)?;
        }
        for actions in self.before_steps.values() {
            for action in actions {
                action.resolve_for_store(store)?;
            }
        }
        Ok(())
    }
}

fn validate_surface_temporal_path(path: &str) -> Result<()> {
    validate_confined_surface_path("surface setup temporal", path)
}

fn validate_surface_git_path(path: &str) -> Result<()> {
    validate_confined_surface_path("surface setup git dirty", path)
}

fn validate_confined_surface_path(label: &str, path: &str) -> Result<()> {
    if path.is_empty() || path.trim() != path || path.contains('\\') {
        bail!("{label} path '{path}' is not a normalized relative path");
    }
    let value = Path::new(path);
    if value.is_absolute()
        || value
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("{label} path '{path}' is not a normalized relative path");
    }
    let normalized = value
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if normalized != path {
        bail!("{label} path '{path}' is not a normalized relative path");
    }
    if value.components().any(|component| match component {
        Component::Normal(value) => matches!(value.to_str(), Some(".loom" | ".git")),
        _ => false,
    }) {
        bail!("{label} path '{path}' targets reserved state");
    }
    Ok(())
}

fn confined_regular_file(root: &Path, path: &str, label: &str) -> Result<PathBuf> {
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing graph root {}", root.display()))?;
    let mut current = root.to_path_buf();
    let components: Vec<_> = Path::new(path).components().collect();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            bail!("{label} '{path}' is not a normalized relative path");
        };
        current.push(component);
        let metadata = std::fs::symlink_metadata(&current)
            .with_context(|| format!("reading {label} '{path}'"))?;
        if metadata.file_type().is_symlink() {
            bail!("{label} '{path}' traverses a symlink");
        }
        let last = index + 1 == components.len();
        if (last && !metadata.file_type().is_file()) || (!last && !metadata.is_dir()) {
            bail!("{label} '{path}' is not a regular file");
        }
    }
    if !current
        .canonicalize()
        .with_context(|| format!("canonicalizing {label} '{path}'"))?
        .starts_with(&canonical_root)
    {
        bail!("{label} '{path}' escapes the graph root");
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_file_actions_reject_dollar_template_syntax_without_changing_valid_semantics() {
        let action = |content: Option<&str>, template: Option<&str>| SurfaceFileAction {
            path: "src/example.rs".into(),
            expected_hash: "0123456789abcdef".into(),
            content: content.map(str::to_owned),
            template: template.map(str::to_owned),
        };

        let content_error = action(Some("literal ${{ inputs.topic }}"), None)
            .validate()
            .unwrap_err();
        assert!(content_error
            .to_string()
            .contains("content must not contain"));

        let template_error = action(None, Some("${{ inputs.topic }}"))
            .validate()
            .unwrap_err();
        assert!(template_error
            .to_string()
            .contains("template must not contain"));

        action(Some("literal {{ inputs.topic }}"), None)
            .validate()
            .unwrap();
        action(None, Some("runtime {{ inputs.topic }}"))
            .validate()
            .unwrap();
    }
}
