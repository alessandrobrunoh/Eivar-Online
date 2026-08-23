//! What the mobs do: aggroing, chasing, hitting, and — for the dragon —
//! phases, threat and an ability rotation.
//!
//! Ported from `gameplay/entity/enemy/systems.rs` (`enemy_chase`,
//! `enemy_auto_cast_attack`), `gameplay/entity/boss/systems.rs` (the phase
//! machine, arena aggro, the rotation driver) and
//! `gameplay/entity/boss/target_select.rs` (the pure selection helpers, which
//! are re-stated here because they were written against `bevy::Entity`).
//!
//! # Where the death transition went
//!
//! `gameplay/entity/systems.rs::mark_dead_entities` is *not* here. It landed as
//! `sim::combat::reap_the_dead`, which runs at the end of `sim::combat::step` —
//! the step immediately before this one — precisely so the AI sees corpses as
//! corpses. Marking the dead again here would give one transition two writers,
//! so this module instead treats `game_entity.state` as authoritative, which is
//! exactly what the Bevy enemy and boss systems did (`if state.is_dead() {
//! continue; }`). The dependency runs the other way too: were the sweep ever
//! moved out of `combat::step`, or reordered after this one, the AI would drive
//! corpses for a tick.
//!
//! # Finding targets without an ECS
//!
//! The Bevy version answered "who is near me" with a `Query<&Position,
//! With<Player>>` and a `min_by` over *every* player, once per mob per tick.
//! That is `O(mobs × players)` and it was affordable only because Bevy handed
//! the system a packed array. Repeating it here would mean a full table scan of
//! `game_entity` per mob per tick, which does not survive a populated map.
//!
//! Instead every "who is within R of P" question goes through
//! [`living_players_near`], which walks the `cell_x`/`cell_z` btree index on
//! `game_entity`: the query rectangle covers `(2R / GRID_CELL_SIZE) + 2` cells
//! per axis, and the index is scanned one `cell_x` column at a time with a
//! range over `cell_z` — the only shape a multi-column btree supports, since
//! every column before the last must be an exact value. With
//! `GRID_CELL_SIZE = 16`, an enemy's 10-unit aggro check touches at most 2×2
//! cells and the boss's 12-unit arena at most 3×3, whatever the population.
//!
//! One full scan of `game_entity` per tick remains, in [`collect_actors`]:
//! there is no index on `kind`, so the mobs have to be found by looking. It is
//! one pass for the whole tick rather than one per mob.

use bevymmo_domain::abilities::AbilityId;
use bevymmo_domain::entity::boss::components::{Boss, BossPhase, BossRotationState};
use bevymmo_domain::entity::enemy::aggro::{
    acquire_center, acquires_by_proximity, horizontal_distance, is_leashed, select_target,
    ThreatCandidate, ThreatPolicy,
};
use bevymmo_domain::entity::enemy::kit::AbilityTargeting;
use bevymmo_domain::entity::enemy::pick::pick_ability;
use bevymmo_domain::entity::enemy::threat::threat_from_damage;
use bevymmo_domain::movement;
use bevymmo_domain::placeables::EnemyConfig;
use bevymmo_domain::EntityId;
use glam::Vec3;
use spacetimedb::{ReducerContext, Table};

use crate::rows::Vec3Row;
use crate::sim::targets;
use crate::sim::{crowd_control, spells};
use crate::tables::{
    boss_state, cast_state, enemy_ai, entity_stats, game_entity, grid_cell, threat, BossPhaseRow,
    BossState, EntityKindRow, EntityStateRow, GameEntity, Threat,
};
use crate::world;

/// Distance a melee attacker stops at, so it fights the target instead of
/// standing inside it. From `boss/systems.rs::MELEE_REACH`.
const MELEE_REACH: f32 = 3.0;

/// How close to its spawn a leashing enemy has to get before it stops walking.
///
/// Purely so it does not twitch between "one centimetre out" and "home".
const HOME_ARRIVAL_EPSILON: f32 = 0.25;

/// Hard enrage safety net: the boss is forced to `Enraged` after this many
/// seconds engaged. From `boss/systems.rs::BERSERK_TIMER_SECONDS`.
const BERSERK_TIMER_SECONDS: f32 = 180.0;

/// HP fraction at which the boss leaves phase one.
const AERIAL_HP_FRACTION: f32 = 0.66;

/// HP fraction at which the boss enrages.
const BERSERK_HP_FRACTION: f32 = 0.33;

/// Ceiling on the radius of a spatial query, in world units.
///
/// A radius is data — an arena row could be seeded with a nonsense value — and
/// the cell loop is quadratic in it. 128 units is eight cells per axis either
/// way, far past anything the encounters use, and bounds the worst tick.
const MAX_SPATIAL_QUERY_RADIUS: f32 = 128.0;

