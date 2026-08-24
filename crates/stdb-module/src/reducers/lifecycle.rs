//! Module startup, connections, and character creation.

use spacetimedb::{reducer, ReducerContext, ScheduleAt, Table, Uuid};
use std::time::Duration;

use crate::reducers::account::caller_session;
use crate::rows::{equipment_to_rows, inventory_to_rows, HotbarRow, StatsRow, Vec3Row};
use crate::tables::{
    active_status, aoe_region, boss_state, cast_state, character_wallet, cooldown, craft_session,
    crowd_control, domain_event_cleanup_schedule, domain_event_config, enemy_ai, entity_stats,
    equipment, game_entity, gather_session, grid_cell, hotbar, inventory, known_ancient_language,
    loot_bag, loot_bag_slot, npc, periodic_effect, player, player_stats, projectile, resonance,
    session, stat_modifier, threat, tick_schedule, tick_stats, CharacterWallet, ColorRow,
    EntityKindRow, EntityStateRow, EquipmentTable, GameEntity, Hotbar, InventoryTable,
    KnownAncientLanguageTable, Player, PlayerStats, Session, TickSchedule,
};
use crate::{
    normalize_name, world, DEFAULT_SPEED_PER_SECOND, MAX_CHARACTERS_PER_ACCOUNT, TICK_INTERVAL_MS,
};

/// Runs once, when the module is first published to an empty database.
#[reducer(init)]
pub fn init(ctx: &ReducerContext) {
    clear_runtime_state(ctx);

    ctx.db.tick_schedule().insert(TickSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(Duration::from_millis(TICK_INTERVAL_MS).into()),
    });
    ctx.db
        .domain_event_config()
        .insert(crate::tables::DomainEventConfig {
            id: 0,
            enabled: true,
            damage_threshold: crate::sim::event_log::DEFAULT_DAMAGE_THRESHOLD,
            retention_seconds: crate::sim::event_log::DEFAULT_RETENTION_SECONDS,
        });
    ctx.db
        .domain_event_cleanup_schedule()
        .insert(crate::tables::DomainEventCleanupSchedule {
            scheduled_id: 0,
            scheduled_at: crate::sim::event_log::schedule(),
        });

    crate::reducers::market::seed_markets(ctx);
    world::seed(ctx);

    log::info!("module initialised; tick every {TICK_INTERVAL_MS} ms");
}

/// Clears and re-seeds the transient half of the world, without touching a
/// single character.
///
/// [`init`] does this too, but `init` only fires against an *empty* database.
/// A normal `spacetime publish` over a live database never runs it, so a
/// republish inherits whatever was mid-flight when the old module stopped:
/// projectiles with no caster, casts that will never resolve, threat tables
/// pointing at entities that no longer exist, loot bags past their expiry.
/// Presence recovers on its own — `expire_stale_presence` reaps players whose
/// heartbeat stopped — but nothing reaps the rest.
///
/// GM-gated and manual on purpose. Doing it automatically needs a signal that
/// the module was replaced, and the two candidates are both worse than a
/// deliberate command: `#[reducer(update)]` parses in 2.8.1 but is dropped
/// before registration (`LifecycleReducer::Update` returns no lifecycle
/// value), so it silently becomes an ordinary client-callable reducer that
/// never runs on update; and keying off a module static assumes the WASM
/// instance outlives a single reducer call, which is not something to bet a
/// destructive sweep on without measuring it against a live host.
///
/// Characters, inventories, equipment, wallets and progression are untouched —
/// `clear_runtime_state` only removes rows whose `owner_character_id` is
/// `None`, plus the tables that model a live session.
#[reducer]
pub fn gm_reset_runtime_state(ctx: &ReducerContext) -> Result<(), String> {
    world::require_gm(ctx)?;

    clear_runtime_state(ctx);
    crate::reducers::market::seed_markets(ctx);
    world::seed(ctx);
    // The tick remembers it has already spawned these; the sweep above just
    // deleted them.
    crate::tick::reset_seed_flags();

    log::info!("runtime state cleared and re-seeded by {}", ctx.sender().to_hex());
    Ok(())
}

