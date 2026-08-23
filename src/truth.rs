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
//! - `commands::orient_cmd::guide` teaches the adaptive per-axis moves.
//!
//! Keeping the mapping here (not duplicated per caller) means there is exactly
//! one place that answers "for this axis, what is the authoritative write, what
//! must NOT be written, and what downstream form must be refreshed after."


use crate::model::{str_enum, ParseEnumError};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

str_enum! {
    /// A form of truth in the graph. Ordered from "what should exist" through
    /// "how the world sees it", mirroring the maturity ladder's ascent.
    ///
    /// Through `str_enum!`, so the canonical strings, `ALL`, `FromStr` and the
    /// serde impls all come from this one variant list. It used to carry a
    /// hand-written `as_str`, a separate `TRUTH_AXES` const, AND
    /// `#[serde(rename_all)]` — three mechanisms that had to agree on every name.
    TruthAxis {
        /// What behavior should exist. Authoritative form: the `Intent`.
        Intent => "intent",
        /// Where behavior is realized. Authoritative form: code + `implements`.
        Implementation => "implementation",
        /// How behavior is proven. Authoritative form: `Validation`/journey + `validates`.
        Proof => "proof",
        /// Whether a recorded claim has been inspected. Authoritative form: an
        /// asserted edge verdict (analyzer/quality/validator, by edge kind).
        Verdict => "verdict",
        /// Evidence-backed findings awaiting adjudication. Authoritative form:
        /// a durable finding verdict on an asserted or derived Finding.
        Signal => "signal",
        /// The world-facing projection. Authoritative form: the exported graph file.
        Projection => "projection",
        /// Where the graph is thinnest relative to what depends on it.
        /// Authoritative form: stronger proof on high-blast-radius behavior.
        /// Unlike every other axis this one never closes — it re-ranks.
        Risk => "risk",
    }
}


/// Every axis, ladder order — an alias for the macro-generated `ALL`, kept
/// because callers and `tests/ring9.rs` name it. Not a second list: change a
/// variant above and this follows.
pub const TRUTH_AXES: &[TruthAxis] = TruthAxis::ALL;

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
                after_write: "loom validation verdict … passed|failed|blocked --evidence …".into(),
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
                authoritative_write: "read both endpoints, then record the verdict for the edge kind (loom edge explore / loom rule verdict / loom validation verdict)".into(),
                forbidden_write: "editing code, or recording a verdict from name similarity instead of evidence".into(),
                after_write: "loom status".into(),
            },
            TruthAxis::Signal => TruthGap {
                axis: self,
                missing_form: "an asserted or derived finding awaits adjudication".into(),
                correct_when: "every material finding carries a durable adjudication — needed, \
                               justified, rejected, deferred, blocked, duplicate, or resolved — with a \
                               concrete reason. The goal is zero unjudged signals, not zero signals"
                    .into(),
                authoritative_write: "loom finding verdict <id> needed|justified|rejected|deferred|blocked|duplicate|resolved --reason …".into(),
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
            TruthAxis::Risk => TruthGap {
                axis: self,
                missing_form: "widely-depended-on behavior rests on the weakest proof in the graph"
                    .into(),
                correct_when: "no behavior's blast radius outruns the strength of the proof \
                               covering it — the proof asserts what the behavior DOES, not merely \
                               that the process exited 0, and it still holds against a frozen \
                               baseline"
                    .into(),
                authoritative_write:
                    "strengthen the proof named in the packet, then re-run it through loom".into(),
                forbidden_write: "weakening the assertion to make the run pass".into(),
                after_write: "loom status".into(),
            },
        }
    }
}

// NOTE: `axis_for_phase(phase, next_command)` used to map a compass phase string
// back to an axis, disambiguating an overloaded `audit` arm by grepping the
// command text. `Lane::axis()` replaces it: one axis per lane, no strings.

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
    fn every_lane_names_exactly_one_axis() {
        // The replacement for `axis_for_phase`: the mapping is total and needs
        // no command-text disambiguation.
        for lane in crate::lane::Lane::LADDER {
            let gap = lane.axis().gap();
            assert_eq!(gap.axis, lane.axis());
        }
        assert_eq!(
            crate::lane::Lane::Validate.axis(),
            TruthAxis::Proof,
            "the validate lane closes proof truth"
        );
        assert_eq!(crate::lane::Lane::Deepen.axis(), TruthAxis::Risk);
    }
}