/// The mobs this tick has to drive, gathered in a single pass.
struct Actors {
    enemies: Vec<GameEntity>,
    bosses: Vec<GameEntity>,
}

/// A living player as the selection helpers want it: id and position.
///
/// The Bevy shape carried a `bevy::Entity`; here it is the `game_entity` key.
#[derive(Clone, Copy)]
struct PlayerRef {
    entity: u64,
    position: Vec3,
}

pub fn step(ctx: &ReducerContext, dt: f32) {
    let online = targets::online_character_ids(ctx);
    let actors = collect_actors(ctx);
    for enemy in actors.enemies {
        step_enemy(ctx, &online, enemy);
    }
    for boss in actors.bosses {
        step_boss(ctx, &online, boss, dt);
    }
}

/// The one full scan of `game_entity` this step takes, splitting out the mobs
/// that have AI.
///
/// Collected into `Vec`s rather than driven inline because everything below
/// writes to `game_entity` — chasing, facing, dying targets — and a tick may
/// not mutate a table it is iterating.
fn collect_actors(ctx: &ReducerContext) -> Actors {
    let mut enemies = Vec::new();
    let mut bosses = Vec::new();

    for entity in ctx.db.game_entity().iter() {
        if entity.state == EntityStateRow::Dead {
            continue;
        }
        match entity.kind {
            EntityKindRow::Enemy => enemies.push(entity),
            EntityKindRow::Boss => bosses.push(entity),
            // Players drive themselves; dummies and NPCs have no AI.
            EntityKindRow::Player
            | EntityKindRow::Dummy
            | EntityKindRow::AllyDummy
            | EntityKindRow::Npc
            | EntityKindRow::ResourceNode => {}
        }
    }

    Actors { enemies, bosses }
}

// ---------------------------------------------------------------------------
// Enemies
// ---------------------------------------------------------------------------

fn config_for(ctx: &ReducerContext, entity_id: u64) -> Option<EnemyConfig> {
    let row = ctx.db.enemy_ai().entity_id().find(&entity_id)?;
    world::enemy_config_for(&row.kind_id)
}

/// One enemy's turn: pick a target, close to melee, swing, or walk home.
///
/// Two departures from `enemy_chase`/`enemy_auto_cast_attack`:
///
/// - Bevy moved the enemy by writing `position += direction * speed` every
///   tick, where `speed` was implicitly per-tick at a fixed 60 Hz. Here the AI
///   only chooses a `move_target` and `sim::movement::step` walks it in units
///   per second, so the enemy covers ground at the same rate whatever the tick
///   length is, and arrives through the same code path a player does.
/// - Bevy had no leash: an enemy that lost its target stopped wherever it
///   stood, which over a session drags every mob out of its camp. This one
///   walks back to its `spawn_point`, the field the schema already carries for
///   exactly that.
fn step_enemy(
    ctx: &ReducerContext,
    online: &std::collections::HashSet<spacetimedb::Uuid>,
    enemy: GameEntity,
) {
    let entity_id = enemy.entity_id;
    let position = Vec3::from(enemy.position);
    let spawn = Vec3::from(enemy.spawn_point);
    let Some(config) = config_for(ctx, entity_id) else {
        return;
    };

    if is_leashed(spawn, position, config.leash_aggro) {
        clear_threat(ctx, entity_id);
        let move_target = if position.distance(spawn) > HOME_ARRIVAL_EPSILON {
            Some(enemy.spawn_point)
        } else {
            None
        };
        let look = enemy.look;
        let move_target = gate_movement(ctx, entity_id, move_target);
        write_pose(ctx, enemy, look, move_target);
        return;
    }

    let target = select_enemy_target(ctx, online, entity_id, position, spawn, &config);

    let (look, move_target) = match &target {
        Some(target) => {
            let look = movement::look_direction(position, target.position)
                .map(Vec3Row::from)
                .unwrap_or(enemy.look);
            // Aim one melee reach short of the target: walking all the way onto
            // it is what let the Bevy enemies climb into the player model.
            let offset = target.position - position;
            let horizontal = Vec3::new(offset.x, 0.0, offset.z);
            let distance = horizontal.length();
            let move_target = if distance > MELEE_REACH {
                let direction = horizontal / distance;
                Some(Vec3Row::from(
                    position + direction * (distance - MELEE_REACH),
                ))
            } else {
                // In reach: hold still and fight.
                None
            };
            (look, move_target)
        }
        None => {
            let move_target = if position.distance(spawn) > HOME_ARRIVAL_EPSILON {
                Some(enemy.spawn_point)
            } else {
                None
            };
            (enemy.look, move_target)
        }
    };

    let move_target = gate_movement(ctx, entity_id, move_target);
    let enemy = write_pose(ctx, enemy, look, move_target);

    if let Some(target) = target {
        try_attack(ctx, online, &enemy, &target, &config);
    }
}

