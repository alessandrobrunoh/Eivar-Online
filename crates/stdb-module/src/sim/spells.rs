//! Ability runtime: casts in progress, projectiles in flight, AoE regions on
//! the ground, and the cooldowns that gate them all.
//!
//! Combat content is [`BaseAbility`]. [`SpellCastContext`] collects pending
//! effects; this module turns those into rows.
//!
//! # What changed, and why
//!
//! - **No `replicate_cast_progress`.** `cast_state` is a public table, so the
//!   client subscribes to the cast it wants to draw instead of being sent a
//!   snapshot every 100 ms.
//! - **Movement interrupts measure from the *start* of the cast**, not from the
//!   previous tick: `cast_state.start_position` is written once and never moved.
//!   Bevy compared against `last_position`, which let a caster drift forever as
//!   long as each single tick stayed under the epsilon.
//! - **No input snapshot.** Bevy also cancelled on a *new movement command*,
//!   because a click could be issued before the character had moved. Here the
//!   click writes `game_entity.move_target` and the character starts moving on
//!   the very next tick, so position alone catches it one tick later.
//! - **Crowd control cancels a running cast.** Bevy only checked CC when a cast
//!   *started*; a stun landing mid-cast left the wind-up running.
//! - **Projectiles carry the id of the spell that fired them.** The Bevy spawner
//!   was literally `spell_id: "fireball".to_string()` regardless of the caster.

use std::sync::OnceLock;

use bevymmo_domain::abilities::{
    cast_ability_preview, resolve_ability, AbilityCastMode, AbilityId, AbilityLoadout,
    AncientWordRegistry, BaseAbilityRegistry,
    ChannelMovementPolicy as AbilityChannelMovementPolicy, KitInscription,
};

use bevymmo_domain::effects::{
    ApplyStatusEffect, CleanseEffect, DamageEffect, EffectBundle, EffectContext, EffectSpec,
    HealEffect, PurgeEffect, StatusFilter, StatusId, StatusSelection,
};
use bevymmo_domain::items::definition::Item;
use bevymmo_domain::items::registry::ItemRegistry;
use bevymmo_domain::items::WeaponFamilyRegistry;
use bevymmo_domain::spells::components::MOVEMENT_INTERRUPT_EPSILON;
use bevymmo_domain::spells::context::{
    AoeShape, AoeSpawnRequest, AoeTargeting, CastKind, ProjectileSpawnRequest, SpellCastContext,
    TargetingMode,
};
use bevymmo_domain::stats::components::{CombatStats, StatsBundleData};
use bevymmo_domain::stats::events::{
    ApplyStatModifierEvent, ModifierEffect, ModifierKind, ModifierOp,
};
use bevymmo_domain::EntityId;
use glam::Vec3;
use spacetimedb::{ReducerContext, Table};

use crate::rows::{
    EffectPayloadFilterRow, EffectPayloadKindRow, EffectPayloadRow, EffectPayloadSelectionRow,
    Vec3Row,
};
use crate::sim::targets;
use crate::tables::{
    aoe_region, boss_state, cast_ended, cast_state, cooldown, enemy_ai, entity_stats, game_entity,
    grid_cell, projectile, spell_visual_effect, AoeRegion, AoeShapeRow, AoeTargetingRow,
    CastEndedEvent, CastKindRow, CastSourceRow, CastState, Cooldown, EntityStateRow, GameEntity,
    ModifierKindRow, Projectile, SpellVisualEffectEvent,
};

/// Resolves an item's ability pools, falling back to its weapon family when
/// the concrete item intentionally only defines variant-specific behavior.
pub fn ability_loadout_for_item<'a>(item: &'a dyn Item) -> Option<&'a AbilityLoadout> {
    if let Some(loadout) = item.ability_loadout() {
        return Some(loadout);
    }

    static FAMILIES: OnceLock<WeaponFamilyRegistry> = OnceLock::new();
    let families = FAMILIES.get_or_init(bevymmo_domain::content::items::default_weapon_families);
    let family_id = item.weapon_family()?;
    families.get(&family_id)?.ability_loadout.as_ref()
}

/// How long a projectile may stay in the air before it gives up, in seconds.
///
/// The Bevy version had no lifetime at all: a projectile lived until its target
/// died or despawned. That is not safe here, where "the projectile entity" is a
/// persisted row — a target that simply outruns it forever would leak a row per
/// cast. Generous enough that no spell in the registry can reach it (the fastest
/// projectile covers 240 units in this window, against a 15-unit cast range).
const PROJECTILE_MAX_LIFETIME_SECONDS: f32 = 10.0;

/// Slack added to the spatial query that builds `potential_targets`.
///
/// The cell scan is centred on the caster with a radius of
/// `cast_range + area_radius`; the margin covers projectile hit radii and the
/// half-second of movement a target can manage between the client aiming and
/// the reducer running.
pub const TARGET_QUERY_MARGIN: f32 = 4.0;

/// Slack allowed on the server-side range check, in world units.
///
/// The client aims against its own extrapolated position, so a cast issued
/// exactly at the limit arrives a few centimetres long. Rejecting those would
/// make max-range casting feel broken.
pub const CAST_RANGE_TOLERANCE: f32 = 1.0;

// ---------------------------------------------------------------------------
// Registries
// ---------------------------------------------------------------------------
//
pub fn base_abilities() -> &'static BaseAbilityRegistry {
    static REGISTRY: OnceLock<BaseAbilityRegistry> = OnceLock::new();
    REGISTRY.get_or_init(bevymmo_domain::content::abilities::default_base_abilities)
}

pub fn ancient_words() -> &'static AncientWordRegistry {
    static REGISTRY: OnceLock<AncientWordRegistry> = OnceLock::new();
    REGISTRY.get_or_init(bevymmo_domain::content::ancient_words::default_ancient_words)
}

pub fn root_words() -> &'static bevymmo_domain::abilities::RootWordRegistry {
    static REGISTRY: OnceLock<bevymmo_domain::abilities::RootWordRegistry> = OnceLock::new();
    REGISTRY.get_or_init(bevymmo_domain::content::root_words::default_root_words)
}

/// The item catalogue, needed to read the equipped weapon's abilities.
///
/// Lives here rather than in `reducers::items` because the spell path is its
/// only consumer today; move it if the inventory reducers grow one.
pub fn items() -> &'static ItemRegistry {
    static REGISTRY: OnceLock<ItemRegistry> = OnceLock::new();
    REGISTRY.get_or_init(bevymmo_domain::content::items::default_items)
}

// ---------------------------------------------------------------------------
// Queries shared with the reducers
// ---------------------------------------------------------------------------

/// The caster's combat stats, or `None` if it has no stats row.
pub fn combat_stats(ctx: &ReducerContext, entity_id: u64) -> Option<CombatStats> {
    let row = ctx.db.entity_stats().entity_id().find(&entity_id)?;
    Some(StatsBundleData::from(row.stats).combat)
}

