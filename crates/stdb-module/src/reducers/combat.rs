//! What a client is allowed to ask of the combat system.
//!
//! Only respawn, for now. Damage and healing are never client-requested: they
//! are consequences the tick produces, and the entry points live in
//! [`crate::sim::combat`].

use spacetimedb::{reducer, ReducerContext, Table};

use crate::reducers::lifecycle::caller_entity;
use crate::sim::combat;
use crate::tables::{entity_stats, player_message, EntityStateRow, PlayerMessageEvent};

/// Brings the caller's character back to life at its spawn point.
///
/// Ported from `handle_respawn_requests`. Two things the Bevy version needed
/// and this does not: it had to scan every player entity to find the one whose
/// `PlayerId` matched the requesting peer — `ctx.sender()` answers that with no
/// scan and no way to spoof — and it had to pick a spawn point from a shared
/// pool because only *some* players carried a `SpawnPoint`. Every
/// `game_entity` has a `spawn_point` column, so the fallback is gone.
///
/// Refusing when the character is alive is deliberate: `RespawnRequest` was a
/// payload-free message, so a live player spamming it would otherwise get a
/// free full heal and a teleport home.
#[reducer]
pub fn respawn(ctx: &ReducerContext) -> Result<(), String> {
    let entity = caller_entity(ctx)?;
    let stats = ctx
        .db
        .entity_stats()
        .entity_id()
        .find(entity.entity_id)
        .ok_or_else(|| "character has no stats".to_string())?;

    // Both conditions, as in the Bevy server: zero health counts as dead even
    // if the state has not caught up yet. The tick's death sweep normally makes
    // them agree, but a respawn arriving in the same tick as the killing blow
    // should not be rejected on a technicality.
    if entity.state != EntityStateRow::Dead && stats.stats.current_health > 0.0 {
        return Err("only dead characters respawn".to_string());
    }

    // Everything a resurrection means — dropping the debuffs, the crowd
    // control and the poisons, refilling, and going home — lives in one place,
    // shared with the enemy respawn timer.
    combat::resurrect(ctx, entity);

    ctx.db.player_message().insert(PlayerMessageEvent {
        target: Some(ctx.sender()),
        text: "You are back on your feet.".to_string(),
    });
    Ok(())
}
