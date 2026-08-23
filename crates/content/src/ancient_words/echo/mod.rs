//! Ancient Word Echo — repeats the ability execution.
//! Uses the EchoCompatible tag and sets blueprint execution to Echo.

use bevymmo_gameplay::abilities::{
    AbilityBlueprint, AbilityParams, AncientWordEffect, BaseAbility,
};
use bevymmo_gameplay::spells::context::SpellCastContext;
use bevymmo_props_macro::ancient_word;

#[ancient_word(
    id = "echo",
    name = "Echo",
    tag = EchoCompatible,
    tags = [EchoCompatible, Ranged, Area],
    rune_cost = 2
)]
pub struct Echo;

impl Echo {
    /// Potency multiplier for the echoed attack (reduced from original).
    pub const ECHO_MULTIPLIER: f32 = 0.6;
}

impl AncientWordEffect for Echo {
    fn post_process(
        &self,
        _ability: &dyn BaseAbility,
        _params: &AbilityParams,
        _ctx: &mut SpellCastContext,
    ) {
    }

    fn transform_blueprint(&self, blueprint: &mut AbilityBlueprint) {
        // Set execution mode to echo
        blueprint.echo = true;

        // Reduce potency for the echo (it's a bonus attack)
        blueprint.params.potency *= Self::ECHO_MULTIPLIER;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::{AbilityGeometry, AbilityId, AbilityTag};

    #[test]
    fn metadata_declares_echo_compatibility() {
        let metadata = <Echo as bevymmo_gameplay::abilities::AncientWord>::metadata(&Echo);
        assert_eq!(metadata.rune_cost, 2);
        assert!(metadata.required_tags.contains(&AbilityTag::EchoCompatible));
        assert!(metadata.is_compatible_with(&[AbilityTag::Ranged, AbilityTag::Area]));
    }

    #[test]
    fn transform_sets_execution_to_echo_and_reduces_potency() {
        let mut blueprint = AbilityBlueprint {
            ability_id: AbilityId::new("test_ability"),
            tags: vec![AbilityTag::EchoCompatible],
            geometry: AbilityGeometry::Projectile { speed: 20.0 },
            cast_mode: crate::abilities::AbilityCastMode::Instant,
            echo: false,
            params: AbilityParams {
                potency: 100.0,
                area: 0.0,
                range: 30.0,
                cast_time: 0.0,
                cooldown: 2.0,
                mana_cost: 15.0,
            },
            animation: "attack",
            impact_vfx: "impact",
            impact_delay: 0.3,
            control: None,
            payload: crate::abilities::ManifestationPayload::default(),
        };

        Echo.transform_blueprint(&mut blueprint);

        assert!(blueprint.echo);
        // Echo reduces potency to 60% of original
        assert!((blueprint.params.potency - 60.0).abs() < 0.001);
    }
}
