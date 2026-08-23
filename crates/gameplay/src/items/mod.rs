//! Item framework data: the `Item` trait, runtime components, effects, the
//! registry and the network commands.
//!
//! The application pipeline (processing equip/unequip requests and recomputing
//! derived stats) is server logic and lives in `bevymmo_server`. This crate
//! only defines the contract and the data, mirroring `crate::spells`.

pub mod components;
pub mod definition;
pub mod effects;
pub mod events;
pub mod gathering_tool;
pub mod instance;
pub mod recipe;
pub mod registry;
pub mod weapon_family;

pub use components::{EquipSlot, Equipment, Inventory, StackOpError, INVENTORY_CAPACITY};
pub use definition::{Item, ItemCategory, ItemConfig, ItemRarity};
pub use effects::ItemEffect;
pub use events::{EquipItemCommand, MoveItemCommand, UnequipItemCommand};
pub use gathering_tool::GatheringToolKind;
pub use instance::{
    ItemInstance, ItemInstanceId, STARTER_WEAPON_ITEM_ID, STARTER_WEAPON_ROOT_WORD,
};
pub use recipe::{CraftIngredient, CraftRecipe};
pub use registry::{ItemId, ItemRegistry};
pub use weapon_family::{WeaponFamily, WeaponFamilyId, WeaponFamilyMetadata, WeaponFamilyRegistry};