/// Whether an entity can still be hit: it exists, is not flagged dead, and has
/// health left. Mirrors Bevy's `!vital.is_dead()` filter on target queries.
fn is_alive(ctx: &ReducerContext, entity: &GameEntity) -> bool {
    if entity.state == EntityStateRow::Dead {
        return false;
    }
    ctx.db
        .entity_stats()
        .entity_id()
        .find(&entity.entity_id)
        .is_none_or(|stats| stats.stats.current_health > 0.0)
}

/// Every living entity within `radius` of `center`, as `Spell::cast` wants them.
///
/// Bevy handed each cast *every* `GameEntity` in the world and let the spell
/// filter; that is a full table scan per cast here, so the candidates come from
/// the `cell_x`/`cell_z` index instead. The result is still a superset of what
/// any single spell will use — the spell's own radius/cone/line test runs on top
/// of it, unchanged.
pub fn potential_targets(ctx: &ReducerContext, center: Vec3, radius: f32) -> Vec<(EntityId, Vec3)> {
    let radius = radius.max(0.0);
    let (min_x, min_z) = grid_cell(Vec3Row {
        x: center.x - radius,
        y: 0.0,
        z: center.z - radius,
    });
    let (max_x, max_z) = grid_cell(Vec3Row {
        x: center.x + radius,
        y: 0.0,
        z: center.z + radius,
    });

    let mut found = Vec::new();
    let online = targets::online_character_ids(ctx);
    for cell_x in min_x..=max_x {
        // The index is `(cell_x, cell_z)`, so the scan fixes the first column
        // and ranges over the second — one syscall per column of cells.
        for entity in ctx.db.game_entity().cell().filter((cell_x, min_z..=max_z)) {
            let online_flag = entity.owner_character_id.map(|id| online.contains(&id));
            if !targets::is_valid_spell_target(entity.kind, entity.state, online_flag) {
                continue;
            }
            if !is_alive(ctx, &entity) {
                continue;
            }
            let position = Vec3::from(entity.position);
            if flat_distance(center, position) > radius {
                continue;
            }
            found.push((EntityId::new(entity.entity_id), position));
        }
    }
    found
}

/// Horizontal distance. Height is discarded everywhere in this game's maths
/// (see `AoeShape::contains`), because combat happens on a plane.
pub fn flat_distance(from: Vec3, to: Vec3) -> f32 {
    Vec3::new(to.x - from.x, 0.0, to.z - from.z).length()
}

/// Whether `point` is inside `range`, plus the aim slack the client needs.
pub fn point_in_cast_range(caster: Vec3, point: Vec3, range: f32) -> bool {
    range <= 0.0 || flat_distance(caster, point) <= range + CAST_RANGE_TOLERANCE
}

/// Rejects a missing, dead, offline, or out-of-range target when the
/// ability actually uses a selected unit.
///
/// The HUD always knows the current target and the cursor. The client used
/// to attach both to every reducer call, so a self-centered cleave failed
/// whenever a dummy selected 20 m away was still targeted — or whenever the
/// isometric camera put the cursor on distant ground. Only
/// [`TargetingMode::SingleEntity`] range-checks a unit: a homing spell must
/// not lock onto someone across the map by aiming at their feet. Ground
/// circles clamp at fire (`clamp_to_range`); cones ignore the cursor for
/// range and use it only as facing. Projectiles treat a selected unit as a
/// preference and fall back to the forward lane.
pub fn validate_cast_target(
    ctx: &ReducerContext,
    caster: &GameEntity,
    range: f32,
    targeting: TargetingMode,
    target_entity: Option<u64>,
    target_position: Option<Vec3>,
) -> Result<(), String> {
    if !targeting.range_checks_selected_entity() {
        return Ok(());
    }

    let origin = Vec3::from(caster.position);
    let online = targets::online_character_ids(ctx);
    let Some(id) = target_entity else {
        return Ok(());
    };
    let Some(target) = ctx.db.game_entity().entity_id().find(&id) else {
        return Err("that target is gone".to_string());
    };
    let online_flag = target.owner_character_id.map(|cid| online.contains(&cid));
    if !targets::is_valid_spell_target(target.kind, target.state, online_flag)
        || !is_alive(ctx, &target)
    {
        return Err("that target cannot be hit".to_string());
    }
    if !point_in_cast_range(origin, Vec3::from(target.position), range) {
        return Err("target is out of range".to_string());
    }
    let _ = target_position;
    Ok(())
}

/// Spends `cost` current mana, or refuses the cast.
pub fn spend_mana(ctx: &ReducerContext, entity_id: u64, cost: f32) -> Result<(), String> {
    if cost <= 0.0 {
        return Ok(());
    }
    let Some(row) = ctx.db.entity_stats().entity_id().find(&entity_id) else {
        return Err("caster has no stats".to_string());
    };
    let current_mana = bevymmo_domain::stats::formulas::spend_mana(row.current_mana, cost)
        .map_err(|_| "not enough mana".to_string())?;
    ctx.db
        .entity_stats()
        .entity_id()
        .update(crate::tables::EntityStats {
            current_mana,
            ..row
        });
    Ok(())
}

/// Whether a crowd control effect currently prevents this entity from casting.
///
/// The domain's `CrowdControlKind` only knows `Stun`, so it cannot classify the
/// `Silence` the row enum carries; the predicate lives here until the two enums
/// agree. `Root` and `Slow` deliberately do not block casting — they are
/// movement effects.
pub fn casting_blocked(ctx: &ReducerContext, entity_id: u64) -> bool {
    // Delegates so the two cannot drift on which effects gag a caster.
    crate::sim::crowd_control::is_casting_blocked(ctx, entity_id)
}

/// Whether `ability_id` (a spell id or a weapon ability id) is still cooling
/// down for this entity.
pub fn is_on_cooldown(ctx: &ReducerContext, entity_id: u64, ability_id: &str) -> bool {
    ctx.db
        .cooldown()
        .owner_ability()
        .filter((entity_id, ability_id))
        .any(|row| row.elapsed_seconds < row.duration_seconds)
}

/// Puts `ability_id` on cooldown, replacing any existing timer for it.
///
/// Replacing rather than adding mirrors `SpellCooldowns::start_cooldown`, which
/// inserted into a map keyed by id: a second cast can only ever refresh.
pub fn start_cooldown(ctx: &ReducerContext, entity_id: u64, ability_id: &str, duration: f32) {
    if duration <= 0.0 {
        return;
    }
    let existing: Vec<u64> = ctx
        .db
        .cooldown()
        .owner_ability()
        .filter((entity_id, ability_id))
        .map(|row| row.id)
        .collect();
    for id in existing {
        ctx.db.cooldown().id().delete(&id);
    }
    ctx.db.cooldown().insert(Cooldown {
        id: 0,
        entity_id,
        ability_id: ability_id.to_string(),
        elapsed_seconds: 0.0,
        duration_seconds: duration,
    });
}

/// Ends whatever `entity_id` is casting, telling subscribers how it ended.
pub fn end_cast(ctx: &ReducerContext, entity_id: u64, spell_id: String, interrupted: bool) {
    ctx.db.cast_state().entity_id().delete(&entity_id);
    ctx.db.cast_ended().insert(CastEndedEvent {
        entity_id,
        spell_id,
        interrupted,
    });
}

