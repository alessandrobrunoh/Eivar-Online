//! Root Word Flame — fire damage with burning.

use bevymmo_props_macro::root_word;

use crate::abilities::{
    AbilityBlueprint, AbilityParams, ManifestationPayload, RootWordEffect, RootWordRegistry,
};

#[root_word(
    id = "flame",
    name = "Flame",
    description = "Applies fire damage with burning effects",
    rune_cost = 1
)]
pub struct FlameRootWord;

/// Adds this content package to the root word registry.
pub fn register(registry: &mut RootWordRegistry) {
    FlameRootWord::register(registry);
}

impl FlameRootWord {
    /// Potency multiplier for flame abilities.
    pub const FLAME_SCALING: f32 = 1.15;
}

impl RootWordEffect for FlameRootWord {
    fn apply_to_blueprint(&self, blueprint: &mut AbilityBlueprint, _params: &AbilityParams) {
        blueprint.params.potency *= Self::FLAME_SCALING;
        blueprint.payload = ManifestationPayload::damage(["burn"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::RootWord;

    #[test]
    fn id_is_stable() {
        assert_eq!(FlameRootWord::ID, "flame");
    }

    #[test]
    fn metadata_values() {
        let word = FlameRootWord;
        let meta = word.metadata();
        assert_eq!(meta.display_name, "Flame");
        assert_eq!(meta.description, "Applies fire damage with burning effects");
        assert_eq!(meta.rune_cost, 1);
    }

    #[test]
    fn apply_to_blueprint_writes_burn_without_changing_tags() {
        let word = FlameRootWord;
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
                potency: 100.0,
                area: 5.0,
                range: 5.0,
                cast_time: 0.25,
                cooldown: 3.0,
                mana_cost: 9.0,
            },
            animation: "cast",
            impact_vfx: "fire_impact",
            impact_delay: 0.0,
            control: None,
            payload: ManifestationPayload::default(),
        };
        let params = blueprint.params;

        crate::abilities::RootWordEffect::apply_to_blueprint(&word, &mut blueprint, &params);

        assert!((blueprint.params.potency - 115.0).abs() < f32::EPSILON);
        assert_eq!(blueprint.tags, vec![crate::abilities::AbilityTag::Melee]);
        assert_eq!(blueprint.payload, ManifestationPayload::damage(["burn"]));
    }
}
