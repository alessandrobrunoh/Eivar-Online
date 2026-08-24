//! Crowd control gates: freeze, interrupt, and the predicates the rest of the
//! simulation asks.
//!
//! Status rows own duration. Each `crowd_control` row is a 1:1 child of an
//! `active_status` instance (`origin_status_instance_id`) whose remaining time
//! is copied onto the child so clients can draw a bar without joining tables.
//! [`step`] does **not** tick those timers: it only enforces freeze/interrupt
//! on rows that still have a living parent, and garbage-collects orphans.
//!
//! # Why the domain type is not stored
//!
//! `bevymmo_domain::crowd_control::CrowdControlKind` is the rulebook for which
//! kinds block movement or casting. The *component* `CrowdControlState` is a
//! client projection (a `Vec` of effects). Rows are not components: an empty
//! row is a row that still has to be scanned every tick, so "no CC" is simply
//! "no rows".
//!
//! `CrowdControlKindRow` still carries `Slow` for schema compatibility; Slow is
//! not a movement/cast gate.

use bevymmo_domain::crowd_control::CrowdControlKind;
use spacetimedb::{ReducerContext, Table};

use crate::tables::{
    active_status, cast_state, crowd_control, game_entity, CrowdControl, CrowdControlKindRow,
    GameEntity,
};

/// What [`materialize`] should do for a status instance's control child.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum MaterializeAction {
    /// Remaining hit zero (or never started): drop the child if it exists.
    Remove,
    /// Insert or refresh the child with this remaining time.
    Write { remaining_seconds: f32 },
}

/// Decides the child-row mutation from the **status** remaining time.
///
/// The incoming apply duration is not an input: callers pass the remaining
/// already resolved through [`crate::sim::status`] refresh policy.
pub(crate) fn plan_materialize_control(remaining_seconds: f32) -> MaterializeAction {
    if remaining_seconds <= 0.0 {
        MaterializeAction::Remove
    } else {
        MaterializeAction::Write { remaining_seconds }
    }
}

/// Whether this child should be deleted because its owning status is gone.
pub(crate) fn is_orphan_child(parent_exists: bool) -> bool {
    !parent_exists
}

/// Whether `child_origin` is the row owned by `removed_status_id`.
///
/// Two instances of the same kind keep two rows; only the matching origin is
/// deleted when one status is cleansed or expires.
#[cfg(test)]
pub(crate) fn is_owned_by(child_origin: u64, removed_status_id: u64) -> bool {
    child_origin == removed_status_id
}

/// Enforces freeze and cast-interrupt for every living CC row.
///
/// Status expiry (`sim::status::step`) already deleted owned children. This
/// pass is the gate: a stunned entity must not keep walking or casting on the
/// same tick the status is still active. Orphans (parent status missing) are
/// deleted so a leak cannot keep a gate closed.
pub fn step(ctx: &ReducerContext) {
    let mut casting_blocked: Vec<u64> = Vec::new();
    let mut movement_blocked: Vec<u64> = Vec::new();
    let mut orphans: Vec<u64> = Vec::new();

    for effect in ctx.db.crowd_control().iter() {
        let parent_alive = ctx
            .db
            .active_status()
            .id()
            .find(effect.origin_status_instance_id)
            .is_some();
        if is_orphan_child(parent_alive) {
            orphans.push(effect.id);
            continue;
        }
        if blocks_casting(effect.kind) {
            casting_blocked.push(effect.entity_id);
        }
        if blocks_movement(effect.kind) {
            movement_blocked.push(effect.entity_id);
        }
    }

    for id in orphans {
        ctx.db.crowd_control().id().delete(id);
    }

    // Duplicates are harmless: the second visit finds nothing left to cancel.
    for entity_id in casting_blocked {
        cancel_cast(ctx, entity_id);
    }
    for entity_id in movement_blocked {
        freeze(ctx, entity_id);
    }
}

