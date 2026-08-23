//! Tema UI condiviso: colori e metriche tipografiche comuni.
//!
//! Risorsa volontariamente minimale — niente token annidati o tabelle di stili.

use bevy::prelude::*;

#[derive(Resource)]
pub struct UiTheme {
    pub text_color: Color,
    pub muted_text_color: Color,
    pub panel_bg: Color,
    pub screen_bg: Color,
    pub bar_bg: Color,
    pub hp_fill: Color,

    pub name_font_size: f32,
    pub hp_font_size: f32,
    pub scoreboard_title_size: f32,
    pub scoreboard_entry_size: f32,

    /// Pulsanti e input (menu/settings/pause).
    pub button_bg: Color,
    pub button_hovered_bg: Color,
    pub button_pressed_bg: Color,
    pub button_text_color: Color,
    pub input_bg: Color,
    pub input_border: Color,
    pub input_border_focused: Color,
    pub error_color: Color,

    pub title_font_size: f32,
    pub button_font_size: f32,
    pub input_font_size: f32,
}

impl Default for UiTheme {
    fn default() -> Self {
        Self {
            text_color: Color::WHITE,
            muted_text_color: Color::srgb(0.8, 0.8, 0.8),
            panel_bg: Color::srgba(0.1, 0.1, 0.1, 0.9),
            screen_bg: Color::srgb(0.055, 0.06, 0.075),
            bar_bg: Color::srgb(0.2, 0.2, 0.2),
            hp_fill: Color::srgb(0.8, 0.1, 0.1),

            name_font_size: 16.0,
            hp_font_size: 12.0,
            scoreboard_title_size: 24.0,
            scoreboard_entry_size: 18.0,

            button_bg: Color::srgb(0.18, 0.18, 0.22),
            button_hovered_bg: Color::srgb(0.28, 0.28, 0.34),
            button_pressed_bg: Color::srgb(0.10, 0.10, 0.14),
            button_text_color: Color::WHITE,
            input_bg: Color::srgb(0.12, 0.12, 0.16),
            input_border: Color::srgb(0.4, 0.4, 0.45),
            input_border_focused: Color::srgb(0.7, 0.7, 0.9),
            error_color: Color::srgb(0.95, 0.3, 0.3),

            title_font_size: 40.0,
            button_font_size: 20.0,
            input_font_size: 18.0,
        }
    }
}

/// Inset of the gold ornaments on `panel_large_left`. Used as a 9-slice
/// border so stretching a panel does not smear the corner gems.
const ORNATE_PANEL_SLICE: f32 = 88.0;

/// Title-screen splash used by the login and character-select roots.
const MAIN_MENU_BACKGROUND_PATH: &str = "ui/menu/title_screen.png";

const ORNATE_MENU_PANEL_PATH: &str = "ui/hud/panel_large_left.png";

/// Viewport fraction for login / character-select chrome. Clamped in px so
/// 800x600 does not fill the window and 1080p does not grow unbounded.
const MENU_PANEL_WIDTH_PERCENT: f32 = 36.0;
const MENU_PANEL_MIN_WIDTH: f32 = 320.0;
const MENU_PANEL_MAX_WIDTH: f32 = 520.0;
const MENU_PANEL_HEIGHT_PERCENT: f32 = 78.0;
const MENU_PANEL_MIN_HEIGHT: f32 = 440.0;
const MENU_PANEL_MAX_HEIGHT: f32 = 720.0;

/// Pixel inset to the dark inner field. Percent insets collapse on a short
/// panel and draw widgets onto the 88px 9-slice corners.
const MENU_PANEL_INSET_X: f32 = 56.0;
const MENU_PANEL_INSET_Y: f32 = 64.0;

