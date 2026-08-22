//! Crowd control kinds and the client projection of active effects.
//!
//! Apply, tick and expiry live in the SpacetimeDB module, owned by `active_status`.
//! This crate holds the shared kind enum (the rulebook for movement/cast gates)
//! and `CrowdControlState`, which the client rebuilds from replicated rows.

pub mod components;

pub use components::{ActiveCrowdControl, CrowdControlKind, CrowdControlState};
