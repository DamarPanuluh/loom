//! Signal plane — smells (structural), debt (statistical), doctor (integrity).
//!
//! Plane boundary (INV-3): smells are structural findings computed from graph
//! shape; debt is a statistical feed computed on demand (optionally reading VCS
//! history) and NEVER stored as an edge or counted as required work. Doctor
//! audits integrity after the fact.
//!
//! All three are pure reads over a `Snapshot`. Nothing here mutates the graph.

mod adjudication;
#[path = "signal/debt.rs"]
mod debt;
mod doctor;
mod graph;
mod imports;
mod smells;

pub use adjudication::{
    adjudication_of, findings_view, needed_findings, smell_det_key,
    smell_has_resolving_adjudication, stale_findings, triage_findings, untriaged_findings,
    FindingView,
};
pub use debt::{debt, debt_cluster_id, DebtCluster};
pub(crate) use debt::{CO_CHANGE_MAX_COMMITS, GIT_TIMEOUT_SECS};
pub use doctor::{doctor, doctor_with, DoctorIssue};
pub use smells::{smells, smells_with, Smell};
