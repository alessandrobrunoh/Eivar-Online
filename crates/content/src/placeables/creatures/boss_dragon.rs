//! Dragon boss archetype.
//!
//! Kit entries carry the old GROUND / AERIAL / BERSERK tables as HP-banded
//! `when` gates so [`crate::placeables::pick_ability`] drives every phase.

use crate::ability_definitions::cataclysm::Cataclysm;
use crate::ability_definitions::cinder_storm::CinderStorm;
use crate::ability_definitions::dragon_claw::DragonClaw;
use crate::ability_definitions::molten_eruption::MoltenEruption;
use crate::ability_definitions::searing_breath::SearingBreath;
use crate::ability_definitions::tail_sweep::TailSweep;
use crate::ability_definitions::wing_buffet::WingBuffet;
use crate::item_definitions::weapons::sword::sword::Sword;
use crate::placeables::enemy;

#[enemy(
    id = "boss_dragon",
    type = Boss,
    name = "Dragon",
    icon = "🐉",
    asset = "models/boss_dragon.glb",
    stats(health = 6000.0, attack_power = 28.0, armor = 30.0, speed = 0.05),
    arena = 12.0,
    aggro = 12.0,
    leash_aggro = 12.0,
    respawn = 10.0,
    enrage_after = 180.0,
    origin = Spawn,
    threat = Table,
    phases = [
        (id = "ground",  hp_below = 1.00, movement = Chase),
        (id = "aerial",  hp_below = 0.66, movement = Hover),
        (id = "berserk", hp_below = 0.33, movement = Chase),
    ],
    abilities = [
        SearingBreath(when = (targeting = Main, priority = 3, hp_above = 0.66)),
        CinderStorm(when = (targeting = Cluster(2), priority = 2, hp_above = 0.66)),
        WingBuffet(when = (targeting = Self, priority = 1, hp_above = 0.66)),
        TailSweep(when = (targeting = Self, hp_above = 0.66)),
        DragonClaw(when = (targeting = Main, hp_above = 0.66, max_range = 4.0)),
        MoltenEruption(when = (targeting = Self, hp_above = 0.33, hp_below = 0.66)),
        CinderStorm(when = (targeting = Cluster(2), hp_above = 0.33, hp_below = 0.66)),
        Cataclysm(when = (targeting = Self, priority = 20, hp_below = 0.33)),
        SearingBreath(when = (targeting = Farthest, hp_below = 0.33)),
        CinderStorm(when = (targeting = Cluster(2), hp_below = 0.33)),
        WingBuffet(when = (targeting = Self, hp_below = 0.33)),
        DragonClaw(when = (targeting = Main, hp_below = 0.33)),
    ],
    loot(
        gold = 200..400,
        items = [(Sword, 5)],
    ),
)]
pub struct BossDragon;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placeables::{
        pick_ability, AcquirePolicy, AggroOrigin, BossPlaceable, EnemyPlaceable, EnemyRank, KindId,
        PlaceableRegistry, ThreatPolicy,
    };

    #[test]
    fn pick_ability_at_hp_0_2_prefers_cataclysm() {
        let kit = BossDragon.boss_config().abilities;
        let picked = pick_ability(&kit, 3.0, 0.2, |_| true).expect("berserk pick");
        assert_eq!(picked.ability_id.as_str(), Cataclysm::ID);
        assert_eq!(picked.use_when.priority, 20);
    }

    #[test]
    fn ground_band_prefers_searing_breath() {
        let kit = BossDragon.boss_config().abilities;
        let picked = pick_ability(&kit, 8.0, 1.0, |_| true).expect("ground pick");
        assert_eq!(picked.ability_id.as_str(), SearingBreath::ID);
    }

    #[test]
    fn aerial_band_prefers_molten_eruption() {
        let kit = BossDragon.boss_config().abilities;
        let picked = pick_ability(&kit, 8.0, 0.5, |_| true).expect("aerial pick");
        assert_eq!(picked.ability_id.as_str(), MoltenEruption::ID);
    }

    #[test]
    fn boss_dragon_authored_identity_and_policies() {
        assert_eq!(BossDragon::ID, "boss_dragon");
        let enemy = BossDragon.enemy_config();
        assert_eq!(enemy.origin, AggroOrigin::Spawn);
        assert_eq!(enemy.threat, ThreatPolicy::Table);
        assert_eq!(enemy.acquire, AcquirePolicy::Proximity);
        assert_eq!(enemy.aggro, 12.0);
        assert_eq!(enemy.leash_aggro, 12.0);
        assert_eq!(enemy.rank, EnemyRank::Boss);
        assert_eq!(enemy.respawn_seconds, 10.0);
        assert_eq!(BossDragon.boss_config().arena_radius, 12.0);
    }

    #[test]
    fn boss_dragon_drops_a_purse_and_a_rare_sword() {
        let loot = BossDragon
            .enemy_config()
            .loot
            .expect("boss dragon has a loot table");
        assert_eq!(*loot.gold.start(), 200);
        assert_eq!(*loot.gold.end(), 400);
        assert_eq!(loot.drops.len(), 1);
        assert_eq!(loot.drops[0].item_id.as_str(), "sword");
        assert_eq!(loot.drops[0].chance_percent, 5);
    }

    #[test]
    fn register_puts_dragon_in_bosses_not_enemies() {
        let mut registry = PlaceableRegistry::default();
        register(&mut registry);
        let id = KindId::new(BossDragon::ID);
        assert!(
            registry.bosses.contains_key(&id),
            "boss dragon must be in registry.bosses"
        );
        assert!(
            !registry.enemies.contains_key(&id),
            "boss dragon must not be in registry.enemies"
        );
    }
}