/// Fires a weapon ability by re-resolving equipment and inscriptions.
///
/// Used by `advance_casts` when a CastTime/Channeling weapon cast completes.
/// Returns the base cooldown duration, or `None` if the caster lost their
/// weapon/stats between starting and finishing the cast.
pub fn fire_weapon_ability(
    ctx: &ReducerContext,
    caster: &GameEntity,
    ability_id_str: &str,
    target_position: Option<Vec3>,
    target_entity: Option<u64>,
    source: CastSourceRow,
    charge_fraction: f32,
) -> Option<f32> {
    use crate::rows::{equipment_from_rows, known_ancient_language_from_rows};
    use crate::tables::{equipment, known_ancient_language};
    use bevymmo_domain::abilities::{
        cast_root_inscribed_slot, resolve_active_ability, resolve_root_inscribed_slot, AbilitySlot,
    };

    let combat = combat_stats(ctx, caster.entity_id)?;
    let caster_position = Vec3::from(caster.position);

    // `ctx.sender()` is the *module's* identity here: this runs from
    // `advance_casts`, inside the scheduled `game_tick` reducer, not from a
    // call the player made directly. The caster's own character — who started
    // the cast — is `caster.owner_character_id`, not the sender of whatever
    // reducer happens to be running this tick. Using `ctx.sender()` made every
    // CastTime/Channeling weapon ability resolve against a character with no
    // rows at all, so every one of them silently failed to fire.
    let character_id = caster.owner_character_id?;

    // Re-resolve the source item. A cast can finish several ticks after it
    // started, so equipment and inscriptions must still be valid at fire time.
    let equip_row = ctx.db.equipment().character_id().find(&character_id)?;
    let equipment = equipment_from_rows(&equip_row.slots);
    let ability_id = bevymmo_domain::abilities::AbilityId::new(ability_id_str.to_string());

    let (item, preview, armor_inscription) = match source {
        CastSourceRow::Weapon => {
            let weapon = equipment.weapon.as_ref()?;
            let item = items().get(&weapon.item_id)?;
            let weapon_abilities = ability_loadout_for_item(item.as_ref())?;
            let slot = [
                AbilitySlot::Primary,
                AbilitySlot::Secondary,
                AbilitySlot::Ultimate,
            ]
            .into_iter()
            .find(|&s| {
                resolve_active_ability(s, weapon_abilities, &weapon.ability_selection)
                    .map_or(false, |id| id.as_str() == ability_id.as_str())
            })?;
            let language_row = ctx
                .db
                .known_ancient_language()
                .character_id()
                .find(&character_id)?;
            let language = known_ancient_language_from_rows(
                &language_row.root_words,
                &language_row.ancient_words,
                &language_row.base_abilities,
            );
            let root_inscription = weapon.root_inscription.as_ref()?;
            let preview = resolve_root_inscribed_slot(
                slot,
                weapon_abilities,
                &weapon.ability_selection,
                root_inscription,
                &language,
                base_abilities(),
                root_words(),
                ancient_words(),
                Some(item.as_ref()),
            )
            .ok()?;
            (item, preview, None)
        }
        armor_source @ (CastSourceRow::Helmet | CastSourceRow::Armor | CastSourceRow::Shoes) => {
            use bevymmo_domain::items::EquipSlot;
            let slot = match armor_source {
                CastSourceRow::Helmet => EquipSlot::Helmet,
                CastSourceRow::Armor => EquipSlot::Armor,
                CastSourceRow::Shoes => EquipSlot::Shoes,
                _ => unreachable!(),
            };
            let armor = equipment.get(slot).as_ref()?;
            let item = items().get(&armor.item_id)?;
            ability_loadout_for_item(item.as_ref())?
                .primary
                .iter()
                .find(|id| id.as_str() == ability_id.as_str())?;
            let language_row = ctx
                .db
                .known_ancient_language()
                .character_id()
                .find(&character_id)?;
            let language = known_ancient_language_from_rows(
                &language_row.root_words,
                &language_row.ancient_words,
                &language_row.base_abilities,
            );
            let preview = bevymmo_domain::abilities::resolve_armor_inscribed_ability(
                &ability_id,
                armor.armor_inscription.as_ref(),
                &language,
                base_abilities(),
                root_words(),
                ancient_words(),
                Some(item.as_ref()),
            )
            .ok()?;
            (item, preview, armor.armor_inscription.clone())
        }
        CastSourceRow::Spell | CastSourceRow::Catalog => return None,
    };

    let targets = potential_targets(
        ctx,
        caster_position,
        preview.params.range + preview.params.area + TARGET_QUERY_MARGIN,
    );

    let mut cast_ctx = SpellCastContext::new(
        EntityId::new(caster.entity_id),
        caster_position,
        &combat,
        Vec3::from(caster.look),
        target_position,
        target_entity.map(EntityId::new),
        &targets,
    );

    match source {
        CastSourceRow::Weapon => {
            let equip_row = ctx.db.equipment().character_id().find(&character_id)?;
            let equipment = equipment_from_rows(&equip_row.slots);
            let weapon = equipment.weapon.as_ref()?;
            let weapon_abilities = ability_loadout_for_item(item.as_ref())?;
            let slot = [
                AbilitySlot::Primary,
                AbilitySlot::Secondary,
                AbilitySlot::Ultimate,
            ]
            .into_iter()
            .find(|&s| {
                resolve_active_ability(s, weapon_abilities, &weapon.ability_selection)
                    .map_or(false, |id| id.as_str() == ability_id.as_str())
            })?;
            let language_row = ctx
                .db
                .known_ancient_language()
                .character_id()
                .find(&character_id)?;
            let language = known_ancient_language_from_rows(
                &language_row.root_words,
                &language_row.ancient_words,
                &language_row.base_abilities,
            );
            let root_inscription = weapon.root_inscription.as_ref()?;
            cast_root_inscribed_slot(
                slot,
                weapon_abilities,
                &weapon.ability_selection,
                root_inscription,
                &language,
                base_abilities(),
                root_words(),
                ancient_words(),
                &mut cast_ctx,
                Some(item.as_ref()),
            )
            .ok()?;
        }
        CastSourceRow::Helmet | CastSourceRow::Armor | CastSourceRow::Shoes => {
            let inscription = armor_inscription.as_ref();
            let language_row = ctx
                .db
                .known_ancient_language()
                .character_id()
                .find(&character_id)?;
            let language = known_ancient_language_from_rows(
                &language_row.root_words,
                &language_row.ancient_words,
                &language_row.base_abilities,
            );
            bevymmo_domain::abilities::cast_armor_inscribed_ability(
                &ability_id,
                inscription,
                &language,
                base_abilities(),
                root_words(),
                ancient_words(),
                &mut cast_ctx,
                Some(item.as_ref()),
            )
            .ok()?;
        }
        CastSourceRow::Spell | CastSourceRow::Catalog => return None,
    }

    cast_ctx.scale_outgoing_potency(charge_fraction);

    apply_pending(
        ctx,
        caster.entity_id,
        caster_position,
        ability_id_str,
        &mut cast_ctx,
    );
    Some(preview.ability.base_params().cooldown)
}

