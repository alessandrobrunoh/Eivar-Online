//! `ItemId` and `ItemRegistry` — the lookup table for all item definitions.
//!
//! Mirrors `crate::spells::registry` so the inventory UI and the server use a
//! single source of truth. Items are registered at startup by
//! [`crate::content::items::default_items`] and looked up by id at
//! equip-time and at UI render time.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::sync::Arc;

use super::definition::{Item, ItemCategory};
use crate::registry::Registry;

/// Unique identifier of an item type.
///
/// Uses `Cow<'static, str>` so it can be built cheaply from a `&'static str`
/// constant in code, and also from an owned `String` arriving from the network
/// or the database.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ItemId(pub(crate) Cow<'static, str>);

impl ItemId {
    /// Builds a new id from either a static string or an owned network string.
    pub fn new(id: impl Into<Cow<'static, str>>) -> Self {
        Self(id.into())
    }

    /// Underlying string value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&'static str> for ItemId {
    fn from(value: &'static str) -> Self {
        Self::new(value)
    }
}

/// Central registry of all item definitions.
///
/// Items are registered at startup and looked up by id when the server
/// validates an equip command, or when the client renders an inventory slot.
/// The registry never changes after startup.
#[cfg_attr(feature = "bevy", derive(bevy_ecs::resource::Resource))]
#[derive(Default)]
pub struct ItemRegistry {
    items: Registry<ItemId, Arc<dyn Item>>,
}

impl ItemRegistry {
    /// Registers a concrete item. If an item with the same id already exists,
    /// it is replaced.
    pub fn register(&mut self, item: Arc<dyn Item>) {
        let id = item.id();
        self.items.insert(id, item);
    }

    /// Looks up an item by id.
    pub fn get(&self, id: &ItemId) -> Option<Arc<dyn Item>> {
        self.items.get(id).cloned()
    }

    /// Returns `true` if an item with the given id is registered.
    pub fn contains(&self, id: &ItemId) -> bool {
        self.items.contains(id)
    }

    /// Number of registered items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// `true` if no item is registered.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// All registered items sorted alphabetically by display name.
    ///
    /// Deterministic iteration order keeps the inventory UI stable across
    /// rebuilds and matches the behavior of other sorted registries.
    pub fn sorted_items(&self) -> Vec<(ItemId, Arc<dyn Item>)> {
        self.items
            .sorted_by(|a, b| a.display_name().cmp(b.display_name()))
    }

    /// Craftable items in `category`, sorted by display name.
    ///
    /// An item is craftable only when it declares a recipe *and* its
    /// catalogue category matches. Unique items (no recipe) are omitted.
    pub fn craftable_in(&self, category: ItemCategory) -> Vec<(ItemId, Arc<dyn Item>)> {
        self.craftable_in_any(&[category])
    }

    /// Craftable items whose category is in `categories`, sorted by display name.
    pub fn craftable_in_any(&self, categories: &[ItemCategory]) -> Vec<(ItemId, Arc<dyn Item>)> {
        self.sorted_items()
            .into_iter()
            .filter(|(_, item)| {
                categories.contains(&item.config().category) && item.craft_recipe().is_some()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::components::EquipSlot;
    use crate::items::definition::{ItemCategory, ItemConfig, ItemRarity};
    use crate::items::effects::ItemEffect;

    struct DummyItem {
        id_str: &'static str,
        name: &'static str,
        config: ItemConfig,
    }

    impl DummyItem {
        fn new(id_str: &'static str, name: &'static str) -> Self {
            Self {
                id_str,
                name,
                config: ItemConfig {
                    display_name: std::borrow::Cow::Borrowed(name),
                    description: std::borrow::Cow::Borrowed(""),
                    category: ItemCategory::Weapon,
                    rarity: ItemRarity::Common,
                    equippable_into: Some(EquipSlot::Weapon),
                    weight: 0.0,
                    tradable: true,
                    icon: "",
                },
            }
        }
    }

    impl Item for DummyItem {
        fn id(&self) -> ItemId {
            ItemId::new(self.id_str)
        }
        fn config(&self) -> &ItemConfig {
            &self.config
        }
        fn display_name(&self) -> &str {
            self.name
        }
        fn effects(&self) -> &[ItemEffect] {
            &[]
        }
    }

    #[test]
    fn register_and_lookup_by_id() {
        let mut registry = ItemRegistry::default();
        registry.register(Arc::new(DummyItem::new("a", "Alpha")));

        let id = ItemId::new("a");
        assert!(registry.contains(&id));
        assert_eq!(
            registry.get(&id).expect("item present").display_name(),
            "Alpha"
        );
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn missing_id_returns_none() {
        let registry = ItemRegistry::default();
        assert!(registry.get(&ItemId::new("nope")).is_none());
        assert!(registry.is_empty());
    }

    #[test]
    fn sorted_items_is_deterministic_by_display_name() {
        let mut registry = ItemRegistry::default();
        registry.register(Arc::new(DummyItem::new("c", "Charlie")));
        registry.register(Arc::new(DummyItem::new("a", "Alpha")));
        registry.register(Arc::new(DummyItem::new("b", "Bravo")));

        let names: Vec<String> = registry
            .sorted_items()
            .into_iter()
            .map(|(_, item)| item.display_name().to_string())
            .collect();
        assert_eq!(names, vec!["Alpha", "Bravo", "Charlie"]);
    }

    #[test]
    fn item_id_roundtrips_through_str() {
        let id = ItemId::new("iron_sword");
        assert_eq!(id.as_str(), "iron_sword");
        let from_static: ItemId = "iron_sword".into();
        assert_eq!(from_static, id);
    }
}
