//! What a client may ask the spell system to do.
//!
//! The port of Bevy's `process_cast_requests`, `handle_cast_release` and
//! `process_cast_weapon_requests`. Everything past validation lives in
//! [`crate::sim::spells`]; these three reducers only decide whether the caller
//! is allowed to do what it asked, and open (or close) a `cast_state`.
//!
//! # What changed, and why
//!
//! - **No caster field on the request.** Bevy's `SpellCastRequest` carried the
//!   entity it was cast by, so the handler had to trust the network layer to
//!   have filled it in correctly. `ctx.sender()` is assigned by SpacetimeDB, so
//!   the caster is derived, never claimed.
//! - **Range is enforced.** The Bevy server never checked `cast_range`: the
//!   client decided whether a cast was in range and the server believed it.
//! - **Cancelling a cast reports the cast that was cancelled.** Bevy emitted the
//!   *incoming* spell's id in the `SpellCastEnded` it sent when a new cast
//!   replaced a running one, which made the client hide the wrong bar.
//! - **AI does not use these reducers.** Enemies fire catalog `BaseAbility`s
//!   through [`crate::sim::spells::request_catalog_ability`].

use bevymmo_domain::abilities::{
    cast_armor_inscribed_ability, cast_root_inscribed_slot, movement_lock_for_ability,
    resolve_active_ability, resolve_armor_ability, resolve_armor_inscribed_ability,
    resolve_root_inscribed_slot, AbilityCastMode, AbilitySlot, CastBlockedReason,
    ChannelMovementPolicy as AbilityChannelMovementPolicy,
};
use bevymmo_domain::movement::{should_face_cast_target, MovementLock};
use bevymmo_domain::items::components::EquipSlot;
use bevymmo_domain::spells::context::SpellCastContext;
use bevymmo_domain::EntityId;
use glam::Vec3;
use spacetimedb::{reducer, ReducerContext, Table};

use crate::reducers::lifecycle::{caller_character, caller_entity};
use crate::rows::{equipment_from_rows, known_ancient_language_from_rows, Vec3Row};
use crate::sim::spells::{self, ability_loadout_for_item};
use crate::tables::{
    cast_state, equipment, game_entity, known_ancient_language, CastKindRow, CastSourceRow,
    CastState, EntityStateRow, GameEntity,
};

/// Ends the caller's cast of `spell_id`, as on key release.
///
/// A channel that is released has *completed* — its effect has been ticking all
/// along — while a cast-time wind-up released early is a cancellation. Releasing
/// a cast that already ended is not an error: the tick may well have finished it
/// between the key going up and this reducer running.
///
/// Ends a running Channeling cast. Instant never opens `cast_state`; CastTime
/// auto-fires in `advance_casts`.
#[reducer]
pub fn release_cast(
    ctx: &ReducerContext,
    spell_id: String,
    target_entity: Option<u64>,
    target_position: Option<Vec3Row>,
) -> Result<(), String> {
    let _ = (target_entity, target_position);
    let caster = caller_entity(ctx)?;
    let Some(cast) = ctx.db.cast_state().entity_id().find(&caster.entity_id) else {
        return Ok(());
    };
    if cast.spell_id != spell_id {
        return Ok(());
    }

    match cast.kind {
        CastKindRow::Channeling => {
            // Channeling ends without interruption (ran full duration or player released).
            spells::end_cast(ctx, caster.entity_id, cast.spell_id, false);
        }
        // Instant does not open a cast_state. CastTime auto-fires in advance_casts.
        _ => {
            spells::end_cast(ctx, caster.entity_id, cast.spell_id, true);
        }
    }
    Ok(())
}

