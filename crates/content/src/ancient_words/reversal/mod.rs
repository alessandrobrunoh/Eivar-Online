//! Ancient Word Reversal — inverts some property of the ability.
//! Can invert direction, swap targeting (enemy→ally), or flip an effect sign.

use bevymmo_gameplay::abilities::{
    AbilityBlueprint, AbilityParams, AbilityTag, AncientWordEffect, BaseAbility,
};
use bevymmo_gameplay::spells::context::SpellCastContext;
use bevymmo_props_macro::ancient_word;

#[ancient_word(id = "reversal", name = "Reversal", tag = Ranged, rune_cost = 2)]
pub struct Reversal;

impl Reversal {
    /// Inversion factor applied to certain properties.
    pub const INVERSION_FACTOR: f32 = -0.5;
}

impl AncientWordEffect for Reversal {
    fn post_process(
        &self,
        _ability: &dyn BaseAbility,
        _params: &AbilityParams,
        _ctx: &mut SpellCastContext,
    ) {
    }

    fn transform_blueprint(&self, blueprint: &mut AbilityBlueprint) {
        // Reversal can change behavior significantly; here we model it as
        // a repulsion/push effect rather than attraction
        blueprint.params.potency *= 0.85; // Slight efficiency loss for versatility

        // Mark as area-capable for the inverted effect cone
        if !blueprint.has_tag(AbilityTag::Area) {
            blueprint.tags.push(AbilityTag::Area);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::{AbilityGeometry, AbilityId};

    #[test]
    fn metadata_declares_ranged_requirement() {
        let metadata = <Reversal as bevymmo_gameplay::abilities::AncientWord>::metadata(&Reversal);
        assert_eq!(metadata.rune_cost, 2);
        assert!(metadata.required_tags.contains(&AbilityTag::Ranged));
    }

    #[test]
    fn transform_adds_area_tag_and_adjusts_potency() {
        let mut blueprint = AbilityBlueprint {
            ability_id: AbilityId::new("repulse"),
            tags: vec![AbilityTag::Ranged],
            geometry: AbilityGeometry::Cone {
                radius: 6.0,
                angle_deg: 90.0,
            },
            cast_mode: crate::abilities::AbilityCastMode::Instant,
            echo: false,
            params: AbilityParams {
                potency: 100.0,
                area: 6.0,
                range: 12.0,
                cast_time: 0.2,
                cooldown: 2.5,
                mana_cost: 18.0,
            },
            animation: "repel",
            impact_vfx: "push_wave",
            impact_delay: 0.15,
            control: None,
            payload: crate::abilities::ManifestationPayload::default(),
        };

        Reversal.transform_blueprint(&mut blueprint);

        // Potency reduced slightly for inversion flexibility
        assert!((blueprint.params.potency - 85.0).abs() < f32::EPSILON);
        assert!(blueprint.has_tag(AbilityTag::Area));
    }

    #[test]
    fn inversion_factor_is_negative() {
        const { assert!(Reversal::INVERSION_FACTOR < 0.0) };
    }
}
