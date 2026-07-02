//! Truth-axis matrix — the single source of the "which form of truth" vocabulary.
//!
//! Plane: pure data + mapping. A codebase carries several forms of truth on
//! different axes (what behavior should exist, where it is realized, how it is
//! proven, whether a claim is judged, and how the world-facing projection is
//! kept fresh). Each maturity rung and each `loom next` work item is really a
//! statement about ONE of these axes being stale or missing.
//!
//! This module names those axes once. Three seams consume it:
//! - `maturity::compass` labels the current phase with its axis,
//! - `workitem` stamps each served item with its `TruthGap`,
//! - `commands::misc_cmd::guide` teaches the adaptive per-axis moves.
//!
//! Keeping the mapping here (not duplicated per caller) means there is exactly
//! one place that answers "for this axis, what is the authoritative write, what
//! must NOT be written, and what downstream form must be refreshed after."

use serde::Serialize;

/// A form of truth in the graph. Ordered from "what should exist" through
/// "how the world sees it", mirroring the maturity ladder's ascent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TruthAxis {
    /// What behavior should exist. Authoritative form: the `Intent`.
    Intent,
    /// Where behavior is realized. Authoritative form: code + `implements`.
    Implementation,
    /// How behavior is proven. Authoritative form: `Validation`/saga + `validates`.
    Proof,
    /// Whether a recorded claim has been inspected. Authoritative form: an
    /// asserted edge verdict (analyzer/quality/validator, by edge kind).
    Verdict,
    /// Programmatic findings/smells awaiting adjudication. Authoritative form:
    /// a durable finding verdict.
    Signal,
    /// The world-facing projection. Authoritative form: the exported graph file.
    Projection,
}

/// Every axis, ladder order. The self-teaching guide walks this.
pub const TRUTH_AXES: &[TruthAxis] = &[
    TruthAxis::Intent,
    TruthAxis::Implementation,
    TruthAxis::Proof,
    TruthAxis::Verdict,
    TruthAxis::Signal,
    TruthAxis::Projection,
];

/// The concrete guidance for one axis: how to make it true, what NOT to write
/// while doing so (lane discipline), and what dependent form to refresh after.
#[derive(Debug, Clone, Serialize)]
pub struct TruthGap {
    pub axis: TruthAxis,
    /// One-line statement of which form is stale/missing.
    pub missing_form: String,
    /// The falsifiable per-axis correctness criterion: what "right" looks like
    /// for this axis, checkable against the graph. This is the self-teaching
    /// line — every LLM operating the graph reads it before writing this axis.
    pub correct_when: String,
    /// The authoritative write that makes this axis true.
    pub authoritative_write: String,
    /// The write that would be a lie from this hat (wrong-axis self-certification).
    pub forbidden_write: String,
    /// The dependent form that must be refreshed once the write lands.
    pub after_write: String,
}

