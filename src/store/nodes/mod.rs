//! Node persistence — the asserted-node write path of the store.
//!
//! Plane: engine (persistence). `add_node` accepts ONLY asserted node kinds —
//! derived nodes must take the deterministic-id path in `derived.rs`, so the
//! truth-class line is enforced at the insert (INV-5). Name resolution is
//! exact-or-unique-fragment; ambiguity is an error with candidates, never a
//! silent guess.

mod lookup;
mod mutate;
mod ratify;
