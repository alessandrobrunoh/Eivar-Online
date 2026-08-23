//! Combat and stats: damage, healing, timed modifiers, mana and death.
//!
//! Ported from `crates/server/src/stats/systems.rs`. The Bevy version was four
//! systems reading three message queues plus two observers; here there are no
//! queues, so the "write a `DamageEvent`" call sites become direct calls to
//! [`apply_damage`] and friends. That collapses the observer/system duplication
//! the Bevy server carried (`apply_damage` and `on_damage_triggered` were the
//! same code twice, once per dispatch style) into one path.
//!
//! # Shared API
//!
//! Spells, AI and item effects all need to hurt, heal and buff things. Those
//! three verbs are the public surface of this module and nothing outside it
//! should write `entity_stats.stats.current_health` directly — armor reduction,
//! clamping, the death transition and the floating-combat-text event all hang
//! off these functions:
//!
//! - [`apply_damage`] — raw damage in, armor reduction and death handled.
//! - [`apply_healing`] — clamped to `max_health`, never resurrects.
//! - [`apply_modifier`] — a timed buff or debuff on one stat field.
//!
//! [`recalculate_effective_stats`] is also public, for whoever changes a
//! character's equipment: `player_stats` holds *base* stats and `entity_stats`
//! holds *effective* ones, and this is the only function that derives the
//! second from the first.

// How long a slain non-player entity stays a corpse. Taken from the domain
// rather than restated, because it *was* restated — as 30 seconds, under a
// comment claiming it matched the domain's 10.
use std::collections::HashMap;
use std::sync::Mutex;

use bevymmo_domain::content::items::default_items;
use bevymmo_domain::entity::dummy::components::DUMMY_RESPAWN_SECONDS;
use bevymmo_domain::entity::enemy::components::ENEMY_RESPAWN_SECONDS;
use bevymmo_domain::items::effects::ItemEffect;
use bevymmo_domain::stats::components::StatsBundleData;
use bevymmo_domain::stats::defaults;
use bevymmo_domain::stats::events::{ModifierOp, StatField};
use bevymmo_domain::stats::formulas::damage_after_armor;
use spacetimedb::{ReducerContext, Table, Uuid};

use crate::rows::{equipment_from_rows, StatsRow, EQUIP_SLOTS};
use crate::tables::{
    boss_state, crowd_control, damage_event, entity_stats, equipment, game_entity, party_member,
    periodic_effect, player_stats, stat_modifier, BossPhaseRow, BossState, DamageEventRow,
    EntityKindRow, EntityStateRow, EntityStats, GameEntity, ModifierKindRow, PeriodicEffect,
    StatModifier,
};

static NON_PLAYER_BASE_STATS: Mutex<Option<HashMap<u64, StatsRow>>> = Mutex::new(None);

fn base_stats_map() -> std::sync::MutexGuard<'static, Option<HashMap<u64, StatsRow>>> {
    NON_PLAYER_BASE_STATS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Remembers the spawn profile of a non-player so later stat recomputes do
/// not silently rewrite it to the kind-wide default.
pub fn record_base_stats(entity_id: u64, stats: StatsRow) {
    base_stats_map()
        .get_or_insert_with(HashMap::new)
        .insert(entity_id, stats);
}

// ---------------------------------------------------------------------------
// The per-tick step
// ---------------------------------------------------------------------------

/// One tick of the combat system.
///
/// Order matters and mirrors `StatsPlugin`'s `.chain()`, minus the message
/// queues: modifiers expire first so that a buff which ended this tick is gone
/// before anything reads the stats it inflated, then the affected entities have
/// their effective stats rebuilt, then mana regenerates against the fresh
/// `mana_regeneration`, and death is settled last so the AI step that follows
/// sees corpses as corpses.
pub fn step(ctx: &ReducerContext, dt: f32) {
    let expired_on = tick_modifiers(ctx, dt);
    for entity_id in expired_on {
        recalculate_effective_stats(ctx, entity_id);
    }
    tick_periodic_effects(ctx, dt);
    regenerate_mana(ctx, dt);
    reap_the_dead(ctx);
    tick_respawns(ctx, dt);
}

/// Fires damage- and heal-over-time effects that are due, and expires the rest.
///
/// The Bevy server drove these from the modifier tick. They are a separate table
/// here — see `tables::PeriodicEffect` — because they change health rather than
/// a stat, so folding them into a stat recompute never made sense.
fn tick_periodic_effects(ctx: &ReducerContext, dt: f32) {
    let mut due = Vec::new();
    let mut updated = Vec::new();
    let mut expired = Vec::new();

    for effect in ctx.db.periodic_effect().iter() {
        let remaining = effect.remaining_seconds - dt;
        let mut since = effect.since_last_tick + dt;

        // A `while` rather than an `if`: a long stall must not silently swallow
        // ticks that were owed. `MAX_STEP_SECONDS` caps `dt`, so this is bounded.
        let mut fires = 0;
        while effect.tick_interval_seconds > 0.0 && since >= effect.tick_interval_seconds {
            since -= effect.tick_interval_seconds;
            fires += 1;
        }
        for _ in 0..fires {
            due.push((effect.entity_id, effect.source, effect.amount_per_tick));
        }

        if remaining > 0.0 {
            updated.push(PeriodicEffect {
                remaining_seconds: remaining,
                since_last_tick: since,
                ..effect
            });
        } else {
            expired.push(effect.id);
        }
    }

    for effect in updated {
        ctx.db.periodic_effect().id().update(effect);
    }
    for id in expired {
        ctx.db.periodic_effect().id().delete(&id);
    }
    for (entity_id, source, amount) in due {
        if amount >= 0.0 {
            apply_healing(ctx, entity_id, amount);
        } else {
            apply_damage(ctx, entity_id, source, -amount);
        }
    }
}

