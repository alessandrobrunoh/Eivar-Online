//! User-facing game settings: graphics, keybinds, and general preferences.
//!
//! Pure data + serialization. Lives in `bevymmo_shared` so that both the
//! client runtime (e.g. `targeting`) and the presentation layer can read the
//! same resource. UI-specific types (panels, widgets, etc.) stay in
//! `bevymmo_presentation::ui::settings`.
//!
//! Persistence: JSON at `<user_config_dir>/bevymmo/settings.json`.

use std::collections::HashMap;
use std::path::PathBuf;

use bevy::input::keyboard::KeyCode;
use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Graphics
// ---------------------------------------------------------------------------

/// Window mode selectable from the graphics panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    /// OS-decorated, resizable window.
    #[default]
    Windowed,
    /// Fullscreen borderless window covering the whole desktop.
    Borderless,
    /// Exclusive fullscreen (changes video mode).
    Exclusive,
}

impl WindowMode {
    pub fn to_bevy(self) -> bevy::window::WindowMode {
        // In Bevy 0.19 fullscreen variants take a MonitorSelection (and
        // exclusive fullscreen also a VideoModeSelection). We default to the
        // primary monitor + the current video mode — the safest choice for a
        // settings dropdown that doesn't yet expose per-monitor selection.
        use bevy::window::{MonitorSelection, VideoModeSelection};
        match self {
            Self::Windowed => bevy::window::WindowMode::Windowed,
            Self::Borderless => {
                bevy::window::WindowMode::BorderlessFullscreen(MonitorSelection::Primary)
            }
            Self::Exclusive => bevy::window::WindowMode::Fullscreen(
                MonitorSelection::Primary,
                VideoModeSelection::Current,
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn label(self) -> String {
        format!("{}x{}", self.width, self.height)
    }

    /// Parses `"1920x1080"`. Missing `x` or non-numeric sides → `None`.
    pub fn parse_label(label: &str) -> Option<Self> {
        let (w_str, h_str) = label.split_once('x')?;
        Some(Self::new(w_str.parse().ok()?, h_str.parse().ok()?))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphicsSettings {
    pub mode: WindowMode,
    /// Active resolution. For borderless/exclusive this matches the chosen
    /// monitor's resolution; for windowed it is the inner window size.
    pub resolution: Resolution,
    pub vsync: bool,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self {
            mode: WindowMode::Windowed,
            resolution: Resolution::new(1280, 720),
            vsync: true,
        }
    }
}

// ---------------------------------------------------------------------------
// General
// ---------------------------------------------------------------------------

/// General preferences. `language` is stored as ISO 639-1 but only "en" is
/// honored today (i18n not yet implemented).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeneralSettings {
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_interface_scale")]
    pub interface_scale: f32,
    #[serde(default)]
    pub show_fps: bool,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_interface_scale() -> f32 {
    1.0
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            language: default_language(),
            interface_scale: default_interface_scale(),
            show_fps: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Gameplay (combat visuals)
// ---------------------------------------------------------------------------

fn default_true() -> bool {
    true
}

/// Combat-visual preferences. Graphics stays window/vsync; these gate what
/// the client draws during a fight.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameplaySettings {
    /// Ground telegraph and lingering area of hostile casters. Own aim
    /// gizmos and impact VFX are not gated by this.
    #[serde(default = "default_true")]
    pub show_enemy_ability_previews: bool,
}

impl Default for GameplaySettings {
    fn default() -> Self {
        Self {
            show_enemy_ability_previews: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Typed setting ids (UI → resource)
// ---------------------------------------------------------------------------

/// On/off setting. Widgets store this instead of a magic string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingToggle {
    Vsync,
    ShowFps,
    ShowEnemyAbilityPreviews,
}

/// Multi-choice setting. Widgets store this instead of a magic string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingChoice {
    WindowMode,
    Resolution,
    Language,
    InterfaceScale,
}

// ---------------------------------------------------------------------------
// Keybinds
// ---------------------------------------------------------------------------

/// Modifier flags for a key binding. Booleans rather than `KeyCode`s because
/// left/right (e.g. `ShiftLeft`/`ShiftRight`) are merged into a single flag:
/// players think in terms of "Shift", not which side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Super = Windows key on PC, Command on macOS.
    pub super_key: bool,
}

impl KeyModifiers {
    /// Returns the modifier flags currently held, normalizing left/right pairs.
    pub fn from_pressed(keys: &bevy::input::ButtonInput<KeyCode>) -> Self {
        Self {
            shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
            ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
            alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
            super_key: keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight),
        }
    }

    /// Human-readable prefix, e.g. "Ctrl+Shift+". Empty if no modifiers.
    pub fn label(self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.super_key {
            parts.push("Super");
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("{}+", parts.join("+"))
        }
    }
}

/// A user-facing, rebindable keyboard action.
///
/// Order of variants = order shown in the keybinds panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    TogglePause,
    ShowScoreboard,
    ToggleInventory,
    ClearTarget,
    CastPrimary,
    CastSecondary,
    CastUltimate,
    CastHelmet,
    CastChestplate,
    CastBoots,
    CastHelmetSecondary,
    CastChestplateSecondary,
    CastBootsSecondary,
    CameraZoomIn,
    CameraZoomOut,
}

impl KeyAction {
    /// All rebindable actions in display order.
    pub const ALL: [Self; 15] = [
        Self::TogglePause,
        Self::ShowScoreboard,
        Self::ToggleInventory,
        Self::ClearTarget,
        Self::CastPrimary,
        Self::CastSecondary,
        Self::CastUltimate,
        Self::CastHelmet,
        Self::CastChestplate,
        Self::CastBoots,
        Self::CastHelmetSecondary,
        Self::CastChestplateSecondary,
        Self::CastBootsSecondary,
        Self::CameraZoomIn,
        Self::CameraZoomOut,
    ];

    /// Display name shown in the keybinds panel.
    pub fn label(self) -> &'static str {
        match self {
            Self::TogglePause => "Toggle Pause",
            Self::ShowScoreboard => "Show Scoreboard",
            Self::ToggleInventory => "Toggle Inventory",
            Self::ClearTarget => "Clear Target",
            Self::CastPrimary => "Cast Weapon Primary",
            Self::CastSecondary => "Cast Weapon Secondary",
            Self::CastUltimate => "Cast Weapon Ultimate",
            Self::CastHelmet => "Cast Helmet Primary",
            Self::CastChestplate => "Cast Chestplate Primary",
            Self::CastBoots => "Cast Boots Primary",
            Self::CastHelmetSecondary => "Cast Helmet Secondary",
            Self::CastChestplateSecondary => "Cast Chestplate Secondary",
            Self::CastBootsSecondary => "Cast Boots Secondary",
            Self::CameraZoomIn => "Camera Zoom In",
            Self::CameraZoomOut => "Camera Zoom Out",
        }
    }

    /// Default binding (no modifiers) when no user config exists.
    pub fn default_binding(self) -> KeyCode {
        match self {
            Self::TogglePause => KeyCode::Escape,
            Self::ShowScoreboard => KeyCode::Tab,
            Self::ToggleInventory => KeyCode::KeyI,
            Self::ClearTarget => KeyCode::Escape,
            Self::CastPrimary => KeyCode::Digit1,
            Self::CastSecondary => KeyCode::Digit2,
            Self::CastUltimate => KeyCode::Digit3,
            Self::CastHelmet => KeyCode::KeyD,
            Self::CastChestplate => KeyCode::KeyR,
            Self::CastBoots => KeyCode::KeyF,
            Self::CastHelmetSecondary => KeyCode::Digit4,
            Self::CastChestplateSecondary => KeyCode::Digit5,
            Self::CastBootsSecondary => KeyCode::Digit6,
            Self::CameraZoomIn => KeyCode::PageUp,
            Self::CameraZoomOut => KeyCode::PageDown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyBinding {
    pub key: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyBinding {
    pub const fn bare(key: KeyCode) -> Self {
        Self {
            key,
            modifiers: KeyModifiers {
                shift: false,
                ctrl: false,
                alt: false,
                super_key: false,
            },
        }
    }

    /// Pretty label, e.g. "Ctrl+Shift+Q" or "Esc".
    pub fn label(self) -> String {
        let prefix = self.modifiers.label();
        format!("{}{}", prefix, key_code_label(self.key))
    }

    /// True when the given key + currently-held modifiers match this binding.
    pub fn matches(self, key: KeyCode, pressed_modifiers: KeyModifiers) -> bool {
        self.key == key && self.modifiers == pressed_modifiers
    }
}

/// Short, player-facing name for a key. Debug names like `KeyQ` become `Q`.
pub fn key_code_label(key: KeyCode) -> String {
    match key {
        KeyCode::Escape => "Esc".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::Enter | KeyCode::NumpadEnter => "Enter".into(),
        KeyCode::Space => "Space".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Delete => "Del".into(),
        KeyCode::PageUp => "Page Up".into(),
        KeyCode::PageDown => "Page Down".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::Insert => "Insert".into(),
        KeyCode::ArrowUp => "Up".into(),
        KeyCode::ArrowDown => "Down".into(),
        KeyCode::ArrowLeft => "Left".into(),
        KeyCode::ArrowRight => "Right".into(),
        other => {
            let debug = format!("{other:?}");
            if let Some(rest) = debug.strip_prefix("Key") {
                rest.to_string()
            } else if let Some(rest) = debug.strip_prefix("Digit") {
                rest.to_string()
            } else {
                debug
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Resource, Serialize, Deserialize)]
pub struct KeybindSettings {
    /// Missing entries fall back to [`KeyAction::default_binding`].
    #[serde(default)]
    pub bindings: HashMap<KeyAction, KeyBinding>,
}

impl KeybindSettings {
    /// Returns the configured binding, or the default one if unset.
    pub fn get(&self, action: KeyAction) -> KeyBinding {
        self.bindings
            .get(&action)
            .copied()
            .unwrap_or_else(|| KeyBinding::bare(action.default_binding()))
    }

    /// True if the key + currently-held modifiers match the binding for `action`.
    pub fn matches(
        &self,
        action: KeyAction,
        key: KeyCode,
        pressed_modifiers: KeyModifiers,
    ) -> bool {
        self.get(action).matches(key, pressed_modifiers)
    }
}

// ---------------------------------------------------------------------------
// Aggregated settings
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameSettings {
    #[serde(default)]
    pub general: GeneralSettings,
    #[serde(default)]
    pub graphics: GraphicsSettings,
    #[serde(default)]
    pub gameplay: GameplaySettings,
    #[serde(default)]
    pub keybinds: KeybindSettings,
}

impl GameSettings {
    /// Current value of an on/off setting.
    pub fn toggle(&self, id: SettingToggle) -> bool {
        match id {
            SettingToggle::Vsync => self.graphics.vsync,
            SettingToggle::ShowFps => self.general.show_fps,
            SettingToggle::ShowEnemyAbilityPreviews => self.gameplay.show_enemy_ability_previews,
        }
    }

    /// Writes an on/off setting.
    pub fn set_toggle(&mut self, id: SettingToggle, on: bool) {
        match id {
            SettingToggle::Vsync => self.graphics.vsync = on,
            SettingToggle::ShowFps => self.general.show_fps = on,
            SettingToggle::ShowEnemyAbilityPreviews => {
                self.gameplay.show_enemy_ability_previews = on
            }
        }
    }

    /// Applies a dropdown value. Returns `false` when `value` is not valid
    /// for `id`, leaving the field unchanged.
    pub fn set_choice(&mut self, id: SettingChoice, value: &str) -> bool {
        match id {
            SettingChoice::WindowMode => {
                self.graphics.mode = match value {
                    "windowed" => WindowMode::Windowed,
                    "borderless" => WindowMode::Borderless,
                    "exclusive" => WindowMode::Exclusive,
                    _ => return false,
                };
                true
            }
            SettingChoice::Resolution => {
                let Some(resolution) = Resolution::parse_label(value) else {
                    return false;
                };
                self.graphics.resolution = resolution;
                true
            }
            SettingChoice::Language => {
                if value.is_empty() {
                    return false;
                }
                self.general.language = value.to_string();
                true
            }
            SettingChoice::InterfaceScale => {
                let Ok(scale) = value.parse::<f32>() else {
                    return false;
                };
                self.general.interface_scale = scale.clamp(0.5, 3.0);
                true
            }
        }
    }
}

/// Bevy resource holding the live, mutable user settings.
#[derive(Clone, Debug, Default, Resource)]
pub struct GameSettingsResource(pub GameSettings);

impl GameSettingsResource {
    /// Returns a copy of the inner settings.
    pub fn snapshot(&self) -> GameSettings {
        self.0.clone()
    }

    /// True if the configured binding for `action` was just pressed (this
    /// frame) with the right modifiers held.
    ///
    /// Single entry point for game systems that need to react to a rebindable
    /// action; replaces scattered `keys.just_pressed(KeyCode::X)` calls.
    pub fn just_pressed(
        &self,
        action: KeyAction,
        keys: &bevy::input::ButtonInput<KeyCode>,
    ) -> bool {
        let binding = self.0.keybinds.get(action);
        keys.just_pressed(binding.key) && KeyModifiers::from_pressed(keys) == binding.modifiers
    }

    /// True if the configured binding for `action` is currently held (this
    /// frame) with the right modifiers.
    ///
    /// Used by continuous-input actions like camera zoom.
    pub fn pressed(&self, action: KeyAction, keys: &bevy::input::ButtonInput<KeyCode>) -> bool {
        let binding = self.0.keybinds.get(action);
        keys.pressed(binding.key) && KeyModifiers::from_pressed(keys) == binding.modifiers
    }

    /// True if the configured binding for `action` was just released (this
    /// frame).
    ///
    /// Deliberately **ignores modifiers**, unlike [`just_pressed`](Self::just_pressed):
    /// a release closes an interaction that a press already opened, and the
    /// player is free to let go of `Shift` before the main key. Requiring the
    /// modifiers again here would swallow the release and leave the action
    /// stuck open.
    pub fn just_released(
        &self,
        action: KeyAction,
        keys: &bevy::input::ButtonInput<KeyCode>,
    ) -> bool {
        keys.just_released(self.0.keybinds.get(action).key)
    }

    /// Consumes this frame's press of `action`, so systems running later see
    /// nothing.
    ///
    /// Needed because a single physical key can be bound to several actions —
    /// `Escape` is `TogglePause` *and* `ClearTarget` — and the most specific
    /// handler must be able to claim it before the others react.
    pub fn consume_press(&self, action: KeyAction, keys: &mut bevy::input::ButtonInput<KeyCode>) {
        keys.clear_just_pressed(self.0.keybinds.get(action).key);
    }
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// Returns the path where user settings are stored.
///
/// `<user config dir>/bevymmo/settings.json` — created on demand by the
/// save routine. Falls back to `./settings.json` if the OS does not expose a
/// config directory.
pub fn settings_path() -> PathBuf {
    match dirs::config_dir() {
        Some(dir) => dir.join("bevymmo").join("settings.json"),
        None => PathBuf::from("settings.json"),
    }
}

/// Loads settings from disk. Missing or malformed file → defaults.
fn parse_settings(contents: &str) -> Result<GameSettings, serde_json::Error> {
    let mut value: serde_json::Value = serde_json::from_str(contents)?;
    if let Some(bindings) = value
        .get_mut("keybinds")
        .and_then(|keybinds| keybinds.get_mut("bindings"))
        .and_then(serde_json::Value::as_object_mut)
    {
        bindings.remove("toggle_spellbook");
        bindings.remove("cast_spell_q");
        bindings.remove("cast_spell_w");
        bindings.remove("cast_spell_e");
    }
    serde_json::from_value(value)
}

pub fn load_settings() -> GameSettings {
    let path = settings_path();
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return GameSettings::default();
    };
    match parse_settings(&contents) {
        Ok(s) => s,
        Err(err) => {
            bevy::log::warn!(
                "Failed to parse settings at {}: {} — using defaults",
                path.display(),
                err
            );
            GameSettings::default()
        }
    }
}

/// Persists settings to disk. Creates parent directories as needed.
pub fn save_settings(settings: &GameSettings) -> std::io::Result<()> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(std::io::Error::other)?;
    std::fs::write(&path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_binding_falls_back_to_default() {
        let kb = KeybindSettings::default();
        assert_eq!(
            kb.get(KeyAction::TogglePause),
            KeyBinding::bare(KeyCode::Escape)
        );
    }

    #[test]
    fn custom_binding_overrides_default() {
        let mut kb = KeybindSettings::default();
        kb.bindings.insert(
            KeyAction::TogglePause,
            KeyBinding {
                key: KeyCode::KeyP,
                modifiers: KeyModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            },
        );
        let b = kb.get(KeyAction::TogglePause);
        assert_eq!(b.key, KeyCode::KeyP);
        assert!(b.modifiers.ctrl);
    }

    #[test]
    fn matches_checks_key_and_modifiers() {
        let mut kb = KeybindSettings::default();
        kb.bindings.insert(
            KeyAction::CastPrimary,
            KeyBinding {
                key: KeyCode::KeyQ,
                modifiers: KeyModifiers {
                    shift: true,
                    ..Default::default()
                },
            },
        );
        assert!(!kb.matches(
            KeyAction::CastPrimary,
            KeyCode::KeyQ,
            KeyModifiers::default()
        ));
        assert!(kb.matches(
            KeyAction::CastPrimary,
            KeyCode::KeyQ,
            KeyModifiers {
                shift: true,
                ..Default::default()
            }
        ));
    }

    #[test]
    fn modifiers_label_is_empty_for_bare_binding() {
        assert_eq!(KeyModifiers::default().label(), String::new());
    }

    #[test]
    fn modifiers_label_orders_ctrl_alt_shift_super() {
        let m = KeyModifiers {
            ctrl: true,
            alt: true,
            shift: true,
            super_key: true,
        };
        assert_eq!(m.label(), "Ctrl+Alt+Shift+Super+");
    }

    #[test]
    fn keybind_label_combines_modifier_prefix_and_key() {
        let b = KeyBinding {
            key: KeyCode::KeyQ,
            modifiers: KeyModifiers {
                ctrl: true,
                ..Default::default()
            },
        };
        assert_eq!(b.label(), "Ctrl+Q");
    }

    #[test]
    fn key_code_label_uses_short_names() {
        assert_eq!(key_code_label(KeyCode::KeyQ), "Q");
        assert_eq!(key_code_label(KeyCode::Escape), "Esc");
        assert_eq!(key_code_label(KeyCode::Digit1), "1");
        assert_eq!(key_code_label(KeyCode::PageUp), "Page Up");
        assert_eq!(key_code_label(KeyCode::Tab), "Tab");
    }

    #[test]
    fn settings_json_roundtrip_preserves_values() {
        let mut settings = GameSettings::default();
        settings.graphics.vsync = false;
        settings.graphics.resolution = Resolution::new(1920, 1080);
        settings.graphics.mode = WindowMode::Borderless;
        settings.general.show_fps = true;
        settings.gameplay.show_enemy_ability_previews = false;
        settings.keybinds.bindings.insert(
            KeyAction::TogglePause,
            KeyBinding {
                key: KeyCode::KeyP,
                modifiers: KeyModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            },
        );

        let json = serde_json::to_string(&settings).unwrap();
        let back: GameSettings = serde_json::from_str(&json).unwrap();
        assert!(!back.graphics.vsync);
        assert_eq!(back.graphics.resolution.width, 1920);
        assert_eq!(back.graphics.mode, WindowMode::Borderless);
        assert!(back.general.show_fps);
        assert!(!back.gameplay.show_enemy_ability_previews);
        assert_eq!(back.keybinds.get(KeyAction::TogglePause).key, KeyCode::KeyP);
    }

    #[test]
    fn missing_gameplay_section_defaults_enemy_previews_on() {
        let settings = parse_settings("{}").expect("empty object is valid");
        assert!(settings.gameplay.show_enemy_ability_previews);
        assert!(settings.toggle(SettingToggle::ShowEnemyAbilityPreviews));
    }

    #[test]
    fn set_toggle_writes_enemy_ability_previews() {
        let mut settings = GameSettings::default();
        assert!(settings.toggle(SettingToggle::ShowEnemyAbilityPreviews));
        settings.set_toggle(SettingToggle::ShowEnemyAbilityPreviews, false);
        assert!(!settings.toggle(SettingToggle::ShowEnemyAbilityPreviews));
        assert!(!settings.gameplay.show_enemy_ability_previews);
    }

    #[test]
    fn set_choice_rejects_unknown_window_mode() {
        let mut settings = GameSettings::default();
        let before = settings.graphics.mode;
        assert!(!settings.set_choice(SettingChoice::WindowMode, "ultrawide"));
        assert_eq!(settings.graphics.mode, before);
    }

    #[test]
    fn set_choice_applies_resolution_and_scale() {
        let mut settings = GameSettings::default();
        assert!(settings.set_choice(SettingChoice::Resolution, "1920x1080"));
        assert_eq!(settings.graphics.resolution, Resolution::new(1920, 1080));
        assert!(settings.set_choice(SettingChoice::InterfaceScale, "1.5"));
        assert!((settings.general.interface_scale - 1.5).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_label_handles_standard_and_garbage() {
        assert_eq!(
            Resolution::parse_label("1920x1080"),
            Some(Resolution::new(1920, 1080))
        );
        assert_eq!(Resolution::parse_label("hd"), None);
        assert_eq!(Resolution::parse_label("1920"), None);
        assert_eq!(Resolution::parse_label("1920x"), None);
    }

    #[test]
    fn partial_eq_detects_gameplay_toggle() {
        let a = GameSettings::default();
        let mut b = GameSettings::default();
        b.set_toggle(SettingToggle::ShowEnemyAbilityPreviews, false);
        assert_ne!(a, b);
        b.set_toggle(SettingToggle::ShowEnemyAbilityPreviews, true);
        assert_eq!(a, b);
    }

    #[test]
    fn malformed_json_falls_back_to_defaults() {
        let settings: GameSettings = serde_json::from_str("{ invalid }").unwrap_or_default();
        assert!(settings.graphics.vsync); // default
    }

    #[test]
    fn legacy_spellbook_binding_is_ignored_during_settings_load() {
        let json = r#"{
            "keybinds": {
                "bindings": {
                    "toggle_spellbook": {
                        "key": "KeyK",
                        "modifiers": {
                            "shift": false,
                            "ctrl": false,
                            "alt": false,
                            "super_key": false
                        }
                    },
                    "cast_spell_q": {
                        "key": "KeyQ",
                        "modifiers": {
                            "shift": false,
                            "ctrl": false,
                            "alt": false,
                            "super_key": false
                        }
                    }
                }
            }
        }"#;
        let settings = parse_settings(json).expect("legacy settings should migrate");
        assert!(settings.keybinds.bindings.is_empty());
    }

    #[test]
    fn pressed_matches_default_binding_with_no_modifiers() {
        use bevy::input::ButtonInput;
        let res = GameSettingsResource::default();
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::KeyI);
        assert!(res.pressed(KeyAction::ToggleInventory, &keys));
    }

    #[test]
    fn pressed_rejects_unwanted_modifiers() {
        use bevy::input::ButtonInput;
        let res = GameSettingsResource::default();
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::KeyI);
        keys.press(KeyCode::ShiftLeft);
        assert!(!res.pressed(KeyAction::ToggleInventory, &keys));
    }

    #[test]
    fn pressed_matches_binding_with_required_modifier() {
        use bevy::input::ButtonInput;
        let mut settings = GameSettings::default();
        settings.keybinds.bindings.insert(
            KeyAction::CastPrimary,
            KeyBinding {
                key: KeyCode::KeyQ,
                modifiers: KeyModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            },
        );
        let res = GameSettingsResource(settings);
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::KeyQ);
        assert!(!res.pressed(KeyAction::CastPrimary, &keys));
        keys.press(KeyCode::ControlLeft);
        assert!(res.pressed(KeyAction::CastPrimary, &keys));
    }
}
