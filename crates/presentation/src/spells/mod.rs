//! Client presentation for spells: cast bars, HUD and visual effects.

pub mod ability_vfx;
pub mod aim_preview;
pub mod aoe_zones;
pub mod cast_bar;
pub mod cursor;
pub mod effects;
pub mod eidolon_effects;
pub mod input;
pub mod ui;

use bevy::prelude::*;
use bevymmo_network::network::protocol::SpellVisualEffect;

use crate::spells::ability_vfx::{populate_registry, AbilityVfxRegistry, AbilityVfxSpec};

/// Registers spell HUD, cast-bar and client visual systems.
pub struct SpellsHudPlugin;

impl Plugin for SpellsHudPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SpellVisualEffect>();
        ui::spell_hud_systems(app);
        cast_bar::cast_bar_systems(app);

        // Initialize and populate the ability VFX registry (18 alpha abilities).
        let mut registry = AbilityVfxRegistry::default();
        populate_registry(&mut registry);
        app.insert_resource(registry);

        app.init_resource::<bevymmo_gameplay::abilities::AbilityAim>();
        app.init_resource::<input::PendingCastRelease>();
        app.configure_sets(Update, bevymmo_client::stdb::ClientSimulation::Predict);

        // `Escape` is bound to both `TogglePause` and `ClearTarget`: cancelling
        // an aim must claim the press before those two see it, otherwise
        // cancelling would also open the pause menu and deselect the target.
        app.add_systems(
            Update,
            aim_preview::cancel_ability_aim_on_escape
                .before(crate::ui::systems::toggle_pause)
                .before(bevymmo_client::targeting::systems::clear_target_with_escape)
                .run_if(crate::game_state::not_typing),
        );

        app.add_systems(
            Update,
            (
                // Before prediction so a rooted cast freezes this frame and
                // aim facing is not overwritten by the walk look.
                input::cast_abilities_on_key
                    .before(bevymmo_client::stdb::ClientSimulation::Predict)
                    .run_if(crate::game_state::not_typing),
                // After input, so the preview draws *this* frame's aim rather
                // than the previous frame's.
                aim_preview::draw_ability_aim_preview.after(input::cast_abilities_on_key),
                dispatch_visual_effects,
                aoe_zones::spawn_aoe_meshes,
                aoe_zones::apply_aoe_preview_visibility.after(aoe_zones::spawn_aoe_meshes),
                aoe_zones::pulse_aoe_meshes,
                ability_vfx::animate_lifecycle,
                eidolon_effects::animate,
            ),
        );
        app.add_systems(
            Update,
            cleanup_spell_visuals.run_if(crate::game_state::not_in_gameplay),
        );
    }
}

fn cleanup_spell_visuals(
    mut commands: Commands,
    visuals: Query<Entity, With<crate::spells::effects::SpellVisual>>,
) {
    for entity in &visuals {
        commands.entity(entity).despawn();
    }
}

fn dispatch_visual_effects(
    mut commands: Commands,
    mut effects: MessageReader<SpellVisualEffect>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    abilities: Res<bevymmo_gameplay::abilities::BaseAbilityRegistry>,
    vfx_registry: Res<AbilityVfxRegistry>,
) {
    for effect in effects.read() {
        let ability = abilities.get(&bevymmo_gameplay::abilities::AbilityId::new(
            effect.spell_id.clone(),
        ));
        let spec = match ability.as_ref() {
            Some(ability) => AbilityVfxSpec::from_ability(effect, ability.as_ref()),
            None => AbilityVfxSpec::from_effect(effect),
        };

        // 1) Try the per-ability VFX registry (alpha abilities with unique geometry).
        if let Some(spawn_fn) = vfx_registry.get(effect.spell_id.as_str()) {
            spawn_fn(&mut commands, &mut meshes, &mut materials, &spec);
            continue;
        }

        // 2) Fall back to legacy geometry-based selector for known BaseAbilities.
        match ability {
            Some(ability) => eidolon_effects::spawn_for_ability(
                &mut commands,
                &mut meshes,
                &mut materials,
                effect,
                &ability,
            ),
            None => eidolon_effects::spawn(&mut commands, &mut meshes, &mut materials, effect),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_state::{init_screen_states, PauseOverlay, Screen};
    use crate::spells::effects::SpellVisual;

    #[test]
    fn leaving_game_despawns_spell_visuals() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        init_screen_states(&mut app);
        app.add_systems(
            Update,
            cleanup_spell_visuals.run_if(crate::game_state::not_in_gameplay),
        );

        app.insert_state(Screen::InGame);
        let visual = app.world_mut().spawn(SpellVisual).id();
        app.update();
        assert!(app.world().get_entity(visual).is_ok());

        app.insert_state(Screen::MainMenu);
        app.update();
        assert!(app.world().get_entity(visual).is_err());
    }

    #[test]
    fn paused_does_not_despawn_spell_visuals() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        init_screen_states(&mut app);
        app.add_systems(
            Update,
            cleanup_spell_visuals.run_if(crate::game_state::not_in_gameplay),
        );

        app.insert_state(Screen::InGame);
        app.insert_state(PauseOverlay::On);
        let visual = app.world_mut().spawn(SpellVisual).id();
        app.update();
        assert!(app.world().get_entity(visual).is_ok());
    }
}