/// Casts the weapon ability inscribed on the caller's equipped weapon.
///
/// `slot` is `"primary"`, `"secondary"` or `"ultimate"` — the gameplay role, not
/// a keyboard key (see `bevymmo_domain::abilities::AbilitySlot`).
///
/// Branches on the resolved ability's [`AbilityCastMode`]: `Instant` resolves
/// and applies the effect on the spot, `CastTime` and `Channeling` open a
/// `cast_state` row that [`crate::sim::spells::step`] advances, the same way
/// the legacy spell path does.
#[reducer]
pub fn cast_weapon(
    ctx: &ReducerContext,
    slot: String,
    target_entity: Option<u64>,
    target_position: Option<Vec3Row>,
) -> Result<(), String> {
    let character_id = caller_character(ctx)?.character_id;
    let caster = caller_entity(ctx)?;
    if caster.state == EntityStateRow::Dead {
        return Err("dead characters do not cast".to_string());
    }
    let slot = parse_slot(&slot)?;

    let equipment = ctx
        .db
        .equipment()
        .character_id()
        .find(&character_id)
        .map(|row| equipment_from_rows(&row.slots))
        .unwrap_or_default();
    let weapon = equipment
        .weapon
        .as_ref()
        .ok_or_else(|| "no weapon equipped".to_string())?;
    let item = spells::items()
        .get(&weapon.item_id)
        .ok_or_else(|| format!("unknown item {:?}", weapon.item_id.as_str()))?;
    let weapon_abilities = ability_loadout_for_item(item.as_ref())
        .ok_or_else(|| format!("{} has no weapon abilities", item.display_name()))?;

    let ability_id = resolve_active_ability(slot, weapon_abilities, &weapon.ability_selection)
        .cloned()
        .ok_or_else(|| format!("the weapon offers no gesture for {slot:?}"))?;

    if spells::is_on_cooldown(ctx, caster.entity_id, ability_id.as_str()) {
        return Err(format!("{:?} is on cooldown", ability_id.as_str()));
    }
    if spells::casting_blocked(ctx, caster.entity_id) {
        return Err("you cannot cast right now".to_string());
    }

    let root_inscription = weapon
        .root_inscription
        .as_ref()
        .ok_or_else(|| "weapon has no Root Word inscription".to_string())?;
    let known_language = ctx
        .db
        .known_ancient_language()
        .character_id()
        .find(&character_id)
        .map(|row| {
            known_ancient_language_from_rows(
                &row.root_words,
                &row.ancient_words,
                &row.base_abilities,
            )
        })
        .ok_or_else(|| "ancient language has not been initialized".to_string())?;
    let preview = resolve_root_inscribed_slot(
        slot,
        weapon_abilities,
        &weapon.ability_selection,
        root_inscription,
        &known_language,
        spells::base_abilities(),
        spells::root_words(),
        spells::ancient_words(),
        Some(item.as_ref()),
    )
    .map_err(describe_block)?;
    spells::validate_cast_target(
        ctx,
        &caster,
        preview.params.range,
        preview.ability.geometry().targeting_mode(),
        target_entity,
        target_position.map(Vec3::from),
    )?;
    spells::spend_mana(ctx, caster.entity_id, preview.params.mana_cost)?;
    let cast_mode = preview.ability.cast_mode();
    // Plant before facing so leftover chase cannot walk the next tick and
    // cancel the wind-up. Instant keeps walking. A later click still
    // interrupts CastTime / InterruptOnMove Channeling in `advance_casts`.
    let caster = if cast_mode.holds_still() {
        spells::stop_movement(ctx, caster)
    } else {
        caster
    };
    let caster = face_target(
        ctx,
        caster,
        target_position.map(Vec3::from),
        movement_lock_for_ability(cast_mode),
    );
    cancel_active_cast(ctx, caster.entity_id);

    match cast_mode {
        AbilityCastMode::Instant => {
            // Original path: execute immediately.
            let combat = spells::combat_stats(ctx, caster.entity_id)
                .ok_or_else(|| "caster has no stats".to_string())?;
            let target_position = target_position.map(Vec3::from);
            let caster_position = Vec3::from(caster.position);
            let targets = spells::potential_targets(
                ctx,
                caster_position,
                preview.params.range + preview.params.area + spells::TARGET_QUERY_MARGIN,
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

            let known_language = ctx
                .db
                .known_ancient_language()
                .character_id()
                .find(&character_id)
                .map(|row| {
                    known_ancient_language_from_rows(
                        &row.root_words,
                        &row.ancient_words,
                        &row.base_abilities,
                    )
                })
                .ok_or_else(|| "ancient language has not been initialized".to_string())?;
            cast_root_inscribed_slot(
                slot,
                weapon_abilities,
                &weapon.ability_selection,
                root_inscription,
                &known_language,
                spells::base_abilities(),
                spells::root_words(),
                spells::ancient_words(),
                &mut cast_ctx,
                Some(item.as_ref()),
            )
            .map_err(describe_block)?;

            spells::apply_pending(
                ctx,
                caster.entity_id,
                caster_position,
                ability_id.as_str(),
                &mut cast_ctx,
            );
            spells::start_cooldown(
                ctx,
                caster.entity_id,
                ability_id.as_str(),
                preview.ability.base_params().cooldown,
            );
            Ok(())
        }
        AbilityCastMode::CastTime => {
            let required_seconds = preview.params.cast_time;
            let target_position = target_position.map(Vec3::from);

            ctx.db.cast_state().insert(CastState {
                entity_id: caster.entity_id,
                spell_id: ability_id.as_str().to_string(),
                kind: CastKindRow::CastTime,
                source: CastSourceRow::Weapon,
                elapsed_seconds: 0.0,
                required_seconds,
                start_position: caster.position,
                target_position: target_position.map(Vec3Row::from),
                target_entity,
                channel_tick_accumulator: 0.0,
                tick_interval_seconds: 0.0,
                channel_movement_interrupts: true,
            });
            Ok(())
        }
        AbilityCastMode::Channeling {
            tick_interval_seconds,
            movement_policy,
        } => {
            let required_seconds = preview.params.cast_time.max(0.1);
            let target_position = target_position.map(Vec3::from);

            // Channel cooldown starts on press (same as legacy).
            spells::start_cooldown(
                ctx,
                caster.entity_id,
                ability_id.as_str(),
                preview.ability.base_params().cooldown,
            );

            // Store the movement policy from AbilityCastMode so advance_casts
            // can honor it without re-resolving the ability.
            let movement_interrupts = matches!(
                movement_policy,
                AbilityChannelMovementPolicy::InterruptOnMove
            );

            // Channel starts armed so first tick lands on next tick.
            ctx.db.cast_state().insert(CastState {
                entity_id: caster.entity_id,
                spell_id: ability_id.as_str().to_string(),
                kind: CastKindRow::Channeling,
                source: CastSourceRow::Weapon,
                elapsed_seconds: 0.0,
                required_seconds,
                start_position: caster.position,
                target_position: target_position.map(Vec3Row::from),
                target_entity,
                channel_tick_accumulator: tick_interval_seconds,
                tick_interval_seconds,
                channel_movement_interrupts: movement_interrupts,
            });
            Ok(())
        }
    }
}

/// Casts the first Primary ability supplied by an equipped armor item.
///
/// Armor abilities intentionally use a separate API from weapon weapon slots.
/// This initial reducer handles instant armor abilities; timed armor casts will
/// reuse the same source-aware resolver in the scheduler.
#[reducer]
pub fn armor_cast(
    ctx: &ReducerContext,
    armor_slot: String,
    ability_slot: String,
    target_entity: Option<u64>,
    target_position: Option<Vec3Row>,
) -> Result<(), String> {
    let character_id = caller_character(ctx)?.character_id;
    let caster = caller_entity(ctx)?;
    if caster.state == EntityStateRow::Dead {
        return Err("dead characters do not cast".to_string());
    }
    let target_slot = match armor_slot.to_ascii_lowercase().as_str() {
        "helmet" => EquipSlot::Helmet,
        "armor" | "chest" | "chestplate" => EquipSlot::Armor,
        "shoes" | "boots" => EquipSlot::Shoes,
        other => return Err(format!("unknown armor slot {other:?}")),
    };
    let equipment = ctx
        .db
        .equipment()
        .character_id()
        .find(&character_id)
        .map(|row| equipment_from_rows(&row.slots))
        .unwrap_or_default();
    let armor = equipment
        .get(target_slot)
        .as_ref()
        .ok_or_else(|| format!("armor slot {armor_slot:?} is empty"))?;
    let item = spells::items()
        .get(&armor.item_id)
        .ok_or_else(|| format!("unknown item {:?}", armor.item_id.as_str()))?;
    let abilities = ability_loadout_for_item(item.as_ref())
        .ok_or_else(|| format!("{} has no armor abilities", item.display_name()))?;
    // Armor has one active ability chosen from the union of all abilities the
    // item offers. The legacy ability_slot argument remains in the reducer
    // shape for generated-client compatibility, but is intentionally ignored.
    let _ = ability_slot;
    let ability_id = resolve_armor_ability(abilities, &armor.ability_selection)
        .cloned()
        .ok_or_else(|| "armor has no ability".to_string())?;
    if spells::is_on_cooldown(ctx, caster.entity_id, ability_id.as_str()) {
        return Err(format!("{:?} is on cooldown", ability_id.as_str()));
    }
    if spells::casting_blocked(ctx, caster.entity_id) {
        return Err("you cannot cast right now".to_string());
    }

    let language_row = ctx
        .db
        .known_ancient_language()
        .character_id()
        .find(&character_id)
        .ok_or_else(|| "ancient language has not been initialized".to_string())?;
    let language = known_ancient_language_from_rows(
        &language_row.root_words,
        &language_row.ancient_words,
        &language_row.base_abilities,
    );
    let preview = resolve_armor_inscribed_ability(
        &ability_id,
        armor.armor_inscription.as_ref(),
        &language,
        spells::base_abilities(),
        spells::root_words(),
        spells::ancient_words(),
        Some(item.as_ref()),
    )
    .map_err(describe_block)?;
    spells::validate_cast_target(
        ctx,
        &caster,
        preview.params.range,
        preview.ability.geometry().targeting_mode(),
        target_entity,
        target_position.map(Vec3::from),
    )?;
    spells::spend_mana(ctx, caster.entity_id, preview.params.mana_cost)?;
    let cast_mode = preview.ability.cast_mode();
    let source = match target_slot {
        EquipSlot::Helmet => CastSourceRow::Helmet,
        EquipSlot::Armor => CastSourceRow::Armor,
        EquipSlot::Shoes => CastSourceRow::Shoes,
        _ => return Err("invalid armor source".to_string()),
    };

    let caster = if cast_mode.holds_still() {
        spells::stop_movement(ctx, caster)
    } else {
        caster
    };
    let caster = face_target(
        ctx,
        caster,
        target_position.map(Vec3::from),
        movement_lock_for_ability(cast_mode),
    );
    cancel_active_cast(ctx, caster.entity_id);
    if matches!(cast_mode, AbilityCastMode::Instant) {
        return cast_armor_instant(
            ctx,
            caster,
            target_position,
            target_entity,
            &ability_id,
            &preview,
            armor,
            &language,
            item.as_ref(),
        );
    }

    let target_position = target_position.map(Vec3::from);
    let (kind, required_seconds, tick_interval_seconds, channel_movement_interrupts) =
        match cast_mode {
            AbilityCastMode::CastTime => {
                (CastKindRow::CastTime, preview.params.cast_time, 0.0, true)
            }
            AbilityCastMode::Channeling {
                tick_interval_seconds,
                movement_policy,
            } => (
                CastKindRow::Channeling,
                preview.params.cast_time.max(0.1),
                tick_interval_seconds,
                matches!(
                    movement_policy,
                    AbilityChannelMovementPolicy::InterruptOnMove
                ),
            ),
            AbilityCastMode::Instant => unreachable!(),
        };
    if matches!(kind, CastKindRow::Channeling) {
        spells::start_cooldown(
            ctx,
            caster.entity_id,
            ability_id.as_str(),
            preview.ability.base_params().cooldown,
        );
    }
    ctx.db.cast_state().insert(CastState {
        entity_id: caster.entity_id,
        spell_id: ability_id.as_str().to_string(),
        kind,
        source,
        elapsed_seconds: 0.0,
        required_seconds,
        start_position: caster.position,
        target_position: target_position.map(Vec3Row::from),
        target_entity,
        channel_tick_accumulator: if matches!(kind, CastKindRow::Channeling) {
            tick_interval_seconds
        } else {
            0.0
        },
        tick_interval_seconds,
        channel_movement_interrupts,
    });
    Ok(())
}

fn cast_armor_instant(
    ctx: &ReducerContext,
    caster: crate::tables::GameEntity,
    target_position: Option<Vec3Row>,
    target_entity: Option<u64>,
    ability_id: &bevymmo_domain::abilities::AbilityId,
    preview: &bevymmo_domain::abilities::SlotPreview,
    armor: &bevymmo_domain::items::instance::ItemInstance,
    language: &bevymmo_domain::abilities::KnownAncientLanguage,
    item: &dyn bevymmo_domain::items::definition::Item,
) -> Result<(), String> {
    let combat = spells::combat_stats(ctx, caster.entity_id)
        .ok_or_else(|| "caster has no stats".to_string())?;
    let caster_position = Vec3::from(caster.position);
    let targets = spells::potential_targets(
        ctx,
        caster_position,
        preview.params.range + preview.params.area + spells::TARGET_QUERY_MARGIN,
    );
    let mut cast_ctx = SpellCastContext::new(
        EntityId::new(caster.entity_id),
        caster_position,
        &combat,
        Vec3::from(caster.look),
        target_position.map(Vec3::from),
        target_entity.map(EntityId::new),
        &targets,
    );
    cast_armor_inscribed_ability(
        ability_id,
        armor.armor_inscription.as_ref(),
        language,
        spells::base_abilities(),
        spells::root_words(),
        spells::ancient_words(),
        &mut cast_ctx,
        Some(item),
    )
    .map_err(describe_block)?;
    spells::apply_pending(
        ctx,
        caster.entity_id,
        caster_position,
        ability_id.as_str(),
        &mut cast_ctx,
    );
    spells::start_cooldown(
        ctx,
        caster.entity_id,
        ability_id.as_str(),
        preview.ability.base_params().cooldown,
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Turns the caster to face the point it aimed at, and returns the updated row.
///
/// Applied only once validation has passed, so a rejected cast cannot silently
/// spin the character around. Self-cast spells send no point and keep the facing
/// they had. Instant casts while walking also keep the walk facing: the next
/// movement tick would overwrite a one-tick turn, which reads as a twitch.
fn face_target(
    ctx: &ReducerContext,
    caster: GameEntity,
    target_position: Option<Vec3>,
    lock: MovementLock,
) -> GameEntity {
    let Some(target) = target_position else {
        return caster;
    };
    if !should_face_cast_target(caster.move_target.is_some(), lock) {
        return caster;
    }
    let offset = target - Vec3::from(caster.position);
    let offset = Vec3::new(offset.x, 0.0, offset.z);
    if offset.length() <= 0.001 {
        return caster;
    }
    ctx.db.game_entity().entity_id().update(GameEntity {
        look: offset.normalize().into(),
        ..caster
    })
}

/// Cancels whatever the caster was casting, so starting a spell always replaces
/// the previous one rather than racing it.
fn cancel_active_cast(ctx: &ReducerContext, entity_id: u64) {
    if let Some(active) = ctx.db.cast_state().entity_id().find(&entity_id) {
        spells::end_cast(ctx, entity_id, active.spell_id, true);
    }
    crate::sim::gathering::cancel_session(ctx, entity_id);
}

fn parse_slot(slot: &str) -> Result<AbilitySlot, String> {
    match slot.to_ascii_lowercase().as_str() {
        "primary" => Ok(AbilitySlot::Primary),
        "secondary" => Ok(AbilitySlot::Secondary),
        "ultimate" => Ok(AbilitySlot::Ultimate),
        other => Err(format!(
            "unknown ability slot {other:?}; expected primary, secondary or ultimate"
        )),
    }
}

fn describe_block(reason: CastBlockedReason) -> String {
    match reason {
        CastBlockedReason::MissingRegistryEntry => {
            "that gesture no longer exists in the registry".to_string()
        }
        CastBlockedReason::UnknownRootWord => {
            "that slot uses an unknown or unavailable Root Word".to_string()
        }
        CastBlockedReason::UnknownAncientWord => {
            "that slot uses an unknown or unavailable Ancient Word".to_string()
        }
        CastBlockedReason::IncompatibleAncientWord => {
            "an Ancient Word is incompatible with the selected gesture".to_string()
        }
    }
}
