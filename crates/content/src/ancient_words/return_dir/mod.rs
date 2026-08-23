//! Ancient Word Return — boomerang/returning effect.
//! The ability returns to the caster after reaching max range or impacting.

use bevymmo_gameplay::abilities::{
    AbilityBlueprint, AbilityParams, AbilityTag, AncientWordEffect, BaseAbility,
};
use bevymmo_gameplay::spells::context::SpellCastContext;
use bevymmo_props_macro::ancient_word;

#[ancient_word(id = "return", name = "Return", tag = Projectile, rune_cost = 1)]
pub struct ReturnWord;

impl ReturnWord {
    /// Return speed multiplier (can be different from outgoing speed).
    pub const RETURN_SPEED_MULT: f32 = 1.2;
}

impl AncientWordEffect for ReturnWord {
    fn post_process(
        &self,
        _ability: &dyn BaseAbility,
        _params: &AbilityParams,
        _ctx: &mut SpellCastContext,
    ) {
    }

    fn transform_blueprint(&self, blueprint: &mut AbilityBlueprint) {
        // Returning abilities have extended effective range but split damage
        blueprint.params.range *= 1.3;

        // Mark as repeat-compatible for the return journey
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
        let metadata =
            <ReturnWord as bevymmo_gameplay::abilities::AncientWord>::metadata(&ReturnWord);
        assert_eq!(metadata.rune_cost, 1);
        assert!(metadata.required_tags.contains(&AbilityTag::Projectile));
    }

    #[test]
    fn transform_extends_range_for_return_journey() {
        let mut blueprint = AbilityBlueprint {
            ability_id: AbilityId::new("boomerang"),
            tags: vec![AbilityTag::Projectile],
            geometry: AbilityGeometry::Projectile { speed: 20.0 },
            cast_mode: crate::abilities::AbilityCastMode::Instant,
            echo: false,
            params: AbilityParams {
                potency: 80.0,
                area: 0.0,
                range: 25.0,
                cast_time: 0.0,
                cooldown: 2.0,
                mana_cost: 10.0,
            },
            animation: "throw",
            impact_vfx: "catch",
            impact_delay: 0.5,
            control: None,
            payload: crate::abilities::ManifestationPayload::default(),
        };

        ReturnWord.transform_blueprint(&mut blueprint);

        // Range should be extended by 30%
        assert!((blueprint.params.range - 32.5).abs() < f32::EPSILON);
        assert!(blueprint.has_tag(AbilityTag::RepeatCompatible));
    }
}
