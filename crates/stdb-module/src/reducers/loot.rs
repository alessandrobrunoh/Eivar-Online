//! Take gold and items out of a world loot bag.

use bevymmo_domain::gathering::in_interact_range;
use bevymmo_domain::items::components::StackOpError;
use bevymmo_domain::items::instance::ItemInstance;
use bevymmo_domain::loot::LOOT_INTERACT_RANGE;
use spacetimedb::{reducer, ReducerContext, Uuid};

use crate::reducers::economy::credit_gold;
use crate::reducers::items::{grant_instance, item_category};
use crate::reducers::lifecycle::caller_entity;
use crate::sim::loot::{bag_is_empty, bag_past_deadline, delete_bag};
use crate::tables::{loot_bag, loot_bag_slot, EntityStateRow, LootBag, LootBagSlot};

/// Moves one bag slot into the caller's inventory.
///
/// The physical `ItemInstance` is preserved (id, inscriptions, ability picks).
/// A full bag leaves the slot where it is.
#[reducer]
pub fn loot_take(ctx: &ReducerContext, bag_id: u64, slot_index: u8) -> Result<(), String> {
    let (bag, _) = require_open_bag(ctx, bag_id)?;
    let Some(slot) = slot_at(ctx, bag.id, slot_index) else {
        return Err("that loot slot is empty".to_string());
    };
    take_slot(ctx, slot)?;
    maybe_despawn(ctx, bag.id);
    Ok(())
}

/// Credits the bag's gold into the caller's wallet.
#[reducer]
pub fn loot_take_gold(ctx: &ReducerContext, bag_id: u64) -> Result<(), String> {
    let (bag, character_id) = require_open_bag(ctx, bag_id)?;
    if bag.gold == 0 {
        return Err("no gold in that bag".to_string());
    }
    credit_gold(ctx, character_id, bag.gold)?;
    ctx.db.loot_bag().id().update(LootBag {
        gold: 0,
        ..bag.clone()
    });
    maybe_despawn(ctx, bag.id);
    Ok(())
}

/// Takes gold, then every remaining item, stopping at the first that does not
/// fit. Partial success is still success.
#[reducer]
pub fn loot_take_all(ctx: &ReducerContext, bag_id: u64) -> Result<(), String> {
    let (bag, character_id) = require_open_bag(ctx, bag_id)?;
    if bag.gold > 0 {
        credit_gold(ctx, character_id, bag.gold)?;
        ctx.db.loot_bag().id().update(LootBag {
            gold: 0,
            ..bag.clone()
        });
    }
    let mut slots: Vec<LootBagSlot> = ctx.db.loot_bag_slot().bag_id().filter(&bag.id).collect();
    slots.sort_by_key(|slot| slot.slot_index);
    for slot in slots {
        match take_slot(ctx, slot) {
            Ok(()) => {}
            Err(reason) if reason == StackOpError::InventoryFull.as_str() => break,
            Err(reason) => return Err(reason),
        }
    }
    maybe_despawn(ctx, bag.id);
    Ok(())
}

fn require_open_bag(ctx: &ReducerContext, bag_id: u64) -> Result<(LootBag, Uuid), String> {
    let entity = caller_entity(ctx)?;
    if entity.state == EntityStateRow::Dead {
        return Err("you are dead".to_string());
    }
    let bag = ctx
        .db
        .loot_bag()
        .id()
        .find(bag_id)
        .ok_or_else(|| "that bag is gone".to_string())?;
    if bag_past_deadline(ctx.timestamp, bag.expires_at) {
        delete_bag(ctx, bag.id);
        return Err("that bag has crumbled".to_string());
    }
    if !in_interact_range(
        entity.position.x,
        entity.position.z,
        bag.position.x,
        bag.position.z,
        LOOT_INTERACT_RANGE,
    ) {
        return Err("too far from the bag".to_string());
    }
    let character_id = entity
        .owner_character_id
        .ok_or_else(|| "no character for this identity; call `join` first".to_string())?;
    Ok((bag, character_id))
}

fn slot_at(ctx: &ReducerContext, bag_id: u64, slot_index: u8) -> Option<LootBagSlot> {
    ctx.db
        .loot_bag_slot()
        .bag_id()
        .filter(&bag_id)
        .find(|row| row.slot_index == slot_index)
}

fn take_slot(ctx: &ReducerContext, slot: LootBagSlot) -> Result<(), String> {
    let instance = ItemInstance::from(&slot.item);
    let stacks = item_category(instance.item_id.as_str())
        .is_some_and(bevymmo_domain::items::components::Inventory::stacks_category);
    let character_id = caller_entity(ctx)?
        .owner_character_id
        .ok_or_else(|| "no character for this identity; call `join` first".to_string())?;
    match grant_instance(ctx, character_id, instance, stacks)? {
        None => {
            ctx.db.loot_bag_slot().id().delete(slot.id);
        }
        Some(leftover) => {
            ctx.db.loot_bag_slot().id().update(LootBagSlot {
                item: (&leftover).into(),
                ..slot
            });
        }
    }
    Ok(())
}

fn maybe_despawn(ctx: &ReducerContext, bag_id: u64) {
    if bag_is_empty(ctx, bag_id) {
        delete_bag(ctx, bag_id);
    }
}
