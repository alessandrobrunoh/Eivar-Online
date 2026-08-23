//! The per-tick simulation, split by concern.
//!
//! Each module exposes a single `step(ctx, dt)` called in a fixed order from
//! [`crate::tick::game_tick`]. They are separate files because they are
//! separate concerns, not because they can run concurrently — a tick is one
//! transaction on one thread.

pub mod ai;
pub mod combat;
pub mod crafting;
pub mod crowd_control;
pub mod effects;
pub mod event_log;
pub mod gathering;
pub mod movement;
pub mod spells;
pub mod status;
pub mod targets;