/// Acquire + threat-policy pick for one trash mob.
///
/// Proximity scans the authored origin (body or spawn). Passive skips that
/// scan and only fights a sticky/table target that damage already wrote.
/// Sticky remembers the first chosen id via the threat table (amount 1).
fn select_enemy_target(
    ctx: &ReducerContext,
    online: &std::collections::HashSet<spacetimedb::Uuid>,
    entity_id: u64,
    position: Vec3,
    spawn: Vec3,
    config: &EnemyConfig,
) -> Option<PlayerRef> {
    let center = acquire_center(config.origin, position, spawn);
    let mut candidates = if acquires_by_proximity(config.acquire) {
        living_players_near(ctx, online, center, config.aggro)
    } else {
        Vec::new()
    };

    let current_id = match config.threat {
        ThreatPolicy::Nearest => None,
        ThreatPolicy::Sticky | ThreatPolicy::Table => current_threat_target(ctx, entity_id),
    };
    if let Some(id) = current_id {
        if !candidates.iter().any(|player| player.entity == id) {
            if let Some(player) = living_player_by_id(ctx, online, id) {
                candidates.push(player);
            }
        }
    }
    if config.threat == ThreatPolicy::Table {
        for row in ctx.db.threat().by_combatant().filter(&entity_id) {
            if candidates
                .iter()
                .any(|player| player.entity == row.target_entity)
            {
                continue;
            }
            if let Some(player) = living_player_by_id(ctx, online, row.target_entity) {
                candidates.push(player);
            }
        }
    }

    let mapped: Vec<ThreatCandidate> = candidates
        .iter()
        .map(|player| ThreatCandidate {
            entity: EntityId::new(player.entity),
            distance: horizontal_distance(position, player.position),
        })
        .collect();
    let selected = select_target(
        config.threat,
        &mapped,
        current_id.map(EntityId::new),
        |id| threat_amount(ctx, entity_id, id.get()),
    );

    if config.threat == ThreatPolicy::Sticky {
        match selected {
            Some(id) => remember_sticky(ctx, entity_id, id.get()),
            None => clear_threat(ctx, entity_id),
        }
    }

    let selected = selected?;
    candidates
        .into_iter()
        .find(|player| player.entity == selected.get())
}

/// Requests the basic attack when the target is genuinely reachable.
///
/// `enemy_auto_cast_attack` fired at *aggro* range — ten units — even though
/// `attack` only lands within its three-unit radius, so every enemy in the zone
/// burned its cooldown swinging at air the whole way in. The gate here is the
/// spell's own radius.
fn try_attack(
    ctx: &ReducerContext,
    online: &std::collections::HashSet<spacetimedb::Uuid>,
    enemy: &GameEntity,
    target: &PlayerRef,
    config: &EnemyConfig,
) {
    if !can_start_cast(ctx, enemy.entity_id) {
        return;
    }
    let position = Vec3::from(enemy.position);
    let distance = crate::sim::spells::flat_distance(position, target.position);
    let hp_fraction = health_fraction(ctx, enemy.entity_id);
    let Some(entry) = pick_ability(&config.abilities, distance, hp_fraction, |id| {
        ability_is_ready(ctx, enemy.entity_id, config, id)
    }) else {
        return;
    };
    let ability_id = &entry.ability_id;
    let range = spells::base_abilities()
        .get(ability_id)
        .map(|ability| ability.base_params().range)
        .unwrap_or(5.0);
    if distance > range {
        return;
    }
    let scan_radius = config.leash_aggro.max(config.aggro).max(16.0);
    let living = living_players_near(ctx, online, position, scan_radius);
    let Some((target_entity, target_position)) =
        resolve_kit_target(&entry.use_when.targeting, position, target, &living)
    else {
        return;
    };
    if !spells::request_catalog_ability(
        ctx,
        enemy,
        ability_id.as_str(),
        target_entity,
        target_position,
    ) {
        return;
    }
    if entry.use_when.interval > 0.0 {
        spells::start_cooldown(
            ctx,
            enemy.entity_id,
            &kit_interval_key(ability_id.as_str()),
            entry.use_when.interval,
        );
    }
}

fn ability_is_ready(
    ctx: &ReducerContext,
    entity_id: u64,
    config: &EnemyConfig,
    ability_id: &AbilityId,
) -> bool {
    if spells::is_on_cooldown(ctx, entity_id, ability_id.as_str()) {
        return false;
    }
    let interval = config
        .abilities
        .iter()
        .find(|entry| &entry.ability_id == ability_id)
        .map(|entry| entry.use_when.interval)
        .unwrap_or(0.0);
    if interval <= 0.0 {
        return true;
    }
    !spells::is_on_cooldown(ctx, entity_id, &kit_interval_key(ability_id.as_str()))
}

fn kit_interval_key(ability_id: &str) -> String {
    format!("kit-interval:{ability_id}")
}

