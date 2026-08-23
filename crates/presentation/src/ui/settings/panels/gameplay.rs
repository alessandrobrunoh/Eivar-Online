//! Gameplay settings: combat-visual toggles.

use bevy::prelude::*;

use crate::ui::theme::UiTheme;

use super::SettingsPanel;
use crate::ui::settings::state::{GameSettingsResource, SettingToggle};
use crate::ui::settings::widgets::toggle::spawn_checkbox;

#[derive(Component)]
pub struct GameplayRoot;

pub fn spawn_gameplay_panel(
    commands: &mut Commands,
    parent: Entity,
    theme: &UiTheme,
    settings: &GameSettingsResource,
) -> Entity {
    let panel = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(24.0)),
                display: Display::None,
                ..default()
            },
            GameplayRoot,
            SettingsPanel::Gameplay,
        ))
        .id();
    commands.entity(parent).add_child(panel);

    let _ = spawn_checkbox(
        commands,
        panel,
        SettingToggle::ShowEnemyAbilityPreviews,
        "Show enemy ability previews",
        settings.0.toggle(SettingToggle::ShowEnemyAbilityPreviews),
        theme,
    );

    panel
}
