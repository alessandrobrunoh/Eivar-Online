//! Player ability input for the weapon cast pipeline.
//!
//! Routes the weapon HUD keys (default 1/2/3) to the press/aim/release path
//! for every equipped weapon that exposes `Item::ability_loadout()`. There is
//! no second, legacy `SpellHotbar` input path.
//!
//! # Cast behavior by [`AbilityCastMode`]
//!
//! Instant, CastTime and Channeling share the same input: press opens an aim
//! window ([`AbilityAim`]); release sends `cast_weapon` and plants leftover
//! movement. Instant fires on that call. CastTime winds up and auto-fires.
//! Channeling ticks until `cast_time` or a movement interrupt — cooldown
//! starts on release, so a later click cuts the channel short but still
//! spends the cooldown.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevymmo_client::local_player::LocalPlayer;
use bevymmo_client::movement::{ClientSurfaceQuery, LocalMovementFreeze};
use bevymmo_client::stdb::{commands as stdb_commands, StdbConnection};
use bevymmo_client::targeting::CurrentTarget;
use bevymmo_client::user_settings::{GameSettingsResource, KeyAction};
use bevymmo_gameplay::abilities::{
    resolve_active_ability, weapon_cast_intent, AbilityAim, AbilityId, AbilitySlot, ArcBaseAbility,
    BaseAbilityRegistry,
};
use bevymmo_gameplay::items::components::Equipment;
use bevymmo_gameplay::items::registry::ItemRegistry;
use bevymmo_gameplay::stats::components::VitalStats;
use bevymmo_gameplay::stats::formulas::can_afford_mana;
use bevymmo_network::network::protocol::{LookDirection, NetworkEntityId, Position};

use crate::game_state::{in_gameplay, Screen};
use crate::spells::cast_bar::ObservedCasts;
use crate::spells::cursor::{cursor_ground_point, flat_direction_towards};
use crate::spells::ui::{HudCooldownKey, SpellHudCooldownStarted, SpellHudState};

/// Canonical HUD + input mapping for the three weapon slots.
///
/// The hotbar labels these actions, and [`cast_abilities_on_key`] reads them
/// so a key bound here aims on press and casts on release.
pub const WEAPON_HUD_BINDINGS: [(KeyAction, AbilitySlot); 3] = [
    (KeyAction::CastPrimary, AbilitySlot::Primary),
    (KeyAction::CastSecondary, AbilitySlot::Secondary),
    (KeyAction::CastUltimate, AbilitySlot::Ultimate),
];

/// Every key that drives a weapon slot through the aim / cast path.
pub fn weapon_slot_bindings() -> impl Iterator<Item = (KeyAction, AbilitySlot)> {
    WEAPON_HUD_BINDINGS.into_iter()
}

/// Charge/Channel key-up that left the client before the replicated snapshot.
///
/// The first `release_cast` still goes out immediately (same-client reducers
/// stay ordered). This retry covers the case where that send raced ahead of
/// `cast_weapon` and no-op'd, which would otherwise leave the charge bar up
/// until the player held the key again.
#[derive(Resource, Default)]
pub struct PendingCastRelease(Option<QueuedCastRelease>);

#[derive(Clone)]
struct QueuedCastRelease {
    ability_id: AbilityId,
    action: KeyAction,
    target_id: Option<u64>,
    target_position: Option<Vec3>,
    stop_movement: bool,
    /// `Some` for Charge (HUD countdown starts on fire). Channeling already
    /// started its countdown on press.
    hud_cooldown_seconds: Option<f32>,
}

impl PendingCastRelease {
    fn clear(&mut self) {
        self.0 = None;
    }
}

/// Raycast and surface query parameters bundled for aiming.
#[derive(SystemParam)]
pub struct AimRaycastParams<'w, 's> {
    pub windows: Query<'w, 's, &'static Window, With<bevy::window::PrimaryWindow>>,
    pub cameras: Query<'w, 's, (&'static Camera, &'static Transform), With<Camera3d>>,
    pub surface_query: Option<Res<'w, ClientSurfaceQuery>>,
}

