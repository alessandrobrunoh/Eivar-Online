//! Systems wiring the settings UI to [`GameSettingsResource`] and to the rest
//! of the app (window, input consumers, persistence).
//!
//! The contract is one-way per widget:
//!
//! - **Click on widget** → mutate the widget component, then either emit an
//!   event (dropdown, key capture) or expose state for a follow-up system
//!   (toggle).
//! - **`apply_widget_events`** → translates widget changes into
//!   `GameSettingsResource` mutations. Single place where UI → settings.
//! - **`apply_graphics_to_window`** → pushes graphics settings to the primary
//!   `Window` when they change.
//! - **`persist_settings_when_changed`** → JSON save when the resource mutates.

use bevy::ecs::query::QueryFilter;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input::ButtonInput;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};

use super::layout::{ActiveSettingsTab, SettingsTabButton};
use super::panels::SettingsPanel;
use super::widgets::dropdown::DropdownChanged;
use super::widgets::key_capture::{KeyBindingChanged, KeyCapture, KeyCaptureValue};
use super::widgets::toggle::Toggle;
use crate::ui::button::{apply_button_image, UiButtonAction, UiButtonImages};
use crate::ui::settings::state::{
    save_settings, GameSettings, GameSettingsResource, KeyBinding, KeyModifiers, WindowMode,
};

// ===========================================================================
// Sidebar / tab switching
// ===========================================================================

/// Highlights the active sidebar tab button and dims the others.
pub fn update_tab_button_visuals(
    active: Res<ActiveSettingsTab>,
    mut buttons: Query<(
        &SettingsTabButton,
        &Interaction,
        &UiButtonImages,
        &mut ImageNode,
    )>,
) {
    for (button, interaction, images, mut image) in buttons.iter_mut() {
        let shown = if button.tab == active.0 {
            Interaction::Hovered
        } else {
            *interaction
        };
        apply_button_image(shown, &mut image, images);
    }
}

/// Click on a sidebar tab button → set [`ActiveSettingsTab`].
pub fn switch_tab_on_click(
    mut active: ResMut<ActiveSettingsTab>,
    interactions: Query<(&Interaction, &SettingsTabButton), Changed<Interaction>>,
) {
    for (interaction, button) in interactions.iter() {
        if *interaction == Interaction::Pressed {
            active.0 = button.tab;
        }
    }
}

// ===========================================================================
// Panel visibility
// ===========================================================================

/// Shows only the panel selected in the sidebar.
pub fn update_panel_visibility(
    active: Res<ActiveSettingsTab>,
    mut panels: Query<(&SettingsPanel, &mut Node)>,
) {
    for (panel, mut node) in panels.iter_mut() {
        node.display = if panel.matches(active.0) {
            Display::Flex
        } else {
            Display::None
        };
    }
}

// ===========================================================================
// Toggle
// ===========================================================================

/// Click on a toggle flips its state. Kit checkbox art is applied by
/// [`super::widgets::toggle::sync_toggle_visuals`].
pub fn toggle_on_click(mut query: Query<(&Interaction, &mut Toggle), Changed<Interaction>>) {
    for (interaction, mut toggle) in query.iter_mut() {
        if *interaction == Interaction::Pressed {
            toggle.on = !toggle.on;
        }
    }
}

// ===========================================================================
// Key capture
// ===========================================================================

/// Click on a key-capture button toggles capture mode and updates the label
/// to "Press a key…" / current binding.
pub fn toggle_key_capture_on_click(
    mut query: Query<(&Interaction, &mut KeyCapture, &Children), Changed<Interaction>>,
    mut value_texts: Query<&mut Text, With<KeyCaptureValue>>,
) {
    for (interaction, mut capture, children) in query.iter_mut() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        capture.capturing = !capture.capturing;
        let new_text = if capture.capturing {
            "Press a key…".to_string()
        } else {
            capture.binding.label()
        };
        update_descendant_text(&mut value_texts, children, &new_text);
    }
}