fn catalog_inscription(
    ctx: &ReducerContext,
    entity_id: u64,
    ability_id: &str,
) -> Option<KitInscription> {
    if let Some(row) = ctx.db.enemy_ai().entity_id().find(&entity_id) {
        if let Some(config) = crate::world::enemy_config_for(&row.kind_id) {
            if let Some(entry) = config
                .abilities
                .into_iter()
                .find(|entry| entry.ability_id.as_str() == ability_id)
            {
                return Some(entry.inscription);
            }
        }
    }
    if ctx.db.boss_state().entity_id().find(&entity_id).is_some() {
        if let Some(config) = crate::world::boss_config_for("boss_dragon") {
            if let Some(entry) = config
                .abilities
                .into_iter()
                .find(|entry| entry.ability_id.as_str() == ability_id)
            {
                return Some(entry.inscription);
            }
        }
    }
    None
}

/// Fires a catalog `BaseAbility` without equipment or known glyphs.
///
/// Used by AI (enemy kits) and by `advance_casts` when a
/// [`CastSourceRow::Catalog`] wind-up completes. Returns the cooldown, or
/// `None` if the ability is not registered or the caster has no stats.
/// Inscription comes from the caster's `enemy_ai.kind_id` kit, so a CastTime
/// that started ticks ago still resolves the same Flame Cleave.
pub fn fire_catalog_ability(
    ctx: &ReducerContext,
    caster: &GameEntity,
    ability_id_str: &str,
    target_position: Option<Vec3>,
    target_entity: Option<u64>,
) -> Option<f32> {
    let ability_id = AbilityId::new(ability_id_str.to_string());
    let inscription = catalog_inscription(ctx, caster.entity_id, ability_id_str);
    let preview = match resolve_ability(
        &ability_id,
        inscription.as_ref(),
        base_abilities(),
        root_words(),
        ancient_words(),
    ) {
        Ok(preview) => preview,
        Err(reason) => {
            log::warn!("catalog ability {ability_id_str} failed to resolve: {reason:?}");
            return None;
        }
    };

    let combat = combat_stats(ctx, caster.entity_id)?;
    let caster_position = Vec3::from(caster.position);
    let targets = potential_targets(
        ctx,
        caster_position,
        preview.params.range + preview.params.area + TARGET_QUERY_MARGIN,
    );
    let mut cast_ctx = SpellCastContext::new(
        EntityId::new(caster.entity_id),
        caster_position,
        &combat,
        Vec3::from(caster.look),
        target_position,
        target_entity.map(EntityId::new),
        &targets,
    );
    cast_ability_preview(&preview, &mut cast_ctx);
    apply_pending(
        ctx,
        caster.entity_id,
        caster_position,
        ability_id_str,
        &mut cast_ctx,
    );
    Some(preview.params.cooldown)
}

/// Starts a catalog ability for an AI caster: Instant fires now, CastTime /
/// Channeling open a [`CastSourceRow::Catalog`] row.
///
/// Caller must have checked [`can_start_cast`]-equivalent (no existing
/// `cast_state`, not silenced). Returns whether a cast started or fired.
///
/// CastTime and Channeling hold still ([`AbilityCastMode::holds_still`]): leftover
/// chase is dropped so the next `sim::movement::step` does not walk the caster
/// and have `advance_casts` cancel the wind-up. Instant keeps walking.
pub fn request_catalog_ability(
    ctx: &ReducerContext,
    caster: &GameEntity,
    ability_id_str: &str,
    target_entity: Option<u64>,
    target_position: Option<Vec3>,
) -> bool {
    let ability_id = AbilityId::new(ability_id_str.to_string());
    let inscription = catalog_inscription(ctx, caster.entity_id, ability_id_str);
    let Ok(preview) = resolve_ability(
        &ability_id,
        inscription.as_ref(),
        base_abilities(),
        root_words(),
        ancient_words(),
    ) else {
        log::warn!("ai: no catalog ability registered for {ability_id_str}");
        return false;
    };

    if spend_mana(ctx, caster.entity_id, preview.params.mana_cost).is_err() {
        return false;
    }

    match preview.ability.cast_mode() {
        AbilityCastMode::Instant => request_catalog_ability_instant(
            ctx,
            caster,
            ability_id_str,
            target_entity,
            target_position,
        ),
        AbilityCastMode::CastTime => {
            let required_seconds = preview.params.cast_time;
            if required_seconds <= 0.0 {
                return request_catalog_ability_instant(
                    ctx,
                    caster,
                    ability_id_str,
                    target_entity,
                    target_position,
                );
            }
            plant_caster(ctx, caster.entity_id);
            ctx.db.cast_state().insert(CastState {
                entity_id: caster.entity_id,
                spell_id: ability_id_str.to_string(),
                kind: CastKindRow::CastTime,
                source: CastSourceRow::Catalog,
                elapsed_seconds: 0.0,
                required_seconds,
                start_position: caster.position,
                target_position: target_position.map(Vec3Row::from),
                target_entity,
                channel_tick_accumulator: 0.0,
                tick_interval_seconds: 0.0,
                channel_movement_interrupts: true,
            });
            true
        }
        AbilityCastMode::Channeling {
            tick_interval_seconds,
            movement_policy,
        } => {
            let required_seconds = preview.params.cast_time.max(0.1);
            start_cooldown(
                ctx,
                caster.entity_id,
                ability_id_str,
                preview.params.cooldown,
            );
            plant_caster(ctx, caster.entity_id);
            ctx.db.cast_state().insert(CastState {
                entity_id: caster.entity_id,
                spell_id: ability_id_str.to_string(),
                kind: CastKindRow::Channeling,
                source: CastSourceRow::Catalog,
                elapsed_seconds: 0.0,
                required_seconds,
                start_position: caster.position,
                target_position: target_position.map(Vec3Row::from),
                target_entity,
                channel_tick_accumulator: tick_interval_seconds,
                tick_interval_seconds,
                channel_movement_interrupts: matches!(
                    movement_policy,
                    AbilityChannelMovementPolicy::InterruptOnMove
                ),
            });
            true
        }
    }
}

/// Drops leftover dest so a CastTime / Channeling wind-up is not cancelled
/// by the next movement step. A later `move_to` is still allowed and
/// interrupts in `advance_casts`.
///
/// Shared by player weapon/armor reducers and AI catalog casts.
pub fn stop_movement(ctx: &ReducerContext, caster: GameEntity) -> GameEntity {
    if caster.move_target.is_none() && caster.state != EntityStateRow::Moving {
        return caster;
    }
    ctx.db.game_entity().entity_id().update(GameEntity {
        move_target: None,
        state: EntityStateRow::Idle,
        ..caster
    })
}

fn plant_caster(ctx: &ReducerContext, entity_id: u64) {
    let Some(caster) = ctx.db.game_entity().entity_id().find(&entity_id) else {
        return;
    };
    let _ = stop_movement(ctx, caster);
}

