//! Runtime components for game stats.
//!
//! Stats are split into separate ECS components to keep queries granular
//! and reduce coupling:
//! - [`MovementStats`] — movement speed and parameters
//! - [`CombatStats`] — attack power, armor, and threat generation
//! - [`VitalStats`] — health, mana, and regeneration
//! - [`GatheringStats`] — gathering speed and bonus
//!
//! [`StatsBundleData`] is a DTO aggregate used at spawn boundaries,
//! configuration, and persistence; it does not replace runtime ECS components.

// `#[reflect(Component)]` expands to a reference to this type.
#[cfg(feature = "bevy")]
use bevy_ecs::reflect::ReflectComponent;

use serde::{Deserialize, Serialize};

/// Movement stats.
#[cfg_attr(
    feature = "bevy",
    derive(bevy_ecs::component::Component, bevy_reflect::Reflect)
)]
#[cfg_attr(feature = "bevy", reflect(Component))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct MovementStats {
    pub speed: f32,
}

/// Combat stats.
#[cfg_attr(
    feature = "bevy",
    derive(bevy_ecs::component::Component, bevy_reflect::Reflect)
)]
#[cfg_attr(feature = "bevy", reflect(Component))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CombatStats {
    pub attack_power: f32,
    pub armor: f32,
    /// Multiplier on threat this entity generates when it deals damage.
    /// `1.0` is a normal hit; tanks author `> 1`.
    pub threat_generation: f32,
}

impl CombatStats {
    /// Fraction of incoming damage prevented by armor.
    ///
    /// Formula: `armor / (armor + 100)`, clamped `[0, 1]`.
    /// Negative armor values are treated as 0.
    pub fn armor_damage_reduction(&self) -> f32 {
        let armor = self.armor.max(0.0);
        (armor / (armor + 100.0)).clamp(0.0, 1.0)
    }
}

/// Vital stats: current/max health, mana, and regeneration.
#[cfg_attr(
    feature = "bevy",
    derive(bevy_ecs::component::Component, bevy_reflect::Reflect)
)]
#[cfg_attr(feature = "bevy", reflect(Component))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct VitalStats {
    pub current_health: f32,
    pub max_health: f32,
    pub current_mana: f32,
    pub max_mana: f32,
    pub mana_regeneration: f32,
}

impl VitalStats {
    /// True if current health is depleted.
    pub fn is_dead(&self) -> bool {
        self.current_health <= 0.0
    }

    /// Adjusts `current_health` not to exceed `max_health` and not to drop
    /// below zero. Useful after modifications to `max_health` or loading from DB.
    pub fn clamp_health(&mut self) {
        self.current_health = self.current_health.clamp(0.0, self.max_health);
    }

    /// Adjusts `current_mana` not to exceed `max_mana` and not to drop below
    /// zero. Useful after modifications to `max_mana` or loading from DB.
    pub fn clamp_mana(&mut self) {
        self.current_mana = self.current_mana.clamp(0.0, self.max_mana.max(0.0));
    }

    /// Spends `cost` current mana, or refuses if the pool cannot cover it.
    ///
    /// A non-positive cost is a no-op. See [`super::formulas::spend_mana`].
    pub fn try_spend_mana(&mut self, cost: f32) -> Result<(), super::formulas::InsufficientMana> {
        self.current_mana = super::formulas::spend_mana(self.current_mana, cost)?;
        Ok(())
    }

    /// Applies [`super::formulas::regenerated_mana`] for `dt` seconds.
    pub fn regenerate_mana(&mut self, dt: f32) {
        self.current_mana = super::formulas::regenerated_mana(
            self.current_mana,
            self.max_mana,
            self.mana_regeneration,
            dt,
        );
    }
}

/// Pure shield pool, kept separate from health and armor.
#[cfg_attr(
    feature = "bevy",
    derive(bevy_ecs::component::Component, bevy_reflect::Reflect)
)]
#[cfg_attr(feature = "bevy", reflect(Component))]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct ShieldStats {
    pub current: f32,
    pub max: f32,
}

/// Gathering stats: channel speed rating and extra-piece bonus.
///
/// `speed` 0 is the authored channel duration; 100 halves it.
/// `bonus` 0.15 is a 15% chance of +1 extra piece.
#[cfg_attr(
    feature = "bevy",
    derive(bevy_ecs::component::Component, bevy_reflect::Reflect)
)]
#[cfg_attr(feature = "bevy", reflect(Component))]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct GatheringStats {
    pub speed: f32,
    pub bonus: f32,
}

