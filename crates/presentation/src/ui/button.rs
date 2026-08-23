//! Reusable UI button.
//!
//! The button pairs a semantic action ([`UiButtonAction`]) with an
//! interactive node; the `action -> effect` mapping lives in the central UI systems
//! (`crate::ui::systems::update_button_actions`) and not in the component.
//!
//! Visuals are the 9-sliced ornate bars used on the character roster
//! (`bar_blue_left_01` at rest, `bar_blue_right_active` on hover/press).

use bevy::prelude::*;

use crate::ui::theme::UiTheme;

/// Effect triggered by pressing the button.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiButtonAction {
    Play,
    Login,
    Register,
    OpenRegister,
    OpenLogin,
    OpenSettings,
    BackToMenu,
    Resume,
    ReturnToMainMenu,
    /// Pause menu's "Leave Character": returns to character select without
    /// logging out of the account.
    Logout,
    /// Character-select screen's "Logout": ends the account session.
    LogoutAccount,
    Exit,
    /// Settings → Keybinds → "Reset to defaults".
    ResetKeybinds,
}

/// UI Button with associated semantic action.
#[derive(Component)]
pub struct UiButton {
    pub action: UiButtonAction,
}

/// Play / primary CTA at rest.
pub const BAR_BUTTON_DEFAULT_PATH: &str = "ui/hud/bar_blue_left_01.png";
/// Hover / pressed glow.
pub const BAR_BUTTON_HOVER_PATH: &str = "ui/hud/bar_blue_right_active.png";
/// Neutral / secondary (Close, Unequip, Delete-like) at rest.
pub const BAR_BUTTON_NEUTRAL_PATH: &str = "ui/hud/bar_neutral_right_01.png";

const BUTTON_WIDTH: f32 = 230.0;
const BUTTON_HEIGHT: f32 = 44.0;
/// Matches the roster Play/Delete chips.
const COMPACT_BUTTON_WIDTH: f32 = 92.0;
const COMPACT_BUTTON_HEIGHT: f32 = 30.0;
/// Inset that keeps the bar-end ornaments from stretching.
const BAR_SLICE: [f32; 2] = [24.0, 10.0];

/// Which bar art to use at rest. Hover/press always swap to the active glow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarButtonKind {
    /// Blue Play bar (`bar_blue_left_01`).
    Primary,
    /// Dark Delete bar (`bar_neutral_right_01`).
    Neutral,
}

/// Textures used by the three button interaction states.
#[derive(Component, Clone)]
pub struct UiButtonImages {
    pub default: Handle<Image>,
    pub hover: Handle<Image>,
    pub clicked: Handle<Image>,
}

impl UiButtonImages {
    /// Primary CTA: blue bar, glow on hover/press.
    pub fn load(asset_server: &AssetServer) -> Self {
        Self::load_kind(asset_server, BarButtonKind::Primary)
    }

    /// Secondary action: neutral bar, glow on hover/press.
    pub fn load_neutral(asset_server: &AssetServer) -> Self {
        Self::load_kind(asset_server, BarButtonKind::Neutral)
    }

    pub fn load_kind(asset_server: &AssetServer, kind: BarButtonKind) -> Self {
        let default_path = match kind {
            BarButtonKind::Primary => BAR_BUTTON_DEFAULT_PATH,
            BarButtonKind::Neutral => BAR_BUTTON_NEUTRAL_PATH,
        };
        Self {
            default: asset_server.load(default_path),
            hover: asset_server.load(BAR_BUTTON_HOVER_PATH),
            clicked: asset_server.load(BAR_BUTTON_HOVER_PATH),
        }
    }

    pub(crate) fn placeholder() -> Self {
        Self {
            default: Handle::default(),
            hover: Handle::default(),
            clicked: Handle::default(),
        }
    }

    fn from_world(world: &World, kind: BarButtonKind) -> Self {
        match world.get_resource::<AssetServer>() {
            Some(server) => Self::load_kind(server, kind),
            None => Self::placeholder(),
        }
    }
}

/// 9-sliced ornate bar. Corners stay native size; the center stretches.
pub fn sliced_bar_image(image: Handle<Image>) -> ImageNode {
    ImageNode::new(image).with_mode(NodeImageMode::Sliced(TextureSlicer {
        border: BorderRect::axes(BAR_SLICE[0], BAR_SLICE[1]),
        center_scale_mode: SliceScaleMode::Stretch,
        sides_scale_mode: SliceScaleMode::Stretch,
        max_corner_scale: 1.0,
    }))
}