/// 9-sliced ornate panel: corners stay at their native size.
pub fn ornate_panel_image(image: Handle<Image>) -> ImageNode {
    ImageNode::new(image).with_mode(NodeImageMode::Sliced(TextureSlicer {
        border: BorderRect::all(ORNATE_PANEL_SLICE),
        center_scale_mode: SliceScaleMode::Stretch,
        sides_scale_mode: SliceScaleMode::Stretch,
        max_corner_scale: 1.0,
    }))
}

/// Full-bleed background, taken out of flex flow so it does not shift the
/// menu column. `Stretch` fills the node; Bevy 0.19 has no cover-fit mode.
pub fn spawn_menu_screen_background(
    commands: &mut Commands,
    parent: Entity,
    asset_server: &AssetServer,
) {
    let background = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            ImageNode::new(asset_server.load(MAIN_MENU_BACKGROUND_PATH))
                .with_mode(NodeImageMode::Stretch),
        ))
        .id();
    commands.entity(parent).add_child(background);
}

/// Login / character-select root: left column, vertically centered so a
/// short window keeps top and bottom margin around the ornate frame.
pub fn menu_screen_root_node() -> Node {
    Node {
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        justify_content: JustifyContent::FlexStart,
        align_items: AlignItems::Center,
        padding: UiRect::left(Val::Percent(4.0)),
        ..default()
    }
}

/// Wide ornate frame used by the settings overlay. Login stays narrow;
/// settings needs a sidebar plus a 520 px content column.
pub fn ornate_settings_panel_node() -> Node {
    Node {
        width: Val::Percent(78.0),
        min_width: Val::Px(700.0),
        max_width: Val::Px(1040.0),
        height: Val::Percent(82.0),
        min_height: Val::Px(500.0),
        max_height: Val::Px(820.0),
        position_type: PositionType::Relative,
        ..default()
    }
}

/// Viewport-relative ornate panel. `min_width` 320 (not 380) so 36% of
/// 800px is not forced to almost half the window.
pub fn ornate_menu_panel_node() -> Node {
    Node {
        width: Val::Percent(MENU_PANEL_WIDTH_PERCENT),
        min_width: Val::Px(MENU_PANEL_MIN_WIDTH),
        max_width: Val::Px(MENU_PANEL_MAX_WIDTH),
        height: Val::Percent(MENU_PANEL_HEIGHT_PERCENT),
        min_height: Val::Px(MENU_PANEL_MIN_HEIGHT),
        max_height: Val::Px(MENU_PANEL_MAX_HEIGHT),
        position_type: PositionType::Relative,
        ..default()
    }
}

/// Inner overlay sized by opposite-edge insets (`width`/`height` Auto).
pub fn ornate_menu_panel_content_node() -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(MENU_PANEL_INSET_X),
        right: Val::Px(MENU_PANEL_INSET_X),
        top: Val::Px(MENU_PANEL_INSET_Y),
        bottom: Val::Px(MENU_PANEL_INSET_Y),
        width: Val::Auto,
        height: Val::Auto,
        flex_direction: FlexDirection::Column,
        ..default()
    }
}

/// Spawns the 9-sliced login / character-select frame on `parent`.
pub fn spawn_ornate_menu_panel(
    commands: &mut Commands,
    parent: Entity,
    asset_server: &AssetServer,
) -> Entity {
    spawn_ornate_panel(commands, parent, asset_server, ornate_menu_panel_node())
}

/// Spawns the wide 9-sliced frame used by settings.
pub fn spawn_ornate_settings_panel(
    commands: &mut Commands,
    parent: Entity,
    asset_server: &AssetServer,
) -> Entity {
    spawn_ornate_panel(commands, parent, asset_server, ornate_settings_panel_node())
}

fn spawn_ornate_panel(
    commands: &mut Commands,
    parent: Entity,
    asset_server: &AssetServer,
    node: Node,
) -> Entity {
    let panel = commands
        .spawn((
            node,
            ornate_panel_image(asset_server.load(ORNATE_MENU_PANEL_PATH)),
        ))
        .id();
    commands.entity(parent).add_child(panel);
    panel
}