/// Upserts the CC child for `origin_status_instance_id`.
///
/// `remaining_seconds` / `total_seconds` are the owning status's timer, already
/// run through refresh policy. Two status instances of the same kind keep two
/// rows; removing one cannot delete the other.
pub(crate) fn materialize(
    ctx: &ReducerContext,
    entity_id: u64,
    source: Option<u64>,
    kind: CrowdControlKindRow,
    remaining_seconds: f32,
    total_seconds: f32,
    origin_status_instance_id: u64,
) {
    match plan_materialize_control(remaining_seconds) {
        MaterializeAction::Remove => {
            remove_owned(ctx, origin_status_instance_id);
            return;
        }
        MaterializeAction::Write { remaining_seconds } => {
            let existing = ctx
                .db
                .crowd_control()
                .origin_status_instance_id()
                .find(origin_status_instance_id);
            match existing {
                Some(effect) => {
                    ctx.db.crowd_control().id().update(CrowdControl {
                        source,
                        kind,
                        remaining_seconds,
                        total_seconds,
                        ..effect
                    });
                }
                None => {
                    ctx.db.crowd_control().insert(CrowdControl {
                        id: 0,
                        entity_id,
                        source,
                        kind,
                        remaining_seconds,
                        total_seconds,
                        origin_status_instance_id,
                    });
                }
            }
        }
    }

    if blocks_casting(kind) {
        cancel_cast(ctx, entity_id);
    }
    if blocks_movement(kind) {
        freeze(ctx, entity_id);
    }
}

/// Copies the owning status timer onto the child, if the child exists.
pub(crate) fn sync_timer(
    ctx: &ReducerContext,
    origin_status_instance_id: u64,
    remaining_seconds: f32,
    total_seconds: f32,
) {
    let Some(effect) = ctx
        .db
        .crowd_control()
        .origin_status_instance_id()
        .find(origin_status_instance_id)
    else {
        return;
    };
    if (effect.remaining_seconds - remaining_seconds).abs() < f32::EPSILON
        && (effect.total_seconds - total_seconds).abs() < f32::EPSILON
    {
        return;
    }
    ctx.db.crowd_control().id().update(CrowdControl {
        remaining_seconds,
        total_seconds,
        ..effect
    });
}

/// Deletes the CC row owned by `origin_status_instance_id`, if any.
pub(crate) fn remove_owned(ctx: &ReducerContext, origin_status_instance_id: u64) {
    let Some(row) = ctx
        .db
        .crowd_control()
        .origin_status_instance_id()
        .find(origin_status_instance_id)
    else {
        return;
    };
    ctx.db.crowd_control().id().delete(row.id);
}

/// Whether `entity_id` is under an effect that suppresses *all* action.
///
/// The predicate `CrowdControlState::has_blocking_cc` answered, and the one
/// callers should use when they mean "this entity cannot act at all". For the
/// narrower questions prefer [`is_casting_blocked`] or [`is_movement_blocked`]:
/// a silenced entity can still walk, and a rooted one can still cast.
pub fn has_blocking_cc(ctx: &ReducerContext, entity_id: u64) -> bool {
    ctx.db
        .crowd_control()
        .victim()
        .filter(entity_id)
        .any(|effect| blocks_all_actions(effect.kind))
}

/// Whether `entity_id` may not start or continue a cast.
pub fn is_casting_blocked(ctx: &ReducerContext, entity_id: u64) -> bool {
    ctx.db
        .crowd_control()
        .victim()
        .filter(entity_id)
        .any(|effect| blocks_casting(effect.kind))
}

/// Whether `entity_id` may not move under its own power.
pub fn is_movement_blocked(ctx: &ReducerContext, entity_id: u64) -> bool {
    ctx.db
        .crowd_control()
        .victim()
        .filter(entity_id)
        .any(|effect| blocks_movement(effect.kind))
}