fn resolve_kit_target(
    targeting: &AbilityTargeting,
    caster_position: Vec3,
    main: &PlayerRef,
    living: &[PlayerRef],
) -> Option<(Option<u64>, Option<Vec3>)> {
    match targeting {
        AbilityTargeting::Main => Some((Some(main.entity), Some(main.position))),
        AbilityTargeting::SelfCentered => Some((None, Some(caster_position))),
        AbilityTargeting::Farthest => {
            farthest_target(living, caster_position).map(|p| (Some(p.entity), Some(p.position)))
        }
        AbilityTargeting::DensestCluster { n } => {
            densest_cluster(living, *n).map(|c| (None, Some(c)))
        }
    }
}

// ---------------------------------------------------------------------------
// Boss
// ---------------------------------------------------------------------------

/// One boss's turn: engage, advance the phase, chase, cast.
///
/// `boss_aggro_check`, `update_boss_phase`, `boss_chase` and
/// `run_boss_rotation` were four systems over the same handful of components.
/// Here they are four steps over one `boss_state` row, written back once at the
/// end — four round trips through the same row would be four times the work for
/// the same answer.
fn step_boss(
    ctx: &ReducerContext,
    online: &std::collections::HashSet<spacetimedb::Uuid>,
    boss: GameEntity,
    dt: f32,
) {
    let entity_id = boss.entity_id;
    let Some(state) = ctx.db.boss_state().entity_id().find(entity_id) else {
        // A boss entity with no arena row is content that has not been seeded
        // yet. Nothing to drive, and nothing worth logging every tick.
        return;
    };

    let arena_center = Vec3::from(state.arena_center);
    // A missing or nonsense radius falls back to the encounter's own constant
    // rather than to zero, which would make the arena impossible to enter.
    let arena_radius = if state.arena_radius > 0.0 {
        state.arena_radius
    } else {
        Boss::ARENA_RADIUS
    };

    // Candidates are the living players *inside the arena*. Bevy considered
    // every player in the world, which is free with an ECS query and is not
    // free here — and a player who has left the ring has left the fight, so the
    // narrower set is also the more correct one. It is what makes the whole
    // encounter cost a 3×3 cell scan per tick.
    let living = living_players_near(ctx, online, arena_center, arena_radius);

    let mut phase = phase_from_row(state.phase);
    let mut is_engaged = state.is_engaged;
    // The domain's own scheduler state, borrowed for the tick so the phase
    // rules below read exactly as `update_boss_phase` did.
    let mut rotation = BossRotationState {
        engaged_seconds: state.engaged_seconds,
        priority_cursor: state.rotation_cursor as usize,
    };

    if !is_engaged {
        if living.is_empty() {
            return;
        }
        is_engaged = true;
        phase = BossPhase::Ground;
        rotation.engaged_seconds = 0.0;
        rotation.priority_cursor = 0;
        log::info!("Boss {entity_id} engaged: a player crossed the arena ring");
    }

    phase = advance_phase(ctx, entity_id, phase, &mut rotation, dt);

    let boss_position = Vec3::from(boss.position);
    match main_target(ctx, entity_id, &living, boss_position) {
        Some(main) => {
            // `chase` hands the row back because it may have rewritten `look`,
            // and a breath fired from a stale facing points at where the target
            // used to be.
            let boss = chase(ctx, boss, phase, main);
            if let Some(config) = world::boss_config_for("boss_dragon") {
                run_rotation(ctx, &boss, &config, &living, main, &mut rotation);
            }
        }
        None => {
            // Engaged with nobody in the ring: the fight is still running (the
            // enrage timer keeps ticking) but there is nothing to aim at.
        }
    }

    ctx.db.boss_state().entity_id().update(BossState {
        phase: phase_to_row(phase),
        is_engaged,
        engaged_seconds: rotation.engaged_seconds,
        rotation_cursor: rotation.priority_cursor as u32,
        ..state
    });
}

