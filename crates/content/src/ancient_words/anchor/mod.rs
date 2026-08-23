//! Ancient Word Anchor — roots/immobilizes target in place.
//! Adds crowd control through rooting effect.

use bevymmo_gameplay::abilities::{
    AbilityBlueprint, AbilityParams, AbilityTag, AncientWordEffect, AppliedControl, BaseAbility,
};
use bevymmo_gameplay::crowd_control::CrowdControlKind;
use bevymmo_gameplay::spells::context::SpellCastContext;
use bevymmo_props_macro::ancient_word;

#[ancient_word(
    id = "anchor",
    name = "Anchor",
    tag = Ground,
    tags = [Ground, Area],
    rune_cost = 2
)]
pub struct Anchor;

impl Anchor {
    /// Root duration in seconds.
    pub const ROOT_DURATION_SECS: f32 = 1.5;
}

impl AncientWordEffect for Anchor {
    fn post_process(
        &self,
        _ability: &dyn BaseAbility,
        _params: &AbilityParams,
        _ctx: &mut SpellCastContext,
    ) {
    }

    fn transform_blueprint(&self, blueprint: &mut AbilityBlueprint) {
        // Root effects have reduced damage but high control value
        blueprint.params.potency *= 0.75;

        blueprint.control = Some(AppliedControl {
            kind: CrowdControlKind::Root,
            duration_seconds: Self::ROOT_DURATION_SECS,
        });

        // Ensure ground tag is present
        if !blueprint.has_tag(AbilityTag::Ground) {
            blueprint.tags.push(AbilityTag::Ground);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::{AbilityGeometry, AbilityId};

    #[test]
    fn metadata_declares_ground_requirement() {
        let metadata = <Anchor as bevymmo_gameplay::abilities::AncientWord>::metadata(&Anchor);
        assert_eq!(metadata.rune_cost, 2);
        assert!(metadata.required_tags.contains(&AbilityTag::Ground));
        assert!(metadata.is_compatible_with(&[AbilityTag::Area]));
    }

    #[test]
    fn transform_adds_root_stun_and_reduces_damage() {
        let mut blueprint = AbilityBlueprint {
            ability_id: AbilityId::new("chain_anchor"),
            tags: vec![AbilityTag::Ground],
            geometry: AbilityGeometry::Circle { radius: 4.0 },
            cast_mode: crate::abilities::AbilityCastMode::Instant,
            echo: false,
            params: AbilityParams {
                potency: 80.0,
                area: 4.0,
                range: 10.0,
                cast_time: 0.3,
                cooldown: 3.0,
                mana_cost: 20.0,
            },
            animation: "anchor_cast",
            impact_vfx: "root_effect",
            impact_delay: 0.4,
            control: None,
            payload: crate::abilities::ManifestationPayload::default(),
        };

        Anchor.transform_blueprint(&mut blueprint);

        // Damage reduced by 25%
        assert!((blueprint.params.potency - 60.0).abs() < f32::EPSILON);
        let control = blueprint.control.expect("root control");
        assert_eq!(control.kind, CrowdControlKind::Root);
        assert!((control.duration_seconds - 1.5).abs() < f32::EPSILON);
        assert!(blueprint.has_tag(AbilityTag::Ground));
    }

    #[test]
    fn root_duration_is_constant() {
        assert_eq!(Anchor::ROOT_DURATION_SECS, 1.5);
    }
}
