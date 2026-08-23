//! Corpse and mob loot: drop tables, player-gear dumps, bag lifetime.
//!
//! Pure functions. The SpacetimeDB module rolls and persists; the client only
//! renders the replicated bag.

use std::ops::RangeInclusive;

use crate::items::components::{EquipSlot, Equipment, Inventory};
use crate::items::instance::ItemInstance;
use crate::items::registry::ItemId;

/// How long a loot bag stays in the world before it and its contents are
/// destroyed. Ten minutes.
pub const LOOT_BAG_LIFETIME_SECS: u64 = 600;

/// Horizontal (XZ) reach to open or take from a bag, in world units.
pub const LOOT_INTERACT_RANGE: f32 = 6.0;

/// One independent chance to drop a catalogue item.
#[derive(Debug, Clone, PartialEq)]
pub struct LootDrop {
    pub item_id: ItemId,
    /// `0` never drops, `100` always drops. Rolls are independent.
    pub chance_percent: u8,
    pub quantity: u32,
}

impl LootDrop {
    /// One piece, with `chance_percent` clamped to `0..=100`.
    pub fn new(item_id: impl Into<ItemId>, chance_percent: u8) -> Self {
        Self {
            item_id: item_id.into(),
            chance_percent: chance_percent.min(100),
            quantity: 1,
        }
    }
}

/// Authored drop table on an enemy archetype.
#[derive(Debug, Clone, PartialEq)]
pub struct LootTable {
    /// Inclusive gold roll. Empty of meaning when `start > end`; [`roll_loot`]
    /// then yields the start.
    pub gold: RangeInclusive<u64>,
    pub drops: Vec<LootDrop>,
}

/// What one roll of a [`LootTable`] produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RolledLoot {
    pub gold: u64,
    pub items: Vec<(ItemId, u32)>,
}

impl RolledLoot {
    pub fn is_empty(&self) -> bool {
        self.gold == 0 && self.items.is_empty()
    }
}

/// Rolls gold uniformly in `table.gold` and each drop independently.
///
/// `next_u32` is the entropy source so tests can inject a sequence. A roll of
/// `n % 100` succeeds when `n % 100 < chance_percent`. Chance `0` is skipped
/// without consuming a roll; chance `100` always drops without rolling.
pub fn roll_loot(table: &LootTable, mut next_u32: impl FnMut() -> u32) -> RolledLoot {
    let gold = roll_inclusive(*table.gold.start(), *table.gold.end(), &mut next_u32);
    let mut items = Vec::new();
    for drop in &table.drops {
        if drop.chance_percent == 0 || drop.quantity == 0 {
            continue;
        }
        if drop.chance_percent >= 100 || (next_u32() % 100) < u32::from(drop.chance_percent) {
            items.push((drop.item_id.clone(), drop.quantity));
        }
    }
    RolledLoot { gold, items }
}

fn roll_inclusive(min: u64, max: u64, next_u32: &mut impl FnMut() -> u32) -> u64 {
    if max <= min {
        return min;
    }
    let span = max - min + 1;
    min + u64::from(next_u32()) % span
}

/// Equipment first (`EquipSlot::ALL` order), then inventory slots 0..N.
/// Empty slots are skipped. Instances are cloned as-is so inscriptions and
/// ability picks survive the trip through a loot bag.
pub fn collect_player_drops(inventory: &Inventory, equipment: &Equipment) -> Vec<ItemInstance> {
    let mut drops = Vec::new();
    for slot in EquipSlot::ALL {
        if let Some(item) = equipment.get(slot).clone() {
            drops.push(item);
        }
    }
    for slot in &inventory.slots {
        if let Some(item) = slot.clone() {
            drops.push(item);
        }
    }
    drops
}

