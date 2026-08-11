//! Intake fidelity — forbid stale inbox-source recipes in operator-facing prose.
//!
//! The binary rejects `inbox add --source question|code_audit` (and related
//! evidence sources). README, docs, and the vendored skill must not teach them.
//! Behavioral gates live in ring9; this test only greps the prose surface.

use std::fs;
use std::path::{Path, PathBuf};

use loom::model::NodeType;
use loom::store::Store;

mod common;
use common::Tmp;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn mark_routed(root: &Path, item: &str, destination: &str) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .arg("--graph")
        .arg(root)
        .arg("--json")
        .args(["inbox", "mark", item, "routed", "--reason", destination])
        .output()
        .unwrap()
}

fn collect_markdown(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if matches!(name, "target" | ".git" | ".loom" | "node_modules") {
                    continue;
                }
                stack.push(path);
            } else if path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                out.push(path);
            }
        }
    }
    out
}

/// Matches teaching forms like `inbox add "…" --source question` while allowing
/// ring9/docs that *describe* the rejection (e.g. `` `inbox add --source question` is rejected ``).
fn is_forbidden_teaching_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if !lower.contains("inbox add") {
        return false;
    }
    if !lower.contains("--source question") && !lower.contains("--source code_audit") {
        return false;
    }
    // Allow contract/test/doc lines that state the rejection.
    if lower.contains("reject")
        || lower.contains("forbidden")
        || lower.contains("never use")
        || lower.contains("must not")
        || lower.contains("belong")
        || lower.contains("pointing to")
        || lower.contains("contract:")
    {
        return false;
    }
    true
}

#[test]
fn operator_prose_never_teaches_rejected_inbox_sources() {
    let root = repo_root();
    let mut scanned = Vec::new();
    for rel in ["README.md", "docs", "skills"] {
        let path = root.join(rel);
        if path.is_file() {
            scanned.push(path);
        } else if path.is_dir() {
            scanned.extend(collect_markdown(&path));
        }
    }
    // The loom-driver skill is authoritative at the global skill root since the
    // repo copy was removed (ecbea8b); the repo's own docs remain the
    // deterministic instruction surface this test guards. The global skill is
    // deliberately NOT scanned: a test must never depend on mutable state
    // outside the checkout.

    let mut violations = Vec::new();
    for path in &scanned {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for (idx, line) in text.lines().enumerate() {
            if is_forbidden_teaching_line(line) {
                violations.push(format!("{}:{}: {}", path.display(), idx + 1, line.trim()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "stale inbox-source recipes found (use loom question add / loom finding add):\n{}",
        violations.join("\n")
    );
}

#[test]
fn door_preserves_exact_topic_and_returns_typed_landing_choices() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("door-intake-fidelity"), false).unwrap();
    let topic = "Let café owners export receipts — without losing accents?";

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .arg("--graph")
        .arg(tmp.path())
        .arg("--json")
        .arg("door")
        .arg(topic)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "door failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["captured"]["description"].as_str(), Some(topic));
    let menu = response["landing_menu"]
        .as_array()
        .expect("door returns structured landing choices");
    let landings: Vec<_> = menu
        .iter()
        .map(|entry| {
            assert!(entry["why"].is_string(), "landing needs a rationale");
            assert!(entry["command"].is_string(), "landing needs a command");
            let landing = entry["landing"]
                .as_str()
                .expect("landing is a typed discriminator");
            if landing != "dismiss" {
                let route = entry["route_command"]
                    .as_str()
                    .expect("actionable landing teaches its typed route");
                assert!(route.contains("<stable-node-id>"), "typed route: {route}");
            }
            landing
        })
        .collect();
    assert_eq!(
        landings,
        [
            "new_journey",
            "hypothesis",
            "spike",
            "external_research",
            "dismiss",
        ]
    );

    let store = Store::open(tmp.path()).unwrap();
    let captured = store
        .list_nodes(Some(NodeType::InboxItem), usize::MAX)
        .unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].description, topic);
    assert_eq!(captured[0].body["source"].as_str(), Some("human"));
    let item_id = captured[0].id.clone();
    let destination = store
        .add_node(
            NodeType::Hypothesis,
            "receipt export experiment",
            "test whether guarded export is preferable",
            "proposed",
            serde_json::json!({
                "proposal": "try guarded receipt export",
                "predicted_outcome": "owners can review the export before sharing",
            }),
        )
        .unwrap();
    let invalid_item = store
        .add_node(
            NodeType::InboxItem,
            "invalid route candidate",
            "must stay new after rejected route references",
            "new",
            serde_json::json!({"source": "human"}),
        )
        .unwrap();
    drop(store);

    let typed_reference = format!("hypothesis:{}", destination.id);
    let routed = mark_routed(tmp.path(), &item_id, &typed_reference);
    assert!(
        routed.status.success(),
        "typed route failed: {}",
        String::from_utf8_lossy(&routed.stderr)
    );
    let routed_json: serde_json::Value = serde_json::from_slice(&routed.stdout).unwrap();
    assert_eq!(routed_json["destination"]["type"], "hypothesis");
    assert_eq!(routed_json["destination"]["ref"], destination.id);

    let shown = std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .arg("--graph")
        .arg(tmp.path())
        .arg("--json")
        .args(["inbox", "show", &item_id])
        .output()
        .unwrap();
    assert!(
        shown.status.success(),
        "independent inbox show failed: {}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let shown_json: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown_json["id"], item_id);
    assert_eq!(shown_json["status"], "routed");
    assert_eq!(
        shown_json["body"]["destination"], routed_json["destination"],
        "inspection must round-trip the exact canonical destination type/ref"
    );

    let store = Store::open(tmp.path()).unwrap();
    let persisted = store.get_node(&item_id).unwrap().unwrap();
    assert_eq!(persisted.status, "routed");
    assert_eq!(persisted.body["destination"]["type"], "hypothesis");
    assert_eq!(persisted.body["destination"]["ref"], destination.id);
    drop(store);

    let short_reference = &destination.id[..8];
    for invalid in [
        format!("hypothesis:{}", destination.name),
        format!("hypothesis:{short_reference}"),
        format!("existing_intent:{}", destination.id),
    ] {
        let rejected = mark_routed(tmp.path(), &invalid_item.id, &invalid);
        assert!(
            !rejected.status.success(),
            "name, fragment, or mismatched type must fail: {invalid}"
        );
        let store = Store::open(tmp.path()).unwrap();
        let unchanged = store.get_node(&invalid_item.id).unwrap().unwrap();
        assert_eq!(unchanged.status, "new");
        assert!(unchanged.body.get("destination").is_none());
    }
}