/// Wipes everything that models a live session.
///
/// Necessary because SpacetimeDB persists every table, including the ones that
/// only make sense while the server is up. Without this a republish inherits
/// mid-flight projectiles, half-finished casts and stale threat tables.
///
/// Player *characters* are deliberately untouched — those are the persistent
/// half, and losing them on every publish would be the opposite of the point.
pub(crate) fn clear_runtime_state(ctx: &ReducerContext) {
    // Table scans and mutations must be separate passes, even during init.
    let projectile_ids: Vec<_> = ctx.db.projectile().iter().map(|row| row.id).collect();
    let aoe_ids: Vec<_> = ctx.db.aoe_region().iter().map(|row| row.id).collect();
    let crowd_control_ids: Vec<_> = ctx.db.crowd_control().iter().map(|row| row.id).collect();
    let active_status_ids: Vec<_> = ctx.db.active_status().iter().map(|row| row.id).collect();
    let modifier_ids: Vec<_> = ctx.db.stat_modifier().iter().map(|row| row.id).collect();
    let threat_ids: Vec<_> = ctx.db.threat().iter().map(|row| row.id).collect();
    let cast_entity_ids: Vec<_> = ctx
        .db
        .cast_state()
        .iter()
        .map(|row| row.entity_id)
        .collect();
    let cooldown_ids: Vec<_> = ctx.db.cooldown().iter().map(|row| row.id).collect();
    let boss_entity_ids: Vec<_> = ctx
        .db
        .boss_state()
        .iter()
        .map(|row| row.entity_id)
        .collect();
    let seeded_entity_ids: Vec<_> = ctx
        .db
        .game_entity()
        .iter()
        .filter(|row| row.owner_character_id.is_none())
        .map(|row| row.entity_id)
        .collect();
    let tick_stat_ids: Vec<_> = ctx.db.tick_stats().iter().map(|row| row.id).collect();
    let npc_ids: Vec<_> = ctx.db.npc().iter().map(|row| row.entity_id).collect();
    let enemy_ai_ids: Vec<_> = ctx.db.enemy_ai().iter().map(|row| row.entity_id).collect();
    let gather_entity_ids: Vec<_> = ctx
        .db
        .gather_session()
        .iter()
        .map(|row| row.entity_id)
        .collect();
    let craft_entity_ids: Vec<_> = ctx
        .db
        .craft_session()
        .iter()
        .map(|row| row.entity_id)
        .collect();
    let loot_bag_ids: Vec<_> = ctx.db.loot_bag().iter().map(|row| row.id).collect();
    let loot_slot_ids: Vec<_> = ctx.db.loot_bag_slot().iter().map(|row| row.id).collect();

    for id in projectile_ids {
        ctx.db.projectile().id().delete(id);
    }
    for id in aoe_ids {
        ctx.db.aoe_region().id().delete(id);
    }
    for id in crowd_control_ids {
        ctx.db.crowd_control().id().delete(id);
    }
    for id in active_status_ids {
        ctx.db.active_status().id().delete(id);
    }
    for id in modifier_ids {
        ctx.db.stat_modifier().id().delete(id);
    }
    for id in threat_ids {
        ctx.db.threat().id().delete(id);
    }
    for entity_id in cast_entity_ids {
        ctx.db.cast_state().entity_id().delete(entity_id);
    }
    for id in cooldown_ids {
        ctx.db.cooldown().id().delete(id);
    }
    for entity_id in boss_entity_ids {
        ctx.db.boss_state().entity_id().delete(entity_id);
    }
    // Non-player entities are respawned from the map manifest by `world::seed`.
    for entity_id in seeded_entity_ids {
        ctx.db.entity_stats().entity_id().delete(entity_id);
        ctx.db.game_entity().entity_id().delete(entity_id);
    }
    for id in tick_stat_ids {
        ctx.db.tick_stats().id().delete(id);
    }
    for entity_id in npc_ids {
        ctx.db.npc().entity_id().delete(entity_id);
    }
    for entity_id in enemy_ai_ids {
        ctx.db.enemy_ai().entity_id().delete(entity_id);
    }
    for entity_id in gather_entity_ids {
        ctx.db.gather_session().entity_id().delete(entity_id);
    }
    for entity_id in craft_entity_ids {
        ctx.db.craft_session().entity_id().delete(entity_id);
    }
    for id in loot_slot_ids {
        ctx.db.loot_bag_slot().id().delete(id);
    }
    for id in loot_bag_ids {
        ctx.db.loot_bag().id().delete(id);
    }
}