/// Whether a bag whose `expires_at` is `expires_at_micros` is gone at `now`.
///
/// Equal counts as expired so a bag that lasts exactly 600s does not survive
/// the tick that lands on the deadline.
pub fn loot_bag_expired(now_micros: i64, expires_at_micros: i64) -> bool {
    now_micros >= expires_at_micros
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::inscription::{SlotInscription, WeaponInscription};
    use crate::abilities::{AbilityId, AbilitySelection, RootWordId};
    use crate::items::instance::ItemInstanceId;

    fn seq(values: &'static [u32]) -> impl FnMut() -> u32 {
        let mut iter = values.iter().copied();
        let last = values.last().copied().unwrap_or(0);
        move || iter.next().unwrap_or(last)
    }

    fn wood(id: u64, quantity: u32) -> ItemInstance {
        let mut instance = ItemInstance::new(ItemId::new("wood"));
        instance.instance_id = ItemInstanceId(id);
        instance.quantity = quantity;
        instance
    }

    fn inscribed_sword(id: u64) -> ItemInstance {
        let mut instance = ItemInstance::new(ItemId::new("sword"));
        instance.instance_id = ItemInstanceId(id);
        instance.ability_selection = AbilitySelection {
            primary: Some(AbilityId::new("cleave")),
            secondary: Some(AbilityId::new("lunge")),
            ultimate: None,
        };
        instance.root_inscription = Some(WeaponInscription {
            root_word: Some(RootWordId::new("flame")),
            primary: SlotInscription::default(),
            secondary: SlotInscription::default(),
            ultimate: SlotInscription::default(),
        });
        instance
    }

    #[test]
    fn gold_roll_stays_inside_the_inclusive_range() {
        let table = LootTable {
            gold: 3..=7,
            drops: Vec::new(),
        };
        assert_eq!(roll_loot(&table, seq(&[0])).gold, 3);
        assert_eq!(roll_loot(&table, seq(&[4])).gold, 7);
        assert_eq!(roll_loot(&table, seq(&[5])).gold, 3);
    }

    #[test]
    fn inverted_gold_range_yields_the_start() {
        let table = LootTable {
            gold: std::ops::RangeInclusive::new(9, 2),
            drops: Vec::new(),
        };
        assert_eq!(roll_loot(&table, seq(&[99])).gold, 9);
    }

    #[test]
    fn chance_zero_never_drops_and_does_not_consume_a_roll() {
        let table = LootTable {
            gold: 1..=1,
            drops: vec![LootDrop::new("wood", 0), LootDrop::new("copper", 100)],
        };
        let rolled = roll_loot(&table, seq(&[0]));
        assert_eq!(rolled.items.len(), 1);
        assert_eq!(rolled.items[0].0.as_str(), "copper");
    }

    #[test]
    fn chance_hundred_always_drops() {
        let table = LootTable {
            gold: 0..=0,
            drops: vec![LootDrop::new("sword", 100)],
        };
        let rolled = roll_loot(&table, seq(&[99]));
        assert_eq!(rolled.items.len(), 1);
        assert_eq!(rolled.items[0].0.as_str(), "sword");
    }

    #[test]
    fn independent_drops_can_both_hit() {
        let table = LootTable {
            gold: 0..=0,
            drops: vec![LootDrop::new("wood", 40), LootDrop::new("copper", 15)],
        };
        // gold 0..=0 consumes no roll; wood 10 < 40; copper 10 < 15.
        let rolled = roll_loot(&table, seq(&[10, 10]));
        assert_eq!(rolled.items.len(), 2);
    }

    #[test]
    fn independent_drops_can_miss() {
        let table = LootTable {
            gold: 0..=0,
            drops: vec![LootDrop::new("wood", 40), LootDrop::new("copper", 15)],
        };
        let rolled = roll_loot(&table, seq(&[40, 15]));
        assert!(rolled.items.is_empty());
    }

    #[test]
    fn collect_player_drops_keeps_inscription_and_ability_picks() {
        let equipment = Equipment {
            weapon: Some(inscribed_sword(7)),
            ..Equipment::default()
        };
        let mut inventory = Inventory::default();
        inventory.slots[2] = Some(wood(11, 4));

        let drops = collect_player_drops(&inventory, &equipment);
        assert_eq!(drops.len(), 2);
        assert_eq!(drops[0].instance_id, ItemInstanceId(7));
        assert_eq!(
            drops[0]
                .root_inscription
                .as_ref()
                .and_then(|inscription| inscription.root_word.as_ref())
                .map(RootWordId::as_str),
            Some("flame")
        );
        assert_eq!(
            drops[0]
                .ability_selection
                .primary
                .as_ref()
                .map(AbilityId::as_str),
            Some("cleave")
        );
        assert_eq!(drops[1].instance_id, ItemInstanceId(11));
        assert_eq!(drops[1].quantity, 4);
    }

    #[test]
    fn collect_player_drops_skips_empty_slots() {
        let drops = collect_player_drops(&Inventory::default(), &Equipment::default());
        assert!(drops.is_empty());
    }

    #[test]
    fn equipment_comes_before_inventory() {
        let equipment = Equipment {
            shoes: Some(ItemInstance::new(ItemId::new("swift_boots"))),
            ..Equipment::default()
        };
        let mut inventory = Inventory::default();
        inventory.slots[0] = Some(wood(1, 1));
        let drops = collect_player_drops(&inventory, &equipment);
        assert_eq!(drops[0].item_id.as_str(), "swift_boots");
        assert_eq!(drops[1].item_id.as_str(), "wood");
    }

    #[test]
    fn bag_expires_on_or_after_the_deadline() {
        assert!(loot_bag_expired(600, 600));
        assert!(loot_bag_expired(601, 600));
        assert!(!loot_bag_expired(599, 600));
    }

    #[test]
    fn lifetime_is_ten_minutes() {
        assert_eq!(LOOT_BAG_LIFETIME_SECS, 600);
    }
}