/// Applies a heal- or damage-over-time effect to `target`.
///
/// A positive `amount_per_tick` heals, a negative one hurts.
///
/// Refreshes rather than stacks, on the same identity rule as
/// [`apply_modifier`]: same source, same magnitude, same interval is the same
/// effect. Without this a channelled DoT re-applied every tick left one row per
/// tick behind, each one still ticking — twenty poisons a second from one cast.
/// The accumulator is deliberately *not* reset on refresh, so a refreshed DoT
/// keeps its rhythm instead of restarting its interval and effectively skipping
/// a tick each time.
pub fn apply_periodic(
    ctx: &ReducerContext,
    target: u64,
    source: Option<u64>,
    amount_per_tick: f32,
    tick_interval_seconds: f32,
    duration_seconds: f32,
) {
    if tick_interval_seconds <= 0.0 || duration_seconds <= 0.0 || amount_per_tick == 0.0 {
        return;
    }

    let existing = ctx
        .db
        .periodic_effect()
        .on_entity()
        .filter(&target)
        .find(|row| {
            row.source == source
                && values_match(row.amount_per_tick, amount_per_tick)
                && values_match(row.tick_interval_seconds, tick_interval_seconds)
        });

    match existing {
        Some(row) => {
            ctx.db.periodic_effect().id().update(PeriodicEffect {
                remaining_seconds: duration_seconds,
                ..row
            });
        }
        None => {
            ctx.db.periodic_effect().insert(PeriodicEffect {
                id: 0,
                entity_id: target,
                source,
                amount_per_tick,
                tick_interval_seconds,
                since_last_tick: 0.0,
                remaining_seconds: duration_seconds,
                origin_status_instance_id: None,
            });
        }
    }
}

/// Brings back anything whose respawn timer ran out.
///
/// Players are not on a timer — they respawn when they ask to — so only entities
/// carrying a `respawn_in_seconds` are considered. Without this a killed enemy
/// stayed a corpse forever, since the module has no equivalent of the Bevy
/// server's despawn-and-respawn scheduling.
fn tick_respawns(ctx: &ReducerContext, dt: f32) {
    let mut due = Vec::new();
    let mut counting = Vec::new();

    for entity in ctx.db.game_entity().iter() {
        if entity.state != EntityStateRow::Dead {
            continue;
        }
        let Some(remaining) = entity.respawn_in_seconds else {
            continue;
        };
        if remaining - dt > 0.0 {
            counting.push(GameEntity {
                respawn_in_seconds: Some(remaining - dt),
                ..entity
            });
        } else {
            due.push(entity);
        }
    }

    for entity in counting {
        ctx.db.game_entity().entity_id().update(entity);
    }
    for entity in due {
        resurrect(ctx, entity);
    }
}

/// Decrements modifier durations and deletes the ones that ran out.
///
/// Returns the entities that lost at least one modifier, so the caller can
/// rebuild their stats exactly once even if several buffs expired together.
///
/// Only stat modifiers. The Bevy version drove heal-over-time and
/// damage-over-time from the same loop; here they are their own table with
/// their own interval and accumulator, ticked by [`tick_periodic_effects`],
/// because a periodic effect never enters the effective-stat fold and walking
/// it on every recompute was pure waste.
fn tick_modifiers(ctx: &ReducerContext, dt: f32) -> Vec<u64> {
    // Two passes: mutating a table while iterating it is undefined here, and a
    // modifier that expires must not be observed half-deleted by the next row.
    let mut expired = Vec::new();
    let mut remaining = Vec::new();
    for modifier in ctx.db.stat_modifier().iter() {
        let Some(left) = modifier.remaining_seconds else {
            // `None` is "until something removes it" — respawn, or an explicit
            // dispel. Nothing to count down.
            continue;
        };
        let left = left - dt;
        if left > 0.0 {
            remaining.push(StatModifier {
                remaining_seconds: Some(left),
                ..modifier
            });
        } else {
            expired.push(modifier);
        }
    }

    for modifier in remaining {
        ctx.db.stat_modifier().id().update(modifier);
    }

    let mut touched: Vec<u64> = Vec::new();
    for modifier in expired {
        ctx.db.stat_modifier().id().delete(&modifier.id);
        if !touched.contains(&modifier.entity_id) {
            touched.push(modifier.entity_id);
        }
    }
    touched
}

/// Refills mana over time.
///
/// New behaviour: the Bevy server had a `max_mana`/`mana_regeneration` pair in
/// `VitalStats` and never spent or regenerated anything — there was no current
/// mana anywhere in the codebase. `entity_stats.current_mana` gives the number
/// a home, so this is where the regeneration the stats were always describing
/// actually happens.
fn regenerate_mana(ctx: &ReducerContext, dt: f32) {
    let mut updates = Vec::new();
    for row in ctx.db.entity_stats().iter() {
        if row.stats.mana_regeneration <= 0.0 || row.current_mana >= row.stats.max_mana {
            continue;
        }
        // Corpses do not regenerate: respawn is what refills the pool.
        if is_dead(ctx, row.entity_id) {
            continue;
        }
        let current_mana = bevymmo_domain::stats::formulas::regenerated_mana(
            row.current_mana,
            row.stats.max_mana,
            row.stats.mana_regeneration,
            dt,
        );
        updates.push(EntityStats {
            current_mana,
            ..row
        });
    }
    for row in updates {
        ctx.db.entity_stats().entity_id().update(row);
    }
}