/// Aggregate DTO for all stats.
///
/// Used for:
/// - entity defaults (`EntityDefinition`, enemy/spell definitions)
/// - serialization/persistence
/// - spawn helpers
///
/// At runtime, values live in separate ECS components; use
/// [`StatsBundleData::into_components`] to get the tuple of components
/// to insert into an entity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct StatsBundleData {
    pub movement: MovementStats,
    pub combat: CombatStats,
    pub vital: VitalStats,
    pub gathering: GatheringStats,
}

impl StatsBundleData {
    /// Constructs the bundle from the runtime components.
    pub fn from_components(
        movement: &MovementStats,
        combat: &CombatStats,
        vital: &VitalStats,
        gathering: &GatheringStats,
    ) -> Self {
        Self {
            movement: *movement,
            combat: *combat,
            vital: *vital,
            gathering: *gathering,
        }
    }

    /// Decomposes the DTO into the tuple of ECS components.
    pub fn into_components(self) -> (MovementStats, CombatStats, VitalStats, GatheringStats) {
        (self.movement, self.combat, self.vital, self.gathering)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armor_reduction_uses_the_expected_curve() {
        let combat = CombatStats {
            attack_power: 10.0,
            armor: 100.0,
            threat_generation: 1.0,
        };
        assert_eq!(combat.armor_damage_reduction(), 0.5);
    }

    #[test]
    fn armor_reduction_clamps_negative_and_high_values() {
        let negative = CombatStats {
            attack_power: 10.0,
            armor: -50.0,
            threat_generation: 1.0,
        };
        let very_high = CombatStats {
            attack_power: 10.0,
            armor: 1.0e30,
            threat_generation: 1.0,
        };
        assert_eq!(negative.armor_damage_reduction(), 0.0);
        assert_eq!(very_high.armor_damage_reduction(), 1.0);
    }

    #[test]
    fn vital_stats_clamp_health_respects_bounds() {
        let mut vital = VitalStats {
            current_health: 150.0,
            max_health: 100.0,
            current_mana: 50.0,
            max_mana: 50.0,
            mana_regeneration: 5.0,
        };
        vital.clamp_health();
        assert_eq!(vital.current_health, 100.0);

        vital.current_health = -10.0;
        vital.clamp_health();
        assert_eq!(vital.current_health, 0.0);
    }

    #[test]
    fn bundle_data_roundtrips_through_components() {
        let movement = MovementStats { speed: 0.15 };
        let combat = CombatStats {
            attack_power: 10.0,
            armor: 25.0,
            threat_generation: 1.0,
        };
        let vital = VitalStats {
            current_health: 80.0,
            max_health: 100.0,
            current_mana: 40.0,
            max_mana: 50.0,
            mana_regeneration: 5.0,
        };

        let gathering = GatheringStats {
            speed: 10.0,
            bonus: 0.15,
        };

        let bundle = StatsBundleData::from_components(&movement, &combat, &vital, &gathering);
        let (m, c, v, g) = bundle.into_components();
        assert_eq!(m, movement);
        assert_eq!(c, combat);
        assert_eq!(v, vital);
        assert_eq!(g, gathering);
    }

    fn sample_vital() -> VitalStats {
        VitalStats {
            current_health: 80.0,
            max_health: 100.0,
            current_mana: 40.0,
            max_mana: 50.0,
            mana_regeneration: 5.0,
        }
    }

    #[test]
    fn clamp_mana_respects_max_mana_not_max_health() {
        let mut vital = sample_vital();
        vital.current_mana = 80.0;
        vital.clamp_mana();
        assert_eq!(vital.current_mana, 50.0);

        vital.current_mana = -10.0;
        vital.clamp_mana();
        assert_eq!(vital.current_mana, 0.0);
    }

    #[test]
    fn try_spend_mana_scales_the_pool_or_refuses() {
        let mut vital = sample_vital();
        assert!(vital.try_spend_mana(15.0).is_ok());
        assert_eq!(vital.current_mana, 25.0);
        assert!(vital.try_spend_mana(25.0).is_ok());
        assert_eq!(vital.current_mana, 0.0);
        assert!(vital.try_spend_mana(1.0).is_err());
        assert_eq!(vital.current_mana, 0.0);
        assert!(vital.try_spend_mana(0.0).is_ok());
        assert_eq!(vital.current_mana, 0.0);
    }

    #[test]
    fn regenerate_mana_uses_rate_and_dt() {
        let mut vital = sample_vital();
        vital.regenerate_mana(1.0);
        assert_eq!(vital.current_mana, 45.0);
        vital.regenerate_mana(2.0);
        assert_eq!(vital.current_mana, 50.0);
    }
}
