//! Start and stop a craft channel. The tick completes the batch.

use std::sync::OnceLock;

use bevymmo_domain::crafting::{channel_duration, preview_craft};
use bevymmo_domain::gathering::in_interact_range;
use bevymmo_domain::items::components::Inventory;
use bevymmo_domain::items::registry::{ItemId, ItemRegistry};
use spacetimedb::{reducer, ReducerContext, Table};

use crate::reducers::lifecycle::caller_entity;
use crate::sim::crafting;
use crate::sim::gathering;
use crate::sim::spells;
use crate::tables::{
    cast_state, craft_session, game_entity, npc, CraftSession, EntityKindRow, EntityStateRow,
};

const NPC_CRAFT_RANGE: f32 = 6.0;

fn item_registry() -> &'static ItemRegistry {
    static REGISTRY: OnceLock<ItemRegistry> = OnceLock::new();
    REGISTRY.get_or_init(bevymmo_domain::content::items::default_items)
}

/// Begins channeling a craft at a nearby crafter NPC.
#[reducer]
pub fn start_craft(
    ctx: &ReducerContext,
    npc_entity_id: u64,
    item_id: String,
    quantity: u32,
) -> Result<(), String> {
    let caller = caller_entity(ctx)?;
    if caller.state == EntityStateRow::Dead {
        return Err("dead characters do not craft".to_string());
    }
    if spells::casting_blocked(ctx, caller.entity_id) {
        return Err("you cannot craft right now".to_string());
    }

    let npc = ctx
        .db
        .game_entity()
        .entity_id()
        .find(&npc_entity_id)
        .ok_or_else(|| "NPC not found".to_string())?;
    if npc.kind != EntityKindRow::Npc {
        return Err("that entity is not a crafter".to_string());
    }
    let npc_row = ctx
        .db
        .npc()
        .entity_id()
        .find(&npc_entity_id)
        .ok_or_else(|| "NPC not found".to_string())?;
    let categories = crafting::npc_craft_categories(&npc_row.kind_id)
        .ok_or_else(|| "that NPC does not craft".to_string())?;

    if !in_interact_range(
        caller.position.x,
        caller.position.z,
        npc.position.x,
        npc.position.z,
        NPC_CRAFT_RANGE,
    ) {
        return Err("too far away".to_string());
    }

    if quantity < 1 {
        return Err("quantity must be at least 1".to_string());
    }

    let item = item_registry()
        .get(&ItemId::new(item_id.clone()))
        .ok_or_else(|| format!("unknown item {item_id:?}"))?;
    let recipe = item
        .craft_recipe()
        .ok_or_else(|| format!("{item_id:?} is not craftable"))?;
    if !categories.contains(&item.config().category) {
        return Err("that crafter does not make that item".to_string());
    }

    let character_id = caller
        .owner_character_id
        .ok_or_else(|| "no character for this identity; call `join` first".to_string())?;
    let inventory = crate::reducers::items::load_inventory(ctx, character_id)?;
    let stacks = Inventory::stacks_category(item.config().category);
    preview_craft(
        &inventory,
        recipe,
        &ItemId::new(item_id.clone()),
        stacks,
        quantity,
    )
    .map_err(|error| error.to_string())?;

    if let Some(active) = ctx.db.cast_state().entity_id().find(&caller.entity_id) {
        spells::end_cast(ctx, caller.entity_id, active.spell_id, true);
    }
    gathering::cancel_session(ctx, caller.entity_id);
    crafting::cancel_session(ctx, caller.entity_id);

    ctx.db.craft_session().insert(CraftSession {
        entity_id: caller.entity_id,
        npc_entity_id,
        item_id,
        quantity,
        elapsed_seconds: 0.0,
        required_seconds: channel_duration(recipe, quantity),
        start_position: caller.position,
    });
    Ok(())
}

/// Stops the caller's craft, if any. Idempotent.
#[reducer]
pub fn stop_craft(ctx: &ReducerContext) -> Result<(), String> {
    let caller = caller_entity(ctx)?;
    crafting::cancel_session(ctx, caller.entity_id);
    Ok(())
}