/// Marks as dead anything whose health reached zero without going through
/// [`apply_damage`].
///
/// [`apply_damage`] already flips the state on the killing blow, so this only
/// catches health written down some other way — a future environmental effect,
/// a GM command, a stat recompute that shrank `max_health` to nothing. Keeping
/// the sweep means "health is zero" and "state is Dead" can never disagree for
/// longer than one tick, which the Bevy server could not promise: its
/// `handle_respawn_requests` had to test *both* conditions because the
/// replicated `EntityState` lagged behind `VitalStats`.
fn reap_the_dead(ctx: &ReducerContext) {
    let mut newly_dead = Vec::new();
    for entity in ctx.db.game_entity().iter() {
        if entity.state == EntityStateRow::Dead {
            continue;
        }
        let Some(stats) = ctx.db.entity_stats().entity_id().find(&entity.entity_id) else {
            continue;
        };
        if stats.stats.current_health <= 0.0 {
            newly_dead.push(entity);
        }
    }

    for entity in newly_dead {
        let entity_id = entity.entity_id;
        kill(ctx, entity);
        // Zero amount: the health was already gone, this event exists only so
        // the client's floating combat text agrees that something died.
        ctx.db.damage_event().insert(DamageEventRow {
            target: entity_id,
            amount: 0.0,
            is_healing: false,
            killed: true,
        });
    }
}

// ---------------------------------------------------------------------------
// Shared API: damage, healing, modifiers
// ---------------------------------------------------------------------------

/// Hurts `target` for `amount` raw damage.
///
/// `amount` is pre-mitigation, exactly as `DamageEvent::amount` was: armor
/// reduction is applied here via [`damage_after_armor`], so callers describe
/// the spell and never the target. Health is clamped at zero, a
/// [`DamageEventRow`] carrying the *post*-armor number is emitted for combat
/// text, and the target transitions to [`EntityStateRow::Dead`] on the killing
/// blow.
///
/// Differs from the Bevy server in one place: damage on an already-dead target
/// is dropped. There, a corpse kept absorbing hits — every projectile still in
/// flight when a mob died re-ran the death path.
///
/// A missing target is not an error. Projectiles outlive their victims and AI
/// picks stale ids; the Bevy version silently skipped those too.
///
/// `source` is who dealt it, and it is what makes a boss fight work: threat is
/// accrued here rather than at each call site, so nothing can deal damage and
/// forget to be hated for it. `None` covers damage with no author — a falling
/// rock, a decaying debuff — which earns no threat.
pub fn apply_damage(ctx: &ReducerContext, target: u64, source: Option<u64>, amount: f32) {
    let Some(row) = ctx.db.entity_stats().entity_id().find(&target) else {
        return;
    };
    let Some(entity) = ctx.db.game_entity().entity_id().find(&target) else {
        return;
    };
    if entity.state == EntityStateRow::Dead {
        return;
    }

    if hostile_effect_blocked(ctx, target, source) {
        if let Some(source_character_id) = source
            .and_then(|id| ctx.db.game_entity().entity_id().find(&id))
            .and_then(|attacker| attacker.owner_character_id)
        {
            let message = if entity.kind == EntityKindRow::AllyDummy {
                "You cannot attack an ally."
            } else {
                "You cannot attack a party member."
            };
            crate::reducers::parties::notify_character(
                ctx,
                source_character_id,
                message.to_string(),
            );
        }
        return;
    }

    let bundle = StatsBundleData::from(row.stats);
    let effective = damage_after_armor(amount, &bundle.combat);
    let current_health = (row.stats.current_health - effective).max(0.0);
    let killed = current_health <= 0.0;
    let interrupt_gather =
        entity.kind == EntityKindRow::Player && entity.owner_character_id.is_some();

    ctx.db.entity_stats().entity_id().update(EntityStats {
        stats: StatsRow {
            current_health,
            ..row.stats
        },
        ..row
    });
    if killed {
        kill(ctx, entity);
    }

    if interrupt_gather {
        crate::sim::gathering::cancel_session(ctx, target);
    }

    // Threat is proportional to damage actually dealt, not damage attempted, so
    // armour on the boss reduces the aggro a hit generates as well as its bite.
    if let Some(source) = source {
        crate::sim::ai::accrue_threat(ctx, target, source, effective);
    }

    ctx.db.damage_event().insert(DamageEventRow {
        target,
        amount: effective,
        is_healing: false,
        killed,
    });
}

/// Whether a hostile payload from `source` must be dropped for `target`.
///
/// Same rule as [`is_friendly_fire`]: party members and the ally dummy are
/// immune to a player's damage and debuffs. The hostile dummy is not.
pub fn hostile_effect_blocked(ctx: &ReducerContext, target: u64, source: Option<u64>) -> bool {
    let Some(entity) = ctx.db.game_entity().entity_id().find(&target) else {
        return false;
    };
    let source_entity = source.and_then(|id| ctx.db.game_entity().entity_id().find(&id));
    let source_character_id = source_entity
        .as_ref()
        .and_then(|attacker| attacker.owner_character_id);
    is_friendly_fire(
        entity.kind,
        entity.owner_character_id,
        source_entity.as_ref().map(|attacker| attacker.kind),
        source_character_id,
        entity
            .owner_character_id
            .and_then(|id| ctx.db.party_member().character_id().find(&id))
            .map(|row| row.party_id),
        source_character_id
            .and_then(|id| ctx.db.party_member().character_id().find(&id))
            .map(|row| row.party_id),
    )
}

