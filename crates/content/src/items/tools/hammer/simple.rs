//! Simple gathering hammer. Speeds copper (and any node that lists `Hammer`).

use bevymmo_props_macro::item;

use crate::items::ItemRegistry;

#[item(
    id = "simple_hammer",
    name = "Martello",
    description = "A plain mining hammer. Faster copper, and a chance of extra ore.",
    category = Tool,
    rarity = Common,
    slot = Weapon,
    gathering_tool = Hammer,
    tradable = true,
    effects = [
        stat_bonus(field = GatheringSpeed, op = Add, value = 50.0),
        stat_bonus(field = GatheringBonus, op = Add, value = 0.25),
    ],
    crafting(
        channel_seconds = 3.0,
        ingredients = [
            ingredient(id = "wood", amount = 3),
            ingredient(id = "copper", amount = 4),
        ],
    ),
)]
pub struct SimpleHammer;

pub fn register(registry: &mut ItemRegistry) {
    SimpleHammer::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::components::EquipSlot;
    use crate::items::definition::{Item, ItemCategory};
    use crate::items::GatheringToolKind;
    use crate::stats::events::{ModifierOp, StatField};

    #[test]
    fn is_a_weapon_slot_hammer() {
        assert_eq!(SimpleHammer.config().category, ItemCategory::Tool);
        assert_eq!(
            SimpleHammer.config().equippable_into,
            Some(EquipSlot::Weapon)
        );
        assert_eq!(
            SimpleHammer.gathering_tool(),
            Some(GatheringToolKind::Hammer)
        );
        assert!(SimpleHammer.ability_loadout().is_none());
        assert!(SimpleHammer.rune_profile().is_none());
    }

    #[test]
    fn recipe_is_three_wood_and_four_copper() {
        let recipe = SimpleHammer.craft_recipe().expect("hammer is craftable");
        assert_eq!(recipe.channel_seconds, 3.0);
        assert_eq!(recipe.ingredients.len(), 2);
        assert_eq!(recipe.ingredients[0].item_id.as_str(), "wood");
        assert_eq!(recipe.ingredients[0].amount, 3);
        assert_eq!(recipe.ingredients[1].item_id.as_str(), "copper");
        assert_eq!(recipe.ingredients[1].amount, 4);
    }

    #[test]
    fn grants_gathering_speed_and_bonus() {
        let effects = SimpleHammer.effects();
        assert_eq!(effects.len(), 2);
        match &effects[0] {
            crate::items::ItemEffect::StatBonus { field, op, value } => {
                assert_eq!(*field, StatField::GatheringSpeed);
                assert_eq!(*op, ModifierOp::Add);
                assert_eq!(*value, 50.0);
            }
            other => panic!("expected gathering speed, got {other:?}"),
        }
        match &effects[1] {
            crate::items::ItemEffect::StatBonus { field, op, value } => {
                assert_eq!(*field, StatField::GatheringBonus);
                assert_eq!(*op, ModifierOp::Add);
                assert_eq!(*value, 0.25);
            }
            other => panic!("expected gathering bonus, got {other:?}"),
        }
    }
}
