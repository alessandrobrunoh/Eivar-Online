//! Toggle widget: kit checkbox on an ornate bar row.

use bevy::ecs::component::Component;
use bevy::prelude::*;

use crate::ui::button::{queue_bar_images, sliced_bar_image, BarButtonKind, UiButtonImages};
use crate::ui::settings::state::SettingToggle;
use crate::ui::theme::UiTheme;

const CHECKBOX_EMPTY: &str = "ui/kit/checkbox_gold_empty.png";
const CHECKBOX_CHECKED: &str = "ui/kit/checkbox_gold_checked.png";
const CHECKBOX_EMPTY_HOVER: &str = "ui/kit/checkbox_blue_glow_empty.png";
const CHECKBOX_CHECKED_HOVER: &str = "ui/kit/checkbox_blue_glow_checked.png";

const CHECKBOX_SIZE: f32 = 36.0;
const ROW_HEIGHT: f32 = 44.0;

/// A labeled on/off toggle.
#[derive(Component, Clone)]
pub struct Toggle {
    /// Stable identifier used by the caller to dispatch the change.
    pub id: SettingToggle,
    pub on: bool,
}

#[derive(Component)]
pub struct ToggleLabel;

/// Checkbox image that follows on/off (and hover glow).
#[derive(Component)]
pub struct ToggleDisplay;

/// Idle / checked / hover textures for a [`ToggleDisplay`].
#[derive(Component, Clone)]
pub struct ToggleImages {
    pub empty: Handle<Image>,
    pub checked: Handle<Image>,
    pub empty_hover: Handle<Image>,
    pub checked_hover: Handle<Image>,
}

impl ToggleImages {
    pub fn load(asset_server: &AssetServer) -> Self {
        Self {
            empty: asset_server.load(CHECKBOX_EMPTY),
            checked: asset_server.load(CHECKBOX_CHECKED),
            empty_hover: asset_server.load(CHECKBOX_EMPTY_HOVER),
            checked_hover: asset_server.load(CHECKBOX_CHECKED_HOVER),
        }
    }

    fn placeholder() -> Self {
        Self {
            empty: Handle::default(),
            checked: Handle::default(),
            empty_hover: Handle::default(),
            checked_hover: Handle::default(),
        }
    }

    fn from_world(world: &World) -> Self {
        match world.get_resource::<AssetServer>() {
            Some(server) => Self::load(server),
            None => Self::placeholder(),
        }
    }
}

/// Checkbox control used by settings panels.
pub type CheckBox = Toggle;

pub fn checkbox_image(on: bool, hovered: bool, images: &ToggleImages) -> Handle<Image> {
    match (on, hovered) {
        (true, true) => images.checked_hover.clone(),
        (true, false) => images.checked.clone(),
        (false, true) => images.empty_hover.clone(),
        (false, false) => images.empty.clone(),
    }
}

/// Applies the checkbox texture for the current on/hover pair.
pub fn apply_toggle_visual(on: bool, hovered: bool, images: &ToggleImages, image: &mut ImageNode) {
    image.image = checkbox_image(on, hovered, images);
}

/// Spawns a labeled switch row and returns its root button entity.
pub fn spawn_toggle(
    commands: &mut Commands,
    parent: Entity,
    id: SettingToggle,
    label: impl Into<String>,
    on: bool,
    theme: &UiTheme,
) -> Entity {
    let button = commands
        .spawn((
            Button,
            Node {
                width: Val::Percent(100.0),
                height: Val::Px(ROW_HEIGHT),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(12.0),
                padding: UiRect::axes(Val::Px(28.0), Val::Px(4.0)),
                flex_shrink: 0.0,
                ..default()
            },
            sliced_bar_image(Handle::default()),
            UiButtonImages::placeholder(),
            Toggle { id, on },
        ))
        .id();
    commands.entity(parent).add_child(button);
    queue_bar_images(commands, button, BarButtonKind::Neutral);

    let label_entity = commands
        .spawn((
            Text::new(label.into()),
            TextFont {
                font_size: FontSize::Px(theme.input_font_size),
                ..default()
            },
            TextColor(theme.button_text_color),
            ToggleLabel,
        ))
        .id();
    commands.entity(button).add_child(label_entity);

    let display = commands
        .spawn((
            Node {
                width: Val::Px(CHECKBOX_SIZE),
                height: Val::Px(CHECKBOX_SIZE),
                margin: UiRect {
                    left: Val::Auto,
                    ..default()
                },
                flex_shrink: 0.0,
                ..default()
            },
            ImageNode::new(Handle::default()),
            ToggleImages::placeholder(),
            ToggleDisplay,
        ))
        .id();
    commands.entity(button).add_child(display);
    queue_toggle_images(commands, display, on);

    button
}

fn queue_toggle_images(commands: &mut Commands, entity: Entity, on: bool) {
    commands.queue(move |world: &mut World| {
        let images = ToggleImages::from_world(world);
        let handle = checkbox_image(on, false, &images);
        world
            .entity_mut(entity)
            .insert((ImageNode::new(handle), images));
    });
}

/// Spawns a [`CheckBox`] control.
pub fn spawn_checkbox(
    commands: &mut Commands,
    parent: Entity,
    id: SettingToggle,
    label: impl Into<String>,
    on: bool,
    theme: &UiTheme,
) -> Entity {
    spawn_toggle(commands, parent, id, label, on, theme)
}

/// Copies [`Toggle::on`] + hover onto the kit checkbox image.
pub fn sync_toggle_visuals(
    toggles: Query<(&Toggle, &Interaction, &Children), Or<(Changed<Toggle>, Changed<Interaction>)>>,
    mut displays: Query<(&ToggleImages, &mut ImageNode), With<ToggleDisplay>>,
) {
    for (toggle, interaction, children) in &toggles {
        let hovered = matches!(*interaction, Interaction::Hovered | Interaction::Pressed);
        for child in children {
            if let Ok((images, mut image)) = displays.get_mut(*child) {
                apply_toggle_visual(toggle.on, hovered, images, &mut image);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::UiTheme;

    #[test]
    fn spawn_puts_an_image_node_on_the_checkbox() {
        let mut world = World::new();
        world.insert_resource(UiTheme::default());
        let parent = world.spawn(Node::default()).id();
        world.resource_scope(|world, theme: Mut<UiTheme>| {
            let mut commands = world.commands();
            spawn_toggle(
                &mut commands,
                parent,
                SettingToggle::Vsync,
                "V-Sync",
                true,
                &theme,
            );
        });
        world.flush();

        let has_image = world
            .query_filtered::<&ImageNode, With<ToggleDisplay>>()
            .iter(&world)
            .next()
            .is_some();
        assert!(
            has_image,
            "kit checkbox must be an ImageNode, not a color fill"
        );
    }
}