impl TruthAxis {
    pub fn as_str(self) -> &'static str {
        match self {
            TruthAxis::Intent => "intent",
            TruthAxis::Implementation => "implementation",
            TruthAxis::Proof => "proof",
            TruthAxis::Verdict => "verdict",
            TruthAxis::Signal => "signal",
            TruthAxis::Projection => "projection",
        }
    }

    /// The guidance for closing this axis. This is the matrix: one arm per axis,
    /// authored once and read by compass, work items, and the guide.
    pub fn gap(self) -> TruthGap {
        match self {
            TruthAxis::Intent => TruthGap {
                axis: self,
                missing_form: "behavior is not named as an intent".into(),
                correct_when: "each active intent names exactly one falsifiable behavior, sized \
                               for reuse: small enough to recur under multiple parents (overlap \
                               with other intents is acceptable), big enough that the graph does \
                               not devolve into noise. If the description needs 'and', split it; \
                               if the name is just a function/symbol, it is a locator on an \
                               implements edge, not an intent"
                    .into(),
                authoritative_write: "loom door \"<utterance>\" then loom intent add --name … --description …".into(),
                forbidden_write: "writing code or proofs before the behavior is named".into(),
                after_write: "loom status".into(),
            },
            TruthAxis::Implementation => TruthGap {
                axis: self,
                missing_form: "intent has no code grounding (or a registered file has no owning intent)".into(),
                correct_when: "every implements edge carries a locator that resolves to a live \
                               symbol which actually performs the named behavior (not merely \
                               references it), and every registered file has an owning intent or \
                               a recorded ignore reason"
                    .into(),
                authoritative_write: "inspect the relevant files, edit code, then loom edge implement <intent> <codefile> --locator <symbol>".into(),
                forbidden_write: "marking quality or validation passing (those are other axes)".into(),
                after_write: "loom sync".into(),
            },
            TruthAxis::Proof => TruthGap {
                axis: self,
                missing_form: "implemented behavior has no passing proof (a flow may need a journey proof)".into(),
                correct_when: "every implemented intent has at least one validation whose latest \
                               result was observed from a real run: passed/failed with actual \
                               output, or blocked naming the concrete missing prerequisite. \
                               not_run is not a proof, and a flow crossing a service boundary \
                               needs a journey proof, not only unit proofs"
                    .into(),
                authoritative_write: "loom validation add … --intent <intent> then run it; for flows, loom journey add <spec> and loom journey run <spec>".into(),
                forbidden_write: "editing code to force a proof green".into(),
                after_write: "loom validation mark … --result passed|failed|blocked --evidence …".into(),
            },
            TruthAxis::Verdict => TruthGap {
                axis: self,
                missing_form: "an asserted claim is uninspected or stale".into(),
                correct_when: "every asserted edge status was earned by fresh inspection: the \
                               criterion states what would falsify the claim, the evidence cites \
                               file/line or runtime output that was actually read, and the \
                               confidence is honest — below 0.7 is a legitimate answer that \
                               routes to review; a confident guess is graph corruption"
                    .into(),
                authoritative_write: "read both endpoints, then record the verdict for the edge kind (loom edge explore / loom rule verdict / loom validation mark)".into(),
                forbidden_write: "editing code, or recording a verdict from name similarity instead of evidence".into(),
                after_write: "loom status".into(),
            },
            TruthAxis::Signal => TruthGap {
                axis: self,
                missing_form: "a derived finding or smell awaits adjudication".into(),
                correct_when: "every derived finding carries a durable adjudication — justified, \
                               needed, or blocked — with a concrete reason. The goal is zero \
                               unjudged signals, not zero signals"
                    .into(),
                authoritative_write: "loom finding verdict <id> justified|needed|blocked --reason …".into(),
                forbidden_write: "deferring the judgment to a human instead of judging it".into(),
                after_write: "loom status".into(),
            },
            TruthAxis::Projection => TruthGap {
                axis: self,
                missing_form: "the exported graph is missing or stale".into(),
                correct_when: "loom.graph.json is byte-identical to a fresh export of the current \
                               store — loom export --check exits clean"
                    .into(),
                authoritative_write: "loom export".into(),
                forbidden_write: "hand-editing the exported file".into(),
                after_write: "loom export --check".into(),
            },
        }
    }
}

/// Map a maturity phase + its compass command to the axis it is about. Phases
/// are stable strings owned by `maturity::compass`. `audit` is overloaded — a
/// `loom doctor` audit is graph-integrity (Verdict), a `loom smells` audit is
/// finding adjudication (Signal) — so the command disambiguates it. The terminal
/// `complete` phase (and any unknown phase) has no open axis and returns `None`.
pub fn axis_for_phase(phase: &str, next_command: &str) -> Option<TruthAxis> {
    match phase {
        "seed" => Some(TruthAxis::Intent),
        "build" => Some(TruthAxis::Implementation),
        "coverage" => Some(TruthAxis::Implementation),
        "validate" => Some(TruthAxis::Proof),
        "fix" | "analyze" | "quality" => Some(TruthAxis::Verdict),
        "audit" if next_command.contains("doctor") => Some(TruthAxis::Verdict),
        "audit" => Some(TruthAxis::Signal),
        "triage" => Some(TruthAxis::Signal),
        "export" => Some(TruthAxis::Projection),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_axis_has_a_gap_and_stable_str() {
        for axis in TRUTH_AXES {
            let gap = axis.gap();
            assert_eq!(gap.axis, *axis);
            assert!(!axis.as_str().is_empty());
            assert!(!gap.missing_form.is_empty());
            assert!(!gap.correct_when.is_empty());
            assert!(!gap.authoritative_write.is_empty());
            assert!(!gap.forbidden_write.is_empty());
            assert!(!gap.after_write.is_empty());
        }
    }

    #[test]
    fn known_phases_map_to_axes_and_complete_is_none() {
        assert_eq!(axis_for_phase("seed", ""), Some(TruthAxis::Intent));
        assert_eq!(axis_for_phase("build", ""), Some(TruthAxis::Implementation));
        assert_eq!(
            axis_for_phase("coverage", "loom coverage"),
            Some(TruthAxis::Implementation)
        );
        assert_eq!(
            axis_for_phase("validate", "loom next --mode validate"),
            Some(TruthAxis::Proof)
        );
        assert_eq!(axis_for_phase("fix", ""), Some(TruthAxis::Verdict));
        assert_eq!(axis_for_phase("analyze", ""), Some(TruthAxis::Verdict));
        assert_eq!(
            axis_for_phase("audit", "loom doctor"),
            Some(TruthAxis::Verdict)
        );
        assert_eq!(
            axis_for_phase("audit", "loom smells"),
            Some(TruthAxis::Signal)
        );
        assert_eq!(axis_for_phase("triage", ""), Some(TruthAxis::Signal));
        assert_eq!(axis_for_phase("export", ""), Some(TruthAxis::Projection));
        assert_eq!(axis_for_phase("complete", "loom status"), None);
    }
}
