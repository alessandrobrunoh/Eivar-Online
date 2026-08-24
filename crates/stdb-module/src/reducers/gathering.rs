//! Start and stop a gather channel. The tick completes pieces.

use bevymmo_domain::gathering::in_interact_range;
use bevymmo_domain::items::components::Inventory;
use bevymmo_domain::items::registry::ItemId;
use spacetimedb::{reducer, ReducerContext, Table};

use crate::reducers::lifecycle::caller_entity;
use crate::sim::gathering::{self, resource_definition};
use crate::sim::spells;
use crate::tables::{
    cast_state, game_entity, gather_session, resource_node, EntityKindRow, EntityStateRow,
    GatherSession,
};

const DEPLETED_MESSAGE: &str = "Questa risorsa è già stata completamente raccolta";

/// Begins channeling the targeted resource node.
#[reducer]
pub fn start_gather(ctx: &ReducerContext, node_entity_id: u64) -> Result<(), String> {
    let caller = caller_entity(ctx)?;
    if caller.state == EntityStateRow::Dead {
        return Err("dead characters do not gather".to_string());
    }
    if spells::casting_blocked(ctx, caller.entity_id) {
        return Err("you cannot gather right now".to_string());
    }

    let node_entity = ctx
        .db
        .game_entity()
        .entity_id()
        .find(node_entity_id)
        .ok_or_else(|| "that resource is gone".to_string())?;
    if node_entity.kind != EntityKindRow::ResourceNode {
        return Err("that is not a resource".to_string());
    }

    let node = ctx
        .db
        .resource_node()
        .entity_id()
        .find(node_entity_id)
        .ok_or_else(|| "that resource is gone".to_string())?;
    let node = gathering::regen_node(ctx, node);
    let definition = resource_definition(&node.kind_id)
        .ok_or_else(|| format!("unknown resource {}", node.kind_id))?;
    let config = definition.resource_config();

    if !in_interact_range(
        caller.position.x,
        caller.position.z,
        node_entity.position.x,
        node_entity.position.z,
        config.interact_range,
    ) {
        return Err("too far away".to_string());
    }

    if node.current_pieces == 0 {
        return Err(DEPLETED_MESSAGE.to_string());
    }

    let stacks = Inventory::stacks_category(
        crate::reducers::items::item_category(config.yield_item.as_str())
            .ok_or_else(|| format!("unknown item {}", config.yield_item.as_str()))?,
    );
    let inventory = crate::reducers::items::load_inventory(
        ctx,
        caller
            .owner_character_id
            .ok_or_else(|| "no character for this identity; call `join` first".to_string())?,
    )?;
    if inventory.space_for(&ItemId::new(config.yield_item.as_str().to_string()), stacks) == 0 {
        return Err("inventory is full".to_string());
    }

    if let Some(active) = ctx.db.cast_state().entity_id().find(caller.entity_id) {
        spells::end_cast(ctx, caller.entity_id, active.spell_id, true);
    }
    gathering::cancel_session(ctx, caller.entity_id);
    crate::sim::crafting::cancel_session(ctx, caller.entity_id);

    let required_seconds = gathering::required_channel_seconds(ctx, caller.entity_id, &config);
    ctx.db.gather_session().insert(GatherSession {
        entity_id: caller.entity_id,
        node_entity_id,
        placement_id: node.placement_id,
        elapsed_seconds: 0.0,
        required_seconds,
        start_position: caller.position,
    });
    Ok(())
}

/// Stops the caller's gather, if any. Idempotent.
#[reducer]
pub fn stop_gather(ctx: &ReducerContext) -> Result<(), String> {
    let caller = caller_entity(ctx)?;
    gathering::cancel_session(ctx, caller.entity_id);
    Ok(())
}
