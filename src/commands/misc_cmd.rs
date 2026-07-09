//! Compatibility re-exports for capture / orientation / discovery handlers.
//!
//! Prefer the concern-specific modules; this facade keeps `commands::run`
//! and older references stable while the oversized bag is split.

pub(crate) use super::capture_cmd::{door, inbox, note, question, task};
pub(crate) use super::discover_cmd::{detect_cmd, explain_cmd, find_cmd, schema_cmd};
pub(crate) use super::orient_cmd::{guide, session, welcome};
