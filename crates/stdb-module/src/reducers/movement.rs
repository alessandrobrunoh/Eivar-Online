//! Where the caller wants to go. The tick does the walking.

use spacetimedb::{reducer, ReducerContext};

use bevymmo_domain::movement::{movement_intent_allowed, MovementLock};

use crate::reducers::lifecycle::caller_entity;
use crate::rows::Vec3Row;
use crate::sim::crowd_control;
use crate::tables::{cast_state, game_entity, CastKindRow, EntityStateRow, GameEntity};
use crate::world;

/// Sets the caller's destination.
///
/// Movement intent is X/Z only. The deprecated Y argument is retained for the
/// generated client binding, but ground height is always resolved from the
/// authoritative embedded map.
#[reducer]
pub fn move_to(ctx: &ReducerContext, x: f32, y: f32, z: f32) -> Result<(), String> {
    let _ = y;
    if !x.is_finite() || !z.is_finite() {
        return Err("movement destination must have finite x and z coordinates".to_string());
    }
    let ground = world::ground_at(x, z)
        .ok_or_else(|| "movement destination is outside a walkable surface".to_string())?;

    let entity = caller_entity(ctx)?;
    if entity.state == EntityStateRow::Dead {
        return Err("dead characters do not walk".to_string());
    }
    let lock = match ctx.db.cast_state().entity_id().find(&entity.entity_id) {
        Some(cast) => match cast.kind {
            CastKindRow::Instant => MovementLock::None,
            CastKindRow::CastTime => MovementLock::CastTime,
            CastKindRow::Channeling => MovementLock::Channel,
        },
        None => MovementLock::None,
    };
    if !movement_intent_allowed(
        lock,
        crowd_control::is_movement_blocked(ctx, entity.entity_id),
    ) {
        return Err("you cannot move while crowd-controlled".to_string());
    }
    ctx.db.game_entity().entity_id().update(GameEntity {
        move_target: Some(Vec3Row {
            x,
            y: ground.height,
            z,
        }),
        ..entity
    });
    Ok(())
}

/// Cancels any pending movement, stopping the character where it stands.
#[reducer]
pub fn stop(ctx: &ReducerContext) -> Result<(), String> {
    let entity = caller_entity(ctx)?;
    ctx.db.game_entity().entity_id().update(GameEntity {
        move_target: None,
        state: EntityStateRow::Idle,
        ..entity
    });
    Ok(())
}
