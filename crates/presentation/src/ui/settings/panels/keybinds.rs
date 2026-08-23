//! Keybinds panel: one `KeyCapture` row per [`KeyAction`], plus a "Reset to
//! defaults" button.

use bevy::prelude::*;

use crate::ui::button::{spawn_button, UiButtonAction};
use crate::ui::theme::UiTheme;

use super::SettingsPanel;
use crate::ui::settings::state::{KeyAction, KeybindSettings};
use crate::ui::settings::widgets::key_capture::spawn_key_capture;

#[derive(Component)]
pub struct KeybindsRoot;

use crate::ui::scrollbar::spawn_scroll_view;

pub fn spawn_keybinds_panel(
    commands: &mut Commands,
    parent: Entity,
    theme: &UiTheme,
    keybinds: &KeybindSettings,
    asset_server: &AssetServer,
) -> Entity {
    let scroll_wrapper = spawn_scroll_view(commands, parent, theme, |commands| {
        let panel = commands
            .spawn((Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(8.0),
                padding: UiRect::all(Val::Px(24.0)),
                ..default()
            },))
            .id();

        for action in KeyAction::ALL {
            let binding = keybinds.get(action);
            let _ = spawn_key_capture(commands, panel, action, binding, theme);
        }

        let _ = spawn_button(
            commands,
            panel,
            "Reset to defaults",
            UiButtonAction::ResetKeybinds,
            theme,
            asset_server,
        );

        panel
    });

    commands
        .entity(scroll_wrapper)
        .insert((KeybindsRoot, SettingsPanel::Keybinds))
        .entry::<Node>()
        .and_modify(|mut node| node.display = Display::None);

    scroll_wrapper
}