/// Marks the caller's active character online, if this connection is
/// authenticated and already playing one.
///
/// A connection with no session, or a session with no character selected yet,
/// is normal: the client calls `login`/`register` and then [`join`] before
/// there is anything to mark online.
#[reducer(client_connected)]
pub fn client_connected(ctx: &ReducerContext) {
    let Some(character) = active_character(ctx) else {
        return;
    };
    let entity_id = character.entity_id;
    ctx.db.player().character_id().update(Player {
        online: true,
        last_seen: ctx.timestamp,
        ..character
    });

    // A returning character arrives with gear already on. `entity_stats` is
    // derived — base plus equipment plus modifiers — and nothing has recomputed
    // it since the equipment last changed, so it is rebuilt here rather than
    // trusted.
    crate::sim::combat::recalculate_effective_stats(ctx, entity_id);
}

/// Marks the active character offline, stops it where it stands, and ends the
/// connection's [`Session`].
///
/// Note what is *not* here: a save. Position, stats and inventory are already
/// rows. The Bevy server wrote its only snapshot at this point, which is why a
/// crash lost the whole session.
///
/// The `Session` is deleted, not merely cleared, so a reconnect — even one
/// reusing the same cached `Identity` — must call `login`/`register` again
/// rather than inheriting a stale authentication.
#[reducer(client_disconnected)]
pub fn client_disconnected(ctx: &ReducerContext) {
    if let Some(character) = active_character(ctx) {
        if let Some(entity) = ctx.db.game_entity().entity_id().find(character.entity_id) {
            ctx.db.game_entity().entity_id().update(GameEntity {
                move_target: None,
                state: EntityStateRow::Idle,
                ..entity
            });
        }
        ctx.db.player().character_id().update(Player {
            online: false,
            last_seen: ctx.timestamp,
            ..character
        });
    }
    ctx.db.session().identity().delete(ctx.sender());
}