/// The pure decision behind [`hostile_effect_blocked`]. Split out so the
/// boundary is unit-testable without a `ReducerContext`.
///
/// A player hitting the allied training dummy is always friendly fire.
/// Two different players in the same party are friendly fire.
///
/// Ambiguous *party* data (`None` character or party ids) still fails open:
/// a missing row never makes a real player invulnerable. The ally dummy does
/// not use party ids, so that rule does not apply to it.
fn is_friendly_fire(
    target_kind: EntityKindRow,
    target_character_id: Option<Uuid>,
    source_kind: Option<EntityKindRow>,
    source_character_id: Option<Uuid>,
    target_party_id: Option<u64>,
    source_party_id: Option<u64>,
) -> bool {
    if source_kind != Some(EntityKindRow::Player) {
        return false;
    }
    if target_kind == EntityKindRow::AllyDummy {
        return true;
    }
    if target_kind != EntityKindRow::Player {
        return false;
    }
    let Some(target_character_id) = target_character_id else {
        return false;
    };
    let Some(source_character_id) = source_character_id else {
        return false;
    };
    if source_character_id == target_character_id {
        // The same character hitting itself (a self-targeted spell) is not
        // friendly fire; `apply_damage`'s existing behaviour is unaffected.
        return false;
    }
    match (target_party_id, source_party_id) {
        (Some(target_party_id), Some(source_party_id)) => target_party_id == source_party_id,
        _ => false,
    }
}

/// Whether a heal from `source` may land on `target`.
///
/// Life (and any other heal payload) restores the caster, their party, and
/// allied training dummies. Enemies, the hostile dummy, bosses and NPCs are
/// never healed — selecting one of them with Life is a no-op.
pub fn can_receive_heal(
    target_kind: EntityKindRow,
    target_character_id: Option<Uuid>,
    source_character_id: Option<Uuid>,
    target_party_id: Option<u64>,
    source_party_id: Option<u64>,
) -> bool {
    match target_kind {
        EntityKindRow::AllyDummy => true,
        EntityKindRow::Player => {
            if target_character_id.is_some() && target_character_id == source_character_id {
                return true;
            }
            match (target_party_id, source_party_id) {
                (Some(target_party), Some(source_party)) => target_party == source_party,
                _ => false,
            }
        }
        EntityKindRow::Dummy
        | EntityKindRow::Enemy
        | EntityKindRow::Boss
        | EntityKindRow::Npc
        | EntityKindRow::ResourceNode => false,
    }
}

/// Looks up party membership and asks [`can_receive_heal`]. Missing rows
/// fail closed: the heal is dropped rather than applied to a stranger.
pub fn heal_allowed_for(ctx: &ReducerContext, target: u64, source: Option<u64>) -> bool {
    let Some(target_entity) = ctx.db.game_entity().entity_id().find(&target) else {
        return false;
    };
    let source_entity = source.and_then(|id| ctx.db.game_entity().entity_id().find(&id));
    let source_character_id = source_entity
        .as_ref()
        .and_then(|entity| entity.owner_character_id);
    can_receive_heal(
        target_entity.kind,
        target_entity.owner_character_id,
        source_character_id,
        target_entity
            .owner_character_id
            .and_then(|id| ctx.db.party_member().character_id().find(&id))
            .map(|row| row.party_id),
        source_character_id
            .and_then(|id| ctx.db.party_member().character_id().find(&id))
            .map(|row| row.party_id),
    )
}

/// Heals `target` by `amount`, clamped to its `max_health`.
///
/// Does not resurrect: a dead target is skipped. The Bevy server would happily
/// heal a corpse back above zero while leaving `EntityState::Dead` in place,
/// producing a character that was alive by health and dead by state — and
/// `handle_respawn_requests` then refused to respawn it, because it tested for
/// dead state *or* zero health.
pub fn apply_healing(ctx: &ReducerContext, target: u64, amount: f32) {
    if amount <= 0.0 {
        return;
    }
    let Some(row) = ctx.db.entity_stats().entity_id().find(&target) else {
        return;
    };
    if is_dead(ctx, target) {
        return;
    }

    let current_health = (row.stats.current_health + amount).min(row.stats.max_health);
    let applied = current_health - row.stats.current_health;
    if applied <= 0.0 {
        // Already at full health: no row write, no combat text.
        return;
    }

    ctx.db.entity_stats().entity_id().update(EntityStats {
        stats: StatsRow {
            current_health,
            ..row.stats
        },
        ..row
    });
    ctx.db.damage_event().insert(DamageEventRow {
        target,
        amount: applied,
        is_healing: true,
        killed: false,
    });
}

