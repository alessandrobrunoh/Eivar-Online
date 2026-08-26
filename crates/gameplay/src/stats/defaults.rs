//! Default statistical profiles for Player and Enemy.
//!
//! Centralizing defaults here maintains consistency across spawn, persistence
//! (backfill), and testing. The values mirror those currently defined
//! in respective `impl EntityDefinition`.

use crate::stats::components::{
    CombatStats, GatheringStats, MovementStats, StatsBundleData, VitalStats,
};

/// Default statistical profile for Player.
pub fn player_defaults() -> StatsBundleData {
    StatsBundleData {
        movement: MovementStats { speed: 0.15 },
        combat: CombatStats {
            attack_power: 10.0,
            armor: 25.0,
            threat_generation: 1.0,
        },
        vital: VitalStats {
            current_health: 1500.0,
            max_health: 1500.0,
            current_mana: 100.0,
            max_mana: 100.0,
            mana_regeneration: 5.0,
        },
        gathering: GatheringStats {
            speed: 0.0,
            bonus: 0.0,
        },
    }
}

/// Default statistical profile for Enemy.
pub fn enemy_defaults() -> StatsBundleData {
    StatsBundleData {
        movement: MovementStats { speed: 0.08 },
        combat: CombatStats {
            attack_power: 20.0,
            armor: 10.0,
            threat_generation: 1.0,
        },
        vital: VitalStats {
            current_health: 50.0,
            max_health: 50.0,
            current_mana: 40.0,
            max_mana: 40.0,
            mana_regeneration: 2.0,
        },
        gathering: GatheringStats {
            speed: 0.0,
            bonus: 0.0,
        },
    }
}

/// Default statistical profile for the dragon boss.
///
/// The boss has slow server-authoritative chase movement, heavy HP for a
/// multi-phase encounter, and solid armor.
pub fn boss_defaults() -> StatsBundleData {
    StatsBundleData {
        movement: MovementStats { speed: 0.05 },
        combat: CombatStats {
            attack_power: 28.0,
            armor: 30.0,
            threat_generation: 1.0,
        },
        vital: VitalStats {
            current_health: 6000.0,
            max_health: 6000.0,
            current_mana: 0.0,
            max_mana: 0.0,
            mana_regeneration: 0.0,
        },
        gathering: GatheringStats {
            speed: 0.0,
            bonus: 0.0,
        },
    }
}

/// Default statistical profile for Dummy.
///
/// The Dummy is a static target with huge HP, used for testing
/// damage systems, UI targeting, and spells. It does not move and has no offensive stats.
pub fn dummy_defaults() -> StatsBundleData {
    StatsBundleData {
        movement: MovementStats { speed: 0.0 },
        combat: CombatStats {
            attack_power: 0.0,
            armor: 0.0,
            threat_generation: 1.0,
        },
        vital: VitalStats {
            current_health: 10_000.0,
            max_health: 10_000.0,
            current_mana: 0.0,
            max_mana: 0.0,
            mana_regeneration: 0.0,
        },
        gathering: GatheringStats {
            speed: 0.0,
            bonus: 0.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_defaults_start_at_full_health() {
        let stats = player_defaults();
        assert_eq!(stats.vital.current_health, stats.vital.max_health);
        assert_eq!(stats.vital.current_mana, stats.vital.max_mana);
        assert_eq!(stats.vital.max_health, 1500.0);
        assert_eq!(stats.vital.max_mana, 100.0);
        assert_eq!(stats.vital.mana_regeneration, 5.0);
        assert_eq!(stats.gathering.speed, 0.0);
        assert_eq!(stats.gathering.bonus, 0.0);
        assert_eq!(stats.combat.threat_generation, 1.0);
    }

    #[test]
    fn enemy_defaults_start_at_full_health() {
        let stats = enemy_defaults();
        assert_eq!(stats.vital.current_health, stats.vital.max_health);
    }

    #[test]
    fn dummy_defaults_start_at_full_health() {
        let stats = dummy_defaults();
        assert_eq!(stats.vital.current_health, stats.vital.max_health);
    }

    #[test]
    fn boss_defaults_start_at_full_health() {
        let stats = boss_defaults();
        assert_eq!(stats.vital.current_health, stats.vital.max_health);
    }

    #[test]
    fn boss_defaults_have_high_hp_pool() {
        let stats = boss_defaults();
        assert_eq!(stats.vital.max_health, 6000.0);
    }

    #[test]
    fn dummy_defaults_have_zero_speed() {
        let stats = dummy_defaults();
        assert_eq!(stats.movement.speed, 0.0);
    }

    #[test]
    fn dummy_defaults_have_huge_hp() {
        let stats = dummy_defaults();
        assert_eq!(stats.vital.max_health, 10_000.0);
    }
}
