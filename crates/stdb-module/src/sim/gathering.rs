//! Per-tick gathering: advance channels, grant pieces, interrupt on move/range.

use std::sync::OnceLock;
use std::time::Duration;

use bevymmo_domain::content::items::default_items;
use bevymmo_domain::content::placeables::register_all;
use bevymmo_domain::gathering::{
    bonus_extra_pieces, channel_duration, gathering_tool_bonuses, in_interact_range, regen_catchup,
    resolve_gather, GatherAttempt,
};
use bevymmo_domain::items::components::Inventory;
use bevymmo_domain::items::registry::{ItemId, ItemRegistry};
use bevymmo_domain::placeables::{PlaceableRegistry, ResourceConfig, ResourceNodePlaceable};
use bevymmo_domain::spells::components::MOVEMENT_INTERRUPT_EPSILON;
use spacetimedb::rand::RngCore;
use spacetimedb::{ReducerContext, Table, Timestamp, Uuid};

use crate::reducers::items::{grant_items, item_category, load_inventory};
use crate::reducers::parties::notify_character;
use crate::rows::{equipment_from_rows, EQUIP_SLOTS};
use crate::tables::{
    cast_state, entity_stats, equipment, game_entity, gather_session, gather_yield, resource_node,
    EntityStateRow, GatherSession, GatherYieldEvent, ResourceNode,
};

const DEPLETED_MESSAGE: &str = "Questa risorsa è già stata completamente raccolta";

/// Sentinel `next_regen_at` for a full node (no pulse scheduled).
pub fn far_future() -> Timestamp {
    Timestamp::from_micros_since_unix_epoch(i64::MAX / 2)
}

fn item_registry() -> &'static ItemRegistry {
    static REGISTRY: OnceLock<ItemRegistry> = OnceLock::new();
    REGISTRY.get_or_init(default_items)
}

fn placeables() -> &'static PlaceableRegistry {
    static PLACEABLES: OnceLock<PlaceableRegistry> = OnceLock::new();
    PLACEABLES.get_or_init(|| {
        let mut registry = PlaceableRegistry::default();
        register_all(&mut registry);
        registry
    })
}

pub fn resource_definition(kind_id: &str) -> Option<&'static dyn ResourceNodePlaceable> {
    let id = bevymmo_domain::placeables::KindId::new(kind_id.to_string());
    placeables().resources.get(&id).map(std::sync::Arc::as_ref)
}

pub fn cancel_session(ctx: &ReducerContext, entity_id: u64) {
    if ctx
        .db
        .gather_session()
        .entity_id()
        .find(&entity_id)
        .is_some()
    {
        ctx.db.gather_session().entity_id().delete(&entity_id);
    }
}

/// Advances every open gather. Called from `game_tick` after movement.
pub fn step(ctx: &ReducerContext, dt: f32) {
    regen_due_nodes(ctx);
    let sessions: Vec<GatherSession> = ctx.db.gather_session().iter().collect();
    for session in sessions {
        advance_session(ctx, session, dt);
    }
}

fn regen_due_nodes(ctx: &ReducerContext) {
    let now = ctx.timestamp;
    let due: Vec<ResourceNode> = ctx.db.resource_node().next_regen().filter(..=now).collect();
    for node in due {
        regen_node(ctx, node);
    }
}

/// Applies catch-up regen and returns the persisted row.
pub fn regen_node(ctx: &ReducerContext, node: ResourceNode) -> ResourceNode {
    let now = ctx.timestamp;
    let Some(definition) = resource_definition(&node.kind_id) else {
        return node;
    };
    let config = definition.resource_config();
    persist_regen(ctx, node, now, &config)
}

fn persist_regen(
    ctx: &ReducerContext,
    node: ResourceNode,
    now: Timestamp,
    config: &ResourceConfig,
) -> ResourceNode {
    if node.current_pieces >= config.max_pieces {
        if node.next_regen_at == far_future() {
            return node;
        }
        let updated = ResourceNode {
            next_regen_at: far_future(),
            ..node
        };
        ctx.db
            .resource_node()
            .placement_id()
            .update(updated.clone());
        return updated;
    }

    let elapsed = now
        .duration_since(node.last_regen_at)
        .map(|d| d.as_secs_f32())
        .unwrap_or(0.0);
    let (current, leftover) = regen_catchup(
        node.current_pieces,
        config.max_pieces,
        elapsed,
        config.regen_interval_seconds,
        config.regen_amount,
    );
    let leftover = leftover.max(0.0);
    let last_regen_at = duration_from_secs(leftover)
        .and_then(|d| now.checked_sub_duration(d))
        .unwrap_or(now);
    let next_regen_at = next_regen_at(last_regen_at, current, config);
    if current == node.current_pieces
        && last_regen_at == node.last_regen_at
        && next_regen_at == node.next_regen_at
    {
        return node;
    }
    let updated = ResourceNode {
        current_pieces: current,
        last_regen_at,
        next_regen_at,
        ..node
    };
    ctx.db
        .resource_node()
        .placement_id()
        .update(updated.clone());
    updated
}

