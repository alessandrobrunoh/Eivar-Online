//! Engine-independent gameplay rules and frameworks.

pub mod abilities;
pub mod crafting;
pub mod crowd_control;
pub mod economy;
pub mod effects;
pub mod entity;
pub mod gathering;
pub mod items;
pub mod loot;
pub mod markets;
pub mod movement;
pub mod placeables;
pub mod registry;
pub mod spells;
pub mod stats;

pub use bevymmo_core::{ids, math};
pub use bevymmo_core::{EntityId, Rgba};
pub use bevymmo_world as world;
