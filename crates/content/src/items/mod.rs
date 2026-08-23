//! Item content and its registry.

pub mod armor;
pub mod materials;

pub mod purity_charm;
pub mod tools;
pub mod weapons;

use crate::items::registry::ItemRegistry;
use crate::items::WeaponFamilyRegistry;

/// Item ids the greeter NPC will hand out. Must stay a subset of [`default_items`].
pub fn greeter_stock() -> &'static [&'static str] {
    &[
        weapons::sword::sword::Sword::ID,
        armor::simple::SimpleHelm::ID,
        armor::simple::SimpleCape::ID,
        armor::simple::SimpleCuirass::ID,
        armor::simple::SimpleBuckler::ID,
        armor::simple::SimpleBoots::ID,
    ]
}

/// Builds the registry containing every item shipped by this game build.
pub fn default_items() -> ItemRegistry {
    let mut registry = ItemRegistry::default();

    armor::register(&mut registry);
    purity_charm::register(&mut registry);
    materials::register(&mut registry);
    tools::register(&mut registry);
    weapons::sword::sword::register(&mut registry);

    registry
}

/// Builds the registry containing every weapon family shipped by this game build.
pub fn default_weapon_families() -> WeaponFamilyRegistry {
    let mut registry = WeaponFamilyRegistry::default();
    weapons::sword::register(&mut registry);
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::registry::ItemId;

    #[test]
    fn default_items_contains_all_items() {
        let registry = default_items();

        assert!(registry.contains(&ItemId::new(weapons::sword::sword::Sword::ID)));
        assert!(registry.contains(&ItemId::new(
            armor::chestplate::robust_cuirass::RobustCuirass::ID
        )));
        assert!(registry.contains(&ItemId::new(armor::helmet::warding_helm::WardingHelm::ID)));
        assert!(registry.contains(&ItemId::new(armor::boots::swift_boots::SwiftBoots::ID)));
        assert!(registry.contains(&ItemId::new(purity_charm::PurityCharm::ID)));
        assert!(registry.contains(&ItemId::new(armor::simple::SimpleHelm::ID)));
        assert!(registry.contains(&ItemId::new(armor::simple::SimpleCape::ID)));
        assert!(registry.contains(&ItemId::new(armor::simple::SimpleCuirass::ID)));
        assert!(registry.contains(&ItemId::new(armor::simple::SimpleBuckler::ID)));
        assert!(registry.contains(&ItemId::new(armor::simple::SimpleBoots::ID)));
        assert!(registry.contains(&ItemId::new(materials::wood::Wood::ID)));
        assert!(registry.contains(&ItemId::new(materials::copper::Copper::ID)));
        assert!(registry.contains(&ItemId::new(tools::axe::simple::SimpleAxe::ID)));
        assert!(registry.contains(&ItemId::new(tools::hammer::simple::SimpleHammer::ID)));
        assert_eq!(registry.len(), 14); // 1 weapon + 3 fancy armor + 5 simple + charm + wood + copper + 2 tools
    }

    #[test]
    fn wood_is_the_oak_tree_yield() {
        use crate::placeables::{PlaceableRegistry, ResourceNodePlaceable};
        let items = default_items();
        let mut placeables = PlaceableRegistry::default();
        crate::placeable_definitions::register_all(&mut placeables);
        let oak = placeables
            .resources
            .get(&crate::placeables::KindId::new("resource_oak_tree"))
            .expect("oak tree is registered");
        let yield_item = ResourceNodePlaceable::resource_config(oak.as_ref()).yield_item;
        assert_eq!(yield_item.as_str(), "wood");
        assert!(items.contains(&yield_item));
    }

    #[test]
    fn copper_is_the_copper_vein_yield() {
        use crate::placeables::{PlaceableRegistry, ResourceNodePlaceable};
        let items = default_items();
        let mut placeables = PlaceableRegistry::default();
        crate::placeable_definitions::register_all(&mut placeables);
        let vein = placeables
            .resources
            .get(&crate::placeables::KindId::new("resource_copper_vein"))
            .expect("copper vein is registered");
        let yield_item = ResourceNodePlaceable::resource_config(vein.as_ref()).yield_item;
        assert_eq!(yield_item.as_str(), "copper");
        assert!(items.contains(&yield_item));
    }

    #[test]
    fn greeter_stock_is_in_the_catalogue() {
        let registry = default_items();
        for id in greeter_stock() {
            assert!(
                registry.contains(&ItemId::new(*id)),
                "greeter offers unknown item {id}"
            );
        }
    }

    #[test]
    fn default_items_armor_slots_are_correct() {
        use crate::items::components::EquipSlot;
        let registry = default_items();

        let cuirass = registry
            .get(&ItemId::new(
                armor::chestplate::robust_cuirass::RobustCuirass::ID,
            ))
            .unwrap();
        assert_eq!(cuirass.config().equippable_into, Some(EquipSlot::Armor));

        let helm = registry
            .get(&ItemId::new(armor::helmet::warding_helm::WardingHelm::ID))
            .unwrap();
        assert_eq!(helm.config().equippable_into, Some(EquipSlot::Helmet));

        let boots = registry
            .get(&ItemId::new(armor::boots::swift_boots::SwiftBoots::ID))
            .unwrap();
        assert_eq!(boots.config().equippable_into, Some(EquipSlot::Shoes));
    }

    #[test]
    fn default_weapon_families_contains_sword() {
        let registry = default_weapon_families();
        assert!(registry.contains(&crate::items::WeaponFamilyId::new("sword")));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn wood_is_not_craftable() {
        use crate::items::Item;
        assert!(materials::wood::Wood.craft_recipe().is_none());
    }

    #[test]
    fn only_the_sword_is_a_craftable_weapon() {
        use crate::items::definition::ItemCategory;
        let registry = default_items();
        let weapons = registry.craftable_in(ItemCategory::Weapon);
        assert_eq!(weapons.len(), 1);
        assert_eq!(weapons[0].0.as_str(), weapons::sword::sword::Sword::ID);
        assert!(registry.craftable_in(ItemCategory::Armor).is_empty());
    }

    #[test]
    fn axe_and_hammer_are_the_craftable_tools() {
        use crate::items::definition::ItemCategory;
        let registry = default_items();
        let tools = registry.craftable_in(ItemCategory::Tool);
        let ids: Vec<&str> = tools.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["simple_axe", "simple_hammer"]);
    }

    #[test]
    fn greeter_does_not_hand_out_gathering_tools() {
        assert!(!greeter_stock().contains(&tools::axe::simple::SimpleAxe::ID));
        assert!(!greeter_stock().contains(&tools::hammer::simple::SimpleHammer::ID));
    }

    #[test]
    fn every_recipe_ingredient_is_in_the_catalogue() {
        let registry = default_items();
        for (_, item) in registry.sorted_items() {
            let Some(recipe) = item.craft_recipe() else {
                continue;
            };
            for ingredient in &recipe.ingredients {
                assert!(
                    registry.contains(&ingredient.item_id),
                    "{} recipe names unknown ingredient {}",
                    item.id().as_str(),
                    ingredient.item_id.as_str()
                );
            }
        }
    }
}
