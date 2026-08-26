//! Warding Helm — a helmet providing head protection with arcane resonance.

use bevymmo_props_macro::item;

use crate::ability_definitions::aegis::Aegis;
use crate::items::ItemRegistry;

#[item(
    id = "warding_helm",
    name = "Warding Helm",
    description = "A helmet inscribed with basic warding glyphs. Protects the wearer's mind as well as their skull.",
    category = Armor,
    rarity = Rare,
    slot = Helmet,
    tradable = true,
    effects = [
        stat_bonus(field = Armor, op = Add, value = 10.0),
        stat_bonus(field = MaxHealth, op = Add, value = 50.0),
    ],
    rune_profile(capacity = 7, stability = 0.90),
    abilities(
        primary = [Aegis],
        secondary = [Aegis],
        ultimate = [Aegis],
    ),
)]
pub struct WardingHelm;

pub fn register(registry: &mut ItemRegistry) {
    WardingHelm::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::components::EquipSlot;
    use crate::items::definition::Item;

    #[test]
    fn id_and_slot_match_design() {
        let item = WardingHelm;
        assert_eq!(item.id().as_str(), "warding_helm");
        assert_eq!(item.config().equippable_into, Some(EquipSlot::Helmet));
    }

    #[test]
    fn category_is_armor() {
        let item = WardingHelm;
        assert!(matches!(
            item.config().category,
            crate::items::ItemCategory::Armor
        ));
    }

    #[test]
    fn exposes_the_aegis_ability() {
        let loadout = WardingHelm
            .ability_loadout()
            .expect("warding helm must expose Aegis");
        assert_eq!(loadout.primary[0].as_str(), "aegis");
    }

    #[test]
    fn has_a_rune_profile_with_stability() {
        let item = WardingHelm;
        let profile = item
            .rune_profile()
            .expect("warding_helm must grant a rune profile");
        assert_eq!(profile.capacity, 7);
        assert!((profile.stability - 0.90).abs() < f32::EPSILON);
    }

    #[test]
    fn grants_armor_and_health_bonuses() {
        let item = WardingHelm;
        assert_eq!(item.effects().len(), 2);
    }
}
