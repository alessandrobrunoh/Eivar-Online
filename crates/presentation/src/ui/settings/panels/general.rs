//! General settings panel.
//!
//! Currently exposes:
//! - Language dropdown (single option: "English"; placeholder for i18n).
//! - Show FPS toggle (wired but only honored when a future FPS overlay
//!   consumes the value).

use bevy::prelude::*;

use crate::ui::theme::UiTheme;

use super::SettingsPanel;
use crate::ui::settings::state::{GameSettingsResource, SettingChoice, SettingToggle};
use crate::ui::settings::widgets::{
    dropdown::{spawn_select, DropdownItem},
    toggle::spawn_checkbox,
};
#[derive(Component)]
pub struct GeneralRoot;

pub fn spawn_general_panel(
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
                display: Display::Flex,
                ..default()
            },
            GeneralRoot,
            SettingsPanel::General,
        ))
        .id();
    commands.entity(parent).add_child(panel);

    let _ = spawn_select(
        commands,
        panel,
        SettingChoice::Language,
        "Language",
        vec![DropdownItem {
            label: "English".to_string(),
            value: "en".to_string(),
        }],
        "en",
        theme,
    );

    let scale_items = [0.75_f32, 1.0, 1.25, 1.5, 2.0]
        .into_iter()
        .map(|scale| DropdownItem {
            label: format!("{}x", scale),
            value: scale.to_string(),
        })
        .collect();
    let _ = spawn_select(
        commands,
        panel,
        SettingChoice::InterfaceScale,
        "Interface scale",
        scale_items,
        &settings.0.general.interface_scale.to_string(),
        theme,
    );

    let _ = spawn_checkbox(
        commands,
        panel,
        SettingToggle::ShowFps,
        "Show FPS overlay",
        settings.0.toggle(SettingToggle::ShowFps),
        theme,
    );

    panel
}
