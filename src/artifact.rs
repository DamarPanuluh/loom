//! Ground-artifact contract — how a kind of artifact presents identity and a
//! change-fingerprint to the engine.
//!
//! Plane: engine seam. The engine stores an artifact's identity as a plain
//! `Node` and its change-fingerprint as a generic `content_hash` facet, so a
//! new artifact class (an image, a dataset, an OKF bundle) registers with no
//! schema change. Everything CodeFile-specific — the derived facets, the
//! structural findings — comes from the code seed's derivers, not from engine
//! columns. The one thing the engine needs generically is a fingerprint that
//! changes iff the artifact's content changed; that is this contract.

use crate::model::NodeType;

/// A deterministic 64-bit FNV-1a content fingerprint as hex. The engine's
/// change-detection compares this to decide whether an artifact really changed
/// (and must ripple). It is a pure content hash — no domain knowledge — so it
/// serves code files, journey specs, and any future artifact class alike.
pub fn fingerprint(content: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in content.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// A ground-artifact class: the seam through which a domain registers a kind of
/// artifact. Identity and the change-fingerprint are all the engine consumes;
/// the class's derived facts arrive through the deriver seam.
pub trait ArtifactClass {
    /// Stable registry key.
    fn name(&self) -> &str;
    /// The node type artifacts of this class register as.
    fn node_type(&self) -> NodeType;
    /// The artifact's change-fingerprint from its content.
    fn fingerprint(&self, path: &str, content: &str) -> String;
}

/// The code-file artifact class — loom's reference (and, per the engine/seed
/// boundary decision, only) artifact class. Its fingerprint is the content
/// hash; its derived facts (language, role, loc, symbols) come from the
/// structural deriver.
pub struct CodeFileArtifact;

impl ArtifactClass for CodeFileArtifact {
    fn name(&self) -> &str {
        "code_file"
    }
    fn node_type(&self) -> NodeType {
        NodeType::CodeFile
    }
    fn fingerprint(&self, _path: &str, content: &str) -> String {
        fingerprint(content)
    }
}

/// Every registered artifact class. A new class registers by adding an entry;
/// the engine's identity + fingerprint storage never changes.
pub fn classes() -> Vec<Box<dyn ArtifactClass>> {
    vec![Box::new(CodeFileArtifact)]
}

/// The artifact class that registers as `node_type`, if any.
pub fn class_for(node_type: NodeType) -> Option<Box<dyn ArtifactClass>> {
    classes().into_iter().find(|c| c.node_type() == node_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_deterministic_and_sensitive() {
        assert_eq!(fingerprint("hello"), fingerprint("hello"));
        assert_ne!(fingerprint("hello"), fingerprint("hellp"));
    }

    #[test]
    fn code_files_register_as_the_code_artifact_class() {
        let class = class_for(NodeType::CodeFile).expect("code_file class registered");
        assert_eq!(class.name(), "code_file");
        // The class fingerprint is the engine's generic content fingerprint.
        assert_eq!(
            class.fingerprint("src/a.rs", "fn a(){}"),
            fingerprint("fn a(){}")
        );
        // No other node type is an artifact class.
        assert!(class_for(NodeType::Intent).is_none());
    }
}