/// The phase machine: HP thresholds first, enrage timer as the safety net.
///
/// Transitions are monotonic forward and never regress, as in
/// `update_boss_phase`. Death is *not* handled here: `BossPhaseRow` has no
/// `Dead` variant, so a defeated boss is one whose `game_entity.state` is
/// `Dead`, and `collect_actors` filtered it out before this ran. The phase
/// column keeps the phase the boss died in, which is the more useful thing for
/// a client to show over the corpse anyway.
fn advance_phase(
    ctx: &ReducerContext,
    entity_id: u64,
    phase: BossPhase,
    rotation: &mut BossRotationState,
    dt: f32,
) -> BossPhase {
    let hp_fraction = health_fraction(ctx, entity_id);

    // Only accrue enrage time while the encounter is live.
    if matches!(phase, BossPhase::Ground | BossPhase::Aerial) {
        rotation.engaged_seconds += dt;
    }

    match phase {
        BossPhase::Ground => {
            if hp_fraction <= BERSERK_HP_FRACTION {
                log::info!("Boss {entity_id} entered Berserk (HP skip)");
                BossPhase::Berserk
            } else if hp_fraction <= AERIAL_HP_FRACTION {
                log::info!("Boss {entity_id} entered the aerial phase");
                BossPhase::Aerial
            } else if rotation.engaged_seconds >= BERSERK_TIMER_SECONDS {
                log::info!("Boss {entity_id} force-enraged by timer");
                BossPhase::Berserk
            } else {
                phase
            }
        }
        BossPhase::Aerial => {
            if hp_fraction <= BERSERK_HP_FRACTION {
                log::info!("Boss {entity_id} entered Berserk");
                BossPhase::Berserk
            } else if rotation.engaged_seconds >= BERSERK_TIMER_SECONDS {
                log::info!("Boss {entity_id} force-enraged by timer");
                BossPhase::Berserk
            } else {
                phase
            }
        }
        // Berserk holds, Dormant is unreachable once engaged, Dead is terminal.
        BossPhase::Berserk | BossPhase::Dormant | BossPhase::Dead => phase,
    }
}

/// Walks the boss towards its main target, stopping at melee reach, and returns
/// the row as it now stands.
///
/// The aerial phase does not chase: the dragon is flying. Facing is updated in
/// every phase, because the cone abilities — searing breath, tail sweep — are
/// aimed by `look` at the moment they fire.
fn chase(
    ctx: &ReducerContext,
    boss: GameEntity,
    phase: BossPhase,
    target: &PlayerRef,
) -> GameEntity {
    let position = Vec3::from(boss.position);
    let offset = target.position - position;
    let horizontal = Vec3::new(offset.x, 0.0, offset.z);
    let distance = horizontal.length();
    if distance < 0.001 {
        return boss;
    }
    let direction = horizontal / distance;
    let look = Vec3Row::from(direction);

    let grounded = matches!(phase, BossPhase::Ground | BossPhase::Berserk);
    let move_target = if grounded && distance > MELEE_REACH {
        Some(Vec3Row::from(
            position + direction * (distance - MELEE_REACH),
        ))
    } else {
        None
    };
    let move_target = gate_movement(ctx, boss.entity_id, move_target);

    write_pose(ctx, boss, look, move_target)
}

/// The rotation driver: first ability in the phase's priority list that is off
/// cooldown and whose target resolves.
///
/// Strict priority, as in `run_boss_rotation` — the order *is* the design of
/// the fight, and the cooldowns are what make it rotate. `rotation_cursor`
/// therefore records which entry was chosen rather than steering the scan;
/// treating it as a round-robin start would quietly reorder every phase.
fn run_rotation(
    ctx: &ReducerContext,
    boss: &GameEntity,
    config: &bevymmo_domain::placeables::BossConfig,
    living: &[PlayerRef],
    main: &PlayerRef,
    rotation: &mut BossRotationState,
) {
    let entity_id = boss.entity_id;
    if !can_start_cast(ctx, entity_id) {
        return;
    }
    let boss_position = Vec3::from(boss.position);
    let hp_fraction = health_fraction(ctx, entity_id);
    let distance = spells::flat_distance(boss_position, main.position);

    let mut skipped: Vec<String> = Vec::new();
    loop {
        let Some(entry) = pick_ability(&config.abilities, distance, hp_fraction, |id| {
            !spells::is_on_cooldown(ctx, entity_id, id.as_str())
                && !skipped.iter().any(|skipped| skipped == id.as_str())
        }) else {
            return;
        };
        let Some((target_entity, target_position)) =
            resolve_kit_target(&entry.use_when.targeting, boss_position, main, living)
        else {
            skipped.push(entry.ability_id.as_str().to_string());
            continue;
        };
        let Some(mut caster) = ctx.db.game_entity().entity_id().find(&entity_id) else {
            return;
        };
        if let Some(aim) = target_position {
            if let Some(direction) = movement::look_direction(boss_position, aim) {
                let move_target = caster.move_target;
                caster = write_pose(ctx, caster, Vec3Row::from(direction), move_target);
            }
        }
        if !spells::request_catalog_ability(
            ctx,
            &caster,
            entry.ability_id.as_str(),
            target_entity,
            target_position,
        ) {
            skipped.push(entry.ability_id.as_str().to_string());
            continue;
        }
        rotation.priority_cursor = config
            .abilities
            .iter()
            .position(|candidate| candidate.ability_id.as_str() == entry.ability_id.as_str())
            .unwrap_or(0);
        break;
    }
}

// ---------------------------------------------------------------------------
// Threat
// ---------------------------------------------------------------------------

