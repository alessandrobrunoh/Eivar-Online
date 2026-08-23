//! Graphics settings panel.
//!
//! Exposes window mode, resolution, and vsync. The available resolutions are
//! derived from the primary monitor at spawn time.

use bevy::prelude::*;
use bevy::window::Monitor;

use crate::ui::theme::UiTheme;

use super::SettingsPanel;
use crate::ui::settings::state::{
    GameSettingsResource, Resolution, SettingChoice, SettingToggle, WindowMode,
};
use crate::ui::settings::widgets::dropdown::{spawn_select, DropdownItem};
use crate::ui::settings::widgets::toggle::spawn_checkbox;

#[derive(Component)]
pub struct GraphicsRoot;

pub fn spawn_graphics_panel(
    commands: &mut Commands,
    parent: Entity,
    theme: &UiTheme,
    monitors: &Query<&Monitor>,
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
            GraphicsRoot,
            SettingsPanel::Graphics,
        ))
        .id();
    commands.entity(parent).add_child(panel);

    // Window mode dropdown.
    let mode_items = vec![
        DropdownItem {
            label: "Windowed".to_string(),
            value: "windowed".to_string(),
        },
        DropdownItem {
            label: "Borderless Fullscreen".to_string(),
            value: "borderless".to_string(),
        },
        DropdownItem {
            label: "Exclusive Fullscreen".to_string(),
            value: "exclusive".to_string(),
        },
    ];
    let _ = spawn_select(
        commands,
        panel,
        SettingChoice::WindowMode,
        "Window Mode",
        mode_items,
        match settings.0.graphics.mode {
            WindowMode::Windowed => "windowed",
            WindowMode::Borderless => "borderless",
            WindowMode::Exclusive => "exclusive",
        },
        theme,
    );

    // Resolution dropdown: built from the primary monitor's video modes,
    // falling back to a small hardcoded list when no monitor is available
    // (headless test, no window yet, etc.).
    let res_items = available_resolutions(monitors);
    let current_res_label = settings.0.graphics.resolution.label();
    let _ = spawn_select(
        commands,
        panel,
        SettingChoice::Resolution,
        "Resolution",
        res_items,
        &current_res_label,
        theme,
    );

    // VSync toggle.
    let _ = spawn_checkbox(
        commands,
        panel,
        SettingToggle::Vsync,
        "V-Sync",
        settings.0.toggle(SettingToggle::Vsync),
        theme,
    );

    panel
}

/// Returns the resolutions supported by the primary monitor, sorted largest
/// first. Falls back to a small common list when no monitor is present.
fn available_resolutions(monitors: &Query<&Monitor>) -> Vec<DropdownItem> {
    let mut seen: Vec<(u32, u32)> = Vec::new();

    if let Some(monitor) = monitors.iter().next() {
        for mode in monitor.video_modes.iter() {
            let res = (mode.physical_size.x, mode.physical_size.y);
            if !seen.contains(&res) {
                seen.push(res);
            }
        }
    }

    if seen.is_empty() {
        seen = vec![
            (1920, 1080),
            (1600, 900),
            (1280, 720),
            (1024, 768),
            (800, 600),
        ];
    }

    // Sort by total pixel count, largest first.
    seen.sort_by_key(|&(w, h)| std::cmp::Reverse(w as u64 * h as u64));

    seen.into_iter()
        .map(|(w, h)| DropdownItem {
            label: format!("{}x{}", w, h),
            value: Resolution::new(w, h).label(),
        })
        .collect()
}
