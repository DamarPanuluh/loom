//! Semantic Git checkpoint recommendations and isolated Journey fixtures.
//!
//! The public recommendation plane is read-only over graph truth, synchronized
//! source, and the Git working tree. The one internal fixture constructor may
//! initialize history only inside a runtime-verified local snapshot; it never
//! stages, commits, or pushes in the caller's repository.

use crate::model::{EdgeKind, GroundingRole, InspectionStatus, Node, NodeType};
use crate::store::Store;
use crate::Result;
use anyhow::{anyhow, bail, Context};
use process_control::{ChildExt, Control};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Component, Path};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const CHECKPOINT_RECOMMENDATION_SCHEMA: &str = "loom.checkpoint-recommendation/v1";
const GIT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const GIT_OUTPUT_CAP: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointStatus {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RepositoryIdentity {
    pub root: String,
    pub head: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CheckpointScope {
    pub intent_ids: Vec<String>,
    pub intent_names: Vec<String>,
    pub journey_ids: Vec<String>,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PathDecision {
    pub path: String,
    pub git_status: String,
    pub reason: String,
    pub intent_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckpointCheck {
    pub id: String,
    pub status: String,
    pub command: Option<String>,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckpointBlocker {
    pub kind: String,
    pub message: String,
    pub paths: Vec<String>,
    pub target_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalCommitPolicy {
    pub authority: &'static str,
    pub may_commit_or_defer: bool,
    pub stage_only_included_paths: bool,
    pub forbidden_command: &'static str,
    pub defer_on_ambiguity: bool,
    pub publication: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PushPolicy {
    pub allowed_without_human_decision: bool,
    pub required_binding: Vec<&'static str>,
    pub drift_requires_new_decision: bool,
    pub silence_or_refusal: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriverPolicy {
    pub local_commit: LocalCommitPolicy,
    pub push: PushPolicy,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckpointReport {
    pub schema: &'static str,
    pub status: CheckpointStatus,
    pub repository: RepositoryIdentity,
    pub scope: CheckpointScope,
    pub included_paths: Vec<PathDecision>,
    pub excluded_paths: Vec<PathDecision>,
    pub checks: Vec<CheckpointCheck>,
    pub suggested_message: Option<String>,
    pub driver_policy: DriverPolicy,
    pub blockers: Vec<CheckpointBlocker>,
}

#[derive(Debug, Clone)]
struct GitPathState {
    path: String,
    original_path: Option<String>,
    index: char,
    worktree: char,
}

impl GitPathState {
    fn status(&self) -> String {
        format!("{}{}", self.index, self.worktree)
    }

    fn staged(&self) -> bool {
        !matches!(self.index, ' ' | '?')
    }

    fn conflicted(&self) -> bool {
        self.index == 'U'
            || self.worktree == 'U'
            || matches!((self.index, self.worktree), ('A', 'A') | ('D', 'D'))
    }

    fn mixed(&self) -> bool {
        !matches!(self.index, ' ' | '?') && self.worktree != ' '
    }
}

#[derive(Debug, Clone)]
struct GitSnapshot {
    repository: RepositoryIdentity,
    paths: Vec<GitPathState>,
}

#[derive(Default)]
struct Assessment {
    repository: RepositoryIdentity,
    scope: CheckpointScope,
    included_paths: Vec<PathDecision>,
    excluded_paths: Vec<PathDecision>,
    checks: Vec<CheckpointCheck>,
    blockers: Vec<CheckpointBlocker>,
    common_journey: Option<Node>,
}

/// Inspect one exact Intent or cohesive bundle and return a deterministic,
/// read-only checkpoint recommendation. Callers must pass the intended scope;
/// Loom verifies it rather than guessing from a dirty tree.
pub fn recommend(store: &Store, intent_keys: &[String]) -> Result<CheckpointReport> {
    let assessment = assess_readiness(store, intent_keys)?;
    Ok(build_report(assessment))
}

/// The single fail-closed readiness decision behind the public projection.
fn assess_readiness(store: &Store, intent_keys: &[String]) -> Result<Assessment> {
    let mut assessment = Assessment::default();
    let mut intents = Vec::new();
    let mut seen = BTreeSet::new();

    if intent_keys.is_empty() {
        block(
            &mut assessment,
            "empty_scope",
            "checkpoint recommend requires at least one --intent",
            Vec::new(),
            Vec::new(),
        );
    }
    for key in intent_keys {
        match store.resolve_node(key, Some(NodeType::Intent)) {
            Ok(intent) if seen.insert(intent.id.clone()) => intents.push(intent),
            Ok(_) => {}
            Err(error) => block(
                &mut assessment,
                "unresolved_intent",
                error.to_string(),
                Vec::new(),
                vec![key.clone()],
            ),
        }
    }
    intents.sort_by(|a, b| a.id.cmp(&b.id));

    for intent in &intents {
        if intent.status != "implemented" {
            block(
                &mut assessment,
                "intent_not_implemented",
                format!(
                    "Intent '{}' is {}, not implemented",
                    intent.name, intent.status
                ),
                Vec::new(),
                vec![intent.id.clone()],
            );
        }
        if store.ratification(&intent.id)? != "ratified" {
            block(
                &mut assessment,
                "intent_not_ratified",
                format!("Intent '{}' is not currently ratified", intent.name),
                Vec::new(),
                vec![intent.id.clone()],
            );
        }
    }

    let selected: BTreeSet<String> = intents.iter().map(|intent| intent.id.clone()).collect();
    let common_journeys = common_journeys(store, &selected)?;
    assessment.common_journey = common_journeys.first().cloned();
    if selected.len() > 1
        && common_journeys.is_empty()
        && !relationship_connected(store, &selected)?
    {
        block(
            &mut assessment,
            "disconnected_scope",
            "selected Intents share neither one accepted Journey nor a connected requires/hierarchy subgraph",
            Vec::new(),
            selected.iter().cloned().collect(),
        );
    }

    assessment.scope = CheckpointScope {
        intent_ids: intents.iter().map(|intent| intent.id.clone()).collect(),
        intent_names: intents.iter().map(|intent| intent.name.clone()).collect(),
        journey_ids: common_journeys
            .iter()
            .filter_map(|journey| journey.body.get("stable_id").and_then(|v| v.as_str()))
            .map(str::to_owned)
            .collect(),
        rationale: scope_rationale(&intents, common_journeys.first()),
    };

    let git = match GitSnapshot::inspect(store.root()) {
        Ok(snapshot) => Some(snapshot),
        Err(error) => {
            block(
                &mut assessment,
                "git_unavailable",
                error.to_string(),
                Vec::new(),
                Vec::new(),
            );
            None
        }
    };
    if let Some(git) = &git {
        assessment.repository = git.repository.clone();
        if git.repository.branch.as_deref() == Some("HEAD") {
            block(
                &mut assessment,
                "detached_head",
                "repository is on a detached HEAD; defer the local checkpoint",
                Vec::new(),
                Vec::new(),
            );
        }
        classify_paths(store, &intents, &common_journeys, git, &mut assessment)?;
    }

    let included_paths = assessment.included_paths.clone();
    collect_validation_checks(store, &selected, &included_paths, &mut assessment)?;

    let sync_preview = crate::sync::preview(store, store.root())?;
    assessment.checks.push(CheckpointCheck {
        id: "loom_sync".into(),
        status: if sync_preview.fresh {
            "passed"
        } else {
            "failed"
        }
        .into(),
        command: Some("loom sync".into()),
        evidence: sync_preview.evidence_lines(),
    });
    if !sync_preview.fresh {
        block(
            &mut assessment,
            "sync_stale",
            "registered repository content does not match the synchronized graph",
            sync_preview.affected_paths(),
            Vec::new(),
        );
    }

    let doctor = crate::signal::doctor(store)?;
    assessment.checks.push(CheckpointCheck {
        id: "loom_doctor".into(),
        status: if doctor.is_empty() {
            "passed"
        } else {
            "failed"
        }
        .into(),
        command: Some("loom doctor".into()),
        evidence: doctor
            .iter()
            .map(|issue| format!("{}: {}", issue.kind, issue.message))
            .collect(),
    });
    if !doctor.is_empty() {
        block(
            &mut assessment,
            "doctor_issues",
            format!("loom doctor reports {} integrity issue(s)", doctor.len()),
            Vec::new(),
            Vec::new(),
        );
    }

    let export_fresh = crate::travel::export_is_fresh(store)?;
    assessment.checks.push(CheckpointCheck {
        id: "loom_export".into(),
        status: if export_fresh { "passed" } else { "failed" }.into(),
        command: Some("loom export --check".into()),
        evidence: vec![if export_fresh {
            "loom.graph.json matches the live graph".into()
        } else {
            "loom.graph.json is missing or stale".into()
        }],
    });
    if !export_fresh {
        block(
            &mut assessment,
            "export_stale",
            "loom.graph.json is missing or stale",
            vec![crate::GRAPH_EXPORT.into()],
            Vec::new(),
        );
    }

    assessment.checks.sort_by(|a, b| a.id.cmp(&b.id));
    assessment.blockers.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(a.message.cmp(&b.message))
            .then(a.paths.cmp(&b.paths))
    });
    Ok(assessment)
}

/// Project the readiness evidence without changing repository or graph state.
fn build_report(assessment: Assessment) -> CheckpointReport {
    let ready = assessment.blockers.is_empty();
    let suggested_message =
        ready.then(|| suggested_message(&assessment.scope, assessment.common_journey.as_ref()));
    CheckpointReport {
        schema: CHECKPOINT_RECOMMENDATION_SCHEMA,
        status: if ready {
            CheckpointStatus::Ready
        } else {
            CheckpointStatus::Blocked
        },
        repository: assessment.repository,
        scope: assessment.scope,
        included_paths: assessment.included_paths,
        excluded_paths: assessment.excluded_paths,
        checks: assessment.checks,
        suggested_message,
        driver_policy: driver_policy(),
        blockers: assessment.blockers,
    }
}

/// Guidance consumed by an acting LLM after Loom's read-only decision.
pub fn driver_policy() -> DriverPolicy {
    DriverPolicy {
        local_commit: LocalCommitPolicy {
            authority: "acting_llm",
            may_commit_or_defer: true,
            stage_only_included_paths: true,
            forbidden_command: "git add -A",
            defer_on_ambiguity: true,
            publication: "local_only",
        },
        push: push_policy(),
    }
}

/// Publication is a separate, human-authorized external action.
pub fn push_policy() -> PushPolicy {
    PushPolicy {
        allowed_without_human_decision: false,
        required_binding: vec!["repository", "remote", "branch", "commit"],
        drift_requires_new_decision: true,
        silence_or_refusal: "keep_local",
    }
}

fn common_journeys(store: &Store, selected: &BTreeSet<String>) -> Result<Vec<Node>> {
    let mut intersection: Option<BTreeSet<String>> = None;
    for intent_id in selected {
        let nodes = store
            .edges_with(Some(EdgeKind::Derives), None, Some(intent_id))?
            .into_iter()
            .map(|edge| store.get_node(&edge.from_id))
            .collect::<Result<Vec<_>>>()?;
        let ids: BTreeSet<String> = nodes
            .into_iter()
            .flatten()
            .filter(|node| node.node_type == NodeType::Journey && node.status == "authored")
            .map(|node| node.id)
            .collect();
        intersection = Some(match intersection {
            None => ids,
            Some(current) => current.intersection(&ids).cloned().collect(),
        });
    }
    let mut journeys = Vec::new();
    for id in intersection.unwrap_or_default() {
        if let Some(node) = store.get_node(&id)? {
            journeys.push(node);
        }
    }
    journeys.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(journeys)
}

fn relationship_connected(store: &Store, selected: &BTreeSet<String>) -> Result<bool> {
    let Some(start) = selected.first() else {
        return Ok(false);
    };
    let mut adjacent: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for kind in [EdgeKind::Requires, EdgeKind::Hierarchy] {
        for edge in store.list_edges(kind.into(), usize::MAX)? {
            if selected.contains(&edge.from_id) && selected.contains(&edge.to_id) {
                adjacent
                    .entry(edge.from_id.clone())
                    .or_default()
                    .insert(edge.to_id.clone());
                adjacent.entry(edge.to_id).or_default().insert(edge.from_id);
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([start.clone()]);
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id.clone()) {
            continue;
        }
        queue.extend(adjacent.get(&id).into_iter().flatten().cloned());
    }
    Ok(seen == *selected)
}

fn scope_rationale(intents: &[Node], journey: Option<&Node>) -> String {
    match journey {
        Some(journey) => format!(
            "{} implemented Intent(s) form the accepted '{}' Journey bundle",
            intents.len(),
            journey.name
        ),
        None if intents.len() == 1 => format!("implemented Intent '{}'", intents[0].name),
        None => "implemented Intents form one connected requires/hierarchy bundle".into(),
    }
}

fn classify_paths(
    store: &Store,
    intents: &[Node],
    common_journeys: &[Node],
    git: &GitSnapshot,
    assessment: &mut Assessment,
) -> Result<()> {
    let selected: BTreeSet<String> = intents.iter().map(|intent| intent.id.clone()).collect();
    let mut files_by_path = BTreeMap::new();
    for file in store.codefiles()? {
        files_by_path.insert(file.name.clone(), file);
    }
    let journey_artifacts: BTreeSet<String> = common_journeys
        .iter()
        .filter_map(|journey| journey.body.get("artifact").and_then(|v| v.as_str()))
        .map(str::to_owned)
        .collect();
    let mut explained_intents = BTreeSet::new();

    for changed in &git.paths {
        let mut path_intents = BTreeSet::new();
        let mut outside_realizers = BTreeSet::new();
        if let Some(file) = files_by_path.get(&changed.path) {
            for edge in store.edges_with(Some(EdgeKind::Implements), None, Some(&file.id))? {
                if store.edge_superseded(&edge.id)? {
                    continue;
                }
                if selected.contains(&edge.from_id) {
                    path_intents.insert(edge.from_id.clone());
                } else if store.grounding_role(&edge.id)? == GroundingRole::Realizes {
                    outside_realizers.insert(edge.from_id);
                }
            }
        }

        let projection = changed.path == crate::GRAPH_EXPORT;
        let journey_source = journey_artifacts.contains(&changed.path);
        let mut include = !path_intents.is_empty() || projection || journey_source;
        let mut reason = if projection {
            "portable graph projection for the selected scope".to_string()
        } else if journey_source {
            "authored Journey source for the selected scope".to_string()
        } else if !path_intents.is_empty() {
            format!(
                "grounded to selected Intent(s): {}",
                path_intents.iter().cloned().collect::<Vec<_>>().join(", ")
            )
        } else {
            "not grounded to the selected Intent scope".to_string()
        };

        if !outside_realizers.is_empty() && !path_intents.is_empty() {
            include = false;
            reason = format!(
                "shared with unselected realizing Intent(s): {}",
                outside_realizers
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            block(
                assessment,
                "ambiguous_path_ownership",
                format!(
                    "'{}' has selected and unselected realizing owners",
                    changed.path
                ),
                vec![changed.path.clone()],
                outside_realizers.iter().cloned().collect(),
            );
        }
        if changed.conflicted() {
            include = false;
            reason = "Git reports an unmerged/conflicted path".into();
            block(
                assessment,
                "git_conflict",
                format!("'{}' is conflicted", changed.path),
                vec![changed.path.clone()],
                Vec::new(),
            );
        }
        if !changed.conflicted() && changed.mixed() {
            include = false;
            reason = "path has both staged and unstaged changes".into();
            block(
                assessment,
                "mixed_path_state",
                format!("'{}' has both staged and unstaged changes", changed.path),
                vec![changed.path.clone()],
                Vec::new(),
            );
        } else if !changed.conflicted() && changed.staged() {
            // The index predates this read-only recommendation. Loom cannot
            // attribute that staging decision to the acting LLM, so treating
            // it as reusable would risk committing user-owned work.
            include = false;
            reason = "path is already staged; staging ownership is ambiguous".into();
        }

        let mut ids: Vec<String> = path_intents.into_iter().collect();
        ids.sort();
        let decision = PathDecision {
            path: changed.path.clone(),
            git_status: changed.status(),
            reason,
            intent_ids: ids.clone(),
        };
        if include {
            explained_intents.extend(ids);
            assessment.included_paths.push(decision);
        } else {
            if changed.staged() {
                block(
                    assessment,
                    "excluded_path_staged",
                    format!("excluded path '{}' is already staged", changed.path),
                    vec![changed.path.clone()],
                    Vec::new(),
                );
            }
            assessment.excluded_paths.push(decision);
        }
        if let Some(original) = &changed.original_path {
            assessment.excluded_paths.push(PathDecision {
                path: original.clone(),
                git_status: changed.status(),
                reason: format!(
                    "rename/copy source of included decision '{}'; review explicitly",
                    changed.path
                ),
                intent_ids: Vec::new(),
            });
        }
    }

    if assessment.included_paths.is_empty() {
        block(
            assessment,
            "empty_diff",
            "no changed path is safely attributable to the selected scope",
            Vec::new(),
            selected.iter().cloned().collect(),
        );
    }
    for intent in intents {
        if !explained_intents.contains(&intent.id) {
            block(
                assessment,
                "intent_without_changed_path",
                format!("Intent '{}' explains no included changed path", intent.name),
                Vec::new(),
                vec![intent.id.clone()],
            );
        }
    }
    assessment
        .included_paths
        .sort_by(|a, b| a.path.cmp(&b.path));
    assessment
        .excluded_paths
        .sort_by(|a, b| a.path.cmp(&b.path));
    Ok(())
}

fn collect_validation_checks(
    store: &Store,
    selected: &BTreeSet<String>,
    included: &[PathDecision],
    assessment: &mut Assessment,
) -> Result<()> {
    let files: BTreeMap<String, String> = store
        .codefiles()?
        .into_iter()
        .map(|file| (file.name, file.id))
        .collect();
    let mut validation_ids = BTreeSet::new();
    for intent_id in selected {
        validation_ids.extend(
            store
                .edges_with(Some(EdgeKind::Validates), None, Some(intent_id))?
                .into_iter()
                .map(|edge| edge.from_id),
        );
    }
    for path in included {
        let Some(file_id) = files.get(&path.path) else {
            continue;
        };
        validation_ids.extend(
            store
                .edges_with(Some(EdgeKind::Exercises), None, Some(file_id))?
                .into_iter()
                .map(|edge| edge.from_id),
        );
    }
    if validation_ids.is_empty() {
        block(
            assessment,
            "no_relevant_validations",
            "selected scope has no relevant registered Validation",
            Vec::new(),
            selected.iter().cloned().collect(),
        );
        return Ok(());
    }

    for validation_id in validation_ids {
        let Some(validation) = store.get_node(&validation_id)? else {
            block(
                assessment,
                "missing_validation",
                format!("relevant Validation '{validation_id}' is missing"),
                Vec::new(),
                vec![validation_id],
            );
            continue;
        };
        let validates = store.edges_with(Some(EdgeKind::Validates), Some(&validation.id), None)?;
        let relevant_edges: Vec<_> = validates
            .iter()
            .filter(|edge| selected.contains(&edge.to_id))
            .collect();
        let current = validation.status == "passed"
            && (!relevant_edges.is_empty()
                && relevant_edges
                    .iter()
                    .all(|edge| edge.status == InspectionStatus::Passing));
        let command = validation
            .body
            .get("command")
            .and_then(|value| value.as_str())
            .map(str::to_owned);
        assessment.checks.push(CheckpointCheck {
            id: format!("validation:{}", validation.id),
            status: if current {
                "passed"
            } else {
                validation.status.as_str()
            }
            .into(),
            command,
            evidence: relevant_edges
                .iter()
                .map(|edge| format!("validates:{}:{}", edge.to_id, edge.status))
                .collect(),
        });
        if !current {
            block(
                assessment,
                "validation_not_current",
                format!(
                    "Validation '{}' is not currently passing for the selected scope",
                    validation.name
                ),
                Vec::new(),
                vec![validation.id],
            );
        }
    }
    Ok(())
}

fn suggested_message(scope: &CheckpointScope, journey: Option<&Node>) -> String {
    if let Some(journey) = journey {
        let id = journey
            .body
            .get("stable_id")
            .and_then(|value| value.as_str())
            .unwrap_or(&journey.name);
        let name = journey
            .body
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or(&journey.name)
            .to_lowercase();
        return format!("feat({id}): {name}");
    }
    format!(
        "feat: {}",
        scope
            .intent_names
            .first()
            .cloned()
            .unwrap_or_else(|| "semantic checkpoint".into())
    )
}

fn block(
    assessment: &mut Assessment,
    kind: &str,
    message: impl Into<String>,
    mut paths: Vec<String>,
    mut target_ids: Vec<String>,
) {
    paths.sort();
    paths.dedup();
    target_ids.sort();
    target_ids.dedup();
    assessment.blockers.push(CheckpointBlocker {
        kind: kind.into(),
        message: message.into(),
        paths,
        target_ids,
    });
}

impl GitSnapshot {
    fn inspect(root: &Path) -> Result<Self> {
        let top = git_text(root, &["rev-parse", "--show-toplevel"])?;
        let canonical_root = root
            .canonicalize()
            .with_context(|| format!("canonicalizing graph root {}", root.display()))?;
        let canonical_top = Path::new(top.trim())
            .canonicalize()
            .with_context(|| format!("canonicalizing Git root {}", top.trim()))?;
        if canonical_root != canonical_top {
            return Err(anyhow!(
                "graph root '{}' is not the Git top-level '{}'",
                canonical_root.display(),
                canonical_top.display()
            ));
        }
        let head = git_text(root, &["rev-parse", "--verify", "HEAD"])?;
        let branch = git_text(root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        let raw = git_bytes(
            root,
            &[
                "-c",
                "core.quotepath=false",
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ],
        )?;
        Ok(Self {
            repository: RepositoryIdentity {
                root: canonical_root.to_string_lossy().into_owned(),
                head: Some(head.trim().into()),
                branch: Some(branch.trim().into()),
            },
            paths: parse_porcelain_v1_z(&raw)?,
        })
    }
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    String::from_utf8(git_bytes(root, args)?).context("Git output is not UTF-8")
}

fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    git_bytes_with_environment(root, args, false)
}

fn isolated_git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    git_bytes_with_environment(root, args, true)
}

fn isolated_git_tracked_paths(root: &Path) -> Result<BTreeSet<String>> {
    isolated_git_bytes(root, &["ls-files", "-z"])?
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(|field| {
            std::str::from_utf8(field)
                .context("isolated Git tracked path is not UTF-8")
                .map(str::to_owned)
        })
        .collect()
}

fn git_bytes_with_environment(root: &Path, args: &[&str], isolated: bool) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if isolated {
        let essentials: Vec<(&str, std::ffi::OsString)> = [
            "PATH",
            "TMPDIR",
            "TEMP",
            "TMP",
            "SYSTEMROOT",
            "WINDIR",
            "PATHEXT",
            "COMSPEC",
        ]
        .into_iter()
        .filter_map(|key| std::env::var_os(key).map(|value| (key, value)))
        .collect();
        command
            .env_clear()
            .envs(essentials)
            .env("LC_ALL", "C")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "true")
            .env("GIT_AUTHOR_NAME", "Loom Journey Fixture")
            .env("GIT_AUTHOR_EMAIL", "journey-fixture@invalid")
            .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
            .env("GIT_COMMITTER_NAME", "Loom Journey Fixture")
            .env("GIT_COMMITTER_EMAIL", "journey-fixture@invalid")
            .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z");
    } else {
        command.env("LC_ALL", "C").env("GIT_OPTIONAL_LOCKS", "0");
    }
    let child = command
        .spawn()
        .with_context(|| format!("starting git {}", args.join(" ")))?;
    let capped = Arc::new(AtomicBool::new(false));
    let capped_flag = Arc::clone(&capped);
    let mut accepted = 0usize;
    let filter = move |chunk: &[u8]| -> std::io::Result<bool> {
        accepted = accepted.saturating_add(chunk.len());
        if accepted > GIT_OUTPUT_CAP {
            capped_flag.store(true, Ordering::Relaxed);
            return Ok(false);
        }
        Ok(true)
    };
    let output = child
        .controlled_with_output()
        .time_limit(GIT_TIMEOUT)
        .stdout_filter(filter)
        .terminate_for_timeout()
        .wait()
        .with_context(|| format!("waiting for git {}", args.join(" ")))?
        .ok_or_else(|| anyhow!("git {} exceeded {}s", args.join(" "), GIT_TIMEOUT.as_secs()))?;
    if capped.load(Ordering::Relaxed) || output.stdout.len() > GIT_OUTPUT_CAP {
        return Err(anyhow!("git {} exceeded the output cap", args.join(" ")));
    }
    if !output.status.success() {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

/// Build a deterministic, one-commit Git fixture only inside a trusted local
/// snapshot. The committed tree contains exactly `dirty_paths`; everything
/// else (including `.loom` and unlisted secrets) is ignored. File bytes remain
/// unchanged while their executable bit is toggled after the baseline commit,
/// yielding exact unstaged evidence for checkpoint recommendation.
pub(crate) fn materialize_isolated_git_snapshot(
    live_root: &Path,
    snapshot_root: &Path,
    dirty_paths: &[String],
) -> Result<()> {
    let live_root = live_root
        .canonicalize()
        .with_context(|| format!("canonicalizing live graph root {}", live_root.display()))?;
    let snapshot_root = snapshot_root.canonicalize().with_context(|| {
        format!(
            "canonicalizing isolated Journey root {}",
            snapshot_root.display()
        )
    })?;
    let temporary_root = std::env::temp_dir()
        .canonicalize()
        .context("canonicalizing system temporary root")?;
    if snapshot_root == live_root
        || snapshot_root.starts_with(&live_root)
        || !snapshot_root.starts_with(&temporary_root)
    {
        bail!(
            "refusing to initialize isolated Git outside detached temporary root '{}'",
            temporary_root.display()
        );
    }
    if !snapshot_root.join(".loom/graph.sqlite").is_file() {
        bail!("isolated Git fixture requires a cloned local snapshot");
    }
    if snapshot_root.join(".git").exists() {
        bail!("isolated Git fixture snapshot already contains Git state");
    }
    if dirty_paths.is_empty() {
        bail!("isolated Git fixture requires at least one dirty path");
    }

    let mut expected = BTreeSet::new();
    let mut original_bytes = BTreeMap::new();
    for path in dirty_paths {
        validate_fixture_path(path)?;
        if !expected.insert(path.clone()) {
            bail!("isolated Git fixture repeats dirty path '{path}'");
        }
        let file = snapshot_root.join(path);
        let metadata = std::fs::symlink_metadata(&file)
            .with_context(|| format!("reading isolated Git fixture path '{path}'"))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            bail!("isolated Git fixture path '{path}' is not a regular file");
        }
        let canonical_file = file
            .canonicalize()
            .with_context(|| format!("canonicalizing isolated Git fixture path '{path}'"))?;
        if !canonical_file.starts_with(&snapshot_root) {
            bail!("isolated Git fixture path '{path}' escapes the snapshot");
        }
        original_bytes.insert(
            path.clone(),
            std::fs::read(&file)
                .with_context(|| format!("reading isolated Git fixture path '{path}'"))?,
        );
    }

    isolated_git_bytes(&snapshot_root, &["init", "--quiet", "--template="])?;
    isolated_git_bytes(
        &snapshot_root,
        &["symbolic-ref", "HEAD", "refs/heads/journey-fixture"],
    )?;
    isolated_git_bytes(
        &snapshot_root,
        &["config", "--local", "core.filemode", "true"],
    )?;
    isolated_git_bytes(
        &snapshot_root,
        &["config", "--local", "commit.gpgsign", "false"],
    )?;
    isolated_git_bytes(
        &snapshot_root,
        &["config", "--local", "core.hookspath", "/dev/null"],
    )?;
    std::fs::create_dir_all(snapshot_root.join(".git/info"))
        .context("creating isolated Git exclude directory")?;
    std::fs::write(snapshot_root.join(".git/info/exclude"), "/*\n")
        .context("excluding non-fixture files from isolated Git")?;

    let mut add_args = vec!["add", "--force", "--"];
    add_args.extend(dirty_paths.iter().map(String::as_str));
    isolated_git_bytes(&snapshot_root, &add_args)?;
    let tracked = isolated_git_tracked_paths(&snapshot_root)?;
    if tracked != expected {
        bail!("isolated Git fixture index contains paths outside the declared scope");
    }
    isolated_git_bytes(
        &snapshot_root,
        &[
            "-c",
            "commit.gpgSign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "--quiet",
            "--no-verify",
            "--no-gpg-sign",
            "-m",
            "Loom Journey fixture baseline",
        ],
    )?;

    for path in dirty_paths {
        toggle_fixture_executable_bit(&snapshot_root.join(path))?;
        if std::fs::read(snapshot_root.join(path))? != original_bytes[path] {
            bail!("isolated Git fixture changed bytes for '{path}'");
        }
    }

    verify_isolated_git_snapshot(&snapshot_root, dirty_paths)
}

/// Recheck the fixture after declarative setup operations so no operation can
/// broaden the index, rewrite the baseline, configure publication, or turn the
/// declared mode-only evidence into content changes.
pub(crate) fn verify_isolated_git_snapshot(
    snapshot_root: &Path,
    dirty_paths: &[String],
) -> Result<()> {
    let mut expected = BTreeSet::new();
    for path in dirty_paths {
        validate_fixture_path(path)?;
        if !expected.insert(path.clone()) {
            bail!("isolated Git fixture repeats dirty path '{path}'");
        }
    }
    let tracked = isolated_git_tracked_paths(snapshot_root)?;
    if tracked != expected {
        bail!("isolated Git fixture index contains paths outside the declared scope");
    }
    let commit_count = git_text_isolated(snapshot_root, &["rev-list", "--count", "HEAD"])?;
    if commit_count.trim() != "1" {
        bail!("isolated Git fixture must contain exactly one baseline commit");
    }
    if !git_text_isolated(snapshot_root, &["remote"])?
        .trim()
        .is_empty()
    {
        bail!("isolated Git fixture must not configure a remote");
    }
    let status = parse_porcelain_v1_z(&isolated_git_bytes(
        snapshot_root,
        &[
            "-c",
            "core.quotepath=false",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?)?;
    let observed: BTreeSet<String> = status.iter().map(|row| row.path.clone()).collect();
    if observed != expected
        || status
            .iter()
            .any(|row| row.index != ' ' || row.worktree != 'M')
    {
        bail!("isolated Git fixture did not produce the exact unstaged dirty scope");
    }
    for path in dirty_paths {
        let revision = format!("HEAD:{path}");
        let baseline = git_text_isolated(snapshot_root, &["rev-parse", &revision])?;
        let current = git_text_isolated(snapshot_root, &["hash-object", "--", path])?;
        if baseline.trim() != current.trim() {
            bail!("isolated Git fixture content changed for '{path}'");
        }
    }
    Ok(())
}

fn git_text_isolated(root: &Path, args: &[&str]) -> Result<String> {
    String::from_utf8(isolated_git_bytes(root, args)?).context("Git output is not UTF-8")
}

fn validate_fixture_path(path: &str) -> Result<()> {
    let value = Path::new(path);
    if path.is_empty()
        || path.trim() != path
        || path.contains('\\')
        || value.is_absolute()
        || value
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        bail!("isolated Git fixture path '{path}' is unsafe");
    }
    let normalized = value
        .components()
        .filter_map(|part| match part {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    let reserved = value.components().any(|part| match part {
        Component::Normal(value) => matches!(value.to_str(), Some(".loom" | ".git")),
        _ => false,
    });
    if normalized != path || reserved {
        bail!("isolated Git fixture path '{path}' is unsafe");
    }
    Ok(())
}

#[cfg(unix)]
fn toggle_fixture_executable_bit(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)?.permissions();
    let mode = permissions.mode();
    let fixture_mode = if mode & 0o111 == 0 {
        mode | 0o100
    } else {
        mode & !0o111
    };
    permissions.set_mode(fixture_mode);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn toggle_fixture_executable_bit(_path: &Path) -> Result<()> {
    bail!("isolated Git Journey fixtures require Unix file modes")
}

fn parse_porcelain_v1_z(bytes: &[u8]) -> Result<Vec<GitPathState>> {
    let fields: Vec<&[u8]> = bytes.split(|byte| *byte == 0).collect();
    let mut out = Vec::new();
    let mut index = 0usize;
    while index < fields.len() {
        let field = fields[index];
        index += 1;
        if field.is_empty() {
            continue;
        }
        if field.len() < 4 || field[2] != b' ' {
            return Err(anyhow!("malformed git status record"));
        }
        let x = char::from(field[0]);
        let y = char::from(field[1]);
        let path = std::str::from_utf8(&field[3..])
            .context("Git path is not UTF-8")?
            .to_string();
        validate_git_path(&path)?;
        let renamed = matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C');
        let original_path = if renamed {
            let original = fields
                .get(index)
                .ok_or_else(|| anyhow!("rename/copy status lacks its original path"))?;
            index += 1;
            let original = std::str::from_utf8(original)
                .context("Git original path is not UTF-8")?
                .to_string();
            validate_git_path(&original)?;
            Some(original)
        } else {
            None
        };
        out.push(GitPathState {
            path,
            original_path,
            index: x,
            worktree: y,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn validate_git_path(path: &str) -> Result<()> {
    let value = Path::new(path);
    if path.is_empty()
        || value.is_absolute()
        || value.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(anyhow!("Git returned an unsafe repository path '{path}'"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_parser_is_nul_safe_and_keeps_rename_sources() {
        let rows = parse_porcelain_v1_z(
            b" M src/a b.rs\0R  src/new.rs\0src/old.rs\0?? notes/new\nfile.md\0",
        )
        .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].path, "notes/new\nfile.md");
        assert_eq!(rows[1].path, "src/a b.rs");
        assert_eq!(rows[2].original_path.as_deref(), Some("src/old.rs"));
    }

    #[test]
    fn acting_llm_local_checkpoint_policy_is_exact_and_deferrable() {
        let policy = driver_policy().local_commit;
        assert_eq!(policy.authority, "acting_llm");
        assert!(policy.may_commit_or_defer);
        assert!(policy.stage_only_included_paths);
        assert_eq!(policy.forbidden_command, "git add -A");
        assert!(policy.defer_on_ambiguity);
        assert_eq!(policy.publication, "local_only");
    }

    #[test]
    fn push_policy_requires_exact_current_human_authorization() {
        let policy = push_policy();
        assert!(!policy.allowed_without_human_decision);
        assert_eq!(
            policy.required_binding,
            ["repository", "remote", "branch", "commit"]
        );
        assert!(policy.drift_requires_new_decision);
        assert_eq!(policy.silence_or_refusal, "keep_local");
    }
}