/// Selects an existing character by name, or creates one, for the caller's
/// authenticated session.
///
/// Requires a prior `login`/`register`: an unauthenticated connection has no
/// `Session` row, so there is no account to own a new character.
#[reducer]
pub fn join(ctx: &ReducerContext, display_name: String) -> Result<(), String> {
    let normalized = normalize_name(&display_name);
    if normalized.chars().count() < 3 || normalized.chars().count() > 16 {
        return Err(format!(
            "name must be 3-16 characters, got {display_name:?}"
        ));
    }

    let session_row = caller_session(ctx)?;
    let account_id = session_row.account_id;

    if let Some(existing) = ctx.db.player().normalized_name().find(&normalized) {
        if existing.account_id != account_id {
            return Err(format!("name {display_name:?} is taken"));
        }
        // The caller's own character: reactivate it and make it this
        // connection's active character — unless another live session already
        // owns it.
        let already_played = ctx.db.session().iter().any(|session| {
            session.character_id == Some(existing.character_id) && session.identity != ctx.sender()
        });
        if already_played {
            return Err(format!(
                "{display_name} is already in the world on another connection"
            ));
        }
        set_active_character(ctx, Some(existing.character_id));
        ctx.db.player().character_id().update(Player {
            online: true,
            last_seen: ctx.timestamp,
            ..existing
        });
        return Ok(());
    }

    let existing_count = ctx.db.player().account_id().filter(&account_id).count();
    if at_character_cap(existing_count) {
        return Err(format!(
            "an account may have at most {MAX_CHARACTERS_PER_ACCOUNT} characters"
        ));
    }

    let spawn = world::player_spawn_point(ctx);
    let (cell_x, cell_z) = grid_cell(spawn);
    let entity = ctx.db.game_entity().insert(GameEntity {
        entity_id: 0,
        kind: EntityKindRow::Player,
        owner_character_id: None, // filled in once the character row exists, below
        display_name: display_name.clone(),
        color: ColorRow::for_kind(EntityKindRow::Player),
        position: spawn,
        look: Vec3Row {
            x: 0.0,
            y: 0.0,
            z: 1.0,
        },
        move_target: None,
        speed: DEFAULT_SPEED_PER_SECOND,
        state: EntityStateRow::Idle,
        cell_x,
        cell_z,
        spawn_point: spawn,
        // Players come back when they ask to, not on a clock.
        respawn_in_seconds: None,
    });

    let defaults = bevymmo_domain::stats::defaults::player_defaults();
    let stats = StatsRow::from(&defaults);

    // Minted before the insert: a failure here must not leave an orphaned
    // `game_entity` row behind, and `insert` takes the value, not a Result.
    let character_id = ctx
        .new_uuid_v4()
        .map_err(|err| format!("failed to mint a character id: {err}"))?;
    let character = ctx.db.player().insert(Player {
        character_id,
        account_id,
        normalized_name: normalized,
        display_name,
        entity_id: entity.entity_id,
        online: true,
        last_seen: ctx.timestamp,
    });

    ctx.db.game_entity().entity_id().update(GameEntity {
        owner_character_id: Some(character_id),
        ..entity
    });

    ctx.db.player_stats().insert(PlayerStats {
        character_id,
        stats,
    });
    ctx.db.entity_stats().insert(crate::tables::EntityStats {
        entity_id: character.entity_id,
        stats,
        current_mana: stats.max_mana,
        shield_remaining_seconds: None,
    });

    ctx.db.hotbar().insert(Hotbar {
        character_id,
        slots: HotbarRow::default(),
    });
    ctx.db.inventory().insert(InventoryTable {
        character_id,
        slots: inventory_to_rows(&Default::default()),
    });
    ctx.db.character_wallet().insert(CharacterWallet {
        character_id,
        gold: 0,
    });
    crate::reducers::economy::ensure_account_economy(ctx, account_id);
    crate::reducers::items::grant_item(ctx, character_id, "sword")?;
    ctx.db.equipment().insert(EquipmentTable {
        character_id,
        slots: equipment_to_rows(&Default::default()),
    });

    ctx.db
        .known_ancient_language()
        .insert(KnownAncientLanguageTable {
            character_id,
            root_words: vec!["flame".to_string(), "life".to_string()],
            ancient_words: vec![
                "echo".to_string(),
                "twin".to_string(),
                "return".to_string(),
                "hunger".to_string(),
                "anchor".to_string(),
                "reversal".to_string(),
            ],
            base_abilities: vec![
                "cleave".to_string(),
                "lunge".to_string(),
                "blade_storm".to_string(),
            ],
        });

    crate::reducers::items::equip_granted_starter_staff(ctx, character_id, character.entity_id)?;

    set_active_character(ctx, Some(character_id));
    Ok(())
}

/// Points the caller's `Session` at `character_id` (`Some`), or clears it
/// back to `None` — one write, since "select" and "clear" were the same
/// `Session` update with a different value for this one field.
fn set_active_character(ctx: &ReducerContext, character_id: Option<Uuid>) {
    let identity = ctx.sender();
    if let Some(session_row) = ctx.db.session().identity().find(identity) {
        ctx.db.session().identity().update(Session {
            character_id,
            ..session_row
        });
    }
}

/// Returns the caller to character select: marks the active character
/// offline and clears the connection's `Session.character_id`, without
/// deleting anything. The account stays authenticated and every other
/// character keeps existing — to permanently delete one, use
/// [`delete_character`].
///
/// Deliberately non-destructive, unlike an earlier version of this reducer:
/// back when a character *was* the `Identity` (no accounts, one throwaway
/// identity per launch), deleting on `leave` was how a name got freed up
/// again. With accounts, a character survives across logins by design, and
/// deleting it here made both closing the game window and pressing the
/// pause menu's "Logout" silently destroy the character being played —
/// exactly the data loss `delete_character`'s two-click confirmation exists
/// to prevent everywhere else.
///
/// A no-op if the caller has no active character selected.
#[reducer]
pub fn leave(ctx: &ReducerContext) -> Result<(), String> {
    let Some(character) = active_character(ctx) else {
        return Ok(());
    };
    if let Some(entity) = ctx.db.game_entity().entity_id().find(character.entity_id) {
        ctx.db.game_entity().entity_id().update(GameEntity {
            move_target: None,
            state: EntityStateRow::Idle,
            ..entity
        });
    }
    ctx.db.player().character_id().update(Player {
        online: false,
        last_seen: ctx.timestamp,
        ..character
    });
    // Free the connection to pick or create a different character without
    // logging out of the account.
    set_active_character(ctx, None);
    Ok(())
}

