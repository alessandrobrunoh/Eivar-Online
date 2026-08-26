//! Shared combat formulas.
//!
//! Pure functions, easy to test, used by stats systems and spells.

use crate::stats::components::CombatStats;

/// The caster does not have enough current mana for this cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsufficientMana;

/// Effective damage after target armor reduction.
///
/// `raw_damage * (1 - armor_damage_reduction)`, clamped to `>= 0`.
/// Negative damage input never heals the target.
pub fn damage_after_armor(raw_damage: f32, target_combat: &CombatStats) -> f32 {
    (raw_damage * (1.0 - target_combat.armor_damage_reduction())).max(0.0)
}

/// Result of resolving raw damage against a pure shield and armor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShieldDamageResult {
    pub remaining_shield: f32,
    pub health_damage: f32,
}

/// Consumes shield before applying armor to overflow damage.
///
/// Armor is intentionally applied only to damage that remains after the shield
/// is depleted.
pub fn damage_after_shield(
    raw_damage: f32,
    current_shield: f32,
    target_combat: &CombatStats,
) -> ShieldDamageResult {
    let shield = current_shield.max(0.0);
    let damage = raw_damage.max(0.0);
    let absorbed = damage.min(shield);

    ShieldDamageResult {
        remaining_shield: shield - absorbed,
        health_damage: damage_after_armor(damage - absorbed, target_combat),
    }
}

/// Advances a temporary shield timer, removing it at or before zero.
pub fn shield_remaining_after_tick(remaining_seconds: f32, dt: f32) -> Option<f32> {
    let remaining = remaining_seconds - dt.max(0.0);
    (remaining > 0.0).then_some(remaining)
}

/// Whether `current` mana can pay `cost`.
///
/// A non-positive cost is always affordable (free casts, empty blueprints).
pub fn can_afford_mana(current: f32, cost: f32) -> bool {
    cost <= 0.0 || current >= cost
}

/// Spends `cost` from `current` mana, or refuses.
///
/// A non-positive cost is a no-op and returns `current` unchanged.
pub fn spend_mana(current: f32, cost: f32) -> Result<f32, InsufficientMana> {
    if !can_afford_mana(current, cost) {
        return Err(InsufficientMana);
    }
    if cost <= 0.0 {
        return Ok(current);
    }
    Ok(current - cost)
}

/// Refills mana over `dt` seconds, clamped to `[0, max]`.
///
/// Non-positive regeneration or `dt` leaves `current` unchanged (still
/// clamped, in case `max` shrank).
pub fn regenerated_mana(current: f32, max: f32, regen_per_second: f32, dt: f32) -> f32 {
    let max = max.max(0.0);
    if regen_per_second <= 0.0 || dt <= 0.0 {
        return current.clamp(0.0, max);
    }
    (current + regen_per_second * dt).clamp(0.0, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_respects_target_armor() {
        let target = CombatStats {
            attack_power: 0.0,
            armor: 100.0,
            threat_generation: 1.0,
        };
        // 100 armor = 50% reduction
        assert_eq!(damage_after_armor(10.0, &target), 5.0);
    }

    #[test]
    fn damage_never_heals_or_goes_below_zero() {
        let target = CombatStats {
            attack_power: 0.0,
            armor: 0.0,
            threat_generation: 1.0,
        };
        assert_eq!(damage_after_armor(-10.0, &target), 0.0);
    }

    #[test]
    fn damage_with_zero_armor_is_unchanged() {
        let target = CombatStats {
            attack_power: 0.0,
            armor: 0.0,
            threat_generation: 1.0,
        };
        assert_eq!(damage_after_armor(25.0, &target), 25.0);
    }

    #[test]
    fn shield_absorbs_damage_before_health_without_armor() {
        let target = CombatStats {
            attack_power: 0.0,
            armor: 100.0,
            threat_generation: 1.0,
        };

        let result = damage_after_shield(40.0, 100.0, &target);

        assert_eq!(result.remaining_shield, 60.0);
        assert_eq!(result.health_damage, 0.0);
    }

    #[test]
    fn shield_overflow_is_the_only_damage_mitigated_by_armor() {
        let target = CombatStats {
            attack_power: 0.0,
            armor: 100.0,
            threat_generation: 1.0,
        };

        let result = damage_after_shield(120.0, 100.0, &target);

        assert_eq!(result.remaining_shield, 0.0);
        assert_eq!(result.health_damage, 10.0);
    }

    #[test]
    fn shield_damage_is_clamped_and_never_increases_the_pool() {
        let target = CombatStats {
            attack_power: 0.0,
            armor: 0.0,
            threat_generation: 1.0,
        };

        assert_eq!(
            damage_after_shield(-10.0, 100.0, &target).remaining_shield,
            100.0
        );
        assert_eq!(
            damage_after_shield(10.0, -5.0, &target).remaining_shield,
            0.0
        );
    }

    #[test]
    fn shield_expires_at_the_duration_boundary() {
        let remaining = shield_remaining_after_tick(5.0, 4.9).expect("shield is still active");
        assert!((remaining - 0.1).abs() < f32::EPSILON);
        assert_eq!(shield_remaining_after_tick(5.0, 5.0), None);
        assert_eq!(shield_remaining_after_tick(5.0, 6.0), None);
    }

    #[test]
    fn shield_timer_ignores_negative_elapsed_time() {
        assert_eq!(shield_remaining_after_tick(5.0, -1.0), Some(5.0));
    }

    #[test]
    fn spend_refuses_when_current_is_strictly_below_cost() {
        assert_eq!(spend_mana(9.0, 10.0), Err(InsufficientMana));
        assert!(!can_afford_mana(9.0, 10.0));
    }

    #[test]
    fn spend_accepts_an_exact_pool() {
        assert_eq!(spend_mana(10.0, 10.0), Ok(0.0));
        assert!(can_afford_mana(10.0, 10.0));
    }

    #[test]
    fn spend_of_zero_or_negative_cost_is_a_noop() {
        assert_eq!(spend_mana(7.0, 0.0), Ok(7.0));
        assert_eq!(spend_mana(7.0, -3.0), Ok(7.0));
        assert!(can_afford_mana(0.0, 0.0));
        assert!(can_afford_mana(0.0, -1.0));
    }

    #[test]
    fn regen_adds_rate_times_dt_and_does_not_exceed_max() {
        assert_eq!(regenerated_mana(90.0, 100.0, 5.0, 1.0), 95.0);
        assert_eq!(regenerated_mana(99.0, 100.0, 5.0, 1.0), 100.0);
    }

    #[test]
    fn regen_is_a_noop_when_rate_or_dt_is_not_positive() {
        assert_eq!(regenerated_mana(40.0, 100.0, 0.0, 1.0), 40.0);
        assert_eq!(regenerated_mana(40.0, 100.0, 5.0, 0.0), 40.0);
        assert_eq!(regenerated_mana(40.0, 100.0, -1.0, 1.0), 40.0);
    }

    #[test]
    fn regen_still_clamps_when_it_does_not_add() {
        assert_eq!(regenerated_mana(150.0, 100.0, 0.0, 1.0), 100.0);
        assert_eq!(regenerated_mana(-4.0, 100.0, 0.0, 1.0), 0.0);
    }
}