fn request_catalog_ability_instant(
    ctx: &ReducerContext,
    caster: &GameEntity,
    ability_id_str: &str,
    target_entity: Option<u64>,
    target_position: Option<Vec3>,
) -> bool {
    if let Some(cooldown) =
        fire_catalog_ability(ctx, caster, ability_id_str, target_position, target_entity)
    {
        start_cooldown(ctx, caster.entity_id, ability_id_str, cooldown);
        true
    } else {
        false
    }
}

/// Drains every `pending_*` list on the context into the database.
///
/// Bevy's `apply_spell_effects`, with message writers replaced by table writes.
/// `spell_id` is the id of whatever produced the effects — a spell for the
/// classic path, a weapon ability for `cast_weapon` — and is what a spawned
/// projectile or region is labelled with.
pub fn apply_pending(
    ctx: &ReducerContext,
    caster: u64,
    caster_position: Vec3,
    spell_id: &str,
    cast: &mut SpellCastContext,
) {
    for bundle in cast.pending_effects.drain(..) {
        crate::sim::effects::resolve_bundle(ctx, 0, bundle);
    }

    for request in cast.pending_projectiles.drain(..) {
        spawn_projectile(ctx, caster, caster_position, spell_id, request);
    }
    for request in cast.pending_aoes.drain(..) {
        spawn_aoe_region(ctx, caster, request);
    }
    for event in cast.pending_modifiers.drain(..) {
        apply_modifier_event(ctx, &event);
    }
    for visual in cast.pending_visuals.drain(..) {
        ctx.db.spell_visual_effect().insert(SpellVisualEffectEvent {
            spell_id: visual.spell_id,
            start: visual.start.into(),
            end: visual.end.into(),
        });
    }
}

/// One homing projectile, as a row.
fn spawn_projectile(
    ctx: &ReducerContext,
    caster: u64,
    start: Vec3,
    spell_id: &str,
    request: ProjectileSpawnRequest,
) {
    ctx.db.projectile().insert(Projectile {
        id: 0,
        caster,
        spell_id: spell_id.to_string(),
        position: start.into(),
        target_entity: Some(request.target.get()),
        target_position: None,
        speed: request.speed,
        effects: request.effects.iter().map(EffectPayloadRow::from).collect(),
        hit_radius: request.hit_radius,
        remaining_seconds: PROJECTILE_MAX_LIFETIME_SECONDS,
    });
}

/// Translates one `ApplyStatModifierEvent` into `stat_modifier` rows.
///
/// Split by effect kind because the two land in different tables: a stat change
/// goes to `stat_modifier` and is folded into the effective stats, while a
/// periodic heal or damage goes to `periodic_effect` and changes health on a
/// schedule instead. `ModifierOp::Override` still has nowhere to go —
/// `stat_modifier` carries a bool, not an operation — and is dropped with a
/// warning rather than approximated.
fn apply_modifier_event(ctx: &ReducerContext, event: &ApplyStatModifierEvent) {
    for effect in &event.effects {
        match effect {
            ModifierEffect::Stat {
                field,
                operation,
                value,
            } => {
                let is_multiplicative = match operation {
                    ModifierOp::Add => false,
                    ModifierOp::Multiply => true,
                    ModifierOp::Override => {
                        log::warn!(
                            "dropping an Override modifier on {}: `stat_modifier` has no column \
                             for it",
                            event.target
                        );
                        continue;
                    }
                };
                crate::sim::combat::apply_modifier(
                    ctx,
                    event.target.get(),
                    event.source.map(|s| s.get()),
                    &format!("{field:?}"),
                    *value,
                    is_multiplicative,
                    modifier_row_kind(event.kind),
                    event.duration_seconds,
                );
            }
            ModifierEffect::HealOverTime {
                amount_per_tick,
                tick_interval,
            } => crate::sim::combat::apply_periodic(
                ctx,
                event.target.get(),
                event.source.map(|s| s.get()),
                *amount_per_tick,
                *tick_interval,
                // A periodic effect with no duration would never stop. The
                // domain allows it; the table would keep ticking forever, so it
                // is treated as a no-op rather than a leak.
                event.duration_seconds.unwrap_or(0.0),
            ),
            ModifierEffect::DamageOverTime {
                amount_per_tick,
                tick_interval,
            } => crate::sim::combat::apply_periodic(
                ctx,
                event.target.get(),
                event.source.map(|s| s.get()),
                // Negative heals: `apply_periodic` takes one signed number.
                -amount_per_tick.abs(),
                *tick_interval,
                event.duration_seconds.unwrap_or(0.0),
            ),
        }
    }
}

/// Carries the caster's own buff/debuff label into the row.
///
/// Inferring it from the sign would get `-0.3 Armor` right and a reduced
/// incoming-damage modifier wrong, so the declared value is the one stored.
fn modifier_row_kind(kind: ModifierKind) -> ModifierKindRow {
    match kind {
        ModifierKind::Buff => ModifierKindRow::Buff,
        ModifierKind::Debuff => ModifierKindRow::Debuff,
    }
}

// ---------------------------------------------------------------------------
// AoE regions
// ---------------------------------------------------------------------------

/// Spawns a requested AoE region, or applies it on the spot.
///
/// Circles and cones with a lifetime persist so their `pending_delay_seconds`
/// telegraph can play out. Zero-duration requests (melee swings) still resolve
/// immediately — they would be applied and despawned on the next tick anyway.
fn spawn_aoe_region(ctx: &ReducerContext, caster: u64, request: AoeSpawnRequest) {
    let Some(row) = persistable_region(caster, &request) else {
        apply_aoe_now(ctx, caster, &request);
        return;
    };
    ctx.db.aoe_region().insert(row);
}

/// The row for `request`, or `None` when it should resolve immediately.
fn persistable_region(caster: u64, request: &AoeSpawnRequest) -> Option<AoeRegion> {
    // A region with no lifetime would be applied and despawned on the tick
    // after it spawned, so resolving it at cast time is the same thing one tick
    // earlier — and saves a row round-trip for every melee swing.
    if request.duration_seconds <= 0.0 {
        return None;
    }
    if request.effects.is_empty() {
        // No effects to persist — take the immediate path.
        return None;
    }
    let (shape, direction, angle_deg) = match request.shape {
        AoeShape::Circle => (AoeShapeRow::Circle, Vec3Row::default(), 0.0),
        AoeShape::Cone {
            direction,
            angle_deg,
        } => (AoeShapeRow::Cone, direction.into(), angle_deg),
    };
    // `affected` seeds the caster for ExcludeCaster so the row alone enforces
    // the policy without a separate targeting column read at tick time.
    let affected = match request.targeting {
        AoeTargeting::Everyone => Vec::new(),
        AoeTargeting::ExcludeCaster => vec![caster],
        AoeTargeting::CasterOnly => vec![caster],
    };

    Some(AoeRegion {
        id: 0,
        caster,
        spell_id: request.spell_id.clone(),
        center: request.center.into(),
        direction,
        radius: request.radius,
        shape,
        angle_deg,
        remaining_seconds: request.duration_seconds,
        pending_delay_seconds: request.initial_delay_seconds.max(0.0),
        affected,
        targeting: targeting_row(request.targeting),
        effects: request.effects.iter().map(EffectPayloadRow::from).collect(),
    })
}