/// Permanently deletes one of the caller's own characters by id, whether or
/// not it is the connection's currently active one — the character-select
/// screen's "delete" action. [`leave`] does not delete anything; this is the
/// only reducer that does.
///
/// Rejects deleting a character belonging to a *different* account: the
/// caller only ever supplies a bare `character_id`, so without this check
/// any authenticated connection could delete anyone's character by guessing
/// or enumerating ids.
#[reducer]
pub fn delete_character(ctx: &ReducerContext, character_id: Uuid) -> Result<(), String> {
    let session_row = caller_session(ctx)?;
    let character = ctx
        .db
        .player()
        .character_id()
        .find(character_id)
        .ok_or_else(|| "no character with this id".to_string())?;
    if character.account_id != session_row.account_id {
        return Err("that character does not belong to your account".to_string());
    }

    delete_character_rows(ctx, &character);
    if session_row.character_id == Some(character_id) {
        set_active_character(ctx, None);
    }
    Ok(())
}

/// Deletes every row `character` owns: its entity, its derived combat state,
/// and its persistent gameplay rows (inventory, equipment, hotbar, glyphs,
/// stats, resonance). Shared by [`leave`] and [`delete_character`] — the only
/// difference between them is which character they resolve and whether the
/// caller's `Session.character_id` needs clearing afterward.
fn delete_character_rows(ctx: &ReducerContext, character: &Player) {
    let character_id = character.character_id;
    let entity_id = character.entity_id;

    // Table scans and mutations must be separate passes, as elsewhere in this
    // module.
    let cooldown_ids: Vec<_> = ctx
        .db
        .cooldown()
        .iter()
        .filter(|row| row.entity_id == entity_id)
        .map(|row| row.id)
        .collect();
    let crowd_control_ids: Vec<_> = ctx
        .db
        .crowd_control()
        .iter()
        .filter(|row| row.entity_id == entity_id)
        .map(|row| row.id)
        .collect();
    let active_status_ids: Vec<_> = ctx
        .db
        .active_status()
        .iter()
        .filter(|row| row.entity_id == entity_id)
        .map(|row| row.id)
        .collect();
    let stat_modifier_ids: Vec<_> = ctx
        .db
        .stat_modifier()
        .iter()
        .filter(|row| row.entity_id == entity_id)
        .map(|row| row.id)
        .collect();
    let periodic_effect_ids: Vec<_> = ctx
        .db
        .periodic_effect()
        .iter()
        .filter(|row| row.entity_id == entity_id)
        .map(|row| row.id)
        .collect();
    // A player entity is only ever a threat *target*, never a `combatant_entity`.
    let threat_ids: Vec<_> = ctx
        .db
        .threat()
        .iter()
        .filter(|row| row.target_entity == entity_id)
        .map(|row| row.id)
        .collect();
    let resonance_ids: Vec<_> = ctx
        .db
        .resonance()
        .iter()
        .filter(|row| row.character_id == character_id)
        .map(|row| row.id)
        .collect();

    for id in cooldown_ids {
        ctx.db.cooldown().id().delete(id);
    }
    for id in crowd_control_ids {
        ctx.db.crowd_control().id().delete(id);
    }
    for id in active_status_ids {
        ctx.db.active_status().id().delete(id);
    }
    for id in stat_modifier_ids {
        ctx.db.stat_modifier().id().delete(id);
    }
    for id in periodic_effect_ids {
        ctx.db.periodic_effect().id().delete(id);
    }
    for id in threat_ids {
        ctx.db.threat().id().delete(id);
    }
    for id in resonance_ids {
        ctx.db.resonance().id().delete(id);
    }

    ctx.db.cast_state().entity_id().delete(entity_id);
    ctx.db.entity_stats().entity_id().delete(entity_id);
    ctx.db.game_entity().entity_id().delete(entity_id);

    ctx.db.equipment().character_id().delete(character_id);
    ctx.db.inventory().character_id().delete(character_id);
    ctx.db.hotbar().character_id().delete(character_id);
    ctx.db
        .known_ancient_language()
        .character_id()
        .delete(character_id);
    ctx.db.player_stats().character_id().delete(character_id);
    ctx.db
        .character_wallet()
        .character_id()
        .delete(character_id);

    crate::reducers::parties::forget_deleted_character(ctx, character);

    ctx.db.player().character_id().delete(character_id);
}