fn next_regen_at(last_regen_at: Timestamp, current: u32, config: &ResourceConfig) -> Timestamp {
    if current >= config.max_pieces || config.regen_interval_seconds <= 0.0 {
        return far_future();
    }
    duration_from_secs(config.regen_interval_seconds)
        .and_then(|d| last_regen_at.checked_add_duration(d))
        .unwrap_or_else(far_future)
}

fn duration_from_secs(seconds: f32) -> Option<Duration> {
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some(Duration::from_secs_f32(seconds))
}

fn persist_after_harvest(
    ctx: &ReducerContext,
    node: &ResourceNode,
    remaining: u32,
    config: &ResourceConfig,
) {
    let now = ctx.timestamp;
    let was_full = node.current_pieces >= config.max_pieces;
    let (last_regen_at, next_regen_at) = if remaining >= config.max_pieces {
        (node.last_regen_at, far_future())
    } else if was_full {
        (now, next_regen_at(now, remaining, config))
    } else {
        (node.last_regen_at, node.next_regen_at)
    };
    ctx.db.resource_node().placement_id().update(ResourceNode {
        placement_id: node.placement_id.clone(),
        entity_id: node.entity_id,
        kind_id: node.kind_id.clone(),
        current_pieces: remaining,
        last_regen_at,
        next_regen_at,
    });
}

fn gathering_stats(ctx: &ReducerContext, entity_id: u64, config: &ResourceConfig) -> (f32, f32) {
    let (base_speed, base_bonus) = ctx
        .db
        .entity_stats()
        .entity_id()
        .find(&entity_id)
        .map(|row| (row.stats.gathering_speed, row.stats.gathering_bonus))
        .unwrap_or((0.0, 0.0));
    let character_id = ctx
        .db
        .game_entity()
        .entity_id()
        .find(&entity_id)
        .and_then(|row| row.owner_character_id);
    let (tool_speed, tool_bonus) = matching_tool_bonuses(ctx, character_id, config);
    (base_speed + tool_speed, base_bonus + tool_bonus)
}

fn matching_tool_bonuses(
    ctx: &ReducerContext,
    character_id: Option<Uuid>,
    config: &ResourceConfig,
) -> (f32, f32) {
    let Some(character_id) = character_id else {
        return (0.0, 0.0);
    };
    let Some(row) = ctx.db.equipment().character_id().find(&character_id) else {
        return (0.0, 0.0);
    };
    let equipped_gear = equipment_from_rows(&row.slots);
    let registry = item_registry();
    let items: Vec<_> = EQUIP_SLOTS
        .iter()
        .filter_map(|slot| {
            let instance = equipped_gear.get(*slot).as_ref()?;
            registry.get(&instance.item_id)
        })
        .collect();
    let equipped = items
        .iter()
        .map(|item| (item.gathering_tool(), item.effects()));
    gathering_tool_bonuses(&config.bonus_tools, equipped)
}

pub fn required_channel_seconds(
    ctx: &ReducerContext,
    entity_id: u64,
    config: &ResourceConfig,
) -> f32 {
    let (speed, _) = gathering_stats(ctx, entity_id, config);
    channel_duration(config.channel_seconds, config.min_channel_seconds, speed)
}

/// Uniform roll in `[0, 1)` from the module RNG.
fn unit_interval_roll(ctx: &ReducerContext) -> f32 {
    let mut bytes = [0u8; 4];
    ctx.rng().fill_bytes(&mut bytes);
    let mantissa = u32::from_le_bytes(bytes) >> 8;
    mantissa as f32 / ((1u32 << 24) as f32)
}

