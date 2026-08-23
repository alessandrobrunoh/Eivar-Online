//! Settings panels — one per sidebar tab.
//!
//! Each panel follows the same contract: a `spawn_*` function builds its UI
//! children inside the content area, and a `refresh_*` system keeps the
//! widgets in sync with [`GameSettingsResource`]. Adding a new tab requires:
//!
//! 1. A new module here implementing the same pattern.
//! 2. Adding the variant to [`SettingsTab`].
//! 3. Wiring spawn + refresh in [`SettingsPlugin::build`].

use bevy::prelude::Component;

pub mod gameplay;
pub mod general;
pub mod graphics;
pub mod keybinds;

/// Marker: root node of a panel. Visibility is toggled when the active tab
/// changes.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsPanel {
    General,
    Graphics,
    Gameplay,
    Keybinds,
}

impl SettingsPanel {
    pub fn matches(self, tab: super::state::SettingsTab) -> bool {
        matches!(
            (self, tab),
            (SettingsPanel::General, super::state::SettingsTab::General)
                | (SettingsPanel::Graphics, super::state::SettingsTab::Graphics)
                | (SettingsPanel::Gameplay, super::state::SettingsTab::Gameplay)
                | (SettingsPanel::Keybinds, super::state::SettingsTab::Keybinds)
        )
    }
}
