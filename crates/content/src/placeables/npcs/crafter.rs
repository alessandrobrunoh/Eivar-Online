//! Weapon crafter NPC. Lists catalogue weapons that declare a recipe.

use crate::placeables::npc;

#[npc(
    id = "npc_weapon_crafter",
    name = "Fabbro",
    icon = "🔨",
    asset = "models/npcs/merchant.glb",
    tint = (0.85, 0.55, 0.25),
    interaction = craft(Weapon, Tool),
)]
pub struct WeaponCrafter;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::ItemCategory;
    use crate::placeables::{
        InteractionKind, NpcPlaceable, PlaceableDefinition, PlaceableRegistry,
    };

    #[test]
    fn weapon_crafter_offers_weapon_recipes() {
        let def = WeaponCrafter;
        assert_eq!(def.id().as_str(), "npc_weapon_crafter");
        assert_eq!(WeaponCrafter::ID, "npc_weapon_crafter");
        assert_eq!(
            def.interaction(),
            InteractionKind::Craft {
                categories: vec![ItemCategory::Weapon, ItemCategory::Tool]
            }
        );

        let mut registry = PlaceableRegistry::default();
        register(&mut registry);
        assert!(registry.npcs.contains_key(&def.id()));
    }
}