/// Applies a timed buff or debuff to one stat field of `target`.
///
/// `field` is a [`StatField`] debug name — `"Speed"`, `"Armor"`,
/// `"AttackPower"`, `"ThreatGeneration"`, `"MaxHealth"`, `"MaxMana"`,
/// `"ManaRegeneration"`, `"GatheringSpeed"`, `"GatheringBonus"` — matching how
/// `stat_modifier.field` is stored. An unrecognised name is logged and ignored
/// rather than panicking: the caller is gameplay code, and a typo in a spell
/// definition should not take down the tick.
///
/// `duration` of `None` means "until removed" (respawn clears them).
///
/// Re-applying an identical modifier refreshes its timer instead of stacking a
/// second copy, which is the behaviour `refresh_or_insert_modifier` gave the
/// Bevy server: channelled spells re-apply their buff every tick, and stacking
/// would turn a 1.5x speed buff into a multiplicative explosion. Identity is
/// the whole tuple the row carries — source, target, field, operation,
/// magnitude — so two casters buffing the same stat keep two rows, and each
/// refreshes only its own.
pub fn apply_modifier(
    ctx: &ReducerContext,
    target: u64,
    source: Option<u64>,
    field: &str,
    amount: f32,
    is_multiplicative: bool,
    kind: ModifierKindRow,
    duration: Option<f32>,
) {
    let Some(parsed) = parse_stat_field(field) else {
        log::warn!("ignoring stat modifier on unknown field {field:?} for entity {target}");
        return;
    };
    let field = stat_field_name(parsed).to_string();

    let existing = ctx.db.stat_modifier().target().filter(&target).find(|row| {
        row.source == source
            && row.field == field
            && row.is_multiplicative == is_multiplicative
            && values_match(row.amount, amount)
    });

    match existing {
        Some(row) => {
            ctx.db.stat_modifier().id().update(StatModifier {
                kind,
                remaining_seconds: duration,
                ..row
            });
            // Same magnitude, same field: the effective stats already include
            // it, so only the timer moved. No recompute needed.
        }
        None => {
            ctx.db.stat_modifier().insert(StatModifier {
                id: 0,
                entity_id: target,
                source,
                field,
                is_multiplicative,
                amount,
                kind,
                remaining_seconds: duration,
                origin_status_instance_id: None,
            });
            recalculate_effective_stats(ctx, target);
        }
    }
}

/// Puts one dead entity back on its feet, whole and unencumbered.
///
/// The single definition of "coming back to life", shared by the player's
/// `respawn` reducer and the enemy respawn timer. They used to be two: the mob
/// path only refilled health, so a mob that died stunned and poisoned came back
/// stunned and poisoned, while the player path did the full cleanup. A boss got
/// the worst of it — it returned at full health still flagged `Enraged` and
/// `is_engaged`, with its threat table intact, so the fight resumed at its last
/// phase against a party that had just wiped it.
///
/// Order matters: the modifiers go first so `max_health` is the unbuffed number
/// before the refill tops it up.
pub fn resurrect(ctx: &ReducerContext, entity: GameEntity) {
    let entity_id = entity.entity_id;

    clear_modifiers(ctx, entity_id);
    clear_crowd_control(ctx, entity_id);
    clear_periodic_effects(ctx, entity_id);
    reset_boss_encounter(ctx, entity_id);

    // Re-read: `clear_modifiers` rewrites this row.
    if let Some(stats) = ctx.db.entity_stats().entity_id().find(&entity_id) {
        let refilled = StatsRow {
            current_health: stats.stats.max_health,
            ..stats.stats
        };
        ctx.db.entity_stats().entity_id().update(EntityStats {
            stats: refilled,
            current_mana: refilled.max_mana,
            ..stats
        });
    }

    let position = entity.spawn_point;
    let (cell_x, cell_z) = crate::tables::grid_cell(position);
    ctx.db.game_entity().entity_id().update(GameEntity {
        position,
        // Whatever it was walking towards when it died is not where it wants to
        // go from the graveyard.
        move_target: None,
        state: EntityStateRow::Idle,
        respawn_in_seconds: None,
        cell_x,
        cell_z,
        ..entity
    });
}

/// Drops every stun, root, silence and slow on an entity.
///
/// Respawning out of a stun is the point: the crowd control that killed the
/// character should not still be running when it stands back up.
fn clear_crowd_control(ctx: &ReducerContext, entity_id: u64) {
    // Collected first: deleting while the index iterator is live is not safe.
    let ids: Vec<u64> = ctx
        .db
        .crowd_control()
        .victim()
        .filter(&entity_id)
        .map(|row| row.id)
        .collect();
    for id in ids {
        ctx.db.crowd_control().id().delete(&id);
    }
}

/// Drops every poison and regeneration still running on an entity.
fn clear_periodic_effects(ctx: &ReducerContext, entity_id: u64) {
    let ids: Vec<u64> = ctx
        .db
        .periodic_effect()
        .on_entity()
        .filter(&entity_id)
        .map(|row| row.id)
        .collect();
    for id in ids {
        ctx.db.periodic_effect().id().delete(&id);
    }
}

/// Rewinds a boss encounter to its dormant state.
///
/// A no-op for anything that is not a boss. For one that is, the phase, the
/// engagement flag, the rotation cursor and the threat table all belong to the
/// *fight*, not to the creature, so they die with it.
fn reset_boss_encounter(ctx: &ReducerContext, entity_id: u64) {
    let Some(boss) = ctx.db.boss_state().entity_id().find(&entity_id) else {
        return;
    };
    ctx.db.boss_state().entity_id().update(BossState {
        phase: BossPhaseRow::Idle,
        is_engaged: false,
        engaged_seconds: 0.0,
        rotation_cursor: 0,
        ..boss
    });

    crate::sim::ai::clear_threat(ctx, entity_id);
}

/// Removes every modifier on `target` and rebuilds its stats.
///
/// Used by respawn. Returns the number of modifiers dropped.
pub fn clear_modifiers(ctx: &ReducerContext, target: u64) -> usize {
    let ids: Vec<u64> = ctx
        .db
        .stat_modifier()
        .target()
        .filter(&target)
        .map(|row| row.id)
        .collect();
    for id in &ids {
        ctx.db.stat_modifier().id().delete(id);
    }
    if !ids.is_empty() {
        recalculate_effective_stats(ctx, target);
    }
    ids.len()
}

// ---------------------------------------------------------------------------
// Base stats versus effective stats
// ---------------------------------------------------------------------------

