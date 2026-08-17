//! `loom intent` command family.
//!
//! Plane: CLI surface over the judgment plane — the asserted intent lifecycle
//! (add/update/waive/reactivate/ratify); it writes assertions, never derived
//! truth.
//!
//! Contract (ratification): every intent carries `origin` (who minted it) and
//! `ratification` (whether the product authority wants it). Any lane may mint;
//! ONLY a human may decide ratification (INV-8). A solo human can write it
//! directly; an LLM lane can only record an explicit answer returned by the
//! host conversation. Redefining a ratified intent stales its ratification to
//! `needs_reconfirmation`, exactly as sync stales a verdict.

use super::{node_json, open, pulse, require_lane};
use crate::cli::{IntentCmd, IntentTagCmd};
use crate::grammar::{
    looks_like_symbol, ACTIVE_LIFECYCLES, ALL_LIFECYCLES, ASPECTS, LEVELS, VISIBILITIES,
};
use crate::model::{EdgeKind, Node, NodeType, TargetKind, TruthClass};
use crate::store::Store;
use crate::Result;
use anyhow::bail;
use std::path::Path;

pub fn dispatch(graph: Option<&Path>, cmd: IntentCmd, json: bool) -> Result<()> {
    match cmd {
        IntentCmd::Add {
            name,
            description,
            level,
            lifecycle,
            visibility,
            layer,
            aspect,
            allow_symbol_name,
        } => intent_add(
            graph,
            IntentAddArgs {
                name,
                description,
                level,
                lifecycle,
                visibility,
                layer,
                aspect,
                allow_symbol_name,
            },
            json,
        ),
        IntentCmd::Show { key } => intent_show(graph, key, json),
        IntentCmd::Waive { key, axis, reason } => intent_waive(graph, key, axis, reason, json),
        IntentCmd::JourneyExempt {
            key,
            kind,
            reason,
            human_decision,
        } => intent_journey_exempt(graph, key, kind, reason, human_decision, json),
        IntentCmd::JourneyRequire {
            key,
            reason,
            human_decision,
        } => intent_journey_require(graph, key, reason, human_decision, json),
        IntentCmd::Reactivate { key, reason } => intent_reactivate(graph, key, reason, json),
        IntentCmd::List { limit, offset } => intent_list(graph, limit, offset, json),
        IntentCmd::Update {
            key,
            description,
            name,
            level,
            visibility,
            aspect,
            lifecycle,
            rectify,
            reason,
            reword,
        } => intent_update(
            graph,
            IntentUpdateArgs {
                key,
                description,
                new_name: name,
                level,
                visibility,
                aspect,
                lifecycle,
                rectify,
                reason,
                reword,
            },
            json,
        ),
        IntentCmd::Remove { key, reason } => intent_remove(graph, key, reason, json),
        IntentCmd::Retire {
            key,
            reason,
            replaced_by,
        } => intent_retire(graph, key, reason, replaced_by, json),
        IntentCmd::Confirm { key } => intent_confirm(graph, key, json),
        IntentCmd::Impact {
            key,
            classification,
            evidence,
        } => intent_impact(graph, key, classification, evidence, json),
        IntentCmd::Dependents { key, depth } => intent_dependents(graph, &key, depth, json),
        IntentCmd::Ratify {
            key,
            all,
            evidence,
            human_decision,
        } => intent_ratify(
            graph,
            RatifyArgs {
                key,
                all,
                evidence,
                human_decision,
            },
            json,
        ),
        IntentCmd::Reject {
            key,
            reason,
            human_decision,
        } => intent_reject(graph, &key, &reason, human_decision, json),
        IntentCmd::Tag { cmd } => intent_tag(graph, cmd, json),
    }
}

include!("create.rs");
include!("commands.rs");
include!("update.rs");
include!("lifecycle.rs");
