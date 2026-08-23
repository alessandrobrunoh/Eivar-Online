//! Settings shell: sidebar (tab buttons) + content area hosting the panels.
//!
//! The shell is spawned once at startup; panel visibility is driven by the
//! active tab stored in [`ActiveSettingsTab`]. The shell lives inside the
//! `SettingsUi` root and inherits its visibility toggling.

use bevy::prelude::*;

use crate::ui::button::{spawn_bar_button, spawn_button, UiButtonAction};
use crate::ui::text::spawn_text;
use crate::ui::theme::{
    ornate_menu_panel_content_node, spawn_menu_screen_background, spawn_ornate_settings_panel,
    UiTheme,
};

use crate::ui::settings::state::{GameSettingsResource, SettingsTab};

/// Resource: which tab is currently shown.
#[derive(Resource, Default, Clone, Copy)]
pub struct ActiveSettingsTab(pub SettingsTab);

/// Marker: a sidebar tab button. `tab` identifies the target panel.
#[derive(Component, Clone, Copy)]
pub struct SettingsTabButton {
    pub tab: SettingsTab,
}

/// Marker: content area that hosts the settings panels.
#[derive(Component)]
pub struct SettingsContentArea;

/// Spawns the whole settings shell (sidebar + content + bottom Back button)
/// under the given root entity. The panels are built by their respective
/// `spawn_*` functions.
pub fn spawn_settings_shell(
    commands: &mut Commands,
    root: Entity,
    theme: &UiTheme,
    settings: &GameSettingsResource,
    monitors: &Query<&bevy::window::Monitor>,
    asset_server: &AssetServer,
) {
    spawn_menu_screen_background(commands, root, asset_server);

    let frame = spawn_ornate_settings_panel(commands, root, asset_server);
    let inner = commands
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(16.0),
            ..ornate_menu_panel_content_node()
        })
        .id();
    commands.entity(frame).add_child(inner);

    // --- Sidebar -----------------------------------------------------------
    let sidebar = commands
        .spawn(Node {
            width: Val::Px(200.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(8.0),
            flex_shrink: 0.0,
            ..default()
        })
        .id();
    commands.entity(inner).add_child(sidebar);

    let _ = spawn_text(
        commands,
        sidebar,
        "Settings",
        theme.title_font_size * 0.55,
        theme.text_color,
    );

    for tab in SettingsTab::ALL {
        let button = spawn_bar_button(
            commands,
            sidebar,
            tab.label(),
            theme,
            SettingsTabButton { tab },
        );
        commands
            .entity(button)
            .entry::<Node>()
            .and_modify(|mut node| {
                node.width = Val::Percent(100.0);
                node.height = Val::Px(40.0);
            });
    }

    let spacer = commands
        .spawn(Node {
            flex_grow: 1.0,
            ..default()
        })
        .id();
    commands.entity(sidebar).add_child(spacer);

    let back = spawn_button(
        commands,
        sidebar,
        "Back",
        UiButtonAction::BackToMenu,
        theme,
        asset_server,
    );
    commands
        .entity(back)
        .entry::<Node>()
        .and_modify(|mut node| {
            node.width = Val::Percent(100.0);
        });

    // --- Content area ------------------------------------------------------
    let content_area = commands
        .spawn((
            Node {
                flex_grow: 1.0,
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Stretch,
                overflow: Overflow::clip_y(),
                ..default()
            },
            SettingsContentArea,
        ))
        .id();
    commands.entity(inner).add_child(content_area);

    // Non-default panels spawn hidden; `update_panel_visibility` runs on tab change.
    let _ = crate::ui::settings::panels::general::spawn_general_panel(
        commands,
        content_area,
        theme,
        settings,
    );
    let _ = crate::ui::settings::panels::graphics::spawn_graphics_panel(
        commands,
        content_area,
        theme,
        monitors,
        settings,
    );
    let _ = crate::ui::settings::panels::gameplay::spawn_gameplay_panel(
        commands,
        content_area,
        theme,
        settings,
    );
    let _ = crate::ui::settings::panels::keybinds::spawn_keybinds_panel(
        commands,
        content_area,
        theme,
        &settings.0.keybinds,
        asset_server,
    );
}