/// Rebuilds `entity_stats` for one entity from its base stats.
///
/// The split this preserves: `player_stats` stores what the character *is*,
/// with no equipment and no buffs folded in, and `entity_stats` stores what it
/// currently *fights like*. The Bevy server kept the same invariant by
/// persisting `base_stats_without_equipment` on disconnect; storing effective
/// values instead would compound the equipment bonus on every login. Here the
/// derived row is rebuilt from scratch every time rather than reverted-and-
/// reapplied, so there is no snapshot to drift.
///
/// Health and mana are *not* derived — they are runtime state — so they survive
/// the rebuild untouched, clamped in case a buff that was raising `max_health`
/// just expired. They are deliberately never copied back into `player_stats`:
/// the Bevy server had to, because its only durable store was the disconnect
/// snapshot, but `entity_stats` is itself a persisted table that `init` leaves
/// alone for player-owned entities. Writing effective health into the base row
/// is exactly the double-counting the split exists to prevent.
pub fn recalculate_effective_stats(ctx: &ReducerContext, entity_id: u64) {
    let Some(current) = ctx.db.entity_stats().entity_id().find(&entity_id) else {
        return;
    };
    let Some(entity) = ctx.db.game_entity().entity_id().find(&entity_id) else {
        return;
    };
    let Some(mut stats) = base_stats(ctx, &entity) else {
        // No reconstructible base: leaving the row alone is the safe failure.
        // Overwriting it with a guess would silently rewrite a hand-tuned spawn.
        log::debug!(
            "entity {entity_id} has no base stats to derive from; leaving stats as they are"
        );
        return;
    };

    apply_modifiers(ctx, entity_id, &mut stats);

    stats.current_health = current.stats.current_health.clamp(0.0, stats.max_health);
    let current_mana = current.current_mana.clamp(0.0, stats.max_mana);

    // `game_entity.speed` is what the movement simulation reads, so a Speed
    // bonus or a Slow that only reached `entity_stats` would do nothing at all.
    //
    // This is the *only* place that writes it, deliberately. Both equipment
    // recomputation and crowd control want to change movement rate, and two
    // owners writing the same column each tick would take turns undoing each
    // other. They change the inputs; this derives the result.
    //
    // The units differ: `StatsRow::movement_speed` is the Bevy value in units
    // per 60 Hz tick, `GameEntity::speed` is units per second.
    let speed = stats.movement_speed * LEGACY_TICKS_PER_SECOND;
    if entity.speed != speed {
        ctx.db
            .game_entity()
            .entity_id()
            .update(GameEntity { speed, ..entity });
    }

    if stats == current.stats && current_mana == current.current_mana {
        return;
    }
    ctx.db.entity_stats().entity_id().update(EntityStats {
        stats,
        current_mana,
        ..current
    });
}

/// The tick rate the Bevy server's `MovementStats::speed` was expressed against.
///
/// Named rather than a bare `60.0` because it is the whole reason
/// `player_stats.movement_speed` and `game_entity.speed` differ by two orders
/// of magnitude.
pub const LEGACY_TICKS_PER_SECOND: f32 = 60.0;

/// The stats an entity has before any timed modifier.
///
/// For a player that is the persisted base plus equipment bonuses; for anything
/// else it is the kind's default profile. `None` means the base cannot be
/// reconstructed — an NPC, or a character whose `player_stats` row is missing —
/// and the caller must then leave the effective stats alone.
fn base_stats(ctx: &ReducerContext, entity: &GameEntity) -> Option<StatsRow> {
    if let Some(character_id) = entity.owner_character_id {
        let persisted = ctx.db.player_stats().character_id().find(&character_id)?;
        let mut stats = persisted.stats;
        apply_equipment_bonuses(ctx, character_id, &mut stats);
        return Some(stats);
    }

    if let Some(spawned) = base_stats_map()
        .as_ref()
        .and_then(|map| map.get(&entity.entity_id))
        .copied()
    {
        return Some(spawned);
    }

    // Non-players have no persisted base row, so the kind's default profile is
    // the fallback when spawn stats were never recorded (legacy rows).
    let defaults = match entity.kind {
        EntityKindRow::Player => defaults::player_defaults(),
        EntityKindRow::Enemy => defaults::enemy_defaults(),
        EntityKindRow::Boss => defaults::boss_defaults(),
        EntityKindRow::Dummy | EntityKindRow::AllyDummy => defaults::dummy_defaults(),
        EntityKindRow::Npc | EntityKindRow::ResourceNode => return None,
    };
    Some(StatsRow::from(&defaults))
}

/// Folds the passive bonuses of everything the character has equipped into
/// `stats`.
///
/// Slot order is `EQUIP_SLOTS`, matching `recompute_equipment_bonuses` in the
/// Bevy server, so a `Multiply` bonus composes with an `Add` bonus the same way
/// it did there.
fn apply_equipment_bonuses(ctx: &ReducerContext, character_id: Uuid, stats: &mut StatsRow) {
    let Some(row) = ctx.db.equipment().character_id().find(&character_id) else {
        return;
    };
    let equipment = equipment_from_rows(&row.slots);
    let registry = default_items();

    for slot in EQUIP_SLOTS {
        let Some(instance) = equipment.get(slot).as_ref() else {
            continue;
        };
        let Some(item) = registry.get(&instance.item_id) else {
            log::warn!(
                "equipped item {} is not in the registry",
                instance.item_id.as_str()
            );
            continue;
        };
        for effect in item.effects() {
            if !effect.is_passive_while_equipped() {
                continue;
            }
            if let ItemEffect::StatBonus { field, op, value } = effect {
                // GatheringSpeed / GatheringBonus are resource-scoped: a node's
                // `bonus_tools` decides when they apply. Folding them here
                // would speed every gather, including the wrong resource.
                if matches!(field, StatField::GatheringSpeed | StatField::GatheringBonus) {
                    continue;
                }
                apply_stat_op(stats, *field, *op, *value);
            }
        }
    }
}