/// While any key-capture widget is in capture mode, the next non-modifier key
/// press becomes the new binding (modifiers are read from the held state).
/// `Escape` cancels capture without rebinding.
pub fn update_key_capture_input(
    mut events: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut captures: Query<(Entity, &mut KeyCapture, &Children)>,
    mut value_texts: Query<&mut Text, With<KeyCaptureValue>>,
    mut changed: MessageWriter<KeyBindingChanged>,
) {
    let mut cancel_requested = false;
    let mut main_key: Option<KeyCode> = None;

    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if is_modifier_key(ev.key_code) {
            continue;
        }
        if ev.key_code == KeyCode::Escape {
            cancel_requested = true;
            continue;
        }
        // First non-modifier, non-Escape key wins.
        if main_key.is_none() {
            main_key = Some(ev.key_code);
        }
    }

    if !cancel_requested && main_key.is_none() {
        return;
    }

    let modifiers = KeyModifiers::from_pressed(&keys);

    for (_entity, mut capture, children) in captures.iter_mut() {
        if !capture.capturing {
            continue;
        }
        if let Some(key) = main_key {
            let binding = KeyBinding { key, modifiers };
            capture.binding = binding;
            capture.capturing = false;
            update_descendant_text(&mut value_texts, children, &binding.label());
            changed.write(KeyBindingChanged {
                action: capture.action,
                binding,
            });
        } else if cancel_requested {
            capture.capturing = false;
            update_descendant_text(&mut value_texts, children, &capture.binding.label());
        }
    }
}

fn is_modifier_key(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::ShiftLeft
            | KeyCode::ShiftRight
            | KeyCode::ControlLeft
            | KeyCode::ControlRight
            | KeyCode::AltLeft
            | KeyCode::AltRight
            | KeyCode::SuperLeft
            | KeyCode::SuperRight
    )
}

/// Writes `new_text` into the first descendant (within `children`) text node
/// found via the value-texts query. The key-capture button has a single child
/// text node, so depth-1 search is enough.
fn update_descendant_text<F: QueryFilter>(
    value_texts: &mut Query<&mut Text, F>,
    children: &Children,
    new_text: &str,
) {
    for child in children.iter() {
        if let Ok(mut text) = value_texts.get_mut(child) {
            text.0 = new_text.to_string();
            return;
        }
    }
}

// ===========================================================================
// Apply widget changes → GameSettingsResource
// ===========================================================================

/// Single place where UI events turn into settings mutations.
pub fn apply_widget_events(
    mut dropdowns: MessageReader<DropdownChanged>,
    mut keybinds: MessageReader<KeyBindingChanged>,
    mut settings: ResMut<GameSettingsResource>,
    toggle_changes: Query<&Toggle, Changed<Toggle>>,
) {
    for ev in dropdowns.read() {
        let _ = settings.0.set_choice(ev.id, &ev.value);
    }

    for toggle in toggle_changes.iter() {
        settings.0.set_toggle(toggle.id, toggle.on);
    }

    for ev in keybinds.read() {
        settings.0.keybinds.bindings.insert(ev.action, ev.binding);
    }
}

/// "Reset to defaults" button → wipe all custom bindings and reset key-capture
/// widgets. Other actions are ignored here.
pub fn reset_keybinds_on_button(
    query: Query<(&Interaction, &crate::ui::button::UiButton), Changed<Interaction>>,
    mut settings: ResMut<GameSettingsResource>,
    mut captures: Query<(&mut KeyCapture, &Children)>,
    mut value_texts: Query<&mut Text, With<KeyCaptureValue>>,
) {
    let mut triggered = false;
    for (interaction, button) in query.iter() {
        if *interaction == Interaction::Pressed && button.action == UiButtonAction::ResetKeybinds {
            triggered = true;
        }
    }
    if !triggered {
        return;
    }
    settings.0.keybinds.bindings.clear();
    for (mut capture, children) in captures.iter_mut() {
        capture.binding = KeyBinding::bare(capture.action.default_binding());
        capture.capturing = false;
        update_descendant_text(&mut value_texts, children, &capture.binding.label());
    }
}

