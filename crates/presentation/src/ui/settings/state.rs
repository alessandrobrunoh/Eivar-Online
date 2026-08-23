//! UI-specific settings types. The data model lives in
//! [`bevymmo_client::user_settings`]; this module re-exports those types for
//! convenience and adds the UI-only [`SettingsTab`] enum (sidebar tabs).

use bevy::prelude::Resource;

pub use bevymmo_client::user_settings::{
    load_settings, save_settings, settings_path, GameSettings, GameSettingsResource,
    GameplaySettings, GeneralSettings, GraphicsSettings, KeyAction, KeyBinding, KeyModifiers,
    KeybindSettings, Resolution, SettingChoice, SettingToggle, WindowMode,
};

/// Where to return when the settings overlay closes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsReturn {
    #[default]
    Menu,
    Pause,
}

/// Overlay session for settings. Independent of [`Screen`] so the pause menu
/// can open settings without leaving [`Screen::InGame`].
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettingsSession {
    pub open: bool,
    pub return_to: SettingsReturn,
}

impl SettingsSession {
    pub fn open_from(&mut self, return_to: SettingsReturn) {
        self.open = true;
        self.return_to = return_to;
    }

    pub fn close(&mut self) {
        self.open = false;
    }
}

/// Identifies one of the panels shown in the settings sidebar.
///
/// Order of variants = order in the sidebar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SettingsTab {
    #[default]
    General,
    Graphics,
    Gameplay,
    Keybinds,
}

impl SettingsTab {
    /// All tabs in sidebar order.
    pub const ALL: [Self; 4] = [
        Self::General,
        Self::Graphics,
        Self::Gameplay,
        Self::Keybinds,
    ];

    /// Sidebar label, shown to the player.
    pub fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Graphics => "Graphics",
            Self::Gameplay => "Gameplay",
            Self::Keybinds => "Keybinds",
        }
    }
}