/// Local movement side-effects of a weapon cast. Bundled so
/// [`cast_abilities_on_key`] stays within Bevy's 16-argument system limit.
#[derive(SystemParam)]
pub struct CastMovementParams<'w> {
    move_target: ResMut<'w, bevymmo_client::movement::MoveTarget>,
    movement_freeze: ResMut<'w, LocalMovementFreeze>,
    time: Res<'w, Time>,
}

#[allow(clippy::too_many_arguments)]
pub fn cast_abilities_on_key(
    keys: Option<Res<ButtonInput<KeyCode>>>,
    settings: Res<GameSettingsResource>,
    screen: Res<State<Screen>>,
    current_target: Res<CurrentTarget>,
    mut aim: ResMut<AbilityAim>,
    target_ids: Query<&NetworkEntityId>,
    aim_ray: AimRaycastParams,
    mut controlled_players: Query<
        (
            &Equipment,
            &Position,
            &NetworkEntityId,
            &mut LookDirection,
            &VitalStats,
        ),
        With<LocalPlayer>,
    >,
    _observed_casts: Res<ObservedCasts>,
    mut pending_release: ResMut<PendingCastRelease>,
    movement: CastMovementParams,
    conn: Option<Res<StdbConnection>>,
    item_registry: Res<ItemRegistry>,
    ability_registry: Res<BaseAbilityRegistry>,
    hud_state: Res<SpellHudState>,
    mut hud_cooldowns: MessageWriter<SpellHudCooldownStarted>,
) {
    let CastMovementParams {
        mut move_target,
        mut movement_freeze,
        time,
    } = movement;
    // Any condition that invalidates the aiming context must close the aim window,
    // otherwise a stale aim would fire on the next unrelated key-release.
    let Some(keys) = keys else {
        aim.clear();
        pending_release.clear();
        return;
    };
    if !in_gameplay(screen) {
        aim.clear();
        pending_release.clear();
        return;
    }

    let Ok((equipment, player_position, _local_network_id, mut look_direction, vitals)) =
        controlled_players.single_mut()
    else {
        aim.clear();
        pending_release.clear();
        return;
    };

    // Only weapons with a loadout drive Primary/Secondary/Ultimate.
    let Some(weapon) = &equipment.weapon else {
        aim.clear();
        pending_release.clear();
        return;
    };
    let Some(item) = item_registry.get(&weapon.item_id) else {
        aim.clear();
        pending_release.clear();
        return;
    };
    let Some(weapon_abilities) = item.ability_loadout() else {
        aim.clear();
        pending_release.clear();
        return;
    };

    let aiming = aim.slot.is_some();
    let needs_ground = aiming
        || weapon_slot_bindings().any(|(action, _)| {
            settings.just_pressed(action, &keys) || settings.just_released(action, &keys)
        });
    let target_position = if needs_ground {
        cursor_ground_point(
            &aim_ray.windows,
            &aim_ray.cameras,
            aim_ray.surface_query.as_deref(),
        )
    } else {
        None
    };

    let selected_id = current_target
        .entity
        .and_then(|entity| target_ids.get(entity).ok())
        .map(|net_id| net_id.0);

    // ── Press handling: open aim only ────────────────────────────────
    for (action, slot) in weapon_slot_bindings() {
        if !settings.just_pressed(action, &keys) {
            continue;
        }
        pending_release.clear();

        let Some((_, ability)) = active_ability(slot, weapon_abilities, weapon, &ability_registry)
        else {
            continue;
        };
        if !can_afford_mana(vitals.current_mana, ability.base_params().mana_cost) {
            continue;
        }

        let intent = weapon_cast_intent(true, false, ability.cast_mode());
        if intent.open_aim {
            aim.begin(slot);
        }
    }

    // ── Per-frame aim tracking ──────────────────────────────────────
    // While aiming, face the cursor every frame so the preview (which reads
    // LookDirection) stays in sync with mouse movement.
    if aim.slot.is_some() {
        aim.ground_point = target_position;
        if let Some(direction) =
            target_position.and_then(|target| flat_direction_towards(player_position.0, target))
        {
            look_direction.0 = direction;
        }
    }

    // ── Release handling: one `cast_weapon` ─────────────────────────
    for (_action, slot) in weapon_slot_bindings() {
        if !settings.just_released(_action, &keys) {
            continue;
        }

        let Some((ability_id, ability)) =
            active_ability(slot, weapon_abilities, weapon, &ability_registry)
        else {
            continue;
        };

        let intent = weapon_cast_intent(false, true, ability.cast_mode());
        if !intent.start_cast {
            continue;
        }
        if !can_afford_mana(vitals.current_mana, ability.base_params().mana_cost) {
            aim.clear();
            continue;
        }
        if aim.slot != Some(slot) {
            continue;
        }
        let cancelled = aim.cancelled;
        aim.clear();
        if cancelled {
            continue;
        }
        if hud_state.ability_on_cooldown(&ability_id) {
            continue;
        }

        let face_direction =
            target_position.and_then(|target| flat_direction_towards(player_position.0, target));
        if let Some(direction) = face_direction {
            look_direction.0 = direction;
        }
        root_local_movement(
            &mut move_target,
            &mut movement_freeze,
            time.elapsed_secs(),
            ability.cast_mode(),
        );

        if let Some(conn) = conn.as_deref() {
            if let Err(err) = stdb_commands::cast_weapon(
                conn,
                slot,
                ability.geometry().selected_entity_payload(selected_id),
                target_position,
            ) {
                error!("could not cast weapon ability: {err}");
            }
        }

        if ability.cast_mode().is_instant() || ability.cast_mode().is_channeling() {
            hud_cooldowns.write(SpellHudCooldownStarted {
                key: HudCooldownKey::Ability(ability_id.clone()),
                cooldown_seconds: ability.base_params().cooldown,
            });
        }
    }
}

