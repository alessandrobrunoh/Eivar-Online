//! Simple gathering axe. Speeds oak (and any node that lists `Axe`).

use bevymmo_props_macro::item;

use crate::items::ItemRegistry;

#[item(
    id = "simple_axe",
    name = "Ascia",
    description = "A plain woodcutting axe. Faster oak, and a chance of extra wood.",
    category = Tool,
    rarity = Common,
    slot = Weapon,
    gathering_tool = Axe,
    tradable = true,
    effects = [
        stat_bonus(field = GatheringSpeed, op = Add, value = 50.0),
        stat_bonus(field = GatheringBonus, op = Add, value = 0.25),
    ],
    crafting(
        channel_seconds = 3.0,
        ingredients = [
            ingredient(id = "wood", amount = 4),
            ingredient(id = "copper", amount = 3),
        ],
    ),
)]
pub struct SimpleAxe;

pub fn register(registry: &mut ItemRegistry) {
    SimpleAxe::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::components::EquipSlot;
    use crate::items::definition::{Item, ItemCategory};
    use crate::items::GatheringToolKind;
    use crate::stats::events::{ModifierOp, StatField};

    #[test]
    fn is_a_weapon_slot_axe() {
        assert_eq!(SimpleAxe.config().category, ItemCategory::Tool);
        assert_eq!(SimpleAxe.config().equippable_into, Some(EquipSlot::Weapon));
        assert_eq!(SimpleAxe.gathering_tool(), Some(GatheringToolKind::Axe));
        assert!(SimpleAxe.ability_loadout().is_none());
        assert!(SimpleAxe.rune_profile().is_none());
    }

    #[test]
    fn recipe_is_four_wood_and_three_copper() {
        let recipe = SimpleAxe.craft_recipe().expect("axe is craftable");
        assert_eq!(recipe.channel_seconds, 3.0);
        assert_eq!(recipe.ingredients.len(), 2);
        assert_eq!(recipe.ingredients[0].item_id.as_str(), "wood");
        assert_eq!(recipe.ingredients[0].amount, 4);
        assert_eq!(recipe.ingredients[1].item_id.as_str(), "copper");
        assert_eq!(recipe.ingredients[1].amount, 3);
    }

    #[test]
    fn grants_gathering_speed_and_bonus() {
        let effects = SimpleAxe.effects();
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