/// Records post-armor damage as threat on `target` from `source`.
///
/// Accrues when `target` is a boss (always Table) or an enemy whose kit
/// `threat` is Table or Sticky. Nearest trash ignores this. The amount is
/// `effective * source.threat_generation` via [`threat_from_damage`]; missing
/// source stats default to `1.0`. Sticky only stores the first attacker
/// (amount 1) so Passive+Sticky still fights whoever hit them without
/// turning into a table.
pub fn accrue_threat(ctx: &ReducerContext, target: u64, source: u64, effective: f32) {
    let Some(policy) = threat_policy_for(ctx, target) else {
        return;
    };
    match policy {
        ThreatPolicy::Nearest => {}
        ThreatPolicy::Sticky => {
            if current_threat_target(ctx, target).is_some() {
                return;
            }
            ctx.db.threat().insert(Threat {
                id: 0,
                combatant_entity: target,
                target_entity: source,
                amount: 1.0,
            });
        }
        ThreatPolicy::Table => {
            let generation = spells::combat_stats(ctx, source)
                .map(|stats| stats.threat_generation)
                .unwrap_or(1.0);
            let amount = threat_from_damage(effective, generation);
            if amount <= 0.0 {
                return;
            }
            let existing = ctx
                .db
                .threat()
                .by_combatant()
                .filter(&target)
                .find(|row| row.target_entity == source);
            match existing {
                Some(row) => {
                    ctx.db.threat().id().update(Threat {
                        amount: row.amount + amount,
                        ..row
                    });
                }
                None => {
                    ctx.db.threat().insert(Threat {
                        id: 0,
                        combatant_entity: target,
                        target_entity: source,
                        amount,
                    });
                }
            }
        }
    }
}

fn threat_policy_for(ctx: &ReducerContext, target: u64) -> Option<ThreatPolicy> {
    if ctx.db.boss_state().entity_id().find(&target).is_some() {
        return Some(ThreatPolicy::Table);
    }
    config_for(ctx, target).map(|config| config.threat)
}

fn threat_amount(ctx: &ReducerContext, combatant: u64, target: u64) -> f32 {
    ctx.db
        .threat()
        .by_combatant()
        .filter(&combatant)
        .find(|row| row.target_entity == target)
        .map(|row| row.amount)
        .unwrap_or(0.0)
}

fn current_threat_target(ctx: &ReducerContext, combatant: u64) -> Option<u64> {
    let mut best: Option<(u64, f32)> = None;
    for row in ctx.db.threat().by_combatant().filter(&combatant) {
        if best.is_none_or(|(_, amount)| row.amount > amount) {
            best = Some((row.target_entity, row.amount));
        }
    }
    best.map(|(id, _)| id)
}

fn remember_sticky(ctx: &ReducerContext, combatant: u64, target: u64) {
    let existing: Vec<Threat> = ctx.db.threat().by_combatant().filter(&combatant).collect();
    let mut have_target = false;
    for row in existing {
        if row.target_entity == target {
            have_target = true;
        } else {
            ctx.db.threat().id().delete(&row.id);
        }
    }
    if !have_target {
        ctx.db.threat().insert(Threat {
            id: 0,
            combatant_entity: combatant,
            target_entity: target,
            amount: 1.0,
        });
    }
}

pub fn clear_threat(ctx: &ReducerContext, combatant: u64) {
    let ids: Vec<u64> = ctx
        .db
        .threat()
        .by_combatant()
        .filter(&combatant)
        .map(|row| row.id)
        .collect();
    for id in ids {
        ctx.db.threat().id().delete(&id);
    }
}

/// The boss's primary target: highest threat, or the nearest player when nobody
/// has any yet.
///
/// The fallback is what makes the dragon start swinging the moment the ring is
/// crossed, instead of standing there until someone hits it first.
///
/// `ThreatTable` is not loaded here even though the rule is its own: the
/// `threat` table *is* that map, already indexed by boss, so rebuilding a
/// `HashMap` from it every tick would allocate for nothing — and would pull in
/// the standard hasher's platform randomness, which this module does not have.
fn main_target<'a>(
    ctx: &ReducerContext,
    boss_entity: u64,
    living: &'a [PlayerRef],
    origin: Vec3,
) -> Option<&'a PlayerRef> {
    let candidates: Vec<ThreatCandidate> = living
        .iter()
        .map(|player| ThreatCandidate {
            entity: EntityId::new(player.entity),
            distance: horizontal_distance(origin, player.position),
        })
        .collect();
    let selected = select_target(ThreatPolicy::Table, &candidates, None, |id| {
        threat_amount(ctx, boss_entity, id.get())
    })?;
    living.iter().find(|player| player.entity == selected.get())
}

// ---------------------------------------------------------------------------
// Target selection (ported from boss/target_select.rs)
// ---------------------------------------------------------------------------