/// Folds every active `stat_modifier` for `entity_id` into `stats`.
///
/// Additive modifiers all land before multiplicative ones. The Bevy
/// `effective_value` documented that order but applied effects in whatever
/// order they happened to sit in the entity's `Vec`, so `+20` then `*1.5` and
/// `*1.5` then `+20` gave different armor. Rows come back from an index in no
/// guaranteed order, so an order-dependent fold would be worse than
/// non-deterministic here — it would be non-reproducible.
fn apply_modifiers(ctx: &ReducerContext, entity_id: u64, stats: &mut StatsRow) {
    let modifiers: Vec<StatModifier> = ctx.db.stat_modifier().target().filter(&entity_id).collect();

    for multiplicative in [false, true] {
        for modifier in &modifiers {
            if modifier.is_multiplicative != multiplicative {
                continue;
            }
            let Some(field) = parse_stat_field(&modifier.field) else {
                log::warn!(
                    "stat modifier {} on entity {entity_id} names unknown field {:?}",
                    modifier.id,
                    modifier.field
                );
                continue;
            };
            let op = if multiplicative {
                ModifierOp::Multiply
            } else {
                ModifierOp::Add
            };
            apply_stat_op(stats, field, op, modifier.amount);
        }
    }
}

/// Applies one `field op value` to a stats row.
///
/// `StatField::Speed` addresses `movement_speed`. The walking system reads
/// `game_entity.speed` rather than this field, but
/// [`recalculate_effective_stats`] derives that column from this one at the end
/// of every fold, so a Speed modifier does reach the legs.
fn apply_stat_op(stats: &mut StatsRow, field: StatField, op: ModifierOp, value: f32) {
    let slot: &mut f32 = match field {
        StatField::Speed => &mut stats.movement_speed,
        StatField::Armor => &mut stats.armor,
        StatField::AttackPower => &mut stats.attack_power,
        StatField::ThreatGeneration => &mut stats.threat_generation,
        StatField::MaxHealth => &mut stats.max_health,
        StatField::MaxMana => &mut stats.max_mana,
        StatField::ManaRegeneration => &mut stats.mana_regeneration,
        StatField::GatheringSpeed => &mut stats.gathering_speed,
        StatField::GatheringBonus => &mut stats.gathering_bonus,
    };
    match op {
        ModifierOp::Add => *slot += value,
        ModifierOp::Multiply => *slot *= value,
        ModifierOp::Override => *slot = value,
    }
}

/// `stat_modifier.field` as the `StatField` it names.
fn parse_stat_field(field: &str) -> Option<StatField> {
    match field {
        "Speed" => Some(StatField::Speed),
        "Armor" => Some(StatField::Armor),
        "AttackPower" => Some(StatField::AttackPower),
        "ThreatGeneration" => Some(StatField::ThreatGeneration),
        "MaxHealth" => Some(StatField::MaxHealth),
        "MaxMana" => Some(StatField::MaxMana),
        "ManaRegeneration" => Some(StatField::ManaRegeneration),
        "GatheringSpeed" => Some(StatField::GatheringSpeed),
        "GatheringBonus" => Some(StatField::GatheringBonus),
        _ => None,
    }
}

/// The spelling [`parse_stat_field`] round-trips.
fn stat_field_name(field: StatField) -> &'static str {
    match field {
        StatField::Speed => "Speed",
        StatField::Armor => "Armor",
        StatField::AttackPower => "AttackPower",
        StatField::ThreatGeneration => "ThreatGeneration",
        StatField::MaxHealth => "MaxHealth",
        StatField::MaxMana => "MaxMana",
        StatField::ManaRegeneration => "ManaRegeneration",
        StatField::GatheringSpeed => "GatheringSpeed",
        StatField::GatheringBonus => "GatheringBonus",
    }
}

// ---------------------------------------------------------------------------
// Death
// ---------------------------------------------------------------------------

/// Flips an entity to [`EntityStateRow::Dead`] and stops it where it fell.
///
/// Clearing `move_target` matters even though the movement step already skips
/// the dead: without it a corpse that respawns mid-walk would resume the walk
/// it was on when it died.
fn respawn_delay(kind: EntityKindRow) -> Option<f32> {
    match kind {
        EntityKindRow::Player => None,
        EntityKindRow::Dummy | EntityKindRow::AllyDummy => Some(DUMMY_RESPAWN_SECONDS),
        _ => Some(ENEMY_RESPAWN_SECONDS),
    }
}

fn kill(ctx: &ReducerContext, entity: GameEntity) {
    // A player waits for the respawn reducer; everything else comes back on a
    // timer. Without this the world empties permanently after one sweep of the
    // map, which is what the Bevy server's despawn-and-respawn scheduling
    // avoided and the port initially lost.
    let respawn_in_seconds = respawn_delay(entity.kind);
    ctx.db.game_entity().entity_id().update(GameEntity {
        state: EntityStateRow::Dead,
        move_target: None,
        respawn_in_seconds,
        ..entity
    });
}

/// Whether the entity is currently a corpse.
fn is_dead(ctx: &ReducerContext, entity_id: u64) -> bool {
    ctx.db
        .game_entity()
        .entity_id()
        .find(&entity_id)
        .map(|entity| entity.state == EntityStateRow::Dead)
        .unwrap_or(false)
}

