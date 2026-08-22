//! The simulation step.
//!
//! Everything the Bevy server ran in `FixedUpdate` happens here, in one
//! transaction, in this order. The order is the same one `crates/server` used
//! via `.chain()`, and it matters: status expiry and crowd-control freeze have
//! to cancel a cast before the cast advances, and deaths have to settle before
//! the AI picks targets.

use spacetimedb::{reducer, ReducerContext, Table, Timestamp};

use std::sync::atomic::{AtomicBool, Ordering};

use crate::sim;
use crate::tables::{tick_stats, TickSchedule, TickStats};

static ALLY_DUMMY_SEEDED: AtomicBool = AtomicBool::new(false);
static RESOURCE_NODES_SEEDED: AtomicBool = AtomicBool::new(false);
static NPCS_SEEDED: AtomicBool = AtomicBool::new(false);

/// Upper bound on a single step's `dt`, in seconds.
///
/// A pause — the host suspended, a long stall, a laptop lid closing — would
/// otherwise produce one enormous `dt` and teleport every walking character to
/// its destination in a single frame.
const MAX_STEP_SECONDS: f32 = 0.25;

#[reducer]
pub fn game_tick(ctx: &ReducerContext, _schedule: TickSchedule) {
    let dt = advance_clock(ctx, ctx.timestamp);
    if dt <= 0.0 {
        return;
    }

    if !ALLY_DUMMY_SEEDED.load(Ordering::Relaxed) && crate::world::ensure_ally_dummy(ctx) {
        ALLY_DUMMY_SEEDED.store(true, Ordering::Relaxed);
    }
    if !RESOURCE_NODES_SEEDED.load(Ordering::Relaxed) && crate::world::ensure_resource_nodes(ctx) {
        RESOURCE_NODES_SEEDED.store(true, Ordering::Relaxed);
    }
    if !NPCS_SEEDED.load(Ordering::Relaxed) && crate::world::ensure_npcs(ctx) {
        NPCS_SEEDED.store(true, Ordering::Relaxed);
    }

    sim::status::step(ctx, dt);
    sim::crowd_control::step(ctx);
    sim::movement::step(ctx, dt);
    sim::gathering::step(ctx, dt);
    sim::crafting::step(ctx, dt);
    sim::spells::step(ctx, dt);
    sim::combat::step(ctx, dt);
    sim::ai::step(ctx, dt);

    crate::reducers::lifecycle::expire_stale_presence(ctx);
}

/// Advances the tick clock and returns the elapsed seconds since the last tick.
fn advance_clock(ctx: &ReducerContext, now: Timestamp) -> f32 {
    match ctx.db.tick_stats().id().find(&0) {
        Some(stats) => {
            let dt = now
                .duration_since(stats.last_tick)
                .map(|d| d.as_secs_f32())
                .unwrap_or(0.0)
                .min(MAX_STEP_SECONDS);
            ctx.db.tick_stats().id().update(TickStats {
                ticks: stats.ticks + 1,
                last_tick: now,
                ..stats
            });
            dt
        }
        None => {
            ctx.db.tick_stats().insert(TickStats {
                id: 0,
                ticks: 1,
                first_tick: now,
                last_tick: now,
            });
            // No previous tick to measure against.
            0.0
        }
    }
}
