//! Ancient Word Twin — duplicates the ability with reduced effectiveness.
//! Splits the effect into two weaker instances.

use bevymmo_gameplay::abilities::{
    AbilityBlueprint, AbilityParams, AbilityTag, AncientWordEffect, BaseAbility,
};
use bevymmo_gameplay::spells::context::SpellCastContext;
use bevymmo_props_macro::ancient_word;

#[ancient_word(
    id = "twin",
    name = "Twin",
    tag = Projectile,
    tags = [Projectile, Area, RepeatCompatible],
    rune_cost = 2
)]
pub struct Twin;

impl Twin {
    /// Each twin deals this fraction of original potency.
    pub const TWIN_FRACTION: f32 = 0.55;
}

impl AncientWordEffect for Twin {
    fn post_process(
        &self,
        _ability: &dyn BaseAbility,
        _params: &AbilityParams,
        _ctx: &mut SpellCastContext,
    ) {
    }

    fn transform_blueprint(&self, blueprint: &mut AbilityBlueprint) {
        // Reduce individual potency since we get two projectiles
        blueprint.params.potency *= Self::TWIN_FRACTION;

        // Mark as repeat-compatible for the second projectile
        if !blueprint.has_tag(AbilityTag::RepeatCompatible) {
            blueprint.tags.push(AbilityTag::RepeatCompatible);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::{AbilityGeometry, AbilityId};

    #[test]
    fn metadata_declares_projectile_requirement() {
        let metadata = <Twin as bevymmo_gameplay::abilities::AncientWord>::metadata(&Twin);
        assert_eq!(metadata.rune_cost, 2);
        assert!(metadata.required_tags.contains(&AbilityTag::Projectile));
        assert!(metadata.is_compatible_with(&[AbilityTag::Area]));
    }

    #[test]
    fn transform_reduces_potency_for_twin_projectiles() {
        let mut blueprint = AbilityBlueprint {
            ability_id: AbilityId::new("projectile_spell"),
            tags: vec![AbilityTag::Projectile],
            geometry: AbilityGeometry::Projectile { speed: 25.0 },
            cast_mode: crate::abilities::AbilityCastMode::Instant,
            echo: false,
            params: AbilityParams {
                potency: 100.0,
                area: 0.0,
                range: 35.0,
                cast_time: 0.0,
                cooldown: 1.5,
                mana_cost: 12.0,
            },
            animation: "cast",
            impact_vfx: "impact",
            impact_delay: 0.2,
            control: None,
            payload: crate::abilities::ManifestationPayload::default(),
        };

        Twin.transform_blueprint(&mut blueprint);

        // Each twin should be 55% of original
        assert!((blueprint.params.potency - 55.0).abs() < f32::EPSILON);
        assert!(blueprint.has_tag(AbilityTag::RepeatCompatible));
    }
}
