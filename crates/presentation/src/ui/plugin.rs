//! Registrazione del plugin UI e della camera dedicata.

use bevy::prelude::*;

use super::{
    boss_bar, card, character_roster, chat, connecting, crafting, crowd_control_bar, death_screen,
    debug_position, entity_bar, floating_text, gather_bar, inventory, login, loot, main_menu, market,
    notices, npc_sidebar, pause_menu, player_stats, scoreboard, scrollbar, settings, status_bar,
    systems, target_frame, target_indicator,
};

use bevymmo_client::pointer::{world_pointer_blocked, PointerOnHud};

use crate::game_state::{not_typing, Screen};
use crate::ui::theme::UiTheme;

/// Camera 2D dedicata alla UI. Resta attiva nel menu e durante la partita,
/// sopra la camera 3D della scena gameplay.
#[derive(Component)]
struct UiCamera;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiTheme>();
        bevymmo_client::pointer::PointerPlugin::ensure(app);
        app.add_systems(Startup, setup_ui_camera);
        app.add_systems(PreUpdate, refresh_pointer_on_hud);
        app.add_plugins((
            card::CardPlugin,
            chat::ChatPlugin,
            entity_bar::EntityBarPlugin,
            scoreboard::ScoreboardPlugin,
            main_menu::MainMenuPlugin,
            character_roster::CharacterRosterPlugin,
            login::LoginPlugin,
            settings::SettingsPlugin,
            pause_menu::PauseMenuPlugin,
            player_stats::PlayerStatsPlugin,
            connecting::ConnectingPlugin,
        ));
        app.add_plugins((
            target_indicator::TargetIndicatorPlugin,
            target_frame::TargetFramePlugin,
            death_screen::DeathScreenPlugin,
            crowd_control_bar::CrowdControlBarPlugin,
            inventory::InventoryUiPlugin,
            loot::LootUiPlugin,
            gather_bar::GatherBarPlugin,
            floating_text::FloatingTextPlugin,
            boss_bar::BossBarPlugin,
            status_bar::StatusBarPlugin,
            scrollbar::ScrollbarPlugin,
            notices::NoticesPlugin,
            npc_sidebar::NpcSidebarPlugin,
            crafting::CraftingUiPlugin,
            market::MarketUiPlugin,
        ));
        app.add_plugins(debug_position::DebugPositionPlugin);

        app.add_systems(
            Update,
            (
                systems::update_button_actions,
                systems::update_auth_button_actions,
                systems::unfocus_inputs_on_gameplay_screen,
                systems::update_button_visuals,
                systems::update_text_input_focus,
                systems::update_text_input_keyboard,
                systems::update_text_input_display,
                systems::scroll_text_input_to_caret,
                systems::update_connection_failure,
                systems::sync_typing_focus.after(systems::unfocus_inputs_on_gameplay_screen),
                systems::toggle_pause
                    .run_if(in_state(Screen::InGame))
                    .run_if(not_typing),
            ),
        );
    }
}

/// Copies UI `Interaction` into [`PointerOnHud`] before world-click systems
/// run, so a hovered inventory / chat / hotbar / inscription node blocks
/// move, targeting and NPC pick on the same frame.
fn refresh_pointer_on_hud(
    interactions: Query<(&Interaction, Option<&Pickable>)>,
    mut pointer: ResMut<PointerOnHud>,
) {
    let mut pressed = false;
    let mut hovered = false;
    for (interaction, pickable) in &interactions {
        if pickable.is_some_and(|pickable| *pickable == Pickable::IGNORE) {
            continue;
        }
        match *interaction {
            Interaction::Pressed => pressed = true,
            Interaction::Hovered => hovered = true,
            Interaction::None => {}
        }
    }
    pointer.0 = world_pointer_blocked(pressed, hovered);
}

fn setup_ui_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Camera {
            order: 1,
            clear_color: ClearColorConfig::None,
            ..default()
        },
        UiCamera,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_camera_is_created_above_the_game_camera_without_clearing_it() {
        let mut app = App::new();
        app.add_systems(Startup, setup_ui_camera);
        app.update();

        let mut cameras = app.world_mut().query_filtered::<&Camera, With<UiCamera>>();
        let camera = cameras.single(app.world()).expect("one UI camera");
        assert_eq!(camera.order, 1);
        assert!(matches!(camera.clear_color, ClearColorConfig::None));
    }
}
