//! Goblin enemy archetype.
//!
//! Frail raider: same Cleave the player sword uses, tighter aggro, leash back
//! to camp.

use crate::ability_definitions::cleave::Cleave;
use crate::placeables::enemy;

#[enemy(
    id = "mob_goblin",
    type = Normal,
    name = "Goblin",
    icon = "👺",
    asset = "models/creatures/goblin.glb",
    stats(health = 30.0, mana = 40.0, mana_regen = 2.0, attack_power = 20.0, armor = 8.0, speed = 0.08),
    aggro = 8.0,
    leash_aggro = 20.0,
    respawn = 10.0,
    abilities = [Cleave],
)]
pub struct Goblin;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::AbilityId;
    use crate::placeables::{
        AcquirePolicy, AggroOrigin, EnemyPlaceable, PlaceableDefinition, ThreatPolicy,
    };

    #[test]
    fn goblin_uses_the_same_cleave_as_the_sword() {
        let config = Goblin.enemy_config();
        assert_eq!(config.abilities.len(), 1);
        assert_eq!(config.abilities[0].ability_id, AbilityId::new(Cleave::ID));
        assert!(config.abilities[0].inscription.is_empty());
        assert!(!config
            .abilities
            .iter()
            .any(|entry| entry.ability_id.as_str() == "fireball"));
    }

    #[test]
    fn goblin_stats_and_leash_are_authored() {
        let config = Goblin.enemy_config();
        assert_eq!(config.stats.vital.max_health, 30.0);
        assert_eq!(config.stats.vital.current_health, 30.0);
        assert_eq!(config.stats.combat.armor, 8.0);
        assert_eq!(config.aggro, 8.0);
        assert_eq!(config.leash_aggro, 20.0);
        assert_eq!(config.respawn_seconds, 10.0);
        assert_eq!(Goblin.id().as_str(), "mob_goblin");
        assert_eq!(Goblin::ID, "mob_goblin");
    }

    #[test]
    fn goblin_uses_normal_acquire_defaults() {
        let config = Goblin.enemy_config();
        assert_eq!(config.acquire, AcquirePolicy::Proximity);
        assert_eq!(config.origin, AggroOrigin::Body);
        assert_eq!(config.threat, ThreatPolicy::Nearest);
        assert_eq!(
            config.abilities[0].use_when,
            crate::placeables::AbilityUse::default()
        );
    }
}
