//! Key-capture widget: shows the current binding and listens for a new key
//! when clicked.
//!
//! Capture state lives in the component itself. The system
//! [`crate::ui::settings::systems::update_key_capture_input`] reads raw
//! keyboard events for widgets in capture mode and mutates the widget, then
//! a separate event ([`KeyBindingChanged`]) propagates the change to the
//! settings resource.

use bevy::ecs::component::Component;
use bevy::ecs::message::Message;
use bevy::prelude::*;

use crate::ui::button::{
    apply_button_image, queue_bar_images, sliced_bar_image, BarButtonKind, UiButtonImages,
};
use crate::ui::theme::UiTheme;

use super::super::state::{KeyAction, KeyBinding};

/// A key-capture widget.
#[derive(Component, Clone)]
pub struct KeyCapture {
    /// Action being rebound.
    pub action: KeyAction,
    /// Current binding shown when not capturing.
    pub binding: KeyBinding,
    /// True while waiting for the next key press.
    pub capturing: bool,
}

#[derive(Component)]
pub struct KeyCaptureLabel;

#[derive(Component)]
pub struct KeyCaptureDisplay;

/// Binding label on the right of the row (`Q`, `Esc`, `Press a key…`).
#[derive(Component)]
pub struct KeyCaptureValue;

/// Event emitted when the user finishes capturing a new binding.
#[derive(Message, Clone, Debug)]
pub struct KeyBindingChanged {
    pub action: KeyAction,
    pub binding: KeyBinding,
}

/// Spawns a key-capture row (label + button showing current binding) and
/// returns its root entity.
pub fn spawn_key_capture(
    commands: &mut Commands,
    parent: Entity,
    action: KeyAction,
    binding: KeyBinding,
    theme: &UiTheme,
) -> Entity {
    let row = commands
        .spawn((
            Button,
            Node {
                width: Val::Percent(100.0),
                height: Val::Px(44.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                padding: UiRect::axes(Val::Px(28.0), Val::Px(6.0)),
                flex_shrink: 0.0,
                ..default()
            },
            sliced_bar_image(Handle::default()),
            UiButtonImages::placeholder(),
            KeyCapture {
                action,
                binding,
                capturing: false,
            },
            KeyCaptureDisplay,
        ))
        .id();
    commands.entity(parent).add_child(row);
    queue_bar_images(commands, row, BarButtonKind::Neutral);

    let label_entity = commands
        .spawn((
            Text::new(action.label().to_string()),
            TextFont {
                font_size: FontSize::Px(theme.input_font_size),
                ..default()
            },
            TextColor(theme.button_text_color),
            KeyCaptureLabel,
        ))
        .id();
    commands.entity(row).add_child(label_entity);

    let value_text = commands
        .spawn((
            Text::new(binding.label()),
            TextFont {
                font_size: FontSize::Px(theme.input_font_size),
                ..default()
            },
            TextColor(theme.muted_text_color),
            KeyCaptureValue,
        ))
        .id();
    commands.entity(row).add_child(value_text);

    row
}

/// Capturing stays on the active bar; otherwise hover follows the pointer.
pub fn sync_key_capture_visuals(
    mut captures: Query<
        (&KeyCapture, &Interaction, &UiButtonImages, &mut ImageNode),
        Or<(Changed<KeyCapture>, Changed<Interaction>)>,
    >,
) {
    for (capture, interaction, images, mut image) in &mut captures {
        let shown = if capture.capturing {
            Interaction::Hovered
        } else {
            *interaction
        };
        apply_button_image(shown, &mut image, images);
    }
}