fn observed_cast_is(observed: &ObservedCasts, caster: u64, ability_id: &AbilityId) -> bool {
    observed
        .0
        .get(&caster)
        .is_some_and(|cast| cast.spell_id == ability_id.as_str())
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
fn send_release_cast(
    conn: Option<&StdbConnection>,
    ability_id: &AbilityId,
    target_id: Option<u64>,
    target_position: Option<Vec3>,
    player_position: Vec3,
    look_direction: &mut LookDirection,
    move_target: &mut bevymmo_client::movement::MoveTarget,
    freeze: &mut LocalMovementFreeze,
    now: f32,
    stop_movement: bool,
    hud_cooldown: Option<(&mut MessageWriter<SpellHudCooldownStarted>, f32)>,
) {
    if let Some(direction) =
        target_position.and_then(|target| flat_direction_towards(player_position, target))
    {
        look_direction.0 = direction;
    }
    if stop_movement {
        move_target.0 = None;
        freeze.arm(now);
    }
    if let Some(conn) = conn {
        if let Err(err) = stdb_commands::release_cast(
            conn,
            ability_id.as_str().to_owned(),
            target_id,
            target_position,
        ) {
            error!("could not release cast: {err}");
        }
    }
    if let Some((hud_cooldowns, cooldown_seconds)) = hud_cooldown {
        hud_cooldowns.write(SpellHudCooldownStarted {
            key: HudCooldownKey::Ability(ability_id.clone()),
            cooldown_seconds,
        });
    }
}

/// Active gesture on the given slot, with its base cooldown.  `None` when the
/// weapon offers nothing for this slot or the inscribed ability id is missing
/// from the registry.
fn active_ability(
    slot: AbilitySlot,
    weapon_abilities: &bevymmo_gameplay::abilities::WeaponAbilities,
    weapon: &bevymmo_gameplay::items::instance::ItemInstance,
    ability_registry: &BaseAbilityRegistry,
) -> Option<(AbilityId, ArcBaseAbility)> {
    let ability_id = resolve_active_ability(slot, weapon_abilities, &weapon.ability_selection)?;
    let ability = ability_registry.get(ability_id)?;
    Some((ability_id.clone(), ability))
}

/// Drop leftover dest and ignore the last server dest until it replicates
/// as cleared. A later right-click still walks (and interrupts) because
/// freeze only suppresses stale dest, not a new click.
fn root_local_movement(
    move_target: &mut bevymmo_client::movement::MoveTarget,
    freeze: &mut LocalMovementFreeze,
    now: f32,
    cast_mode: bevymmo_gameplay::abilities::AbilityCastMode,
) {
    if !cast_mode.holds_still() {
        return;
    }
    move_target.0 = None;
    freeze.arm(now);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unaffordable_weapon_casts_are_skipped() {
        assert!(!can_afford_mana(5.0, 12.0));
        assert!(can_afford_mana(12.0, 12.0));
    }

    #[test]
    fn hud_keys_drive_each_slot_once() {
        assert_eq!(WEAPON_HUD_BINDINGS[0].1, AbilitySlot::Primary);
        assert_eq!(WEAPON_HUD_BINDINGS[1].1, AbilitySlot::Secondary);
        assert_eq!(WEAPON_HUD_BINDINGS[2].1, AbilitySlot::Ultimate);
        assert_eq!(WEAPON_HUD_BINDINGS[0].0, KeyAction::CastPrimary);

        let slots: Vec<AbilitySlot> = weapon_slot_bindings().map(|(_, slot)| slot).collect();
        assert_eq!(
            slots,
            vec![
                AbilitySlot::Primary,
                AbilitySlot::Secondary,
                AbilitySlot::Ultimate,
            ]
        );
    }

    #[test]
    fn observed_cast_matches_the_named_ability_only() {
        let mut observed = ObservedCasts::default();
        observed.0.insert(
            1,
            crate::spells::cast_bar::ObservedCast {
                spell_id: "cleave".into(),
                kind: 2,
                elapsed_seconds: 0.0,
                required_seconds: 0.15,
                since_update_seconds: 0.0,
                stale_after_seconds: 1.0,
            },
        );
        assert!(observed_cast_is(&observed, 1, &AbilityId::new("cleave")));
        assert!(!observed_cast_is(&observed, 1, &AbilityId::new("lunge")));
        assert!(!observed_cast_is(&observed, 2, &AbilityId::new("cleave")));
    }

    #[test]
    fn instant_cast_does_not_plant() {
        let mut dest = bevymmo_client::movement::MoveTarget(Some(Vec3::X));
        let mut freeze = LocalMovementFreeze::default();
        root_local_movement(
            &mut dest,
            &mut freeze,
            1.0,
            bevymmo_gameplay::abilities::AbilityCastMode::Instant,
        );
        assert_eq!(dest.0, Some(Vec3::X));
        assert!(!freeze.is_active(1.0));
    }

    #[test]
    fn cast_time_plants_leftover_dest() {
        let mut dest = bevymmo_client::movement::MoveTarget(Some(Vec3::X));
        let mut freeze = LocalMovementFreeze::default();
        root_local_movement(
            &mut dest,
            &mut freeze,
            1.0,
            bevymmo_gameplay::abilities::AbilityCastMode::CastTime,
        );
        assert!(dest.0.is_none());
        assert!(freeze.is_active(1.0));
    }

    #[test]
    fn channeling_plants_leftover_dest() {
        let mut dest = bevymmo_client::movement::MoveTarget(Some(Vec3::X));
        let mut freeze = LocalMovementFreeze::default();
        root_local_movement(
            &mut dest,
            &mut freeze,
            1.0,
            bevymmo_gameplay::abilities::AbilityCastMode::Channeling {
                tick_interval_seconds: 0.2,
                movement_policy:
                    bevymmo_gameplay::abilities::ChannelMovementPolicy::InterruptOnMove,
            },
        );
        assert!(dest.0.is_none());
        assert!(freeze.is_active(1.0));
    }
}