/// Resolves the caller's active character, if this connection is
/// authenticated and one is selected. Unlike [`caller_character`], this never
/// errors — it is for lifecycle hooks (`client_connected`, `heartbeat`) where
/// "nothing to do yet" is a normal, silent case rather than a rejection.
fn active_character(ctx: &ReducerContext) -> Option<Player> {
    let session_row = ctx.db.session().identity().find(ctx.sender())?;
    let character_id = session_row.character_id?;
    ctx.db.player().character_id().find(character_id)
}

/// Resolves the caller's active character, or explains why there isn't one.
pub fn caller_character(ctx: &ReducerContext) -> Result<Player, String> {
    active_character(ctx)
        .ok_or_else(|| "no character selected for this connection; call `join` first".to_string())
}

/// Resolves the caller's active character's entity, or explains why there
/// isn't one.
pub fn caller_entity(ctx: &ReducerContext) -> Result<GameEntity, String> {
    let character = caller_character(ctx)?;
    ctx.db
        .game_entity()
        .entity_id()
        .find(character.entity_id)
        .ok_or_else(|| "character has no entity".to_string())
}

/// How long a character stays "online" without hearing from its client.
///
/// Presence cannot be read from the database: the module has no way to
/// enumerate live connections, and `client_disconnected` does not fire for
/// connections that died with a previous instance of the server. So it is
/// inferred from a heartbeat instead. Generous enough to survive a slow frame,
/// short enough that a restarted server does not show a lobby full of ghosts.
const PRESENCE_TIMEOUT_SECONDS: i64 = 15;

/// Says the caller's active character is still here. The client calls this
/// every few seconds.
#[reducer]
pub fn heartbeat(ctx: &ReducerContext) -> Result<(), String> {
    let character = caller_character(ctx)?;
    ctx.db.player().character_id().update(Player {
        online: true,
        last_seen: ctx.timestamp,
        ..character
    });
    Ok(())
}

/// Marks characters offline once their client stops checking in.
///
/// Called from the tick. Note that this is what makes a server restart settle
/// correctly: the tick resumes, but nothing refreshes `last_seen`, so every
/// character that was online when the instance died decays within the timeout.
pub fn expire_stale_presence(ctx: &ReducerContext) {
    let now = ctx.timestamp;
    let stale: Vec<_> = ctx
        .db
        .player()
        .online()
        .filter(&true)
        .filter(|player| {
            now.duration_since(player.last_seen)
                .map(|elapsed| elapsed.as_secs() as i64 >= PRESENCE_TIMEOUT_SECONDS)
                // A `last_seen` in the future means clock weirdness, not
                // absence; leave those alone rather than kicking them.
                .unwrap_or(false)
        })
        .collect();

    for player in stale {
        log::info!("{} timed out", player.display_name);
        if let Some(entity) = ctx.db.game_entity().entity_id().find(player.entity_id) {
            ctx.db.game_entity().entity_id().update(GameEntity {
                move_target: None,
                state: if entity.state == EntityStateRow::Dead {
                    entity.state
                } else {
                    EntityStateRow::Idle
                },
                ..entity
            });
        }
        ctx.db.cast_state().entity_id().delete(player.entity_id);
        ctx.db.player().character_id().update(Player {
            online: false,
            ..player
        });
    }
}

/// Whether an account holding `existing_count` characters may create another.
///
/// Split out of [`join`] so the boundary is testable without a
/// `ReducerContext`: the off-by-one that matters here (`>` instead of `>=`,
/// which would silently allow one character too many) is invisible to a test
/// that can only re-read the constant.
fn at_character_cap(existing_count: usize) -> bool {
    existing_count >= MAX_CHARACTERS_PER_ACCOUNT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_cap_admits_accounts_below_the_limit() {
        for count in 0..MAX_CHARACTERS_PER_ACCOUNT {
            assert!(
                !at_character_cap(count),
                "{count} characters must still fit"
            );
        }
    }

    #[test]
    fn character_cap_rejects_at_and_above_the_limit() {
        assert!(at_character_cap(MAX_CHARACTERS_PER_ACCOUNT));
        assert!(at_character_cap(MAX_CHARACTERS_PER_ACCOUNT + 1));
    }
}