/// Reconstructs the domain shape stored on a region row.
fn region_shape(region: &AoeRegion) -> AoeShape {
    match region.shape {
        AoeShapeRow::Circle => AoeShape::Circle,
        AoeShapeRow::Cone => AoeShape::Cone {
            direction: Vec3::from(region.direction),
            angle_deg: region.angle_deg,
        },
    }
}

/// `true` once the telegraph has elapsed and the region may apply its payload.
fn region_applies_this_tick(pending_delay_seconds: f32) -> bool {
    pending_delay_seconds <= 0.0
}

/// Applies an AoE request immediately to everything currently inside it.
fn apply_aoe_now(ctx: &ReducerContext, caster: u64, request: &AoeSpawnRequest) {
    let targeting = request.targeting;
    let caster_id = EntityId::new(caster);
    let inside: Vec<EntityId> = potential_targets(ctx, request.center, request.radius)
        .into_iter()
        .filter(|(target, position)| {
            targeting.allows(caster_id, *target)
                && request
                    .shape
                    .contains(request.center, request.radius, *position)
        })
        .map(|(target, _)| target)
        .collect();

    for target in inside {
        let payloads: Vec<_> = request.effects.iter().map(EffectPayloadRow::from).collect();
        if !payloads.is_empty() {
            resolve_payloads(ctx, &payloads, target.get(), Some(caster));
        }
    }
}

// ---------------------------------------------------------------------------
// The tick
// ---------------------------------------------------------------------------

/// One simulation step of the spell system.
///
/// Same order as the Bevy `.chain()`: casts advance (and may fire), then what
/// they spawned moves, then cooldowns tick. Anything the fire produced this tick
/// therefore waits for the next one before it acts, exactly as before.
pub fn step(ctx: &ReducerContext, dt: f32) {
    advance_casts(ctx, dt);
    update_projectiles(ctx, dt);
    update_aoe_regions(ctx, dt);
    tick_cooldowns(ctx, dt);
}

/// A cast that will not survive this tick.
struct EndedCast {
    entity_id: u64,
    spell_id: String,
    interrupted: bool,
}

/// Bevy's `advance_cast_progress`: ticks every wind-up and channel, fires the
/// ones that came due, and cancels the ones that were interrupted.
///
/// Handles weapon, armor, and catalog [`CastSourceRow`] casts.
fn advance_casts(ctx: &ReducerContext, dt: f32) {
    // Collected up front because firing writes to `entity_stats`, `projectile`,
    // `aoe_region` and `cooldown`, and a tick is one transaction: iterating a
    // table while the same transaction writes it is not something to rely on.
    let casts: Vec<CastState> = ctx.db.cast_state().iter().collect();
    let mut ended: Vec<EndedCast> = Vec::new();

    for cast in casts {
        let Some(caster) = ctx.db.game_entity().entity_id().find(&cast.entity_id) else {
            // The caster was removed between starting the cast and this tick.
            ended.push(EndedCast {
                entity_id: cast.entity_id,
                spell_id: cast.spell_id,
                interrupted: true,
            });
            continue;
        };

        if !is_alive(ctx, &caster) || casting_blocked(ctx, caster.entity_id) {
            ended.push(EndedCast {
                entity_id: cast.entity_id,
                spell_id: cast.spell_id,
                interrupted: true,
            });
            continue;
        }

        // --- Movement interrupt check (source-agnostic) ---
        // CastTime always interrupts on movement.
        // Channeling respects the stored channel_movement_interrupts policy,
        // which was captured from SpellConfig (legacy) or AbilityCastMode (weapon)
        // at cast start time.
        let movement_cancels = match cast.kind {
            CastKindRow::CastTime => true,
            CastKindRow::Channeling => cast.channel_movement_interrupts,
            CastKindRow::Instant => false,
        };
        let moved = flat_distance(Vec3::from(caster.position), Vec3::from(cast.start_position))
            > MOVEMENT_INTERRUPT_EPSILON;
        if movement_cancels && moved {
            ended.push(EndedCast {
                entity_id: cast.entity_id,
                spell_id: cast.spell_id,
                interrupted: true,
            });
            continue;
        }

        let target_position = cast.target_position.map(Vec3::from);
        let elapsed_seconds = cast.elapsed_seconds + dt;
        let mut channel_tick_accumulator = cast.channel_tick_accumulator;
        let mut weapon_cast_failed = false; // Tracks resolution failure for CastTime

        let finished = match (cast.source, cast.kind) {
            (CastSourceRow::Spell, _) => {
                log::warn!(
                    "legacy Spell cast {:?} for entity {} — catalog is gone; interrupting",
                    cast.spell_id,
                    cast.entity_id
                );
                weapon_cast_failed = true;
                true
            }

            // --- weapon ability paths ---
            (
                source @ (CastSourceRow::Weapon
                | CastSourceRow::Helmet
                | CastSourceRow::Armor
                | CastSourceRow::Shoes),
                CastKindRow::CastTime,
            ) => {
                let due = elapsed_seconds >= cast.required_seconds;
                if due {
                    // Resolution may fail if equipment/selection changed during wind-up.
                    // Treat as interrupted: no effect, no cooldown (client shows cancelled bar).
                    match fire_weapon_ability(
                        ctx,
                        &caster,
                        &cast.spell_id,
                        target_position,
                        cast.target_entity,
                        source,
                        1.0,
                    ) {
                        Some(cd) => {
                            start_cooldown(ctx, caster.entity_id, &cast.spell_id, cd);
                            true // Completed successfully
                        }
                        None => {
                            // Equipment changed or weapon removed during cast.
                            log::info!(
                                "weapon cast {:?} for entity {} failed at completion; interrupting",
                                cast.spell_id,
                                cast.entity_id
                            );
                            weapon_cast_failed = true;
                            true // End the cast (will be marked as interrupted below)
                        }
                    }
                } else {
                    false
                }
            }
            (
                source @ (CastSourceRow::Weapon
                | CastSourceRow::Helmet
                | CastSourceRow::Armor
                | CastSourceRow::Shoes),
                CastKindRow::Channeling,
            ) => {
                channel_tick_accumulator += dt;
                let interval = if cast.tick_interval_seconds > 0.0 {
                    cast.tick_interval_seconds
                } else {
                    dt.max(f32::EPSILON)
                };
                while channel_tick_accumulator >= interval {
                    channel_tick_accumulator -= interval;
                    // Re-fire each tick (same as legacy channeling).
                    // Tick failures are logged but don't interrupt the channel:
                    // the player may have moved out of range or the target died,
                    // but the channel itself is still valid.
                    if fire_weapon_ability(
                        ctx,
                        &caster,
                        &cast.spell_id,
                        target_position,
                        cast.target_entity,
                        source,
                        1.0,
                    )
                    .is_none()
                    {
                        log::debug!(
                            "weapon channel tick {:?} for entity {} failed to resolve",
                            cast.spell_id,
                            cast.entity_id
                        );
                    }
                }
                cast.required_seconds > 0.0 && elapsed_seconds >= cast.required_seconds
            }

            (CastSourceRow::Catalog, CastKindRow::CastTime) => {
                let due = elapsed_seconds >= cast.required_seconds;
                if due {
                    match fire_catalog_ability(
                        ctx,
                        &caster,
                        &cast.spell_id,
                        target_position,
                        cast.target_entity,
                    ) {
                        Some(cd) => {
                            start_cooldown(ctx, caster.entity_id, &cast.spell_id, cd);
                            true
                        }
                        None => {
                            log::info!(
                                "catalog cast {:?} for entity {} failed at completion; interrupting",
                                cast.spell_id,
                                cast.entity_id
                            );
                            weapon_cast_failed = true;
                            true
                        }
                    }
                } else {
                    false
                }
            }
            (CastSourceRow::Catalog, CastKindRow::Channeling) => {
                channel_tick_accumulator += dt;
                let interval = if cast.tick_interval_seconds > 0.0 {
                    cast.tick_interval_seconds
                } else {
                    dt.max(f32::EPSILON)
                };
                while channel_tick_accumulator >= interval {
                    channel_tick_accumulator -= interval;
                    if fire_catalog_ability(
                        ctx,
                        &caster,
                        &cast.spell_id,
                        target_position,
                        cast.target_entity,
                    )
                    .is_none()
                    {
                        log::debug!(
                            "catalog channel tick {:?} for entity {} failed to resolve",
                            cast.spell_id,
                            cast.entity_id
                        );
                    }
                }
                cast.required_seconds > 0.0 && elapsed_seconds >= cast.required_seconds
            }

            // Defensive: an instant spell/ability never opens a `cast_state`.
            (_, CastKindRow::Instant) => true,
        };

        if finished {
            // Determine if this was a true completion or an interruption.
            // Instant casts are never interruptions. weapon CastTime that failed
            // resolution is interrupted. Everything else depends on kind.
            let interrupted = matches!(cast.kind, CastKindRow::Instant) || weapon_cast_failed;

            ended.push(EndedCast {
                entity_id: cast.entity_id,
                spell_id: cast.spell_id,
                interrupted,
            });
        } else {
            ctx.db.cast_state().entity_id().update(CastState {
                elapsed_seconds,
                channel_tick_accumulator,
                ..cast
            });
        }
    }

    for cast in ended {
        end_cast(ctx, cast.entity_id, cast.spell_id, cast.interrupted);
    }
}