/// Compares two modifier magnitudes for "is this the same buff".
///
/// A tolerance rather than `==` for the same reason the Bevy server used one:
/// the values are authored constants today, but the matcher should not start
/// stacking duplicates the day they come from a config file.
fn values_match(left: f32, right: f32) -> bool {
    (left - right).abs() <= f32::EPSILON
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYER: EntityKindRow = EntityKindRow::Player;
    const ENEMY: EntityKindRow = EntityKindRow::Enemy;
    const DUMMY: EntityKindRow = EntityKindRow::Dummy;
    const ALLY_DUMMY: EntityKindRow = EntityKindRow::AllyDummy;

    /// A deterministic, readable stand-in for a real `ctx.new_uuid_v4()`
    /// character id — tests only care that these compare equal/unequal
    /// consistently with the small integer they were built from.
    fn cid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn two_different_players_in_the_same_party_is_friendly_fire() {
        assert!(is_friendly_fire(
            PLAYER,
            Some(cid(1)),
            Some(PLAYER),
            Some(cid(2)),
            Some(100),
            Some(100),
        ));
    }

    #[test]
    fn two_players_in_different_parties_is_not_friendly_fire() {
        assert!(!is_friendly_fire(
            PLAYER,
            Some(cid(1)),
            Some(PLAYER),
            Some(cid(2)),
            Some(100),
            Some(200),
        ));
    }

    #[test]
    fn a_character_hitting_itself_is_not_friendly_fire_even_in_the_same_party() {
        assert!(!is_friendly_fire(
            PLAYER,
            Some(cid(1)),
            Some(PLAYER),
            Some(cid(1)),
            Some(100),
            Some(100),
        ));
    }

    #[test]
    fn a_non_player_target_is_never_friendly_fire() {
        // Same party ids on both sides: if the `EntityKindRow::Player` guard
        // were ever dropped, this would flip to `true` and a boss/dummy/enemy
        // could suddenly become unhittable by a grouped player.
        assert!(!is_friendly_fire(
            ENEMY,
            None,
            Some(PLAYER),
            Some(cid(2)),
            Some(100),
            Some(100),
        ));
    }

    #[test]
    fn a_non_player_source_is_never_friendly_fire() {
        assert!(!is_friendly_fire(
            PLAYER,
            Some(cid(1)),
            Some(ENEMY),
            None,
            Some(100),
            Some(100),
        ));
    }

    #[test]
    fn damage_with_no_source_is_never_friendly_fire() {
        assert!(!is_friendly_fire(
            PLAYER,
            Some(cid(1)),
            None,
            None,
            Some(100),
            None
        ));
    }

    #[test]
    fn an_ungrouped_target_is_never_friendly_fire() {
        assert!(!is_friendly_fire(
            PLAYER,
            Some(cid(1)),
            Some(PLAYER),
            Some(cid(2)),
            None,
            Some(100),
        ));
    }

    #[test]
    fn an_ungrouped_source_is_never_friendly_fire() {
        assert!(!is_friendly_fire(
            PLAYER,
            Some(cid(1)),
            Some(PLAYER),
            Some(cid(2)),
            Some(100),
            None,
        ));
    }

    #[test]
    fn a_player_entity_with_no_owning_character_is_never_friendly_fire() {
        // Defensive: should not happen in practice (every `Player`-kind
        // entity is owned), but ambiguous data must fail closed, not panic
        // or, worse, block a legitimate hit.
        assert!(!is_friendly_fire(
            PLAYER,
            None,
            Some(PLAYER),
            Some(cid(2)),
            Some(100),
            Some(100),
        ));
    }

    #[test]
    fn a_dummy_comes_back_after_ten_seconds() {
        assert_eq!(respawn_delay(EntityKindRow::Dummy), Some(10.0));
        assert_eq!(respawn_delay(EntityKindRow::AllyDummy), Some(10.0));
        assert_eq!(respawn_delay(EntityKindRow::Player), None);
    }

    #[test]
    fn life_heals_the_caster() {
        assert!(can_receive_heal(
            PLAYER,
            Some(cid(1)),
            Some(cid(1)),
            None,
            None,
        ));
    }

    #[test]
    fn life_heals_a_party_member() {
        assert!(can_receive_heal(
            PLAYER,
            Some(cid(1)),
            Some(cid(2)),
            Some(100),
            Some(100),
        ));
    }

    #[test]
    fn life_does_not_heal_a_stranger() {
        assert!(!can_receive_heal(
            PLAYER,
            Some(cid(1)),
            Some(cid(2)),
            None,
            None,
        ));
        assert!(!can_receive_heal(
            PLAYER,
            Some(cid(1)),
            Some(cid(2)),
            Some(100),
            Some(200),
        ));
    }

    #[test]
    fn life_heals_an_ally_dummy_and_not_the_hostile_one() {
        assert!(can_receive_heal(ALLY_DUMMY, None, Some(cid(1)), None, None));
        assert!(!can_receive_heal(DUMMY, None, Some(cid(1)), None, None));
        assert!(!can_receive_heal(ENEMY, None, Some(cid(1)), None, None));
    }

    #[test]
    fn a_player_cannot_damage_the_ally_dummy() {
        assert!(is_friendly_fire(
            ALLY_DUMMY,
            None,
            Some(PLAYER),
            Some(cid(1)),
            None,
            None,
        ));
    }

    #[test]
    fn a_player_can_still_damage_the_hostile_dummy() {
        assert!(!is_friendly_fire(
            DUMMY,
            None,
            Some(PLAYER),
            Some(cid(1)),
            None,
            None,
        ));
    }

    #[test]
    fn an_enemy_can_still_damage_the_ally_dummy() {
        assert!(!is_friendly_fire(
            ALLY_DUMMY,
            None,
            Some(ENEMY),
            None,
            None,
            None,
        ));
    }
}
