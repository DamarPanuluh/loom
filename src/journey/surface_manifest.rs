use super::lint::{JourneyLintFinding, JourneyLintReport, JourneyLintSeverity};
use super::sources::{validate_operation_references, validate_temporal_action_references};
use super::spec::{canonicalize_value, JourneyInput, JourneySpec};
use super::surface_ops::{
    CliOperation, InterfaceSurfaceDefinition, OutputAssertion, SurfaceBinding,
};
use super::surface_setup::{SetupGraph, SurfaceSetup};
use super::{JOURNEY_COMPILER_VERSION, SURFACE_SCHEMA};
use crate::Result;
use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceManifest {
    pub schema: String,
    pub journey_id: String,
    pub journey_hash: String,
    pub surface: InterfaceSurfaceDefinition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<SurfaceSetup>,
    pub bindings: Vec<SurfaceBinding>,
}

/// Both id shapes the graph mints: asserted nodes and edges carry a 32-hex
/// `randomblob(16)`, derived nodes a 17-hex content fingerprint. Checking only
/// the 32-hex form left every derived identity — the smell findings a fixture
/// is most tempted to adjudicate by id — invisible to the lint, and those are
/// the *least* stable ids in the graph: they are recomputed from the code, so
/// an ordinary source edit retires them.
fn is_exact_graph_identity(text: &str) -> bool {
    matches!(text.len(), 17 | 32) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_undeclared_graph_identity(store: &crate::store::Store, text: &str) -> Result<bool> {
    if !is_exact_graph_identity(text) {
        return Ok(false);
    }
    Ok(store.get_node(text)?.is_none() && store.get_edge(text)?.is_none())
}

/// Every maximal hex run in `text` that has an identity's exact shape.
///
/// An argv token is not always the identity itself: an operation that drives a
/// batch — `loom mcp transcript --requests-json '[…]'`, an inline `loom apply`
/// fragment — carries its ids *inside* one JSON argument, where a whole-token
/// check sees only a long string and reports nothing. Those are exactly the
/// fixtures that adjudicate findings by id, so the rule was blind where it was
/// needed most. Boundaries are enforced on both sides so a longer hash is
/// never mistaken for an identity.
fn embedded_graph_identities(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = None;
    for index in 0..=bytes.len() {
        let is_hex = index < bytes.len() && bytes[index].is_ascii_hexdigit();
        match (is_hex, start) {
            (true, None) => start = Some(index),
            (false, Some(begin)) => {
                if is_exact_graph_identity(&text[begin..index]) {
                    out.push(&text[begin..index]);
                }
                start = None;
            }
            _ => {}
        }
    }
    out
}

fn value_contains_undeclared_graph_identity(
    store: &crate::store::Store,
    value: &Value,
) -> Result<bool> {
    match value {
        Value::String(text) => is_undeclared_graph_identity(store, text),
        Value::Array(values) => {
            for value in values {
                if value_contains_undeclared_graph_identity(store, value)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Value::Object(values) => {
            for value in values.values() {
                if value_contains_undeclared_graph_identity(store, value)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

/// How many requests this operation authored, if it drives an MCP transcript.
///
/// An operation that carries its own `--requests-json` decides both the number
/// of requests and the order of the replies. Counts and indices derived from
/// that list are fixed by the fixture, not observed from the graph, so flagging
/// them is a false positive — and these were the overwhelming majority. A rule
/// that cries wolf 326 times is a rule nobody reads, which is how the one pin
/// that actually mattered (a release inventory pinned to the repository's exact
/// file count) sat unnoticed until it cost a sealed authority token.
fn authored_transcript_len(operation: &CliOperation) -> Option<usize> {
    let mut argv = operation.argv.iter();
    while let Some(arg) = argv.next() {
        if arg == "--requests-json" {
            let raw = argv.next()?;
            return crate::mcp::inspect_transcript_requests(raw)
                .ok()
                .map(|r| r.len());
        }
    }
    None
}

fn exact_census_pin(assertion: &OutputAssertion, operation: &CliOperation) -> bool {
    let Some(value) = &assertion.equals else {
        return false;
    };
    // `request_count`/`response_count` over a transcript the fixture wrote is
    // arithmetic on its own input, not a census of the graph.
    if matches!(
        assertion.pointer.as_str(),
        "/request_count" | "/response_count"
    ) {
        if let (Some(authored), Some(pinned)) = (authored_transcript_len(operation), value.as_u64())
        {
            if authored as u64 == pinned {
                return false;
            }
        }
    }
    let census_name = assertion
        .pointer
        .split('/')
        .next_back()
        .is_some_and(|segment| {
            let segment = segment.to_ascii_lowercase();
            matches!(
                segment.as_str(),
                "count" | "counts" | "total" | "totals" | "census"
            ) || segment.ends_with("_count")
                || segment.ends_with("_total")
        });
    census_name && (value.is_number() || value.is_array() || value.is_object())
}

fn positional_census_pointer(pointer: &str, operation: &CliOperation) -> bool {
    let segments: Vec<&str> = pointer.split('/').skip(1).collect();
    // `/responses/N/...` indexes the replies to the fixture's own authored
    // requests, one for one. That index is as fixed as the request list itself,
    // so long as it addresses a request that was actually authored.
    if let (Some(&"responses"), Some(index), Some(authored)) = (
        segments.first(),
        segments.get(1).and_then(|s| s.parse::<usize>().ok()),
        authored_transcript_len(operation),
    ) {
        if index < authored {
            // Anything deeper is still positional into whatever the reply
            // contained, which the fixture does not author — except the tool
            // reply envelope itself. `mcp::tool_content` is the sole builder of
            // a tool result and always emits exactly one content element, so
            // `/result/content/0` is fixed by loom's own protocol code rather
            // than by graph state. A *different* content index would be a real
            // mistake and still fires.
            let tail = &segments[2..];
            // `/result/content/0` is the tool reply envelope; skip exactly that
            // prefix and judge whatever the fixture reached into beyond it.
            let body = match tail {
                ["result", "content", "0", rest @ ..] => rest,
                _ => tail,
            };
            return body
                .iter()
                .any(|segment| !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()));
        }
    }
    segments
        .iter()
        .any(|segment| !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()))
}

fn relies_on_real_clock_minute_bucket(operation: &CliOperation) -> bool {
    // The known self-audit fixture creates many adjudications and audits the
    // resulting judgment burst in one MCP transcript. Audit groups that burst
    // by the host clock's current minute, so crossing a minute is correctness-
    // affecting. Requiring both structural signals avoids matching prose.
    let joined = operation.argv.join(" ");
    joined.contains("\"adjudications\"")
        && (joined.contains("[\"loom\",\"audit\",\"--json\"]")
            || operation
                .argv
                .windows(2)
                .any(|parts| parts == ["loom", "audit"]))
}

impl SurfaceManifest {
    /// Static portability and durability policy shared by lint and acceptance.
    /// Call only after schema validation and setup confinement validation.
    pub fn lint(
        &self,
        store: &crate::store::Store,
        journey: &JourneySpec,
        manifest_path: &str,
    ) -> Result<JourneyLintReport> {
        let mut findings = Vec::new();
        let mut add = |rule: &str,
                       severity,
                       operation: Option<&str>,
                       assertion: Option<&str>,
                       message: String| {
            findings.push(JourneyLintFinding {
                rule: rule.into(),
                severity,
                journey_id: self.journey_id.clone(),
                manifest_path: manifest_path.into(),
                operation: operation.map(str::to_owned),
                assertion: assertion.map(str::to_owned),
                message,
            });
        };
        for operation in &self.surface.operations {
            for arg in &operation.argv {
                let mut reported = false;
                for candidate in embedded_graph_identities(arg) {
                    if is_undeclared_graph_identity(store, candidate)? {
                        add("graph-local-identity", JourneyLintSeverity::Blocking, Some(&operation.id), None, format!("replace the undeclared graph identity '{candidate}' in argv with a repository-declared identity, stable name, or captured value"));
                        reported = true;
                        break;
                    }
                }
                if reported {
                    break;
                }
            }
            if relies_on_real_clock_minute_bucket(operation) {
                add("real-clock-minute-bucket", JourneyLintSeverity::Advisory, Some(&operation.id), None, "replace the real-clock judgment-burst/minute-bucket fixture with deterministic clock-controlled evidence".into());
            }
            for assertion in &operation.output.assertions {
                // A compiler-version pin is a deliberate tripwire — a bump
                // means every proof must be recompiled — but it is only a
                // tripwire while it names the version that is actually
                // current. Left stale it is the opposite: an assertion that
                // refuses a correct run, and one whose repair the runtime can
                // compute exactly. Three of these sat at "3" from compiler v3
                // through v6 because no release run ever reached them.
                if assertion.pointer.ends_with("/compiler_version") {
                    if let Some(Value::String(pinned)) = &assertion.equals {
                        if pinned != JOURNEY_COMPILER_VERSION {
                            add("stale-compiler-version-pin", JourneyLintSeverity::Blocking, Some(&operation.id), Some(&assertion.id), format!("update the pinned Journey compiler version '{pinned}' to the current '{JOURNEY_COMPILER_VERSION}'"));
                        }
                    }
                }
                let undeclared_equals = match &assertion.equals {
                    Some(value) => value_contains_undeclared_graph_identity(store, value)?,
                    None => false,
                };
                let mut undeclared_pointer = false;
                for segment in assertion.pointer.split('/') {
                    if is_undeclared_graph_identity(store, segment)? {
                        undeclared_pointer = true;
                        break;
                    }
                }
                if undeclared_equals || undeclared_pointer {
                    add("graph-local-identity", JourneyLintSeverity::Blocking, Some(&operation.id), Some(&assertion.id), "replace the undeclared 32-hex identity with a repository-declared identity, stable name, or captured value".into());
                }
                if exact_census_pin(assertion, operation) {
                    add("exact-census-pin", JourneyLintSeverity::Advisory, Some(&operation.id), Some(&assertion.id), "assert an invariant or bounded relationship instead of an exact whole-graph count or total".into());
                }
                if positional_census_pointer(&assertion.pointer, operation) {
                    add("positional-census-pointer", JourneyLintSeverity::Advisory, Some(&operation.id), Some(&assertion.id), "select census data by stable identity instead of a numeric JSON-pointer position".into());
                }
                if assertion.not_equals_value() == Some(Value::String(String::new())) {
                    add("not-equals-empty", JourneyLintSeverity::Advisory, Some(&operation.id), Some(&assertion.id), "use an explicit existence, type, or semantic assertion instead of not_equals empty string".into());
                }
            }
        }
        if let Some(setup) = &self.setup {
            let mut transitioned_paths = BTreeSet::new();
            for step in &journey.steps {
                let Some(actions) = setup.before_steps.get(&step.id) else {
                    continue;
                };
                for action in actions {
                    if !transitioned_paths.insert(action.path.clone()) {
                        continue;
                    }
                    let content = std::fs::read_to_string(action.resolve_for_store(store)?)?;
                    if crate::artifact::fingerprint(&content) != action.expected_hash {
                        add("stale-temporal-expected-hash", JourneyLintSeverity::Blocking, None, None, format!("update setup path '{}' expected_hash to the current repository content fingerprint", action.path));
                    }
                }
            }
        }
        Ok(JourneyLintReport::new(1, findings))
    }

    pub fn parse_json(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading surface manifest {}", path.display()))?;
        let manifest: Self = serde_json::from_str(&text)
            .with_context(|| format!("parsing {} as {SURFACE_SCHEMA}", path.display()))?;
        Ok(manifest)
    }

    pub fn validate_for(&self, journey: &JourneySpec, hash: &str) -> Result<()> {
        if self.schema != SURFACE_SCHEMA {
            bail!(
                "unsupported surface schema '{}' (expected '{SURFACE_SCHEMA}')",
                self.schema
            );
        }
        if self.journey_id != journey.id {
            bail!(
                "surface manifest targets journey '{}', not '{}'",
                self.journey_id,
                journey.id
            );
        }
        if self.journey_hash != hash {
            bail!(
                "surface manifest is stale for journey '{}' (hash mismatch)",
                journey.id
            );
        }
        self.surface.validate()?;
        let operations: BTreeSet<&str> = self
            .surface
            .operations
            .iter()
            .map(|operation| operation.id.as_str())
            .collect();
        let operation_by_id: BTreeMap<&str, &CliOperation> = self
            .surface
            .operations
            .iter()
            .map(|operation| (operation.id.as_str(), operation))
            .collect();
        let input_by_id: BTreeMap<&str, &JourneyInput> = journey
            .inputs
            .iter()
            .map(|(id, input)| (id.as_str(), input))
            .collect();
        let journey_steps: BTreeSet<&str> =
            journey.steps.iter().map(|step| step.id.as_str()).collect();
        let mut bound = BTreeSet::new();
        let mut bound_operations = BTreeSet::new();
        let mut has_human_decision = false;
        for binding in &self.bindings {
            let step_id = binding.step_id();
            if !journey_steps.contains(step_id) {
                bail!("surface binding references unknown step '{}'", step_id);
            }
            if !bound.insert(step_id) {
                bail!(
                    "journey step '{}' has more than one surface binding",
                    step_id
                );
            }
            match binding {
                SurfaceBinding::Operation(binding) => {
                    if !operations.contains(binding.operation_id.as_str()) {
                        bail!(
                            "surface binding for step '{}' references unknown operation '{}'",
                            binding.step_id,
                            binding.operation_id
                        );
                    }
                    if !bound_operations.insert(binding.operation_id.as_str()) {
                        bail!(
                            "surface operation '{}' is bound more than once; each Journey step requires one primary operation",
                            binding.operation_id
                        );
                    }
                }
                SurfaceBinding::HumanDecision(binding) => {
                    has_human_decision = true;
                    binding.human_decision.validate()?;
                }
            }
        }
        let missing: Vec<&str> = journey_steps.difference(&bound).copied().collect();
        if !missing.is_empty() {
            bail!(
                "surface manifest does not bind journey step(s): {}",
                missing.join(", ")
            );
        }

        if has_human_decision && self.setup.is_none() {
            bail!("human decision bindings require setup.graph=local_snapshot");
        }
        if let Some(setup) = &self.setup {
            if setup.operations.is_empty() && !setup.has_temporal_actions() && !has_human_decision {
                bail!("surface setup must name an operation or declare a before_steps file action");
            }
            if let Some(git) = &setup.git {
                match setup.graph {
                    SetupGraph::LocalSnapshot => git.validate()?,
                }
            }
            let mut setup_operations = BTreeSet::new();
            let no_outputs = BTreeMap::new();
            for (step_id, actions) in &setup.before_steps {
                if !journey_steps.contains(step_id.as_str()) {
                    bail!("surface setup before_steps references unknown step '{step_id}'");
                }
                if actions.is_empty() {
                    bail!("surface setup before_steps.{step_id} must contain a file action");
                }
                let mut paths = BTreeSet::new();
                for action in actions {
                    action.validate()?;
                    if !paths.insert(action.path.as_str()) {
                        bail!(
                            "surface setup before_steps.{step_id} repeats path '{}'",
                            action.path
                        );
                    }
                }
            }
            for operation_id in &setup.operations {
                if !setup_operations.insert(operation_id.as_str()) {
                    bail!("surface setup repeats operation '{operation_id}'");
                }
                let operation = operation_by_id.get(operation_id.as_str()).ok_or_else(|| {
                    anyhow!("surface setup references unknown operation '{operation_id}'")
                })?;
                if bound_operations.contains(operation_id.as_str()) {
                    bail!(
                        "surface setup operation '{operation_id}' is also bound to an authored step"
                    );
                }
                if operation.read_only {
                    bail!(
                        "surface setup operation '{operation_id}' must be mutable so it can establish the isolated fixture"
                    );
                }
                if !operation.output.captures.is_empty() {
                    bail!(
                        "surface setup operation '{operation_id}' must not capture authored step outputs"
                    );
                }
                if operation.output.assertions.is_empty() {
                    bail!(
                        "surface setup operation '{operation_id}' must assert the fixture it establishes"
                    );
                }
                validate_operation_references(operation, &input_by_id, &no_outputs)?;
            }
        }

        // References are checked in semantic step order. A surface may read
        // authored inputs, this execution's run.id, or typed outputs captured
        // by an operation bound to an earlier step—never a forward/global
        // capture name.
        let binding_by_step: BTreeMap<&str, &SurfaceBinding> = self
            .bindings
            .iter()
            .map(|binding| (binding.step_id(), binding))
            .collect();
        let mut prior_outputs = BTreeMap::new();
        let mut prior_operations = BTreeSet::new();
        for step in &journey.steps {
            if let Some(actions) = self
                .setup
                .as_ref()
                .and_then(|setup| setup.before_steps.get(&step.id))
            {
                for action in actions {
                    validate_temporal_action_references(action, &input_by_id, &prior_outputs)
                        .with_context(|| {
                            format!(
                                "surface setup before_steps.{} path '{}'",
                                step.id, action.path
                            )
                        })?;
                }
            }
            let binding = binding_by_step
                .get(step.id.as_str())
                .expect("complete bindings validated above");
            match binding {
                SurfaceBinding::Operation(binding) => {
                    let operation = operation_by_id
                        .get(binding.operation_id.as_str())
                        .expect("operation binding validated above");
                    validate_operation_references(operation, &input_by_id, &prior_outputs)?;
                    for capture in &operation.output.captures {
                        let authored = step.produces.get(&capture.id).ok_or_else(|| {
                            anyhow!(
                                "operation '{}' captures undeclared output '{}' for Journey step '{}'",
                                operation.id,
                                capture.id,
                                step.id
                            )
                        })?;
                        if authored.value_type != capture.value_type {
                            bail!(
                                "operation '{}' capture '{}' type does not match Journey step '{}' output type",
                                operation.id,
                                capture.id,
                                step.id
                            );
                        }
                        prior_outputs.insert(
                            format!("steps.{}.outputs.{}", step.id, capture.id),
                            (capture.value_type, capture.redact),
                        );
                    }
                    let captured: BTreeSet<&str> = operation
                        .output
                        .captures
                        .iter()
                        .map(|capture| capture.id.as_str())
                        .collect();
                    let missing: Vec<&str> = step
                        .produces
                        .keys()
                        .map(String::as_str)
                        .filter(|id| !captured.contains(id))
                        .collect();
                    if !missing.is_empty() {
                        bail!(
                            "operation '{}' does not capture Journey step '{}' output(s): {}",
                            operation.id,
                            step.id,
                            missing.join(", ")
                        );
                    }
                    prior_operations.insert(operation.id.as_str());
                }
                SurfaceBinding::HumanDecision(binding) => {
                    if !prior_operations.contains(binding.human_decision.operation_id.as_str()) {
                        bail!(
                            "human decision step '{}' must reference an operation bound to an earlier authored step (found '{}')",
                            step.id,
                            binding.human_decision.operation_id
                        );
                    }
                    if !step.produces.is_empty() {
                        bail!(
                            "human decision step '{}' cannot declare produced machine outputs",
                            step.id
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Validate setup paths against local graph authority before accepting a
    /// reusable surface. Compilation repeats this check in the isolated clone.
    pub fn validate_setup_for_store(&self, store: &crate::store::Store) -> Result<()> {
        if let Some(setup) = &self.setup {
            setup.validate_for_store(store)?;
        }
        self.validate_exercises_for_store(store)?;
        Ok(())
    }

    /// Resolve every operation exercise against live CodeFiles and callable
    /// locators. Schema validation already checked ids/assertions; this binds
    /// the declaration to repository content.
    pub fn validate_exercises_for_store(&self, store: &crate::store::Store) -> Result<()> {
        for operation in &self.surface.operations {
            for exercise in &operation.exercises {
                let codefile = store
                    .resolve_node(&exercise.codefile, Some(crate::model::NodeType::CodeFile))
                    .with_context(|| {
                        format!(
                            "operation '{}' exercise '{}' codefile '{}'",
                            operation.id, exercise.id, exercise.codefile
                        )
                    })?;
                if !store.root().join(&codefile.name).is_file() {
                    bail!(
                        "operation '{}' exercise '{}' codefile '{}' is not a live file",
                        operation.id,
                        exercise.id,
                        codefile.name
                    );
                }
                crate::journey_exercises::require_callable_exercise_locator(
                    store,
                    &codefile,
                    &exercise.locator,
                )
                .with_context(|| {
                    format!(
                        "operation '{}' exercise '{}' locator '{}'",
                        operation.id, exercise.id, exercise.locator
                    )
                })?;
            }
        }
        Ok(())
    }

    pub fn canonical_bindings(&self, journey: &JourneySpec) -> Value {
        let by_step: BTreeMap<&str, &SurfaceBinding> = self
            .bindings
            .iter()
            .map(|binding| (binding.step_id(), binding))
            .collect();
        Value::Array(
            journey
                .steps
                .iter()
                .filter_map(|step| by_step.get(step.id.as_str()))
                .map(|binding| {
                    serde_json::to_value(binding).expect("surface binding is serializable")
                })
                .collect(),
        )
    }

    pub fn canonical_setup(&self) -> Result<Option<Value>> {
        self.setup
            .as_ref()
            .map(|setup| serde_json::to_value(setup).map(canonicalize_value))
            .transpose()
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::super::spec::{JourneySpec, JourneyStep, JOURNEY_SCHEMA};
    use super::super::surface_ops::{
        CliOperation, OperationOutput, OutputAssertion, OutputFormat, ASSERTION_NOT_EQUALS,
    };
    use super::super::SURFACE_SCHEMA;
    use super::*;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::path::Path;

    #[test]
    fn journey_lint_static_predicates_are_narrow() {
        assert!(is_exact_graph_identity("0123456789abcdef0123456789ABCDEF"));
        assert!(!is_exact_graph_identity(
            "id=0123456789abcdef0123456789abcdef"
        ));

        let assertion =
            |pointer: &str, equals: Option<Value>, source: Option<String>| OutputAssertion {
                id: "check".into(),
                pointer: pointer.into(),
                value_type: None,
                equals,
                source,
            };
        // No authored transcript, so every pre-existing case below is judged
        // exactly as it was before the exemption existed.
        let plain = |argv: Vec<String>| CliOperation {
            id: "probe".into(),
            summary: String::new(),
            argv,
            environment: Vec::new(),
            read_only: false,
            timeout_seconds: None,
            expected_exit: 0,
            arguments: Vec::new(),
            output: OperationOutput {
                format: OutputFormat::Json,
                captures: Vec::new(),
                assertions: Vec::new(),
                redact: Vec::new(),
            },
            exercises: Vec::new(),
        };
        let no_transcript = plain(vec!["loom".into(), "status".into()]);
        assert!(exact_census_pin(
            &assertion("/request_count", Some(json!(16)), None),
            &no_transcript
        ));
        for pointer in [
            "/entry_count",
            "/file_count",
            "/tombstone_count",
            "/operation_count",
            "/byte_total",
        ] {
            assert!(exact_census_pin(
                &assertion(pointer, Some(json!(1)), None),
                &no_transcript
            ));
        }
        assert!(!exact_census_pin(
            &assertion("/exit_code", Some(json!(0)), None),
            &no_transcript
        ));
        assert!(positional_census_pointer(
            "/findings/0/kind",
            &no_transcript
        ));
        assert!(!positional_census_pointer(
            "/finding/by-id/kind",
            &no_transcript
        ));

        // A transcript the fixture authored: counts and reply indices over it
        // are arithmetic on its own input, so they are exempt — but anything
        // deeper, or past the end of the authored list, still counts.
        let two_requests = plain(vec![
            "loom".into(),
            "mcp".into(),
            "transcript".into(),
            "--requests-json".into(),
            "[{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}},\
              {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}]"
                .into(),
            "--json".into(),
        ]);
        assert_eq!(authored_transcript_len(&two_requests), Some(2));
        assert!(!exact_census_pin(
            &assertion("/request_count", Some(json!(2)), None),
            &two_requests
        ));
        assert!(!exact_census_pin(
            &assertion("/response_count", Some(json!(2)), None),
            &two_requests
        ));
        // A count that disagrees with the authored list is not arithmetic on it.
        assert!(exact_census_pin(
            &assertion("/request_count", Some(json!(9)), None),
            &two_requests
        ));
        assert!(!positional_census_pointer(
            "/responses/1/result",
            &two_requests
        ));
        // Past the authored end, and deeper than the reply index, still fire.
        assert!(positional_census_pointer(
            "/responses/7/result",
            &two_requests
        ));
        assert!(positional_census_pointer(
            "/responses/1/result/tools/2/name",
            &two_requests
        ));
        assert_eq!(
            assertion("/name", None, Some(format!("{ASSERTION_NOT_EQUALS}\"\"")))
                .not_equals_value(),
            Some(json!(""))
        );

        let operation = CliOperation {
            id: "audit-burst".into(),
            summary: String::new(),
            argv: vec![
                "loom".into(),
                "mcp".into(),
                "{\"adjudications\":[],\"command\":[\"loom\",\"audit\",\"--json\"]}".into(),
            ],
            environment: Vec::new(),
            read_only: false,
            timeout_seconds: None,
            expected_exit: 0,
            arguments: Vec::new(),
            output: OperationOutput {
                format: OutputFormat::Json,
                captures: Vec::new(),
                assertions: Vec::new(),
                redact: Vec::new(),
            },
            exercises: Vec::new(),
        };
        assert!(relies_on_real_clock_minute_bucket(&operation));
        let mut merely_mentions_audit = operation;
        merely_mentions_audit.argv[2] = "audit prose without adjudications".into();
        assert!(!relies_on_real_clock_minute_bucket(&merely_mentions_audit));
    }

    #[test]
    fn journey_lint_reports_every_source_and_stable_contract() {
        let root =
            std::env::temp_dir().join(format!("loom-journey-lint-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("current.rs"), "current\n").unwrap();
        std::fs::write(root.join("stale.rs"), "changed\n").unwrap();
        let store = crate::store::Store::init(&root, Some("lint-unit"), false).unwrap();
        for path in ["current.rs", "stale.rs"] {
            store
                .add_node(
                    crate::model::NodeType::CodeFile,
                    path,
                    "lint fixture",
                    "active",
                    json!({}),
                )
                .unwrap();
        }
        let current_hash = crate::artifact::fingerprint("current\n");
        let graph_id = "0123456789abcdef0123456789abcdef";
        let manifest: SurfaceManifest = serde_json::from_value(json!({
            "schema": SURFACE_SCHEMA,
            "journey_id": "lint.fixture",
            "journey_hash": "hash",
            "surface": {
                "id": "lint-cli", "title": "Lint CLI", "identity": "lint",
                "codefile": "src/main.rs", "locator": "main",
                "operations": [{
                    "id": "inspect", "summary": "inspect",
                    "argv": ["tool", graph_id], "arguments": [],
                    "output": {"format": "json", "assertions": [
                        {"id":"equals-id", "pointer":"/id", "equals":{"nested":graph_id}},
                        {"id":"pointer-id", "pointer":format!("/nodes/{graph_id}/name"), "equals":"ok"},
                        {"id":"count", "pointer":"/entry_count", "equals":2},
                        {"id":"position", "pointer":"/entries/0/name", "equals":"first"},
                        {"id":"non-empty", "pointer":"/name", "not_equals":""},
                        {"id":"exit", "pointer":"/exit_code", "equals":0}
                    ]}
                }]
            },
            "setup": {
                "graph":"local_snapshot", "operations":[],
                "before_steps": {"step": [
                    {"path":"current.rs", "expected_hash":current_hash, "content":"next"},
                    {"path":"stale.rs", "expected_hash":"0000000000000000", "content":"next"}
                ]}
            },
            "bindings": []
        })).unwrap();
        let journey = JourneySpec {
            schema: JOURNEY_SCHEMA.into(),
            id: "lint.fixture".into(),
            name: "Lint fixture".into(),
            actor: "tester".into(),
            goal: "Exercise lint".into(),
            description: None,
            inputs: BTreeMap::new(),
            preconditions: Vec::new(),
            steps: vec![JourneyStep {
                id: "step".into(),
                name: "Step".into(),
                action: "Inspect".into(),
                expects: Vec::new(),
                produces: BTreeMap::new(),
            }],
            profiles: BTreeMap::new(),
        };
        let report = manifest
            .lint(&store, &journey, "surfaces/lint.fixture.surface.json")
            .unwrap();
        let rules: Vec<_> = report
            .findings
            .iter()
            .map(|finding| finding.rule.as_str())
            .collect();
        assert_eq!(
            rules
                .iter()
                .filter(|rule| **rule == "graph-local-identity")
                .count(),
            3
        );
        for rule in [
            "exact-census-pin",
            "positional-census-pointer",
            "not-equals-empty",
        ] {
            assert!(rules.contains(&rule), "missing {rule}: {rules:?}");
        }
        assert_eq!(
            rules
                .iter()
                .filter(|rule| **rule == "stale-temporal-expected-hash")
                .count(),
            1
        );
        assert!(report.findings.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            json!({
                "schema":"loom.journey-lint/v1", "status":"blocked", "scanned":1,
                "blocking":4, "advisory":3, "findings": report.findings
            })
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn self_audit_real_clock_rule_matches_only_authorized_batch() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let path = root.join("journeys/surfaces/self-audit.surface.json");
        let manifest = SurfaceManifest::parse_json(&path).unwrap();
        for operation in &manifest.surface.operations {
            let expected = operation.id == "audit-authorized-batch";
            if matches!(
                operation.id.as_str(),
                "audit-authorized-batch" | "audit-clean-graph" | "audit-defective-graph"
            ) {
                assert_eq!(
                    relies_on_real_clock_minute_bucket(operation),
                    expected,
                    "{}",
                    operation.id
                );
            }
        }
    }
}
