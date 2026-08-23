//! Gathering rules: channel duration, bonus rolls, regen catch-up, yield math.
//!
//! Pure functions. The SpacetimeDB module applies them inside a tick; the
//! client never recomputes a yield.

pub mod components;
pub mod formulas;

pub use crate::items::GatheringToolKind;
pub use components::{ActiveGather, Harvestable};
pub use formulas::{
    bonus_extra_pieces, channel_duration, gathering_tool_bonuses, in_interact_range, regen_catchup,
    resolve_gather, GatherAttempt, GatherOutcome, DEFAULT_MIN_CHANNEL_SECONDS,
};