/// Swaps the displayed bar texture to match `interaction`.
pub fn apply_button_image(
    interaction: Interaction,
    image: &mut ImageNode,
    images: &UiButtonImages,
) {
    image.image = match interaction {
        Interaction::None => images.default.clone(),
        Interaction::Hovered => images.hover.clone(),
        Interaction::Pressed => images.clicked.clone(),
    };
}

/// Spawns a full-size ornate bar button wired to [`UiButtonAction`].
///
/// Returns the button entity (useful for testing or future references).
pub fn spawn_button(
    commands: &mut Commands,
    parent: Entity,
    label: impl Into<String>,
    action: UiButtonAction,
    theme: &UiTheme,
    asset_server: &AssetServer,
) -> Entity {
    spawn_sized_action_button(
        commands,
        parent,
        label,
        action,
        theme,
        asset_server,
        BUTTON_WIDTH,
        BUTTON_HEIGHT,
        theme.button_font_size,
    )
}

/// Compact Play/Delete-sized bar (`~92×30`), still wired to [`UiButtonAction`].
pub fn spawn_compact_button(
    commands: &mut Commands,
    parent: Entity,
    label: impl Into<String>,
    action: UiButtonAction,
    theme: &UiTheme,
    asset_server: &AssetServer,
) -> Entity {
    spawn_sized_action_button(
        commands,
        parent,
        label,
        action,
        theme,
        asset_server,
        COMPACT_BUTTON_WIDTH,
        COMPACT_BUTTON_HEIGHT,
        theme.button_font_size * 0.55,
    )
}

/// Full-size ornate bar that is *not* wired to [`UiButtonAction`] (Respawn, …).
///
/// Loads textures from [`AssetServer`] when the resource exists (production);
/// tests without it still get a sliced `ImageNode` and [`UiButtonImages`].
pub fn spawn_bar_button(
    commands: &mut Commands,
    parent: Entity,
    label: impl Into<String>,
    theme: &UiTheme,
    extras: impl Bundle,
) -> Entity {
    let entity = spawn_bar_node(
        commands,
        parent,
        label.into(),
        theme.button_font_size,
        theme.button_text_color,
        Val::Px(BUTTON_WIDTH),
        Val::Px(BUTTON_HEIGHT),
        UiButtonImages::placeholder(),
        extras,
    );
    queue_bar_images(commands, entity, BarButtonKind::Primary);
    entity
}

/// Compact ornate bar as a child of a `ChildSpawnerCommands` tree (card Close,
/// inventory Equip/Unequip). Same art as [`spawn_button`]; hover/press swap
/// via [`UiButtonImages`].
pub fn spawn_bar_child(
    parent: &mut ChildSpawnerCommands,
    label: impl Into<String>,
    font_size: f32,
    text_color: Color,
    width: Val,
    height: Val,
    kind: BarButtonKind,
    extras: impl Bundle,
) -> Entity {
    let entity = parent
        .spawn((
            Button,
            bar_node(width, height),
            sliced_bar_image(Handle::default()),
            UiButtonImages::placeholder(),
            extras,
        ))
        .with_children(|button| {
            spawn_label(button, label.into(), font_size, text_color);
        })
        .id();
    parent.commands().queue(move |world: &mut World| {
        insert_bar_images(world, entity, kind);
    });
    entity
}

fn spawn_sized_action_button(
    commands: &mut Commands,
    parent: Entity,
    label: impl Into<String>,
    action: UiButtonAction,
    theme: &UiTheme,
    asset_server: &AssetServer,
    width: f32,
    height: f32,
    font_size: f32,
) -> Entity {
    spawn_bar_node(
        commands,
        parent,
        label.into(),
        font_size,
        theme.button_text_color,
        Val::Px(width),
        Val::Px(height),
        UiButtonImages::load(asset_server),
        UiButton { action },
    )
}