fn resolve_payloads(
    ctx: &ReducerContext,
    payloads: &[EffectPayloadRow],
    target: u64,
    source: Option<u64>,
) {
    let effects = payloads
        .iter()
        .filter_map(|payload| match payload.kind {
            EffectPayloadKindRow::Damage => Some(EffectSpec::Damage(DamageEffect {
                amount: payload.amount,
            })),
            EffectPayloadKindRow::Heal => Some(EffectSpec::Heal(HealEffect {
                amount: payload.amount,
            })),
            EffectPayloadKindRow::ApplyStatus => {
                let status_id = payload.status_id.as_deref()?.to_string();
                Some(EffectSpec::ApplyStatus(ApplyStatusEffect {
                    status_id: StatusId::new(status_id),
                    duration_override_seconds: payload.duration_override_seconds,
                    potency: payload.potency,
                }))
            }
            EffectPayloadKindRow::Cleanse => Some(EffectSpec::Cleanse(CleanseEffect {
                filter: payload
                    .status_filter
                    .map(status_filter)
                    .unwrap_or(StatusFilter::All),
                max_statuses: payload.max_statuses,
                selection: payload
                    .selection
                    .map(status_selection)
                    .unwrap_or(StatusSelection::Oldest),
            })),
            EffectPayloadKindRow::Purge => Some(EffectSpec::Purge(PurgeEffect {
                filter: payload
                    .status_filter
                    .map(status_filter)
                    .unwrap_or(StatusFilter::All),
                max_statuses: payload.max_statuses,
                selection: payload
                    .selection
                    .map(status_selection)
                    .unwrap_or(StatusSelection::Oldest),
            })),
        })
        .collect();

    let mut context = EffectContext::new(EntityId::new(target));
    context.source = source.map(EntityId::new);
    crate::sim::effects::resolve_bundle(ctx, 0, EffectBundle::new(context, effects));
}

fn status_filter(filter: EffectPayloadFilterRow) -> StatusFilter {
    match filter {
        EffectPayloadFilterRow::Buffs => StatusFilter::Buffs,
        EffectPayloadFilterRow::Debuffs => StatusFilter::Debuffs,
        EffectPayloadFilterRow::All => StatusFilter::All,
    }
}

fn status_selection(selection: EffectPayloadSelectionRow) -> StatusSelection {
    match selection {
        EffectPayloadSelectionRow::Oldest => StatusSelection::Oldest,
        EffectPayloadSelectionRow::Newest => StatusSelection::Newest,
        EffectPayloadSelectionRow::ShortestRemaining => StatusSelection::ShortestRemaining,
    }
}

/// Bevy's `update_homing_projectiles`, plus the fixed-point case the row schema
/// allows (`target_position` without `target_entity`), which nothing emits yet.
fn update_projectiles(ctx: &ReducerContext, dt: f32) {
    let projectiles: Vec<Projectile> = ctx.db.projectile().iter().collect();

    for mut proj in projectiles {
        proj.remaining_seconds -= dt;
        if proj.remaining_seconds <= 0.0 {
            ctx.db.projectile().id().delete(&proj.id);
            continue;
        }

        let position = Vec3::from(proj.position);
        let destination = match proj.target_entity {
            Some(target) => {
                // A target that died or vanished takes the projectile with it,
                // as it did under Bevy: a homing shot has nothing left to home.
                let Some(entity) = ctx.db.game_entity().entity_id().find(&target) else {
                    ctx.db.projectile().id().delete(&proj.id);
                    continue;
                };
                if !is_alive(ctx, &entity) {
                    ctx.db.projectile().id().delete(&proj.id);
                    continue;
                }
                Vec3::from(entity.position)
            }
            None => match proj.target_position {
                Some(point) => Vec3::from(point),
                None => {
                    log::warn!("projectile {} has no target; removing", proj.id);
                    ctx.db.projectile().id().delete(&proj.id);
                    continue;
                }
            },
        };

        let offset = destination - position;
        let distance = offset.length();
        if distance <= proj.hit_radius {
            match proj.target_entity {
                Some(target) => {
                    resolve_payloads(ctx, &proj.effects, target, Some(proj.caster));
                }
                // A ground-targeted shot has no single victim, so it hits
                // whatever is standing on the impact point — the caster
                // excluded, as for every other area effect in the game.
                None => {
                    for (target, _) in potential_targets(ctx, destination, proj.hit_radius) {
                        if target.get() != proj.caster {
                            resolve_payloads(ctx, &proj.effects, target.get(), Some(proj.caster));
                        }
                    }
                }
            }
            ctx.db.projectile().id().delete(&proj.id);
            continue;
        }

        let step = (proj.speed * dt).min(distance);
        proj.position = (position + offset / distance * step).into();
        ctx.db.projectile().id().update(proj);
    }
}

