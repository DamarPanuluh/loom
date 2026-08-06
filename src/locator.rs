//! Locator parsing — one plane for what a grounding locator names.
//!
//! Plane: pure parse of locator strings into symbol names. Resolution against
//! a live file (match cardinality, fingerprints) stays in [`crate::runner`];
//! this module is the single definition of which tokens count as symbols so
//! proof strength, risk, divergence, absorb, and live resolution cannot
//! disagree about the same grounding.

use crate::model::{EdgeKind, GroundingRole, TargetKind};
use crate::store::Store;
use crate::Result;

/// Whether this locator is a whole-file scope (`module …`), not a symbol.
pub fn is_module_scope(locator: &str) -> bool {
    let t = locator.trim().to_ascii_lowercase();
    t == "module" || t.starts_with("module ")
}

/// Parse the symbol names carried by one locator.
///
/// A locator may name several symbols with `;`. Each member must still look
/// like a locator, not prose: a bare/qualified symbol, optionally with a line
/// suffix, or a symbol preceded only by declaration modifiers (`fn`, `enum`,
/// `pub`, ...). Taking the final word of prose such as
/// `subject case state-machine tests` would let an unrelated symbol named
/// `tests` manufacture a witness or inflate blast radius.
pub fn symbols(locator: &str) -> Vec<String> {
    locator.split(';').filter_map(symbol).collect()
}

/// Parse one locator member (one side of `;`).
pub fn symbol(member: &str) -> Option<String> {
    let member = member.trim();
    if member.is_empty() || is_module_scope(member) {
        return None;
    }

    let words: Vec<&str> = member.split_whitespace().collect();
    let token = *words.last()?;
    if words.len() > 1 {
        let prefixes = &words[..words.len() - 1];
        let declaration = prefixes.iter().any(|word| {
            matches!(
                *word,
                "fn" | "struct"
                    | "enum"
                    | "trait"
                    | "impl"
                    | "class"
                    | "def"
                    | "function"
                    | "interface"
                    | "type"
                    // JS/TS declarations commonly use a bound const as the
                    // callable surface (`export const load = …`).
                    | "const"
            )
        });
        let all_are_declaration_words = prefixes.iter().all(|word| {
            matches!(
                *word,
                "fn" | "struct"
                    | "enum"
                    | "trait"
                    | "impl"
                    | "class"
                    | "def"
                    | "function"
                    | "interface"
                    | "type"
                    | "async"
                    | "unsafe"
                    | "extern"
                    | "const"
                    // TS/JS/JVM visibility + member modifiers — `export` is
                    // TypeScript's `pub`; without these, a locator such as
                    // `export async function getDeck` parses as prose and the
                    // grounded symbol silently drops out of every consumer.
                    | "export"
                    | "default"
                    | "public"
                    | "private"
                    | "protected"
                    | "static"
                    | "readonly"
                    | "abstract"
                    | "override"
            ) || *word == "pub"
                || word.starts_with("pub(")
        });
        if !declaration || !all_are_declaration_words {
            return None;
        }
    }

    // Qualification must be removed before a `:line` suffix: splitting
    // `Type::method` at `:` first yields `Type` and makes the method branch
    // unreachable.
    let token = token.rsplit("::").next().unwrap_or(token);
    let token = token.split(':').next().unwrap_or(token);
    is_symbol_name(token).then(|| token.to_string())
}

fn is_symbol_name(candidate: &str) -> bool {
    let mut chars = candidate.chars();
    matches!(chars.next(), Some(first) if first == '_' || first == '$' || first.is_alphabetic())
        && chars.all(|c| c == '_' || c == '$' || c.is_alphanumeric())
}

