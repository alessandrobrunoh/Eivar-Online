//! The BevyMMO SpacetimeDB module — the authoritative server.
//!
//! Replaces the Bevy/lightyear/Postgres server that used to live in
//! `crates/server`. Tables are simultaneously the authoritative state, the
//! persistence and the replication; reducers are the only way to change them.
//!
//! Three constraints govern everything here, none of which the compiler will
//! remind you about:
//!
//! - **No wall clock, no OS RNG, no filesystem.** Use `ctx.timestamp` and
//!   `ctx.rng()`; map data comes from `include_bytes!`, never `std::fs`.
//! - **Every table persists**, including the ones modelling transient state.
//!   Anything runtime-only is cleared in [`reducers::lifecycle::init`], which
//!   runs once per fresh publish.
//! - **A tick is one transaction**, single-threaded. There is no Bevy scheduler
//!   and no parallelism: what used to be several systems is now ordered calls
//!   inside [`tick::game_tick`].
//!
//! The game rules themselves are not here — they are in `bevymmo_domain`, which
//! the client links too. This crate is the part that knows about storage,
//! scheduling and who is allowed to ask for what.

pub mod reducers;
pub mod rows;
pub mod sim;
pub mod tables;
pub mod tick;
pub mod views;
pub mod world;

/// Simulation step, in milliseconds.
///
/// The Bevy server ran `FixedUpdate` at 60 Hz. 20 Hz is the interval
/// SpacetimeDB's docs use for simulation, and it is a starting point rather than
/// a decision: measured cadence is ~18-19 Hz because the interval runs from the
/// end of the previous execution. Watch how long `game_tick` actually takes
/// before lowering it.
pub const TICK_INTERVAL_MS: u64 = 50;

/// `MovementStats::speed` was 0.15 units per tick at a fixed 60 Hz.
pub const DEFAULT_SPEED_PER_SECOND: f32 = 0.15 * 60.0;

/// How many characters one account may own at once. See `Player::account_id`.
pub const MAX_CHARACTERS_PER_ACCOUNT: usize = 3;

/// How many characters one party may hold at once. See `tables::PartyRow`.
pub const MAX_PARTY_SIZE: usize = 5;

/// How many API keys one account may hold at once. See `tables::ApiKey`.
pub const MAX_API_KEYS_PER_ACCOUNT: usize = 20;

/// Normalises a display name into its uniqueness key.
pub fn normalize_name(name: &str) -> String {
    name.trim().to_lowercase()
}
