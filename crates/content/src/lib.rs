//! Concrete game content built on the generic gameplay and world frameworks.

pub mod catalog;

#[path = "abilities/mod.rs"]
pub mod ability_definitions;
#[path = "ancient_words/mod.rs"]
pub mod ancient_word_definitions;

#[path = "items/mod.rs"]
pub mod item_definitions;
#[path = "markets/mod.rs"]
pub mod market_definitions;

#[path = "placeables/mod.rs"]
pub mod placeable_definitions;
#[path = "root_words/mod.rs"]
pub mod root_word_definitions;
#[path = "statuses/mod.rs"]
pub mod status_definitions;

// Macro-generated definitions use these stable framework paths.
pub use bevymmo_gameplay::{
    abilities, crafting, crowd_control, effects, entity, items, loot, placeables, spells, stats,
};
pub use bevymmo_gameplay::{ids, math};
pub use bevymmo_gameplay::{EntityId, Rgba};
pub use bevymmo_world as world;