/// The farthest of `players` from `origin`.
fn farthest_target(players: &[PlayerRef], origin: Vec3) -> Option<&PlayerRef> {
    players.iter().max_by(|left, right| {
        left.position
            .distance_squared(origin)
            .total_cmp(&right.position.distance_squared(origin))
    })
}

/// The most players a cluster search will look at.
///
/// The search is `O(C(p, n))`. The arena bounds `p` in practice, but the bound
/// is content, not code, so it is stated: past this many candidates the search
/// takes the first `CLUSTER_CANDIDATE_LIMIT` and accepts a slightly worse
/// circle over a tick that grows like a binomial.
const CLUSTER_CANDIDATE_LIMIT: usize = 12;

/// The centroid of the `n` most tightly packed players.
///
/// Every combination of `n` is tried and the one with the smallest bounding
/// sphere — largest pairwise distance, halved — wins. Brute force, but `n` is
/// two in every rotation entry that exists, and the result is deterministic,
/// which matters more here than it did in Bevy: a tick is a transaction, and a
/// transaction that picks a different answer on a replay is a bug. (The Bevy
/// version iterated a `HashMap` and said so in its own doc comment.)
///
/// `None` when fewer than `n` players are alive.
fn densest_cluster(players: &[PlayerRef], n: usize) -> Option<Vec3> {
    if n == 0 || players.len() < n {
        return None;
    }
    let considered = players.len().min(CLUSTER_CANDIDATE_LIMIT);
    let indices: Vec<usize> = (0..considered).collect();
    let mut best: Option<(f32, Vec3)> = None;

    for combo in combinations(&indices, n) {
        let spread = max_pairwise_distance(&combo, players);
        if spread.is_nan() {
            continue;
        }
        let centroid = combo
            .iter()
            .map(|&i| players[i].position)
            .fold(Vec3::ZERO, |sum, position| sum + position)
            / n as f32;
        if best.is_none_or(|(best_spread, _)| spread < best_spread) {
            best = Some((spread, centroid));
        }
    }
    best.map(|(_, centroid)| centroid)
}

/// Every length-`k` combination of `indices`.
fn combinations(indices: &[usize], k: usize) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    let mut current: Vec<usize> = Vec::with_capacity(k);
    combine_recursive(indices, 0, k, &mut current, &mut out);
    out
}

fn combine_recursive(
    indices: &[usize],
    start: usize,
    k: usize,
    current: &mut Vec<usize>,
    out: &mut Vec<Vec<usize>>,
) {
    if current.len() == k {
        out.push(current.clone());
        return;
    }
    for i in start..indices.len() {
        current.push(indices[i]);
        combine_recursive(indices, i + 1, k, current, out);
        current.pop();
    }
}

fn max_pairwise_distance(combo: &[usize], players: &[PlayerRef]) -> f32 {
    let mut max_squared = 0.0_f32;
    for a in 0..combo.len() {
        for b in (a + 1)..combo.len() {
            let distance = players[combo[a]]
                .position
                .distance_squared(players[combo[b]].position);
            if distance > max_squared {
                max_squared = distance;
            }
        }
    }
    max_squared.sqrt()
}

// ---------------------------------------------------------------------------
// Spatial queries
// ---------------------------------------------------------------------------

/// A living online player by id, or `None` if they have left / died.
fn living_player_by_id(
    ctx: &ReducerContext,
    online: &std::collections::HashSet<spacetimedb::Uuid>,
    entity_id: u64,
) -> Option<PlayerRef> {
    let entity = ctx.db.game_entity().entity_id().find(&entity_id)?;
    let online_flag = entity.owner_character_id.map(|id| online.contains(&id));
    if !targets::is_online_living_player(entity.kind, entity.state, online_flag) {
        return None;
    }
    Some(PlayerRef {
        entity: entity.entity_id,
        position: Vec3::from(entity.position),
    })
}

