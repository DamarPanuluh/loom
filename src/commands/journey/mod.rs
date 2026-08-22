//! Semantic Journey command handlers.
//!
//! `add` registers only the authored root artifact. `derive` and `surface`
//! emit read-only JSON packets. Their corresponding `*-accept` commands apply
//! strict, hash-bound manifests atomically.

pub(crate) use super::*;
use super::{domain_cmd, intent, pulse};
use crate::cli::JourneyCmd;
pub(crate) use crate::model::{EdgeKind, InspectionStatus, Node, NodeType, TargetKind, TruthClass};
pub(crate) use crate::store::Store;
pub(crate) use crate::Result;
pub(crate) use anyhow::{anyhow, bail, Context};
pub(crate) use serde_json::{json, Value};
pub(crate) use std::collections::{BTreeMap, BTreeSet};
pub(crate) use std::path::{Path, PathBuf};

mod derive;
mod lint;
mod registry;
mod runtime;
mod support;
mod surface;

pub(crate) use derive::{journey_derive, journey_derive_accept};
pub(crate) use lint::journey_lint;
pub(crate) use registry::{journey_add, journey_list, journey_map, journey_remove, journey_show};
pub(crate) use runtime::{
    journey_compile, journey_diagnose, journey_drift, journey_freeze, journey_rehearse_cold,
    journey_resume, journey_run,
};
pub(crate) use support::{
    edge_json_facet, emit_packet, emit_report, emit_runtime_value, journey_nodes,
    load_registered_journey, node_json, ordered_subset, resolve_journey,
};
pub(crate) use surface::{journey_surface, journey_surface_accept};

pub fn dispatch(graph: Option<&Path>, cmd: JourneyCmd, json: bool) -> Result<()> {
    match cmd {
        JourneyCmd::Lint { journey } => journey_lint(graph, journey.as_deref(), json),
        JourneyCmd::Add { spec } => journey_add(graph, spec, json),
        JourneyCmd::Show { journey } => journey_show(graph, &journey, json),
        JourneyCmd::Remove { journey } => journey_remove(graph, &journey, json),
        JourneyCmd::List { limit, offset } => journey_list(graph, limit, offset, json),
        JourneyCmd::Map => journey_map(graph, json),
        JourneyCmd::Derive {
            journey,
            candidate_json,
        } => journey_derive(graph, &journey, candidate_json.as_deref(), json),
        JourneyCmd::DeriveAccept {
            journey,
            manifest,
            human_decision,
        } => journey_derive_accept(graph, &journey, &manifest, human_decision, json),
        JourneyCmd::Surface { journey } => journey_surface(graph, &journey, json),
        JourneyCmd::SurfaceAccept { journey, manifest } => {
            journey_surface_accept(graph, &journey, &manifest, json)
        }
        JourneyCmd::Compile { journey, profile } => {
            journey_compile(graph, &journey, &profile, json)
        }
        JourneyCmd::Run { journey, profile } => journey_run(graph, &journey, &profile, json),
        JourneyCmd::Resume {
            token,
            choice,
            human_decision,
            free_form,
        } => journey_resume(
            graph,
            &token,
            &choice,
            &human_decision,
            free_form.as_deref(),
            json,
        ),
        JourneyCmd::Diagnose {
            journey,
            profile,
            input,
        } => journey_diagnose(graph, &journey, &profile, &input, json),
        JourneyCmd::RehearseCold { journey } => journey_rehearse_cold(graph, &journey, json),
        JourneyCmd::Freeze { journey, profile } => journey_freeze(graph, &journey, &profile, json),
        JourneyCmd::Drift { journey } => journey_drift(graph, journey.as_deref(), json),
    }
}
