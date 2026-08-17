//! Diagnostics command family — smells, debt, doctor, findings, coverage,
//! completeness, scan, calibrate, thresholds, policy, ignore, whoami.
//!
//! Plane: CLI surface over the signal plane. Renders advisory reads computed
//! live from the graph (INV-3: smells/debt are feeds, never stored as required
//! work or edges). Graph writes here are limited to durable finding
//! adjudications (`record_finding_verdict`), debt promotions
//! (`add_promoted_debt_finding` — asserted facts only, never converting the
//! signal), and configuration (ignore globs, thresholds, policy) — never
//! structural or derived truth.

pub(crate) use super::*;

mod advisory;
mod coverage;
mod findings;
mod impact;
mod scan_config;

pub(crate) use advisory::{debt, smells_cmd};
pub(crate) use coverage::{coverage_cmd, ignore_cmd, whoami_cmd};
pub(crate) use findings::{adjudicate_finding_batch, doctor_cmd, finding};
pub(crate) use impact::{absorb_cmd, audit_cmd, deepen_cmd, impact_cmd, impact_report};
pub(crate) use scan_config::{
    calibrate_cmd, completeness_cmd, policy_cmd, scan_cmd, threshold_cmd,
};
