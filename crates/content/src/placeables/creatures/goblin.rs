//! Goblin enemy archetype.
//!
//! Frail raider: same Cleave the player sword uses, tighter aggro, leash back
//! to camp.

use crate::ability_definitions::cleave::Cleave;
use crate::item_definitions::materials::copper::Copper;
use crate::item_definitions::materials::wood::Wood;
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
    loot(
        gold = 1..5,
        items = [
            (Wood, 40),
            (Copper, 15),
        ],
    ),
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

    #[test]
    fn goblin_drops_gold_and_materials() {
        let loot = Goblin
            .enemy_config()
            .loot
            .expect("goblin has a loot table");
        assert_eq!(*loot.gold.start(), 1);
        assert_eq!(*loot.gold.end(), 5);
        assert_eq!(loot.drops.len(), 2);
        assert_eq!(loot.drops[0].item_id.as_str(), "wood");
        assert_eq!(loot.drops[0].chance_percent, 40);
        assert_eq!(loot.drops[1].item_id.as_str(), "copper");
        assert_eq!(loot.drops[1].chance_percent, 15);
    }
}