/// Symbol names an intent is grounded in, via its realizing locators only.
///
/// Non-realizing roles (`consumes`, `configures`, `verifies`) are seams and
/// proofs, not the behavior's home — counting them as blast-radius / proof
/// symbols lets a test helper inflate urgency and manufacture witnesses.
pub fn realizing_symbols(store: &Store, intent_id: &str) -> Result<Vec<String>> {
    let mut out: Vec<String> = realizing_targets(store, intent_id)?
        .into_iter()
        .map(|(_, symbol)| symbol)
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

/// Realizing groundings as `(codefile path, symbol)` pairs.
///
/// Grading must keep the file: a bare symbol shared by two definitions can
/// otherwise pull callers of the wrong definition into the call witness.
pub fn realizing_targets(store: &Store, intent_id: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Implements), Some(intent_id), None)? {
        if store.edge_superseded(&e.id)? {
            continue;
        }
        if store.grounding_role(&e.id)? != GroundingRole::Realizes {
            continue;
        }
        let Some(file) = store.get_node(&e.to_id)? else {
            continue;
        };
        if let Some(loc) = store.get_facet(&e.id, TargetKind::Edge, "locator")? {
            for symbol in symbols(&loc) {
                out.push((file.name.clone(), symbol));
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeType, TruthClass};
    use crate::store::Store;

    /// Shared regression table: every consumer of locator parsing must agree
    /// with these answers. Multi-symbol, Type::method, prose, declaration
    /// modifiers, and grounding roles are all covered here so a second parser
    /// cannot quietly reappear elsewhere.
    #[test]
    fn shared_locator_regression_table() {
        let cases: &[(&str, &[&str])] = &[
            // multi-symbol
            (
                "getSubjectCase; listSubjectCases; a;b;c",
                &["getSubjectCase", "listSubjectCases", "a", "b", "c"],
            ),
            // Type::method and line suffixes — path qualification before `:line`
            (
                "DurableSignalLedger::rotate_checkpoint_authority_exact",
                &["rotate_checkpoint_authority_exact"],
            ),
            ("Type::method:42-57", &["method"]),
            ("capture_payment:88", &["capture_payment"]),
            // declaration modifiers
            ("fn perform_behavior", &["perform_behavior"]),
            ("pub async fn perform_behavior", &["perform_behavior"]),
            ("export function getDeck", &["getDeck"]),
            ("export async function getDeck", &["getDeck"]),
            ("export default class RoomDeck", &["RoomDeck"]),
            ("public static def render", &["render"]),
            // prose / module scopes — must not invent a symbol
            ("subject case state-machine tests", &[]),
            ("private-CA PostgreSQL acceptance runner", &[]),
            ("module proof strength grading", &[]),
            ("export the deck roster", &[]),
        ];
        for (locator, expected) in cases {
            let got = symbols(locator);
            assert_eq!(
                got, *expected,
                "locator `{locator}`: got {got:?}, expected {expected:?}"
            );
        }
        assert!(is_module_scope("module the thing this file is about"));
        assert!(!is_module_scope("mod_helper"));
    }

    #[test]
    fn realizing_symbols_ignores_non_realizing_roles() {
        let root = std::env::temp_dir().join(format!(
            "loom-locator-roles-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let store = Store::init(&root, Some("locator roles"), false).unwrap();
        let intent = store
            .add_node(
                NodeType::Intent,
                "behavior",
                "d",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();

        for (role, symbol) in [
            (GroundingRole::Realizes, "real_symbol"),
            (GroundingRole::Consumes, "consumed_symbol"),
            (GroundingRole::Configures, "configured_symbol"),
            (GroundingRole::Verifies, "verifying_symbol"),
        ] {
            let file = store
                .add_node(
                    NodeType::CodeFile,
                    &format!("{symbol}.rs"),
                    "",
                    "",
                    serde_json::json!({}),
                )
                .unwrap();
            let edge = store
                .add_edge(
                    EdgeKind::Implements,
                    &intent.id,
                    &file.id,
                    TruthClass::Asserted,
                )
                .unwrap();
            store.set_grounding_role(&edge.id, role).unwrap();
            store
                .set_facet(
                    &edge.id,
                    TargetKind::Edge,
                    "locator",
                    symbol,
                    TruthClass::Asserted,
                )
                .unwrap();
        }

        assert_eq!(
            realizing_symbols(&store, &intent.id).unwrap(),
            ["real_symbol"]
        );
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }
}