/// Every living player within `radius` of `center`, found through the grid.
///
/// The index is `(cell_x, cell_z)`, and a multi-column btree only accepts exact
/// values for the columns before the last — so the scan is one `filter` per
/// `cell_x` column with a range over `cell_z`, rather than one call for the
/// rectangle. The cells are a conservative superset of the circle (they are
/// squares, and they ignore `y`), so each candidate still gets a real distance
/// test.
///
/// `state` is what aliveness is read from, not `entity_stats`, because
/// `sim::combat::reap_the_dead` settled the two one step ago — which saves a
/// point lookup per candidate.
fn living_players_near(
    ctx: &ReducerContext,
    online: &std::collections::HashSet<spacetimedb::Uuid>,
    center: Vec3,
    radius: f32,
) -> Vec<PlayerRef> {
    let mut found = Vec::new();
    if radius.is_nan() || radius <= 0.0 {
        return found;
    }
    let radius = radius.min(MAX_SPATIAL_QUERY_RADIUS);

    let (min_x, min_z) = grid_cell(Vec3Row::from(center - Vec3::splat(radius)));
    let (max_x, max_z) = grid_cell(Vec3Row::from(center + Vec3::splat(radius)));
    let radius_squared = radius * radius;

    for cell_x in min_x..=max_x {
        for entity in ctx.db.game_entity().cell().filter((cell_x, min_z..=max_z)) {
            let online_flag = entity.owner_character_id.map(|id| online.contains(&id));
            if !targets::is_online_living_player(entity.kind, entity.state, online_flag) {
                continue;
            }
            let position = Vec3::from(entity.position);
            let dx = position.x - center.x;
            let dz = position.z - center.z;
            if dx * dx + dz * dz > radius_squared {
                continue;
            }
            found.push(PlayerRef {
                entity: entity.entity_id,
                position,
            });
        }
    }
    found
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Drops a chosen destination when the mob is in no position to walk to it.
///
/// Two reasons it may not:
///
/// - It is rooted or stunned. `crowd_control::step` already cleared whatever it
///   had; this is what stops the AI handing it a fresh one every tick.
/// - It is mid-cast. `sim::spells::advance_casts` interrupts a cast-time spell
///   whose caster moved, so a dragon that kept walking into melee would cancel
///   its own searing breath on the tick after it started it. Opening the cast
///   also plants via `request_catalog_ability` (clears leftover chase); this
///   gate is what stops the AI handing a fresh dest every later tick.
fn gate_movement(
    ctx: &ReducerContext,
    entity_id: u64,
    move_target: Option<Vec3Row>,
) -> Option<Vec3Row> {
    move_target?;
    if crowd_control::is_movement_blocked(ctx, entity_id) {
        return None;
    }
    if ctx.db.cast_state().entity_id().find(entity_id).is_some() {
        return None;
    }
    move_target
}

/// Writes a mob's facing and destination, and hands back the current row.
///
/// Skips the write when nothing changed: a mob standing at its spawn with
/// nobody around should cost the database nothing per tick.
fn write_pose(
    ctx: &ReducerContext,
    entity: GameEntity,
    look: Vec3Row,
    move_target: Option<Vec3Row>,
) -> GameEntity {
    let changed = look != entity.look || move_target != entity.move_target;
    let entity = GameEntity {
        look,
        move_target,
        ..entity
    };
    if changed {
        ctx.db.game_entity().entity_id().update(entity)
    } else {
        entity
    }
}

/// `current_health / max_health`, clamped, and zero when there is no health to
/// speak of.
fn health_fraction(ctx: &ReducerContext, entity_id: u64) -> f32 {
    let Some(stats) = ctx.db.entity_stats().entity_id().find(entity_id) else {
        return 0.0;
    };
    if stats.stats.max_health <= 0.0 {
        return 0.0;
    }
    (stats.stats.current_health / stats.stats.max_health).clamp(0.0, 1.0)
}

/// Whether `entity_id` is free to begin a cast: not already casting, not
/// silenced, not stunned.
///
/// The "already casting" half is what Bevy expressed as `Without<CastProgress>`
/// on the rotation query. The CC half goes through `crowd_control` rather than
/// `spells::casting_blocked`, which is the same predicate spelled twice — see
/// the port report.
fn can_start_cast(ctx: &ReducerContext, entity_id: u64) -> bool {
    !crowd_control::is_casting_blocked(ctx, entity_id)
        && ctx.db.cast_state().entity_id().find(entity_id).is_none()
}

// ---------------------------------------------------------------------------
// Phase mapping
// ---------------------------------------------------------------------------

/// `BossPhaseRow` -> `BossPhase`.
///
/// The two enums are the same machine under different names: the schema calls
/// the phases by number, the domain calls them by what the dragon is doing.
/// `BossPhase::Dead` has no row spelling — a defeated boss is one whose
/// `game_entity.state` is `Dead` — so the mapping is total in this direction
/// and lossy in the other.
fn phase_from_row(phase: BossPhaseRow) -> BossPhase {
    match phase {
        BossPhaseRow::Idle => BossPhase::Dormant,
        BossPhaseRow::PhaseOne => BossPhase::Ground,
        BossPhaseRow::PhaseTwo => BossPhase::Aerial,
        BossPhaseRow::Enraged => BossPhase::Berserk,
    }
}

/// `BossPhase` -> `BossPhaseRow`.
///
/// `Dead` maps to `Enraged`, the last phase a boss can die in; it is
/// unreachable in practice, because a dead boss never reaches the phase
/// machine.
fn phase_to_row(phase: BossPhase) -> BossPhaseRow {
    match phase {
        BossPhase::Dormant => BossPhaseRow::Idle,
        BossPhase::Ground => BossPhaseRow::PhaseOne,
        BossPhase::Aerial => BossPhaseRow::PhaseTwo,
        BossPhase::Berserk | BossPhase::Dead => BossPhaseRow::Enraged,
    }
}