/// Cancels an in-progress cast, if there is one.
///
/// Bevy removed `CastProgress` silently and let the cast bar UI time itself out
/// after roughly a second. `sim::spells::end_cast` emits a `cast_ended` event
/// instead, so the interrupt is stated rather than inferred — a second of stale
/// cast bar on a stunned character is exactly the moment the player is looking
/// at it. Ending a cast belongs to the cast pipeline, so that is what does it;
/// this only decides *when*.
fn cancel_cast(ctx: &ReducerContext, entity_id: u64) {
    let Some(cast) = ctx.db.cast_state().entity_id().find(entity_id) else {
        return;
    };
    crate::sim::spells::end_cast(ctx, entity_id, cast.spell_id, true);
}

/// Drops whatever destination an entity was walking to.
///
/// Bevy gated the movement *system* on the absence of blocking CC. There is no
/// system to gate here — `sim::movement::step` walks whatever `move_target`
/// says — so the destination is cleared instead. The visible difference is that
/// a stunned character does not resume its interrupted path when the stun ends;
/// it stands still until it is told where to go again, which is what a stun is
/// supposed to feel like.
fn freeze(ctx: &ReducerContext, entity_id: u64) {
    let Some(entity) = ctx.db.game_entity().entity_id().find(entity_id) else {
        return;
    };
    if entity.move_target.is_none() {
        // Already stopped; skip the write so a long stun costs one row update
        // rather than one per tick.
        return;
    }
    ctx.db.game_entity().entity_id().update(GameEntity {
        move_target: None,
        ..entity
    });
}

/// Maps a replicated row kind onto the domain rulebook. `Slow` is not a gate.
fn domain_kind(kind: CrowdControlKindRow) -> Option<CrowdControlKind> {
    match kind {
        CrowdControlKindRow::Stun => Some(CrowdControlKind::Stun),
        CrowdControlKindRow::Root => Some(CrowdControlKind::Root),
        CrowdControlKindRow::Silence => Some(CrowdControlKind::Silence),
        CrowdControlKindRow::Slow => None,
    }
}

fn blocks_all_actions(kind: CrowdControlKindRow) -> bool {
    domain_kind(kind).is_some_and(CrowdControlKind::is_blocking)
}

fn blocks_casting(kind: CrowdControlKindRow) -> bool {
    domain_kind(kind).is_some_and(CrowdControlKind::blocks_casting)
}

fn blocks_movement(kind: CrowdControlKindRow) -> bool {
    domain_kind(kind).is_some_and(CrowdControlKind::blocks_movement)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_remaining_removes_the_child_instead_of_writing() {
        assert_eq!(plan_materialize_control(0.0), MaterializeAction::Remove);
        assert_eq!(plan_materialize_control(-0.1), MaterializeAction::Remove);
    }

    #[test]
    fn positive_remaining_writes_the_status_timer() {
        assert_eq!(
            plan_materialize_control(3.0),
            MaterializeAction::Write {
                remaining_seconds: 3.0
            }
        );
    }

    #[test]
    fn missing_parent_is_an_orphan() {
        assert!(is_orphan_child(false));
        assert!(!is_orphan_child(true));
    }

    #[test]
    fn removing_one_stun_instance_does_not_claim_the_other() {
        let first = 11_u64;
        let second = 17_u64;
        assert!(is_owned_by(first, first));
        assert!(!is_owned_by(second, first));
        assert!(is_owned_by(second, second));
    }

    #[test]
    fn slow_row_is_not_a_movement_or_cast_gate() {
        assert!(!blocks_movement(CrowdControlKindRow::Slow));
        assert!(!blocks_casting(CrowdControlKindRow::Slow));
        assert!(!blocks_all_actions(CrowdControlKindRow::Slow));
        assert_eq!(domain_kind(CrowdControlKindRow::Slow), None);
    }

    #[test]
    fn domain_predicates_match_row_predicates() {
        assert!(blocks_movement(CrowdControlKindRow::Stun));
        assert!(blocks_casting(CrowdControlKindRow::Stun));
        assert!(blocks_movement(CrowdControlKindRow::Root));
        assert!(!blocks_casting(CrowdControlKindRow::Root));
        assert!(blocks_casting(CrowdControlKindRow::Silence));
        assert!(!blocks_movement(CrowdControlKindRow::Silence));
    }
}
