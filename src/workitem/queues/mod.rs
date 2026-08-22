//! Queue partition — one candidate work item per lane.
//!
//! Plane: judgment-plane routing (pure reads over the store). Each `*_item`
//! function selects the single next candidate for its lane using the SAME
//! predicates the maturity ladder and completeness scorecard use — the compass
//! must never route a lane at work its queue would not serve. Selection only:
//! nothing here writes to the graph or decides verdicts.

mod packets;
mod predicates;
mod prescreen;
mod roster;

pub(crate) use predicates::{
    analyze_serves, needed_finding_repair_lane, ungrounded_implemented_intents,
    unmeasured_quality_pairs, unproven_implemented_intents,
};
pub use predicates::{unratified_intents, validation_work_units, ValidationWorkUnit};

pub(super) use packets::{
    analyze_item, audit_item, build_item, coverage_item, deepen_item, derive_item, elaborate_item,
    fix_item, prove_item, quality_item, ratify_item, rectify_item, review_item, surface_item,
    triage_item, validate_item,
};

pub use roster::{queue_items, QueueEntry};

pub(super) use prescreen::prescreen_for;
pub use prescreen::PreScreen;