fn spawn_bar_node(
    commands: &mut Commands,
    parent: Entity,
    label: String,
    font_size: f32,
    text_color: Color,
    width: Val,
    height: Val,
    images: UiButtonImages,
    extras: impl Bundle,
) -> Entity {
    let button = commands
        .spawn((
            Button,
            bar_node(width, height),
            sliced_bar_image(images.default.clone()),
            images,
            extras,
        ))
        .id();
    commands.entity(parent).add_child(button);

    let label_entity = commands
        .spawn((
            Text::new(label),
            TextFont {
                font_size: FontSize::Px(font_size),
                ..default()
            },
            TextColor(text_color),
        ))
        .id();
    commands.entity(button).add_child(label_entity);

    button
}

fn bar_node(width: Val, height: Val) -> Node {
    Node {
        width,
        height,
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        flex_shrink: 0.0,
        ..default()
    }
}

fn spawn_label(
    parent: &mut ChildSpawnerCommands,
    label: String,
    font_size: f32,
    text_color: Color,
) {
    parent.spawn((
        Text::new(label),
        TextFont {
            font_size: FontSize::Px(font_size),
            ..default()
        },
        TextColor(text_color),
    ));
}

/// Loads ornate bar textures onto `entity` once `AssetServer` is available.
pub fn queue_bar_images(commands: &mut Commands, entity: Entity, kind: BarButtonKind) {
    commands.queue(move |world: &mut World| {
        insert_bar_images(world, entity, kind);
    });
}

fn insert_bar_images(world: &mut World, entity: Entity, kind: BarButtonKind) {
    let images = UiButtonImages::from_world(world, kind);
    world
        .entity_mut(entity)
        .insert((sliced_bar_image(images.default.clone()), images));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_asset::<Image>();
        app.init_resource::<UiTheme>();
        app
    }

    fn spawn_with(
        app: &mut App,
        spawn: impl FnOnce(&mut Commands, Entity, &UiTheme, &AssetServer) -> Entity,
    ) -> Entity {
        let theme = UiTheme::default();
        let parent = app.world_mut().spawn(Node::default()).id();
        let asset_server = app.world().resource::<AssetServer>().clone();
        let entity = {
            let mut commands = app.world_mut().commands();
            spawn(&mut commands, parent, &theme, &asset_server)
        };
        app.world_mut().flush();
        entity
    }

    fn assert_sliced(app: &App, entity: Entity) {
        let image = app.world().get::<ImageNode>(entity).expect("ImageNode");
        assert!(
            matches!(image.image_mode, NodeImageMode::Sliced(_)),
            "bar buttons must 9-slice so they scale without smearing ornaments"
        );
        assert!(app.world().get::<UiButtonImages>(entity).is_some());
    }

    #[test]
    fn spawn_button_keeps_action_and_sliced_bar() {
        let mut app = test_app();
        let entity = spawn_with(&mut app, |commands, parent, theme, assets| {
            spawn_button(
                commands,
                parent,
                "Login",
                UiButtonAction::Login,
                theme,
                assets,
            )
        });

        let button = app.world().get::<UiButton>(entity).expect("UiButton");
        assert_eq!(button.action, UiButtonAction::Login);
        let node = app.world().get::<Node>(entity).expect("Node");
        assert_eq!(node.width, Val::Px(BUTTON_WIDTH));
        assert_eq!(node.height, Val::Px(BUTTON_HEIGHT));
        assert_sliced(&app, entity);
    }

    #[test]
    fn spawn_compact_button_is_roster_sized() {
        let mut app = test_app();
        let entity = spawn_with(&mut app, |commands, parent, theme, assets| {
            spawn_compact_button(
                commands,
                parent,
                "Play",
                UiButtonAction::Play,
                theme,
                assets,
            )
        });

        let node = app.world().get::<Node>(entity).expect("Node");
        assert_eq!(node.width, Val::Px(COMPACT_BUTTON_WIDTH));
        assert_eq!(node.height, Val::Px(COMPACT_BUTTON_HEIGHT));
        assert_sliced(&app, entity);
    }

    #[test]
    fn spawn_bar_button_works_without_asset_server() {
        let mut app = App::new();
        app.init_resource::<UiTheme>();
        let theme = UiTheme::default();
        let parent = app.world_mut().spawn(Node::default()).id();
        let entity = {
            let mut commands = app.world_mut().commands();
            spawn_bar_button(&mut commands, parent, "Respawn", &theme, ())
        };
        app.world_mut().flush();

        assert_sliced(&app, entity);
        assert!(app.world().get::<UiButton>(entity).is_none());
    }
}