fn advance_session(ctx: &ReducerContext, mut session: GatherSession, dt: f32) {
    let Some(gatherer) = ctx.db.game_entity().entity_id().find(&session.entity_id) else {
        cancel_session(ctx, session.entity_id);
        return;
    };
    if gatherer.state == EntityStateRow::Dead
        || crate::sim::spells::casting_blocked(ctx, gatherer.entity_id)
        || ctx
            .db
            .cast_state()
            .entity_id()
            .find(&gatherer.entity_id)
            .is_some()
    {
        cancel_session(ctx, session.entity_id);
        return;
    }

    let dx = gatherer.position.x - session.start_position.x;
    let dz = gatherer.position.z - session.start_position.z;
    if gatherer.move_target.is_some()
        || dx * dx + dz * dz > MOVEMENT_INTERRUPT_EPSILON * MOVEMENT_INTERRUPT_EPSILON
    {
        cancel_session(ctx, session.entity_id);
        return;
    }

    let Some(node_entity) = ctx
        .db
        .game_entity()
        .entity_id()
        .find(&session.node_entity_id)
    else {
        cancel_session(ctx, session.entity_id);
        return;
    };
    let Some(node) = ctx
        .db
        .resource_node()
        .entity_id()
        .find(&session.node_entity_id)
    else {
        cancel_session(ctx, session.entity_id);
        return;
    };
    let Some(definition) = resource_definition(&node.kind_id) else {
        cancel_session(ctx, session.entity_id);
        return;
    };
    let config = definition.resource_config();

    if !in_interact_range(
        gatherer.position.x,
        gatherer.position.z,
        node_entity.position.x,
        node_entity.position.z,
        config.interact_range,
    ) {
        cancel_session(ctx, session.entity_id);
        return;
    }

    if node.current_pieces == 0 {
        if let Some(character_id) = gatherer.owner_character_id {
            notify_character(ctx, character_id, DEPLETED_MESSAGE.to_string());
        }
        cancel_session(ctx, session.entity_id);
        return;
    }

    session.elapsed_seconds += dt;
    if session.elapsed_seconds < session.required_seconds {
        ctx.db.gather_session().entity_id().update(session);
        return;
    }

    complete_channel(ctx, session, gatherer.owner_character_id, &node, &config);
}

fn complete_channel(
    ctx: &ReducerContext,
    session: GatherSession,
    character_id: Option<spacetimedb::Uuid>,
    node: &ResourceNode,
    config: &ResourceConfig,
) {
    let Some(character_id) = character_id else {
        cancel_session(ctx, session.entity_id);
        return;
    };
    let Ok(inventory) = load_inventory(ctx, character_id) else {
        cancel_session(ctx, session.entity_id);
        return;
    };
    let Some(category) = item_category(config.yield_item.as_str()) else {
        cancel_session(ctx, session.entity_id);
        return;
    };
    let stacks = Inventory::stacks_category(category);
    let space = inventory.space_for(&ItemId::new(config.yield_item.as_str().to_string()), stacks);
    let (_, gather_bonus) = gathering_stats(ctx, session.entity_id, config);
    let bonus_extra = bonus_extra_pieces(gather_bonus, unit_interval_roll(ctx));
    let outcome = resolve_gather(GatherAttempt {
        yield_amount: config.yield_amount,
        bonus_extra,
        current_pieces: node.current_pieces,
        inventory_space: space,
    });

    if outcome.granted == 0 {
        if outcome.node_depleted {
            notify_character(ctx, character_id, DEPLETED_MESSAGE.to_string());
        }
        cancel_session(ctx, session.entity_id);
        return;
    }

    if let Err(reason) = grant_items(
        ctx,
        character_id,
        config.yield_item.as_str(),
        outcome.granted,
    ) {
        notify_character(ctx, character_id, reason);
        cancel_session(ctx, session.entity_id);
        return;
    }

    persist_after_harvest(ctx, node, outcome.remaining_pieces, config);
    ctx.db.gather_yield().insert(GatherYieldEvent {
        entity_id: session.entity_id,
        node_entity_id: session.node_entity_id,
        item_id: config.yield_item.as_str().to_string(),
        amount: outcome.granted,
        extra: outcome.extra,
        node_depleted: outcome.node_depleted,
    });

    if outcome.session_ends {
        cancel_session(ctx, session.entity_id);
        return;
    }

    let required_seconds = required_channel_seconds(ctx, session.entity_id, config);
    ctx.db.gather_session().entity_id().update(GatherSession {
        elapsed_seconds: 0.0,
        required_seconds,
        ..session
    });
}