// ===========================================================================
// Apply GameSettingsResource → Window
// ===========================================================================

/// Pushes graphics settings to the primary window whenever they change.
pub fn apply_graphics_to_window(
    settings: Res<GameSettingsResource>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if !settings.is_changed() {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    let g = &settings.0.graphics;
    window.mode = g.mode.to_bevy();
    // Fullscreen modes own their surface size. Reassigning a windowed
    // resolution here on every settings change can shrink or letterbox the
    // fullscreen surface when an unrelated setting (for example Show FPS) is
    // toggled.
    if matches!(g.mode, WindowMode::Windowed) {
        window.resolution =
            bevy::window::WindowResolution::new(g.resolution.width, g.resolution.height);
    }
    window.present_mode = if g.vsync {
        bevy::window::PresentMode::AutoVsync
    } else {
        bevy::window::PresentMode::AutoNoVsync
    };
}

/// Applies the persisted interface scale to Bevy's UI scale resource.
pub fn apply_interface_scale(settings: Res<GameSettingsResource>, mut ui_scale: ResMut<UiScale>) {
    if settings.is_changed() {
        ui_scale.0 = settings.0.general.interface_scale.clamp(0.5, 3.0);
    }
}

// ===========================================================================
// Persistence
// ===========================================================================

/// Persists [`GameSettingsResource`] to disk whenever its fingerprint changes.
pub(crate) fn persist_settings_when_changed(
    settings: Res<GameSettingsResource>,
    mut last_saved: Local<Option<GameSettings>>,
) {
    // Change detection first: a clone + `PartialEq` is cheap compared to a
    // disk write, and settings change a handful of times per session.
    // Neighbours `apply_graphics_to_window` and `apply_interface_scale`
    // already guard on `is_changed()`.
    if !settings.is_changed() {
        return;
    }
    if last_saved.as_ref() == Some(&settings.0) {
        return;
    }
    if let Err(err) = save_settings(&settings.0) {
        bevy::log::warn!("Failed to save settings: {}", err);
        return;
    }
    *last_saved = Some(settings.0.clone());
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::settings::state::{KeyAction, SettingChoice, SettingToggle};

    #[test]
    fn apply_widget_events_writes_typed_toggle_and_choice() {
        let mut app = App::new();
        app.add_message::<DropdownChanged>();
        app.add_message::<KeyBindingChanged>();
        app.insert_resource(GameSettingsResource(GameSettings::default()));
        app.add_systems(Update, apply_widget_events);

        app.world_mut().write_message(DropdownChanged {
            id: SettingChoice::WindowMode,
            value: "borderless".into(),
        });
        app.world_mut().spawn(Toggle {
            id: SettingToggle::ShowEnemyAbilityPreviews,
            on: false,
        });
        app.update();

        let settings = &app.world().resource::<GameSettingsResource>().0;
        assert_eq!(settings.graphics.mode, WindowMode::Borderless);
        assert!(!settings.gameplay.show_enemy_ability_previews);
    }

    #[test]
    fn persist_skips_identical_snapshots() {
        let mut s1 = GameSettings::default();
        let mut s2 = GameSettings::default();
        s1.keybinds
            .bindings
            .insert(KeyAction::CastPrimary, KeyBinding::bare(KeyCode::KeyQ));
        s1.keybinds
            .bindings
            .insert(KeyAction::CastSecondary, KeyBinding::bare(KeyCode::KeyW));
        s2.keybinds
            .bindings
            .insert(KeyAction::CastSecondary, KeyBinding::bare(KeyCode::KeyW));
        s2.keybinds
            .bindings
            .insert(KeyAction::CastPrimary, KeyBinding::bare(KeyCode::KeyQ));
        assert_eq!(s1, s2);

        s2.set_toggle(SettingToggle::Vsync, false);
        assert_ne!(s1, s2);
    }
}
