//! Per-tick crafting: advance channels, grant the batch, interrupt on move/range.

use std::sync::OnceLock;

use bevymmo_domain::content::items::default_items;
use bevymmo_domain::content::placeables::register_all;
use bevymmo_domain::crafting::{apply_craft, preview_craft};
use bevymmo_domain::gathering::in_interact_range;
use bevymmo_domain::items::components::Inventory;
use bevymmo_domain::items::instance::ItemInstanceId;
use bevymmo_domain::items::registry::{ItemId, ItemRegistry};
use bevymmo_domain::placeables::{InteractionKind, KindId, PlaceableRegistry};
use bevymmo_domain::spells::components::MOVEMENT_INTERRUPT_EPSILON;
use spacetimedb::{ReducerContext, Table};

use crate::reducers::items::{load_inventory, next_instance_id, store_inventory};
use crate::reducers::parties::notify_character;
use crate::tables::{cast_state, craft_session, game_entity, npc, CraftSession, EntityStateRow};

const NPC_CRAFT_RANGE: f32 = 6.0;
const INTERRUPTED_MESSAGE: &str = "crafting interrupted";

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

pub(crate) fn npc_craft_categories(
    kind_id: &str,
) -> Option<Vec<bevymmo_domain::items::definition::ItemCategory>> {
    let id = KindId::new(kind_id.to_string());
    let definition = placeables().npcs.get(&id)?;
    match definition.interaction() {
        InteractionKind::Craft { categories } => Some(categories),
        _ => None,
    }
}

pub fn cancel_session(ctx: &ReducerContext, entity_id: u64) {
    if ctx
        .db
        .craft_session()
        .entity_id()
        .find(&entity_id)
        .is_some()
    {
        ctx.db.craft_session().entity_id().delete(&entity_id);
    }
}

/// Advances every open craft. Called from `game_tick` after gathering.
pub fn step(ctx: &ReducerContext, dt: f32) {
    let sessions: Vec<CraftSession> = ctx.db.craft_session().iter().collect();
    for session in sessions {
        advance_session(ctx, session, dt);
    }
}

fn advance_session(ctx: &ReducerContext, mut session: CraftSession, dt: f32) {
    let Some(crafter) = ctx.db.game_entity().entity_id().find(&session.entity_id) else {
        cancel_session(ctx, session.entity_id);
        return;
    };
    if crafter.state == EntityStateRow::Dead
        || crate::sim::spells::casting_blocked(ctx, crafter.entity_id)
        || ctx
            .db
            .cast_state()
            .entity_id()
            .find(&crafter.entity_id)
            .is_some()
    {
        interrupt(ctx, &session, crafter.owner_character_id);
        return;
    }

    let dx = crafter.position.x - session.start_position.x;
    let dz = crafter.position.z - session.start_position.z;
    if crafter.move_target.is_some()
        || dx * dx + dz * dz > MOVEMENT_INTERRUPT_EPSILON * MOVEMENT_INTERRUPT_EPSILON
    {
        interrupt(ctx, &session, crafter.owner_character_id);
        return;
    }

    let Some(npc_entity) = ctx
        .db
        .game_entity()
        .entity_id()
        .find(&session.npc_entity_id)
    else {
        interrupt(ctx, &session, crafter.owner_character_id);
        return;
    };
    if !in_interact_range(
        crafter.position.x,
        crafter.position.z,
        npc_entity.position.x,
        npc_entity.position.z,
        NPC_CRAFT_RANGE,
    ) {
        interrupt(ctx, &session, crafter.owner_character_id);
        return;
    }

    session.elapsed_seconds += dt;
    if session.elapsed_seconds < session.required_seconds {
        ctx.db.craft_session().entity_id().update(session);
        return;
    }

    complete_channel(ctx, session, crafter.owner_character_id);
}

fn interrupt(
    ctx: &ReducerContext,
    session: &CraftSession,
    character_id: Option<spacetimedb::Uuid>,
) {
    if let Some(character_id) = character_id {
        notify_character(ctx, character_id, INTERRUPTED_MESSAGE.to_string());
    }
    cancel_session(ctx, session.entity_id);
}

fn complete_channel(
    ctx: &ReducerContext,
    session: CraftSession,
    character_id: Option<spacetimedb::Uuid>,
) {
    let Some(character_id) = character_id else {
        cancel_session(ctx, session.entity_id);
        return;
    };
    let Some(npc_row) = ctx.db.npc().entity_id().find(&session.npc_entity_id) else {
        interrupt(ctx, &session, Some(character_id));
        return;
    };
    let Some(categories) = npc_craft_categories(&npc_row.kind_id) else {
        interrupt(ctx, &session, Some(character_id));
        return;
    };
    let Some(item) = item_registry().get(&ItemId::new(session.item_id.clone())) else {
        interrupt(ctx, &session, Some(character_id));
        return;
    };
    let Some(recipe) = item.craft_recipe() else {
        interrupt(ctx, &session, Some(character_id));
        return;
    };
    if !categories.contains(&item.config().category) {
        interrupt(ctx, &session, Some(character_id));
        return;
    }

    let Ok(mut inventory) = load_inventory(ctx, character_id) else {
        interrupt(ctx, &session, Some(character_id));
        return;
    };
    let stacks = Inventory::stacks_category(item.config().category);
    let output_id = ItemId::new(session.item_id.clone());
    let plan = match preview_craft(&inventory, recipe, &output_id, stacks, session.quantity) {
        Ok(plan) => plan,
        Err(error) => {
            notify_character(ctx, character_id, error.to_string());
            cancel_session(ctx, session.entity_id);
            return;
        }
    };

    let mut next_id = next_instance_id(ctx);
    if let Err(error) = apply_craft(&mut inventory, &plan, stacks, || {
        let minted = ItemInstanceId(next_id);
        next_id += 1;
        minted
    }) {
        notify_character(ctx, character_id, error.to_string());
        cancel_session(ctx, session.entity_id);
        return;
    }
    store_inventory(ctx, character_id, &inventory);
    cancel_session(ctx, session.entity_id);
}
