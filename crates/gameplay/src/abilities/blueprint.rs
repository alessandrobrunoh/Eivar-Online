//! Derived ability blueprint shared by preview and execution.
//!
//! A blueprint is not persisted. It is rebuilt from the base ability, the item
//! and later the item's Root Words/Ancient Words. Persisted state contains only
//! stable ids and selections.

use super::base_ability::{
    AbilityCastMode, AbilityGeometry, AbilityId, AbilityParams, AbilityTag, AppliedControl,
    BaseAbility,
};
use crate::crowd_control::CrowdControlKind;
use crate::effects::{ApplyStatusEffect, DamageEffect, EffectSpec, HealEffect, StatusId};

fn status_id_for_control(kind: CrowdControlKind) -> StatusId {
    match kind {
        CrowdControlKind::Stun => StatusId::new("stun"),
        CrowdControlKind::Root => StatusId::new("root"),
        CrowdControlKind::Silence => StatusId::new("silence"),
    }
}

/// What the Root Word makes the gesture *do*. Geometry stays on the ability;
/// this is the payload written by `RootWordEffect::apply_to_blueprint`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ManifestationKind {
    #[default]
    Damage,
    Heal,
}

/// Neutral effect identity carried through preview and authoritative cast.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ManifestationPayload {
    pub kind: ManifestationKind,
    pub status_ids: Vec<StatusId>,
}

impl ManifestationPayload {
    pub fn damage(status_ids: impl IntoIterator<Item = &'static str>) -> Self {
        Self {
            kind: ManifestationKind::Damage,
            status_ids: status_ids.into_iter().map(StatusId::new).collect(),
        }
    }

    pub fn heal(status_ids: impl IntoIterator<Item = &'static str>) -> Self {
        Self {
            kind: ManifestationKind::Heal,
            status_ids: status_ids.into_iter().map(StatusId::new).collect(),
        }
    }

    /// Effect specs for this payload at `potency`. Statuses are applied after
    /// the primary hit so a burn lands on a living target that just took damage.
    pub fn effect_specs(&self, potency: f32) -> Vec<EffectSpec> {
        let mut effects = vec![match self.kind {
            ManifestationKind::Damage => EffectSpec::Damage(DamageEffect { amount: potency }),
            ManifestationKind::Heal => EffectSpec::Heal(HealEffect { amount: potency }),
        }];
        for status_id in &self.status_ids {
            effects.push(EffectSpec::ApplyStatus(ApplyStatusEffect {
                status_id: status_id.clone(),
                duration_override_seconds: None,
                potency: 1.0,
            }));
        }
        effects
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AbilityBlueprint {
    pub ability_id: AbilityId,
    pub tags: Vec<AbilityTag>,
    pub geometry: AbilityGeometry,
    pub cast_mode: AbilityCastMode,
    /// Ancient Word Echo repeats the manifestation once. Not a cast mode.
    pub echo: bool,
    pub params: AbilityParams,
    pub animation: &'static str,
    pub impact_vfx: &'static str,
    pub impact_delay: f32,
    pub control: Option<AppliedControl>,
    /// Written by the Root Word. Empty/damage is the neutral pre-root state.
    pub payload: ManifestationPayload,
}

impl AbilityBlueprint {
    pub fn from_base_ability<T: BaseAbility + ?Sized>(ability: &T) -> Self {
        Self {
            ability_id: ability.id(),
            tags: ability.tags().to_vec(),
            geometry: ability.geometry(),
            cast_mode: ability.cast_mode(),
            echo: false,
            params: ability.base_params(),
            animation: ability.animation(),
            impact_vfx: ability.impact_vfx(),
            impact_delay: ability.impact_delay(),
            control: ability.control(),
            payload: ManifestationPayload::default(),
        }
    }

    pub fn has_tag(&self, tag: AbilityTag) -> bool {
        self.tags.contains(&tag)
    }

    /// Effects the authoritative cast must emit: Root Word payload, plus the
    /// gesture's impact control when this is still a damaging hit.
    pub fn payload_effects(&self) -> Vec<EffectSpec> {
        let mut effects = self.payload.effect_specs(self.params.potency);
        if let Some(control) = self.control {
            if control.duration_seconds > 0.0 && self.payload.kind == ManifestationKind::Damage {
                effects.push(EffectSpec::ApplyStatus(ApplyStatusEffect {
                    status_id: status_id_for_control(control.kind),
                    duration_override_seconds: Some(control.duration_seconds),
                    potency: 1.0,
                }));
            }
        }
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flame_payload_is_damage_plus_burn() {
        let specs = ManifestationPayload::damage(["burn"]).effect_specs(165.0);
        assert!(matches!(
            specs[0],
            EffectSpec::Damage(DamageEffect { amount }) if (amount - 165.0).abs() < f32::EPSILON
        ));
        assert!(matches!(
            &specs[1],
            EffectSpec::ApplyStatus(effect) if effect.status_id.as_str() == "burn"
        ));
        assert_eq!(specs.len(), 2);
    }

    #[test]
    fn life_payload_is_heal_without_burn() {
        let specs = ManifestationPayload::heal([]).effect_specs(165.0);
        assert!(matches!(specs[0], EffectSpec::Heal(_)));
        assert_eq!(specs.len(), 1);
    }

    #[test]
    fn frost_payload_is_damage_plus_slow() {
        let specs = ManifestationPayload::damage(["slow"]).effect_specs(100.0);
        assert!(matches!(specs[0], EffectSpec::Damage(_)));
        assert!(matches!(
            &specs[1],
            EffectSpec::ApplyStatus(effect) if effect.status_id.as_str() == "slow"
        ));
    }

    #[test]
    fn heal_does_not_keep_the_gesture_stun() {
        let blueprint = AbilityBlueprint {
            ability_id: AbilityId::new("test"),
            tags: vec![],
            geometry: AbilityGeometry::Circle { radius: 4.0 },
            cast_mode: AbilityCastMode::Instant,
            echo: false,
            params: AbilityParams {
                potency: 50.0,
                area: 4.0,
                range: 0.0,
                cast_time: 0.0,
                cooldown: 1.0,
                mana_cost: 0.0,
            },
            animation: "a",
            impact_vfx: "v",
            impact_delay: 0.0,
            control: Some(AppliedControl {
                kind: CrowdControlKind::Stun,
                duration_seconds: 0.8,
            }),
            payload: ManifestationPayload::heal([]),
        };
        let specs = blueprint.payload_effects();
        assert_eq!(specs.len(), 1);
        assert!(matches!(specs[0], EffectSpec::Heal(_)));
    }

    #[test]
    fn damaging_hit_emits_root_when_control_is_root() {
        let blueprint = AbilityBlueprint {
            ability_id: AbilityId::new("test"),
            tags: vec![],
            geometry: AbilityGeometry::Circle { radius: 4.0 },
            cast_mode: AbilityCastMode::Instant,
            echo: false,
            params: AbilityParams {
                potency: 50.0,
                area: 4.0,
                range: 0.0,
                cast_time: 0.0,
                cooldown: 1.0,
                mana_cost: 0.0,
            },
            animation: "a",
            impact_vfx: "v",
            impact_delay: 0.0,
            control: Some(AppliedControl {
                kind: CrowdControlKind::Root,
                duration_seconds: 1.5,
            }),
            payload: ManifestationPayload::damage([]),
        };
        let specs = blueprint.payload_effects();
        assert!(matches!(
            &specs[1],
            EffectSpec::ApplyStatus(effect)
                if effect.status_id.as_str() == "root"
                    && effect.duration_override_seconds == Some(1.5)
        ));
    }
}
