//! Multi-tab settings screen (General / Graphics / Gameplay / Keybinds).
//!
//! Architecture:
//!
//! - [`state`] — plain-data source of truth (`GameSettings` + serialization).
//! - [`layout`] — shell spawn (sidebar + content area).
//! - [`panels`] — one module per tab (`general`, `graphics`, `gameplay`, `keybinds`).
//! - [`widgets`] — reusable widgets (`dropdown`, `toggle`, `key_capture`).
//! - [`systems`] — interaction handling, apply to `GameSettingsResource`,
//!   apply to `Window`, persistence.
//!
//! Adding a new tab = new module in `panels/`, new variant in `SettingsTab`,
//! new line in [`SettingsPlugin::build`].

pub mod layout;
pub mod panels;
pub mod state;
pub mod systems;
pub mod widgets;

use bevy::prelude::*;

use crate::game_state::Screen;
use crate::ui::theme::UiTheme;

use bevymmo_client::user_settings::{GameSettingsResource, KeyAction};

use layout::ActiveSettingsTab;
use state::load_settings;
use widgets::dropdown::Dropdown;

pub use state::{SettingsReturn, SettingsSession};

/// Marker: root of the whole settings screen (carries visibility).
#[derive(Component)]
pub struct SettingsUi;

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        // Load once at startup; subsequent mutations go through the resource.
        let settings = load_settings();
        app.insert_resource(GameSettingsResource(settings))
            .init_resource::<ActiveSettingsTab>()
            .init_resource::<SettingsSession>()
            .add_message::<widgets::dropdown::DropdownChanged>()
            .add_message::<widgets::key_capture::KeyBindingChanged>()
            .add_systems(Startup, setup_settings)
            .add_systems(
                Update,
                close_settings_on_escape
                    .after(widgets::dropdown::close_dropdowns_on_escape)
                    .before(crate::ui::systems::toggle_pause),
            )
            .add_systems(
                Update,
                (
                    update_settings_visibility.run_if(
                        state_changed::<Screen>.or_eager(resource_changed::<SettingsSession>),
                    ),
                    systems::update_panel_visibility.run_if(resource_changed::<ActiveSettingsTab>),
                    systems::update_tab_button_visuals,
                    systems::switch_tab_on_click,
                    (
                        widgets::dropdown::toggle_dropdown_on_header_click,
                        widgets::dropdown::pick_dropdown_option,
                        widgets::dropdown::close_dropdowns_on_escape,
                        widgets::dropdown::close_dropdowns_on_outside_click,
                        widgets::dropdown::sync_dropdown_open_state,
                        widgets::dropdown::sync_dropdown_bar_visuals,
                    )
                        .chain(),
                    systems::toggle_on_click,
                    widgets::toggle::sync_toggle_visuals,
                    systems::toggle_key_capture_on_click,
                    widgets::key_capture::sync_key_capture_visuals,
                    systems::update_key_capture_input,
                ),
            )
            .add_systems(
                Update,
                (
                    systems::reset_keybinds_on_button,
                    systems::apply_widget_events,
                    systems::apply_graphics_to_window,
                    systems::apply_interface_scale,
                    systems::persist_settings_when_changed,
                ),
            );
    }
}

fn setup_settings(
    mut commands: Commands,
    theme: Res<UiTheme>,
    settings: Res<GameSettingsResource>,
    monitors: Query<&bevy::window::Monitor>,
    asset_server: Res<AssetServer>,
) {
    let root = commands
        .spawn((
            Name::new("Settings Screen"),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                display: Display::None, // toggled by update_settings_visibility
                ..default()
            },
            BackgroundColor(theme.screen_bg),
            GlobalZIndex(20),
            SettingsUi,
        ))
        .id();

    layout::spawn_settings_shell(
        &mut commands,
        root,
        &theme,
        &settings,
        &monitors,
        &asset_server,
    );
}

/// Toggles the settings overlay. Menu path still uses `Screen::Settings`;
/// pause path only flips [`SettingsSession`].
pub fn update_settings_visibility(
    screen: Res<State<Screen>>,
    session: Res<SettingsSession>,
    mut query: Query<&mut Node, With<SettingsUi>>,
) {
    let display = if session.open || *screen.get() == Screen::Settings {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in query.iter_mut() {
        node.display = display;
    }
}

/// Escape / pause binding closes the overlay. A still-open dropdown owns the
/// press first (see [`widgets::dropdown::close_dropdowns_on_escape`]).
pub(crate) fn close_settings_on_escape(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    binds: Res<GameSettingsResource>,
    mut session: ResMut<SettingsSession>,
    mut next_screen: ResMut<NextState<Screen>>,
    dropdowns: Query<&Dropdown>,
) {
    if !session.open {
        return;
    }
    if dropdowns.iter().any(|dropdown| dropdown.open) {
        return;
    }
    if !binds.just_pressed(KeyAction::TogglePause, &keys) {
        return;
    }
    let return_to = session.return_to;
    session.close();
    if return_to == SettingsReturn::Menu {
        next_screen.set(Screen::MainMenu);
    }
    binds.consume_press(KeyAction::TogglePause, &mut keys);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_state::init_screen_states;
    use crate::game_state::PauseOverlay;
    use bevy::input::ButtonInput;
    use bevymmo_client::user_settings::GameSettings;

    #[test]
    fn escape_from_pause_settings_closes_overlay_and_stays_paused() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(GameSettingsResource(GameSettings::default()));
        app.init_resource::<SettingsSession>();
        init_screen_states(&mut app);
        app.insert_state(Screen::InGame);
        app.update();
        app.world_mut()
            .resource_mut::<NextState<PauseOverlay>>()
            .set(PauseOverlay::On);
        app.update();
        app.world_mut()
            .resource_mut::<SettingsSession>()
            .open_from(SettingsReturn::Pause);

        app.add_systems(
            Update,
            close_settings_on_escape.before(crate::ui::systems::toggle_pause),
        );
        app.add_systems(Update, crate::ui::systems::toggle_pause);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        app.update();

        assert!(
            !app.world().resource::<SettingsSession>().open,
            "settings overlay must close"
        );
        assert_eq!(
            *app.world().resource::<State<Screen>>().get(),
            Screen::InGame
        );
        assert_eq!(
            *app.world().resource::<State<PauseOverlay>>().get(),
            PauseOverlay::On,
            "pause must stay on"
        );
        assert!(
            !app.world()
                .resource::<ButtonInput<KeyCode>>()
                .just_pressed(KeyCode::Escape),
            "the press must be consumed so pause does not toggle"
        );
    }
}
