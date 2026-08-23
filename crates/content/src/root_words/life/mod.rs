//! Root Word Life — healing.

use bevymmo_props_macro::root_word;

use crate::abilities::{
    AbilityBlueprint, AbilityParams, ManifestationPayload, RootWordEffect, RootWordRegistry,
};

#[root_word(
    id = "life",
    name = "Life",
    description = "Restores health to allies",
    rune_cost = 1
)]
pub struct LifeRootWord;

/// Adds this content package to the root word registry.
pub fn register(registry: &mut RootWordRegistry) {
    LifeRootWord::register(registry);
}

impl LifeRootWord {
    /// Healing efficiency multiplier.
    pub const HEALING_EFFICIENCY: f32 = 1.2;
}

impl RootWordEffect for LifeRootWord {
    fn apply_to_blueprint(&self, blueprint: &mut AbilityBlueprint, _params: &AbilityParams) {
        blueprint.params.potency *= Self::HEALING_EFFICIENCY;
        blueprint.payload = ManifestationPayload::heal([]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::RootWord;

    #[test]
    fn id_is_stable() {
        assert_eq!(LifeRootWord::ID, "life");
    }

    #[test]
    fn metadata_values() {
        let word = LifeRootWord;
        let meta = word.metadata();
        assert_eq!(meta.display_name, "Life");
        assert_eq!(meta.description, "Restores health to allies");
        assert_eq!(meta.rune_cost, 1);
    }

    #[test]
    fn apply_to_blueprint_converts_to_healing_without_changing_tags() {
        let word = LifeRootWord;
        let mut blueprint = AbilityBlueprint {
            ability_id: crate::abilities::AbilityId::new("test"),
            tags: vec![crate::abilities::AbilityTag::Melee],
            geometry: crate::abilities::AbilityGeometry::Cone {
                radius: 5.0,
                angle_deg: 85.0,
            },
            cast_mode: crate::abilities::AbilityCastMode::CastTime,
            echo: false,
            params: crate::abilities::AbilityParams {
                potency: 50.0,
                area: 5.0,
                range: 5.0,
                cast_time: 0.25,
                cooldown: 3.0,
                mana_cost: 9.0,
            },
            animation: "heal",
            impact_vfx: "heal_effect",
            impact_delay: 0.0,
            control: None,
            payload: ManifestationPayload::default(),
        };
        let params = blueprint.params;

        crate::abilities::RootWordEffect::apply_to_blueprint(&word, &mut blueprint, &params);

        assert!((blueprint.params.potency - 60.0).abs() < 0.001);
        assert_eq!(blueprint.tags, vec![crate::abilities::AbilityTag::Melee]);
        assert_eq!(blueprint.payload, ManifestationPayload::heal([]));
    }
}