/// Bevy's `update_aoe_regions`: tick the wind-up, tick the lifetime, apply to
/// whoever is inside and has not been hit yet, despawn on expiry.
///
/// Generic with respect to the spell, exactly as the original: it reads the
/// payload off the row and never dispatches on `spell_id`.
fn update_aoe_regions(ctx: &ReducerContext, dt: f32) {
    let regions: Vec<AoeRegion> = ctx.db.aoe_region().iter().collect();

    for mut region in regions {
        if region.pending_delay_seconds > 0.0 {
            region.pending_delay_seconds = (region.pending_delay_seconds - dt).max(0.0);
        }
        region.remaining_seconds -= dt;

        // The order matters and is the original's: the delay is ticked first,
        // so a region whose lifetime equals its delay (Meteorite) still gets its
        // one impact on the tick it expires.
        if region_applies_this_tick(region.pending_delay_seconds) {
            let shape = region_shape(&region);
            let center = Vec3::from(region.center);
            let targeting = targeting_from_row(region.targeting);
            let caster_id = EntityId::new(region.caster);
            for (target, position) in potential_targets(ctx, center, region.radius) {
                if region.affected.contains(&target.get()) {
                    continue;
                }
                if !shape.contains(center, region.radius, position) {
                    continue;
                }
                if !targeting.allows(caster_id, target) {
                    continue;
                }
                resolve_payloads(ctx, &region.effects, target.get(), Some(region.caster));
                region.affected.push(target.get());
            }
        }

        if region.remaining_seconds <= 0.0 {
            ctx.db.aoe_region().id().delete(&region.id);
        } else {
            ctx.db.aoe_region().id().update(region);
        }
    }
}

/// Bevy's `tick_spell_cooldowns` and `tick_ability_cooldowns` in one pass —
/// spells and weapon abilities share the `cooldown` table, so they share the
/// tick as well.
///
/// Finished timers are deleted rather than kept at full elapsed, which is what
/// `cleanup_finished` did to the map every tick. The clamp is the one from
/// `spells::components::Cooldown::tick`, restated here because that type hides
/// its fields and cannot be rebuilt from a stored `elapsed`.
fn tick_cooldowns(ctx: &ReducerContext, dt: f32) {
    let cooldowns: Vec<Cooldown> = ctx.db.cooldown().iter().collect();
    for row in cooldowns {
        let elapsed_seconds = (row.elapsed_seconds + dt).min(row.duration_seconds);
        if elapsed_seconds >= row.duration_seconds {
            ctx.db.cooldown().id().delete(&row.id);
        } else {
            ctx.db.cooldown().id().update(Cooldown {
                elapsed_seconds,
                ..row
            });
        }
    }
}

/// The row spelling of a domain [`CastKind`], for the reducers that open a cast.
pub fn cast_kind_row(kind: CastKind) -> CastKindRow {
    match kind {
        CastKind::Instant => CastKindRow::Instant,
        CastKind::CastTime => CastKindRow::CastTime,
        CastKind::Channeling => CastKindRow::Channeling,
    }
}

/// Domain [`AoeTargeting`] → row [`AoeTargetingRow`].
fn targeting_row(targeting: AoeTargeting) -> AoeTargetingRow {
    match targeting {
        AoeTargeting::Everyone => AoeTargetingRow::Everyone,
        AoeTargeting::CasterOnly => AoeTargetingRow::CasterOnly,
        AoeTargeting::ExcludeCaster => AoeTargetingRow::ExcludeCaster,
    }
}

/// Row [`AoeTargetingRow`] → domain [`AoeTargeting`].
fn targeting_from_row(row: AoeTargetingRow) -> AoeTargeting {
    match row {
        AoeTargetingRow::Everyone => AoeTargeting::Everyone,
        AoeTargetingRow::CasterOnly => AoeTargeting::CasterOnly,
        AoeTargetingRow::ExcludeCaster => AoeTargeting::ExcludeCaster,
    }
}

#[cfg(test)]
mod persistable_region_tests {
    use super::*;

    fn damage_request(shape: AoeShape, duration: f32, delay: f32) -> AoeSpawnRequest {
        AoeSpawnRequest {
            center: Vec3::ZERO,
            radius: 8.0,
            shape,
            duration_seconds: duration,
            initial_delay_seconds: delay,
            spell_id: "arcane_wave".into(),
            effects: vec![EffectSpec::Damage(DamageEffect { amount: 10.0 })],
            targeting: AoeTargeting::Everyone,
        }
    }

    #[test]
    fn delayed_cone_persists_with_its_aperture() {
        let request = damage_request(
            AoeShape::Cone {
                direction: Vec3::Z,
                angle_deg: 70.0,
            },
            0.15,
            0.15,
        );
        let row = persistable_region(1, &request).expect("delayed cone");
        assert_eq!(row.shape, AoeShapeRow::Cone);
        assert_eq!(row.angle_deg, 70.0);
        assert!(row.pending_delay_seconds > 0.0);
        assert!(!region_applies_this_tick(row.pending_delay_seconds));
        match region_shape(&row) {
            AoeShape::Cone { angle_deg, .. } => assert_eq!(angle_deg, 70.0),
            other => panic!("expected cone, got {other:?}"),
        }
    }

    #[test]
    fn zero_duration_cone_resolves_immediately() {
        let request = damage_request(
            AoeShape::Cone {
                direction: Vec3::Z,
                angle_deg: 70.0,
            },
            0.0,
            0.0,
        );
        assert!(persistable_region(1, &request).is_none());
    }

    #[test]
    fn delayed_circle_still_persists() {
        let request = damage_request(AoeShape::Circle, 1.0, 0.4);
        let row = persistable_region(1, &request).expect("delayed circle");
        assert_eq!(row.shape, AoeShapeRow::Circle);
        assert_eq!(region_shape(&row), AoeShape::Circle);
    }

    #[test]
    fn persisted_cone_does_not_zero_the_aperture() {
        let request = damage_request(
            AoeShape::Cone {
                direction: Vec3::X,
                angle_deg: 50.0,
            },
            0.2,
            0.2,
        );
        let row = persistable_region(7, &request).expect("cone");
        assert_ne!(row.angle_deg, 0.0);
        assert_eq!(row.angle_deg, 50.0);
    }

    #[test]
    fn windup_does_not_apply_before_delay_elapses() {
        assert!(!region_applies_this_tick(0.15));
        assert!(region_applies_this_tick(0.0));
    }

    #[test]
    fn exclude_caster_still_seeds_affected() {
        let mut request = damage_request(AoeShape::Circle, 1.0, 0.0);
        request.targeting = AoeTargeting::ExcludeCaster;
        let row = persistable_region(42, &request).expect("circle");
        assert_eq!(row.affected, vec![42]);
    }
}
