//! Pause overlay: Resume, Settings, and Return to Main Menu.
//!
//! Pure UI overlay: does not mutate `Time`, `FixedUpdate`, or network. Pause
//! is a [`PauseOverlay`] sub-state of InGame, managed by
//! [`crate::ui::systems::toggle_pause`] and by the buttons themselves.

use bevy::prelude::*;

use crate::game_state::{PauseOverlay, Screen};
use crate::ui::button::{spawn_button, UiButtonAction};
use crate::ui::settings::SettingsSession;
use crate::ui::text::spawn_text;
use crate::ui::theme::UiTheme;

/// Marker: pause overlay root.
#[derive(Component)]
pub struct PauseMenuUi;

pub struct PauseMenuPlugin;

impl Plugin for PauseMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_pause_menu);
        app.add_systems(
            Update,
            update_pause_menu_visibility.run_if(
                state_changed::<Screen>
                    .or_eager(state_changed::<PauseOverlay>)
                    .or_eager(resource_changed::<SettingsSession>),
            ),
        );
    }
}

fn setup_pause_menu(mut commands: Commands, theme: Res<UiTheme>, asset_server: Res<AssetServer>) {
    let backdrop = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                display: Display::None,
                ..default()
            },
            BackgroundColor(theme.panel_bg),
            PauseMenuUi,
        ))
        .id();

    let panel = commands
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            row_gap: Val::Px(16.0),
            padding: UiRect::all(Val::Px(32.0)),
            ..default()
        })
        .id();
    commands.entity(backdrop).add_child(panel);

    spawn_text(
        &mut commands,
        panel,
        "Paused",
        theme.title_font_size,
        theme.text_color,
    );

    spawn_button(
        &mut commands,
        panel,
        "Resume",
        UiButtonAction::Resume,
        &theme,
        &asset_server,
    );
    spawn_button(
        &mut commands,
        panel,
        "Settings",
        UiButtonAction::OpenSettings,
        &theme,
        &asset_server,
    );
    spawn_button(
        &mut commands,
        panel,
        "Leave Character",
        UiButtonAction::Logout,
        &theme,
        &asset_server,
    );
    spawn_button(
        &mut commands,
        panel,
        "Return to Main Menu",
        UiButtonAction::ReturnToMainMenu,
        &theme,
        &asset_server,
    );
}

fn update_pause_menu_visibility(
    pause: Option<Res<State<PauseOverlay>>>,
    settings: Option<Res<SettingsSession>>,
    mut query: Query<&mut Node, With<PauseMenuUi>>,
) {
    let settings_open = settings.is_some_and(|session| session.open);
    let display = if pause.is_some_and(|pause| *pause.get() == PauseOverlay::On) && !settings_open {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in query.iter_mut() {
        node.display = display;
    }
}
