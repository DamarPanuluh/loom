//! Domain command family — hypotheses, interface surfaces, vocabulary, layers.
//!
//! Plane: CLI surface over asserted domain knowledge (judgment-plane inputs).
//! Owns the human-declared vocabulary the detectors read: hypothesis lifecycle,
//! surface registration, vocab terms, and the layer order that arms the
//! layering detector. Declarations only — this module never records verdicts
//! on behalf of a declaration and never writes derived truth.

pub(crate) use super::*;

mod hypothesis;
mod layers;
mod surface;
mod vocab;

pub(crate) use hypothesis::hypothesis;
pub(crate) use layers::{layer, layer_detector_state, layer_detector_state_with};
pub(crate) use surface::{create_or_reuse_interface_surface, surface};
pub(crate) use vocab::vocab;
