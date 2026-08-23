//! Corpse and mob loot bags: spawn on death, expire on the tick.

use std::time::Duration;

use bevymmo_domain::items::components::{Equipment, Inventory};
use bevymmo_domain::items::instance::{ItemInstance, ItemInstanceId};
use bevymmo_domain::loot::{
    collect_player_drops, loot_bag_expired, roll_loot, LOOT_BAG_LIFETIME_SECS, LootTable,
};
use bevymmo_domain::placeables::KindId;
use spacetimedb::rand::RngCore;
use spacetimedb::{ReducerContext, Table, Timestamp};

use crate::reducers::items::{
    item_category, load_equipment, load_inventory, next_instance_id, store_equipment,
    store_inventory,
};
use crate::rows::ItemInstanceRow;
use crate::tables::{
    enemy_ai, loot_bag, loot_bag_slot, EntityKindRow, GameEntity, LootBag, LootBagSlot,
    LootSourceRow,
};
use crate::world::placeables;

/// Drops a bag at the corpse, if anything actually fell.
pub fn on_death(ctx: &ReducerContext, entity: &GameEntity) {
    match entity.kind {
        EntityKindRow::Player => drop_player(ctx, entity),
        EntityKindRow::Enemy | EntityKindRow::Boss => drop_enemy(ctx, entity),
        _ => {}
    }
}

/// Deletes bags whose deadline has passed. Contents go with them.
pub fn step(ctx: &ReducerContext) {
    let now = ctx.timestamp;
    let now_micros = now.to_micros_since_unix_epoch();
    let due: Vec<LootBag> = ctx
        .db
        .loot_bag()
        .by_expiry()
        .filter(..=now)
        .collect();
    for bag in due {
        if loot_bag_expired(now_micros, bag.expires_at.to_micros_since_unix_epoch()) {
            delete_bag(ctx, bag.id);
        }
    }
}

fn drop_player(ctx: &ReducerContext, entity: &GameEntity) {
    let Some(character_id) = entity.owner_character_id else {
        return;
    };
    let Ok(inventory) = load_inventory(ctx, character_id) else {
        return;
    };
    let Ok(equipment) = load_equipment(ctx, character_id) else {
        return;
    };
    let drops = collect_player_drops(&inventory, &equipment);
    if drops.is_empty() {
        return;
    }
    store_inventory(ctx, character_id, &Inventory::default());
    store_equipment(ctx, character_id, &Equipment::default());
    crate::sim::combat::recalculate_effective_stats(ctx, entity.entity_id);
    spawn_bag(ctx, entity.position, 0, &drops, LootSourceRow::PlayerCorpse);
}

fn drop_enemy(ctx: &ReducerContext, entity: &GameEntity) {
    let Some(ai) = ctx.db.enemy_ai().entity_id().find(entity.entity_id) else {
        return;
    };
    let Some(table) = loot_table_for(&ai.kind_id) else {
        return;
    };
    let rolled = roll_loot(&table, || next_u32(ctx));
    if rolled.is_empty() {
        return;
    }
    let mut next_id = next_instance_id(ctx);
    let mut items = Vec::new();
    for (item_id, quantity) in rolled.items {
        if item_category(item_id.as_str()).is_none() {
            continue;
        }
        let mut instance = ItemInstance::new(item_id);
        instance.quantity = quantity.max(1);
        instance.instance_id = ItemInstanceId(next_id);
        next_id += 1;
        items.push(instance);
    }
    if rolled.gold == 0 && items.is_empty() {
        return;
    }
    spawn_bag(
        ctx,
        entity.position,
        rolled.gold,
        &items,
        LootSourceRow::Enemy,
    );
}

fn loot_table_for(kind_id: &str) -> Option<LootTable> {
    let id = KindId::new(kind_id.to_string());
    let registry = placeables();
    if let Some(definition) = registry.enemies.get(&id) {
        return definition.enemy_config().loot;
    }
    registry
        .bosses
        .get(&id)
        .and_then(|definition| definition.enemy_config().loot)
}

fn spawn_bag(
    ctx: &ReducerContext,
    position: crate::rows::Vec3Row,
    gold: u64,
    items: &[ItemInstance],
    source: LootSourceRow,
) {
    let Some(expires_at) = ctx
        .timestamp
        .checked_add_duration(Duration::from_secs(LOOT_BAG_LIFETIME_SECS))
    else {
        return;
    };
    let bag = ctx.db.loot_bag().insert(LootBag {
        id: 0,
        position,
        gold,
        expires_at,
        source,
    });
    for (index, item) in items.iter().enumerate() {
        let Ok(slot_index) = u8::try_from(index) else {
            break;
        };
        ctx.db.loot_bag_slot().insert(LootBagSlot {
            id: 0,
            bag_id: bag.id,
            slot_index,
            item: ItemInstanceRow::from(item),
        });
    }
}

pub(crate) fn delete_bag(ctx: &ReducerContext, bag_id: u64) {
    let slot_ids: Vec<u64> = ctx
        .db
        .loot_bag_slot()
        .bag_id()
        .filter(&bag_id)
        .map(|row| row.id)
        .collect();
    for id in slot_ids {
        ctx.db.loot_bag_slot().id().delete(id);
    }
    ctx.db.loot_bag().id().delete(bag_id);
}

pub(crate) fn bag_is_empty(ctx: &ReducerContext, bag_id: u64) -> bool {
    let Some(bag) = ctx.db.loot_bag().id().find(bag_id) else {
        return true;
    };
    if bag.gold > 0 {
        return false;
    }
    ctx.db
        .loot_bag_slot()
        .bag_id()
        .filter(&bag_id)
        .next()
        .is_none()
}

fn next_u32(ctx: &ReducerContext) -> u32 {
    let mut bytes = [0u8; 4];
    ctx.rng().fill_bytes(&mut bytes);
    u32::from_le_bytes(bytes)
}

/// True when `now` is at or past the bag's deadline. Used by reducers so a
/// take on a just-expired bag is refused even before the tick sweeps it.
pub(crate) fn bag_past_deadline(now: Timestamp, expires_at: Timestamp) -> bool {
    loot_bag_expired(
        now.to_micros_since_unix_epoch(),
        expires_at.to_micros_since_unix_epoch(),
    )
}
