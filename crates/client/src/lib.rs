//! Client-side network and input logic for BevyMMO.
//!
//! Hosts client-only helpers such as key mapping and targeting systems.
//! Transport extraction is still in progress during the crate-split migration.

pub mod app_state;
pub mod gathering;
pub mod local_player;
pub mod loot;
pub mod movement;
pub mod network;
pub mod player_movement;
pub mod pointer;
pub mod server_feed;
pub mod stdb;
pub mod targeting;
pub mod user_settings;

pub mod prelude {
    pub use crate::app_state::{
        ConnectionFailure, ConnectionIntent, ConnectionRequest, GameStatePlugin, PauseOverlay,
        PlayerNameError, Screen,
    };
    pub use crate::local_player::LocalPlayer;
    pub use crate::network::types::{ClientConnectionConfig, ConnectedClient};
    pub use crate::targeting::TargetingPlugin;
    pub use crate::user_settings::{GameSettingsResource, KeyAction};
}
