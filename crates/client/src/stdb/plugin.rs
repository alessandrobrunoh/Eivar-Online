//! Connection lifecycle and row-to-entity mirroring.
//!
//! # How state gets from the database into the ECS
//!
//! The SDK delivers rows through callbacks that run on whichever thread calls
//! [`DbConnection::frame_tick`], and those callbacks cannot borrow the Bevy
//! `World`. So they do the least possible work — clone the row into a
//! [`crossbeam_channel`] — and [`drain_events`] applies them from a normal
//! system, where mutating the world is safe.
//!
//! The components written out are the *same ones lightyear replicated*
//! (`Position`, `VitalStats`, `Inventory`, ...), which is what keeps
//! `bevymmo_presentation` from noticing the change of transport at all.
//!
//! # Why the client simulates too
//!
//! lightyear gave prediction and interpolation for free; SpacetimeDB gives
//! neither. The server ticks at roughly 18-19 Hz, so rendering raw authoritative
//! positions would visibly stutter. Instead every entity carries its destination
//! ([`StdbAuthoritative::move_target`], replicated on purpose), and the client
//! walks towards it every frame using the same terrain stepping the module
//! runs — `bevymmo_domain::movement::step_on_terrain` over the manifest the
//! presentation layer publishes through [`crate::movement::ClientCollision`].
//! Sharing the stepper is the point: prediction that ignored blockers would
//! render the character through walls and off ledges, and reconciliation only
//! eases the error away, so the wrong position is what the player sees for as
//! long as they hold the button.

use crate::app_state::{
    screen_after_connection_loss, AuthFailure, AuthIntent, AuthRequest, AuthState, AuthStatus,
    ConnectionFailure, ConnectionIntent, ConnectionRequest, DeleteCharacterRequest, Screen,
};
use crate::local_player::LocalPlayer;
use crate::movement::{
    snap_to_ground, step_on_terrain, ClientCollision, ClientSurfaceQuery, LocalMovementFreeze,
    MoveTarget, TerrainStep,
};
use crate::server_feed::{ChatLine, ServerNotice, SpellCooldownState, WorldTextCue};
use bevy::prelude::*;
use bevy::window::WindowCloseRequested;
use bevymmo_domain::movement::{self, predicted_move_dest, reconcile_offset, Reconcile, Step};
use bevymmo_domain::movement::{movement_intent_allowed, MovementLock};

use bevymmo_domain::stats::events::{ModifierKind, ModifierOp, StatField};
use bevymmo_domain::stats::modifiers::{
    ActiveStatModifiers, ModifierEffectInstance, ModifierId as StatModifierId, StatModifierInstance,
};
use bevymmo_domain::EntityId;
use bevymmo_gameplay::abilities::{AbilityAim, AncientWordId, KnownAncientLanguage};
use bevymmo_gameplay::crafting::ActiveCraft;
use bevymmo_gameplay::crowd_control::{ActiveCrowdControl, CrowdControlKind, CrowdControlState};
use bevymmo_gameplay::effects::{ActiveStatusSnapshot, ActiveStatuses};
use bevymmo_gameplay::entity::boss::components::{Boss, BossArena, BossPhase};
use bevymmo_gameplay::entity::components::{EntityKind, EntityState, GameEntity, PlayerName};
use bevymmo_gameplay::gathering::{ActiveGather, Harvestable};
use bevymmo_gameplay::items::components::{Equipment, Inventory};
use bevymmo_gameplay::items::{ItemId, ItemRegistry};
use bevymmo_gameplay::stats::components::{CombatStats, GatheringStats, MovementStats, VitalStats};
use bevymmo_network::network::protocol::{SpellCastEnded, SpellCastProgress, SpellVisualEffect};
use bevymmo_network::world_components::{
    AoeZone, EntityColor, LookDirection, NetworkEntityId, Position, ProjectileFlight,
    ProjectileVisual,
};
use bevymmo_world::{CollisionGrid, SurfaceQuery};
use crossbeam_channel::{unbounded, Receiver, Sender};
use spacetimedb_sdk::{
    credentials, DbContext, EventTable, Identity, Table, TableWithPrimaryKey, Uuid,
};
use std::collections::{HashMap, HashSet};

use super::combat_input::send_combat_inputs;
use super::module_bindings::active_status_table::ActiveStatusTableAccess;
use super::module_bindings::aoe_region_table::AoeRegionTableAccess;
use super::module_bindings::boss_state_table::BossStateTableAccess;
use super::module_bindings::cast_ended_table::CastEndedTableAccess;
use super::module_bindings::cast_state_table::CastStateTableAccess;
use super::module_bindings::character_wallet_table::CharacterWalletTableAccess;
use super::module_bindings::cooldown_table::CooldownTableAccess;
use super::module_bindings::craft_session_table::CraftSessionTableAccess;
use super::module_bindings::crowd_control_table::CrowdControlTableAccess;
use super::module_bindings::delete_character_reducer::delete_character;
use super::module_bindings::entity_stats_table::EntityStatsTableAccess;
use super::module_bindings::equipment_table::EquipmentTableAccess;
use super::module_bindings::game_entity_table::GameEntityTableAccess;
use super::module_bindings::gather_session_table::GatherSessionTableAccess;
use super::module_bindings::gather_yield_table::GatherYieldTableAccess;
use super::module_bindings::heartbeat_reducer::heartbeat;
use super::module_bindings::hotbar_table::HotbarTableAccess;
use super::module_bindings::inventory_table::InventoryTableAccess;
use super::module_bindings::join_reducer::join;
use super::module_bindings::known_ancient_language_table::KnownAncientLanguageTableAccess;
use super::module_bindings::leave_reducer::leave;
use super::module_bindings::login_reducer::login;
use super::module_bindings::logout_reducer::logout;
use super::module_bindings::market_buy_order_table::MarketBuyOrderTableAccess;
use super::module_bindings::market_sell_order_table::MarketSellOrderTableAccess;
use super::module_bindings::move_to_reducer::move_to;
use super::module_bindings::npc_table::NpcTableAccess;
use super::module_bindings::periodic_effect_table::PeriodicEffectTableAccess;
use super::module_bindings::player_message_table::PlayerMessageTableAccess;
use super::module_bindings::player_table::PlayerTableAccess;
use super::module_bindings::projectile_table::ProjectileTableAccess;
use super::module_bindings::register_reducer::register;
use super::module_bindings::resource_node_table::ResourceNodeTableAccess;
use super::module_bindings::session_table::SessionTableAccess;
use super::module_bindings::spell_visual_effect_table::SpellVisualEffectTableAccess;
use super::module_bindings::stat_modifier_table::StatModifierTableAccess;
use super::module_bindings::{
    ActiveStatus, AoeRegion, BossPhaseRow, BossState, CastEndedEvent, CastKindRow, CastState,
    CharacterWallet, ColorRow, Cooldown, CraftSession, CrowdControl, CrowdControlKindRow,
    DbConnection, EntityKindRow, EntityStateRow, EntityStats, EquipmentTable,
    GameEntity as EntityRow, GatherSession, GatherYieldEvent, Hotbar, InventoryTable,
    ItemInstanceRow, KnownAncientLanguageTable, MarketBuyOrder, MarketSellOrder, ModifierKindRow,
    Npc, PeriodicEffect, Player, PlayerMessageEvent, Projectile, ReducerEventContext,
    RemoteReducers, ResourceNode, Session, SpellVisualEffectEvent, StatModifier, Vec3Row,
};

/// How fast predicted position is pulled back towards the authoritative one, as
/// a rate per second. Higher snaps harder and shows correction jitter; lower
/// drifts visibly before catching up.
const RECONCILE_RATE: f32 = 8.0;

/// Seconds between destination updates while the mouse button is held.
const MOVE_COMMAND_INTERVAL: f32 = 0.1;

/// Seconds between presence heartbeats.
///
/// Comfortably inside the module's `PRESENCE_TIMEOUT_SECONDS` so an ordinary
/// stall does not read as a disconnect. The module cannot enumerate live
/// connections, so this is how it knows anyone is still here — and why a
/// restarted server settles to "nobody online" instead of showing ghosts.
const HEARTBEAT_INTERVAL: f32 = 5.0;

/// How long a graceful shutdown waits for a queued disconnect to actually
/// reach the socket before letting the process die anyway.
///
/// `disconnect()` only queues the close — [`DbConnection::frame_tick`] is what
/// sends it — so exiting on the same frame the shutdown was requested would
/// tear the connection down mid-send almost every time, leaving `Player.online`
/// stuck `true` until the server's own presence timeout notices. This is a few
/// frames' worth of margin, not a real wait: [`finish_shutdown`] exits the
/// moment [`DbContext::is_active`] reports the disconnect went through.
const SHUTDOWN_GRACE_SECONDS: f32 = 0.5;

/// The server's last word on an entity, kept apart from the rendered
/// [`Position`] so prediction has something to reconcile against.
#[derive(Component, Debug, Clone, Copy)]
pub struct StdbAuthoritative {
    pub position: Vec3,
    pub move_target: Option<Vec3>,
    pub speed: f32,
}

/// Local mirror of the lock `move_to` consults, so a held RMB does not
/// spam Charge-cancelling destinations.
#[derive(Component, Debug, Clone, Copy)]
pub struct ActiveCastLock(pub MovementLock);

/// Client simulation that presentation systems order against.
///
/// Ability input arms [`LocalMovementFreeze`] and writes aim facing *before*
/// prediction runs, so a rooted cast stops on the same frame and the walk
/// look does not overwrite the cursor facing.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientSimulation {
    Predict,
}

/// Row changes handed from the SDK's thread to the Bevy schedule.
enum RowEvent {
    Entity(EntityRow),
    EntityRemoved(u64),
    Stats(EntityStats),
    Player(Player),
    Session(Session),
    Inventory(InventoryTable),
    Wallet(CharacterWallet),
    Equipment(EquipmentTable),
    Hotbar(Hotbar),
    KnownAncientLanguage(KnownAncientLanguageTable),
    CastState(CastState),
    CastEnded(CastEndedEvent),
    SpellVisualEffect(SpellVisualEffectEvent),
    BossState(BossState),
    ActiveStatus(ActiveStatus),
    ActiveStatusRemoved(ActiveStatus),
    CrowdControl(CrowdControl),
    CrowdControlRemoved(CrowdControl),
    StatModifier(StatModifier),
    StatModifierRemoved(StatModifier),
    PeriodicEffect(PeriodicEffect),
    PeriodicEffectRemoved(PeriodicEffect),
    Cooldown(Cooldown),
    CooldownRemoved(Cooldown),
    Projectile(Projectile),
    ProjectileRemoved(u64),
    AoeRegion(AoeRegion),
    AoeRegionRemoved(u64),
    Npc(Npc),
    ResourceNode(ResourceNode),
    GatherSession(GatherSession),
    GatherSessionRemoved(u64),
    CraftSession(CraftSession),
    CraftSessionRemoved(u64),
    GatherYield(GatherYieldEvent),
    SellOrder(MarketSellOrder),
    SellOrderRemoved(u64),
    BuyOrder(MarketBuyOrder),
    BuyOrderRemoved(u64),
    PlayerMessage(PlayerMessageEvent),
    /// A reducer the client called came back with the module's own `Err`.
    ReducerRejected(String),
    JoinRejected(String),
    /// `register`/`login` confirmed the caller's connection as an account.
    AuthAccepted,
    AuthRejected(String),
}

/// Latest rows retained until the dependent Bevy entity exists. Initial
/// subscription rows have no delivery order guarantee.
///
/// Caches the latest received rows per entity so that `replay_entity` can
/// restore full component state on (re)connect or after a reconcile gap.
#[derive(Resource, Default)]
struct PendingRows {
    entities: HashMap<u64, EntityRow>,
    /// Every `player` row seen, regardless of which account owns it —
    /// unlike [`CharacterRoster`], which only ever holds *this* account's
    /// characters. Kept so [`recompute_roster`] can rebuild the roster from
    /// scratch once [`LocalCharacter::account_id`] becomes known, since
    /// `player` rows and the `session` row that reveals the account id have
    /// no guaranteed delivery order — see `RowEvent::Session`'s comment.
    players: HashMap<Uuid, Player>,
    offline_players: HashSet<Uuid>,
    stats: HashMap<u64, EntityStats>,
    inventory: HashMap<Uuid, InventoryTable>,
    equipment: HashMap<Uuid, EquipmentTable>,
    hotbar: HashMap<Uuid, Hotbar>,
    known_ancient_language: HashMap<Uuid, KnownAncientLanguageTable>,
    npcs: HashMap<u64, Npc>,
    resource_nodes: HashMap<u64, ResourceNode>,
    gather_sessions: HashMap<u64, GatherSession>,
    craft_sessions: HashMap<u64, CraftSession>,
    boss_state: HashMap<u64, BossState>,
    /// Keyed by `active_status.id`, not by entity: one entity can carry several.
    active_status: HashMap<u64, ActiveStatus>,
    /// Keyed by `crowd_control.id`, not by entity: one entity can carry several.
    crowd_control: HashMap<u64, CrowdControl>,
    /// Keyed by `stat_modifier.id`.
    stat_modifier: HashMap<u64, StatModifier>,
    /// Keyed by `periodic_effect.id`.
    periodic_effect: HashMap<u64, PeriodicEffect>,
    /// What was last written to each entity's `CrowdControlState` and
    /// `ActiveStatModifiers`.
    ///
    /// Both are rebuilt from scratch whenever any contributing row changes, and
    /// `replay_entity` runs on every `game_entity` update — which is every
    /// moving entity, twenty times a second. Without this an unchanged, empty
    /// state was re-inserted on each of those, so every `Changed<>` filter in
    /// the UI fired continuously on entities that had no effects at all.
    applied: HashMap<u64, AppliedEffects>,
}

/// The last effect state written to one entity, for change suppression.
#[derive(Default, PartialEq, Debug)]
struct AppliedEffects {
    active_status: ActiveStatuses,
    crowd_control: CrowdControlState,
    /// The contributing rows, as `(id, remaining_seconds bits)`.
    status_signature: Vec<(u64, u32)>,
    /// `StatModifierInstance` has no `PartialEq` to compare the built component
    /// with, and the rows are what decides it anyway.
    modifier_signature: Vec<(u64, u32)>,
}

/// Owns the connection. Call reducers through [`StdbConnection::reducers`].
#[derive(Resource)]
pub struct StdbConnection {
    conn: DbConnection,
    events: Receiver<RowEvent>,
    /// Handed to reducer callbacks so a server-side refusal can travel the same
    /// path as a row change, and reach the schedule where it can be shown.
    reports: Sender<RowEvent>,
}

#[derive(Resource, Clone)]
struct StdbConnectionConfig {
    uri: String,
    module: String,
}

impl StdbConnection {
    /// The reducer handle — how the client asks the server to do anything.
    pub fn reducers(&self) -> &RemoteReducers {
        self.conn.reducers()
    }

    /// This client's identity, once the connection has been established.
    pub fn identity(&self) -> Option<Identity> {
        self.conn.try_identity()
    }

    /// Inventory, equipment, hotbar, language and cooldowns for *this*
    /// character only. The initial subscribe is world-wide combat state;
    /// bags stay off the wire until we know who we are playing.
    fn subscribe_owned_rows(&self, character_id: Uuid, entity_id: Option<u64>) {
        let mut queries = vec![
            format!("SELECT * FROM inventory WHERE character_id = '{character_id}'"),
            format!("SELECT * FROM equipment WHERE character_id = '{character_id}'"),
            format!("SELECT * FROM hotbar WHERE character_id = '{character_id}'"),
            format!("SELECT * FROM known_ancient_language WHERE character_id = '{character_id}'"),
            format!("SELECT * FROM character_wallet WHERE character_id = '{character_id}'"),
        ];
        if let Some(entity_id) = entity_id {
            queries.push(format!(
                "SELECT * FROM cooldown WHERE entity_id = {entity_id}"
            ));
        }
        let _ = self
            .conn
            .subscription_builder()
            .on_error(|_ctx, err| error!("owned-row subscription failed: {err}"))
            .subscribe(queries);
    }

    /// Builds the callback every reducer wrapper hands to its `*_then` form.
    ///
    /// The module answers a rejected call with a sentence written for the
    /// player — "inventory is full", "target is out of range", "that name is
    /// taken". Before this existed every one of them was discarded: the
    /// fire-and-forget send reported only whether the *request* left the
    /// machine, never what the server made of it.
    ///
    /// `action` names what the player was trying to do, so the notice reads as
    /// a sentence rather than as a bare server string.
    pub(super) fn report_rejection(
        &self,
        action: &'static str,
    ) -> impl FnOnce(&ReducerEventContext, ReducerOutcome) + Send + 'static {
        let reports = self.reports.clone();
        move |_ctx, outcome| {
            let message = match outcome {
                Ok(Ok(())) => return,
                Ok(Err(reason)) => format!("{action}: {reason}"),
                // The call reached the module but its result could not be
                // decoded. Not the player's fault, but silence would be worse.
                Err(err) => format!("{action}: {err}"),
            };
            let _ = reports.send(RowEvent::ReducerRejected(message));
        }
    }

    fn report_join_rejection(
        &self,
    ) -> impl FnOnce(&ReducerEventContext, ReducerOutcome) + Send + 'static {
        let reports = self.reports.clone();
        move |_ctx, outcome| {
            let message = match outcome {
                Ok(Ok(())) => return,
                Ok(Err(reason)) => format!("Impossibile entrare: {reason}"),
                Err(err) => format!("Impossibile entrare: {err}"),
            };
            let _ = reports.send(RowEvent::JoinRejected(message));
        }
    }

    /// Unlike [`Self::report_rejection`], `register`/`login` also report
    /// success: `AuthState::Authenticating` is a real waiting period the UI
    /// shows while the round trip is in flight, not an optimistic guess, so
    /// something has to confirm it turned into `Authenticated`.
    fn report_auth_outcome(
        &self,
    ) -> impl FnOnce(&ReducerEventContext, ReducerOutcome) + Send + 'static {
        let reports = self.reports.clone();
        move |_ctx, outcome| {
            let event = match outcome {
                Ok(Ok(())) => RowEvent::AuthAccepted,
                Ok(Err(reason)) => RowEvent::AuthRejected(reason),
                Err(err) => RowEvent::AuthRejected(err.to_string()),
            };
            let _ = reports.send(event);
        }
    }
}

/// What a `*_then` callback is handed: the module's own `Result`, or the SDK
/// failing to decode one.
type ReducerOutcome = Result<Result<(), String>, spacetimedb_sdk::__codegen::InternalError>;

/// Maps server entity ids to the Bevy entities mirroring them.
#[derive(Resource, Default)]
pub struct StdbEntityMap {
    by_entity_id: HashMap<u64, Entity>,
    /// Which server entity belongs to which character, so the per-character
    /// tables (inventory, equipment, hotbar) can find the entity to attach to.
    entity_of_character: HashMap<Uuid, u64>,
    /// Projectiles live in their own id space — `projectile.id` counts
    /// separately from `game_entity.entity_id` — so they get their own map
    /// rather than colliding in the one above.
    projectiles: HashMap<u64, Entity>,
    aoes: HashMap<u64, Entity>,
}

impl StdbEntityMap {
    pub fn get(&self, entity_id: u64) -> Option<Entity> {
        self.by_entity_id.get(&entity_id).copied()
    }
}

/// Which account and character (if any) this connection is authenticated as
/// and playing.
///
/// Resolved from the `session` table, which — unlike `account` — is
/// `public` precisely so the owning client can learn its own `account_id`
/// (see `tables::Session`'s doc comment) and, once `join` selects one, its
/// `character_id`.
#[derive(Resource, Default)]
struct LocalCharacter {
    /// Last character we opened a personal inventory/equipment subscription for.
    subscribed_character: Option<Uuid>,
    account_id: Option<u64>,
    character_id: Option<Uuid>,
}

/// One of the caller's own characters, for the character-select screen.
///
/// Cheap to keep around: a handful of fields per character, at most
/// [`crate::app_state::MAX_CHARACTERS_PER_ACCOUNT`] of them.
#[derive(Clone, Debug, PartialEq)]
pub struct RosterCharacter {
    pub character_id: Uuid,
    pub display_name: String,
    pub online: bool,
}

/// Gold of the character this client is currently playing.
///
/// The table `character_wallet` is the source of truth; this resource is the
/// HUD-facing copy for the local character only. Other characters' wallets
/// are ignored even though the table is public.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalGold {
    pub amount: u64,
}

/// Sell orders replicated from `market_sell_order`. UI filters by `market_id`.
#[derive(Resource, Default)]
pub struct MarketOrderBook {
    pub orders: HashMap<u64, MarketSellOrder>,
}

/// Buy orders replicated from `market_buy_order`. UI filters by `market_id`.
#[derive(Resource, Default)]
pub struct MarketBuyBook {
    pub orders: HashMap<u64, MarketBuyOrder>,
}

/// Present on an NPC entity that opens an isolated player market.
#[derive(Component, Debug, Clone)]
pub struct NpcMarket {
    pub market_id: String,
}

/// Catalogue kind of a mirrored NPC (`npc_greeter`, `npc_weapon_crafter`, …).
#[derive(Component, Debug, Clone)]
pub struct NpcKind {
    pub kind_id: String,
}

/// The caller's own characters (from the public `player` table, filtered to
/// [`LocalCharacter::account_id`]), for the character-select screen.
///
/// A plain resource, not ECS entities: an offline character has no mirrored
/// `game_entity` (see [`RowEvent::Player`]'s `else` branch), so there is
/// nothing in the world for the character-select screen to query — this is
/// the only place its name and id are still available.
#[derive(Resource, Default)]
pub struct CharacterRoster {
    characters: HashMap<Uuid, RosterCharacter>,
}

impl CharacterRoster {
    pub fn iter(&self) -> impl Iterator<Item = &RosterCharacter> {
        self.characters.values()
    }

    pub fn len(&self) -> usize {
        self.characters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.characters.is_empty()
    }
}

/// Rebuilds `roster` from scratch out of every cached `player` row, filtered
/// to `local.account_id`. A full rebuild rather than an incremental
/// insert/remove because the order `player` and `session` rows arrive in is
/// not guaranteed — see the call sites on `RowEvent::Player`/`RowEvent::Session`.
/// Cheap enough: at most `MAX_CHARACTERS_PER_ACCOUNT` characters ever belong
/// to one account, and this only runs when one of those two row kinds changes.
fn recompute_roster(pending: &PendingRows, local: &LocalCharacter, roster: &mut CharacterRoster) {
    let Some(account_id) = local.account_id else {
        roster.characters.clear();
        return;
    };
    roster.characters = pending
        .players
        .values()
        .filter(|player| player.account_id == account_id)
        .map(|player| {
            (
                player.character_id,
                RosterCharacter {
                    character_id: player.character_id,
                    display_name: player.display_name.clone(),
                    online: player.online,
                },
            )
        })
        .collect();
}

/// The client-side replication state, bundled into one [`SystemParam`] so
/// `drain_events` — which already touches every mirrored table — does not
/// cross the workspace's raised `too-many-arguments` threshold (see
/// `clippy.toml`) just by also tracking [`LocalCharacter`].
#[derive(bevy::ecs::system::SystemParam)]
struct ReplicationState<'w> {
    map: ResMut<'w, StdbEntityMap>,
    pending: ResMut<'w, PendingRows>,
    local: ResMut<'w, LocalCharacter>,
    roster: ResMut<'w, CharacterRoster>,
    gold: ResMut<'w, LocalGold>,
    markets: ResMut<'w, MarketOrderBook>,
    bids: ResMut<'w, MarketBuyBook>,
}

/// [`AuthState`]/[`AuthFailure`], bundled for the same reason as
/// [`ReplicationState`]: `drain_events` is already near the workspace's
/// raised `too-many-arguments` threshold.
#[derive(bevy::ecs::system::SystemParam)]
struct AuthResources<'w> {
    state: ResMut<'w, AuthState>,
    failure: ResMut<'w, AuthFailure>,
}

/// Player-facing messages and the world anchors they need, bundled so adding
/// [`WorldTextCue`] does not push [`drain_events`] over the argument threshold.
#[derive(bevy::ecs::system::SystemParam)]
struct PlayerFacingMessages<'w, 's> {
    notices: MessageWriter<'w, ServerNotice>,
    chat_lines: MessageWriter<'w, ChatLine>,
    world_text: MessageWriter<'w, WorldTextCue>,
    local_player: Query<'w, 's, (Entity, &'static Position), With<LocalPlayer>>,
    positions: Query<'w, 's, &'static Position>,
    items: Option<Res<'w, ItemRegistry>>,
}

/// Amber for a normal gather tick; brighter gold when the roll granted extra.
const GATHER_AMBER: Color = Color::srgb(1.0, 0.72, 0.18);
const GATHER_BONUS_GOLD: Color = Color::srgb(1.0, 0.88, 0.32);

impl LocalCharacter {
    /// Whether `character_id` (from a `game_entity` row's `owner_character_id`,
    /// or a `player` row's own id) is this connection's active character.
    fn is(&self, character_id: Option<Uuid>) -> bool {
        matches!((self.character_id, character_id), (Some(a), Some(b)) if a == b)
    }
}

pub struct StdbPlugin {
    pub uri: String,
    pub module: String,
}

impl Plugin for StdbPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ChatLine>();
        app.add_message::<SpellVisualEffect>();
        app.add_message::<SpellCastProgress>();
        app.add_message::<SpellCastEnded>();
        app.add_message::<ServerNotice>();
        app.add_message::<SpellCooldownState>();
        app.add_message::<WorldTextCue>();

        let uri = self.uri.clone();
        let module = self.module.clone();

        app.init_resource::<StdbEntityMap>();
        app.init_resource::<PendingRows>();
        // Owned by the presentation layer's map loader, which fills them in
        // once a map is loaded. Initialised here too so `predict_and_reconcile`
        // can take them as plain `Res` no matter which plugins an app builds
        // with — a missing resource is a panic, not a skipped system.
        app.init_resource::<ClientSurfaceQuery>();
        app.init_resource::<ClientCollision>();
        app.init_resource::<LocalMovementFreeze>();
        app.init_resource::<LocalCharacter>();
        app.init_resource::<CharacterRoster>();
        app.init_resource::<LocalGold>();
        app.init_resource::<MarketOrderBook>();
        app.init_resource::<MarketBuyBook>();
        app.init_resource::<ShuttingDown>();
        app.insert_resource(StdbConnectionConfig {
            uri: uri.clone(),
            module: module.clone(),
        });
        app.init_resource::<PartyRoster>();
        app.add_systems(Startup, move |world: &mut World| {
            match connect(&uri, &module) {
                Ok((connection, party_events)) => {
                    world.insert_resource(connection);
                    world.insert_resource(party_events);
                }
                // Not fatal: the menu stays usable and the player can retry
                // rather than the process dying on a cold database.
                Err(err) => error!("SpacetimeDB connection to {uri} failed: {err}"),
            }
        });
        // Unconditional, and ordered before the connection pump: a shutdown
        // request must queue its disconnect before `frame_tick` runs so the
        // very next pump has something to send, and it must be caught even if
        // `StdbConnection` never existed (a close during a failed connection
        // attempt).
        app.add_systems(PreUpdate, begin_shutdown.before(pump_connection));
        app.add_systems(
            PreUpdate,
            (pump_connection, drain_events)
                .chain()
                .run_if(resource_exists::<StdbConnection>),
        );
        app.add_systems(
            PreUpdate,
            drain_party_events
                .after(pump_connection)
                .run_if(resource_exists::<StdbConnection>)
                .run_if(resource_exists::<PartyEvents>),
        );
        app.add_systems(Update, finish_shutdown);
        app.add_systems(Update, retry_connect_on_play);
        app.add_systems(
            Update,
            (
                auth_on_request,
                delete_character_on_request,
                join_on_request,
            )
                .run_if(resource_exists::<StdbConnection>),
        );
        app.add_systems(
            Update,
            (
                // `select_move_target` writes the `MoveTarget` this system
                // consumes; ordering after it keeps the value read here from
                // the same frame's click instead of one frame stale.
                send_move_commands.after(crate::player_movement::select_move_target),
                send_combat_inputs,
            )
                .run_if(resource_exists::<StdbConnection>)
                // InGame includes the pause overlay: pause is client-only and
                // does not stop the network or simulation (see `Screen`).
                .run_if(in_state(Screen::InGame))
                .run_if(crate::app_state::not_typing),
        );
        app.add_systems(
            Update,
            send_heartbeat.run_if(resource_exists::<StdbConnection>),
        );
        app.configure_sets(Update, ClientSimulation::Predict);
        app.add_systems(
            Update,
            (predict_and_reconcile, predict_projectiles).in_set(ClientSimulation::Predict),
        );
    }
}

/// Produces a filesystem-safe cache key per SpacetimeDB instance and module.
/// Tokens are signed by a server's key, so reusing one across instances causes
/// the new server to reject the connection before it can issue a new identity.
fn credential_key(uri: &str, module: &str) -> String {
    let server: String = uri
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect();

    format!("bevymmo_{server}_{module}")
}

/// Forgets a cached token for `uri`/`module`, so the next [`connect`] mints a
/// fresh [`Identity`](spacetimedb_sdk::Identity) instead of resuming whichever
/// character was last attached to this machine.
///
/// [`credentials::File`] has no delete method, so this reconstructs its own
/// path (`~/.spacetimedb_client_credentials/<key>`) by hand.
fn forget_cached_credentials(uri: &str, module: &str) {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let path = home
        .join(".spacetimedb_client_credentials")
        .join(credential_key(uri, module));
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        // Nothing was cached yet — the common case on a fresh machine.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => warn!("could not remove cached SpacetimeDB token at {path:?}: {err}"),
    }
}

fn connect(
    uri: &str,
    module: &str,
) -> Result<(StdbConnection, PartyEvents), Box<dyn std::error::Error>> {
    let (tx, events) = unbounded();
    let reports = tx.clone();
    let credential_key = credential_key(uri, module);

    let cached_token = credentials::File::new(&credential_key)
        .load()
        .ok()
        .flatten();
    let conn = match build_connection(uri, module, &credential_key, cached_token.clone()) {
        Ok(conn) => conn,
        // A cached token is signed by the server that issued it. A dev server
        // restarted with a fresh keypair (or a module recreated from scratch)
        // rejects it at the handshake itself — a 401, not a normal
        // `on_connect_error` — so every future launch would otherwise retry
        // the same dead token forever. Drop it and mint a new identity
        // instead, the same recovery `forget_cached_credentials` already does
        // for an explicit logout.
        Err(err) if cached_token.is_some() && looks_like_auth_rejection(&*err) => {
            warn!(
                "cached SpacetimeDB token was rejected ({err}); reconnecting with a fresh identity"
            );
            forget_cached_credentials(uri, module);
            build_connection(uri, module, &credential_key, None)?
        }
        Err(err) => return Err(err),
    };

    register_callbacks(&conn, tx);

    let (party_tx, party_events) = unbounded();
    register_party_callbacks(&conn, party_tx);

    conn.subscription_builder()
        .on_applied(|_ctx| info!("SpacetimeDB subscription applied"))
        .on_error(|_ctx, err| error!("SpacetimeDB subscription failed: {err}"))
        .subscribe([
            "SELECT * FROM game_entity",
            "SELECT * FROM entity_stats",
            "SELECT * FROM player",
            "SELECT * FROM session",
            "SELECT * FROM cast_state",
            "SELECT * FROM boss_state",
            "SELECT * FROM active_status",
            "SELECT * FROM crowd_control",
            "SELECT * FROM stat_modifier",
            "SELECT * FROM periodic_effect",
            "SELECT * FROM projectile",
            "SELECT * FROM aoe_region",
            "SELECT * FROM cast_ended",
            "SELECT * FROM spell_visual_effect",
            "SELECT * FROM player_message",
            "SELECT * FROM party",
            "SELECT * FROM party_member",
            "SELECT * FROM npc",
            "SELECT * FROM enemy_ai",
            "SELECT * FROM resource_node",
            "SELECT * FROM gather_session",
            "SELECT * FROM craft_session",
            "SELECT * FROM gather_yield",
            "SELECT * FROM market",
            "SELECT * FROM market_sell_order",
            "SELECT * FROM market_buy_order",
        ]);

    Ok((
        StdbConnection {
            conn,
            events,
            reports,
        },
        PartyEvents(party_events),
    ))
}

/// Builds and opens the actual SDK connection, with or without a token.
/// Factored out of [`connect`] so a rejected cached token can be retried once
/// anonymously without duplicating the callback wiring.
fn build_connection(
    uri: &str,
    module: &str,
    credential_key: &str,
    token: Option<String>,
) -> Result<DbConnection, Box<dyn std::error::Error>> {
    let credential_key = credential_key.to_string();
    let conn = DbConnection::builder()
        .with_uri(uri)
        .with_database_name(module)
        .with_token(token)
        .on_connect(move |_ctx, _identity, token| {
            if let Err(err) = credentials::File::new(&credential_key).save(token) {
                warn!("could not cache the SpacetimeDB token: {err}");
            }
        })
        .on_connect_error(|_ctx, err| error!("SpacetimeDB connection error: {err}"))
        .on_disconnect(|_ctx, err| match err {
            Some(err) => error!("disconnected from SpacetimeDB: {err}"),
            None => info!("disconnected from SpacetimeDB"),
        })
        .build()?;
    Ok(conn)
}

/// Whether a failed connection attempt looks like the server refusing a
/// specific token, as opposed to being unreachable altogether — the
/// distinction that decides if retrying anonymously can help.
fn looks_like_auth_rejection(err: &(dyn std::error::Error + 'static)) -> bool {
    let message = err.to_string();
    message.contains("401") || message.contains("Unauthorized")
}

/// Every callback does the same thing: clone the row onto the channel. They stay
/// this dumb on purpose — they run outside the Bevy schedule and must not touch
/// the world.
fn register_callbacks(conn: &DbConnection, tx: Sender<RowEvent>) {
    macro_rules! mirror {
        ($table:ident, $variant:ident) => {{
            let inserted = tx.clone();
            conn.db().$table().on_insert(move |_ctx, row| {
                let _ = inserted.send(RowEvent::$variant(row.clone()));
            });
            let updated = tx.clone();
            conn.db().$table().on_update(move |_ctx, _old, new| {
                let _ = updated.send(RowEvent::$variant(new.clone()));
            });
        }};
    }

    mirror!(game_entity, Entity);
    mirror!(entity_stats, Stats);
    mirror!(player, Player);
    mirror!(session, Session);
    mirror!(inventory, Inventory);
    mirror!(character_wallet, Wallet);
    mirror!(equipment, Equipment);
    mirror!(hotbar, Hotbar);
    mirror!(known_ancient_language, KnownAncientLanguage);
    mirror!(cast_state, CastState);
    mirror!(boss_state, BossState);
    mirror!(active_status, ActiveStatus);
    mirror!(crowd_control, CrowdControl);
    mirror!(stat_modifier, StatModifier);
    mirror!(periodic_effect, PeriodicEffect);
    mirror!(cooldown, Cooldown);
    mirror!(projectile, Projectile);
    mirror!(aoe_region, AoeRegion);
    mirror!(npc, Npc);
    mirror!(resource_node, ResourceNode);
    mirror!(gather_session, GatherSession);
    mirror!(craft_session, CraftSession);
    mirror!(market_sell_order, SellOrder);
    mirror!(market_buy_order, BuyOrder);

    // Deletions matter for anything the client keeps a copy of: a stun that
    // ends, a buff that expires, a projectile that lands. Without these the
    // effect would stay on screen with a frozen timer.
    macro_rules! mirror_delete {
        ($table:ident, $variant:ident) => {{
            let deleted = tx.clone();
            conn.db().$table().on_delete(move |_ctx, row| {
                let _ = deleted.send(RowEvent::$variant(row.clone()));
            });
        }};
    }

    mirror_delete!(active_status, ActiveStatusRemoved);
    mirror_delete!(crowd_control, CrowdControlRemoved);
    mirror_delete!(stat_modifier, StatModifierRemoved);
    mirror_delete!(periodic_effect, PeriodicEffectRemoved);
    mirror_delete!(cooldown, CooldownRemoved);

    {
        let deleted = tx.clone();
        conn.db().market_sell_order().on_delete(move |_ctx, row| {
            let _ = deleted.send(RowEvent::SellOrderRemoved(row.id));
        });
    }

    {
        let deleted = tx.clone();
        conn.db().market_buy_order().on_delete(move |_ctx, row| {
            let _ = deleted.send(RowEvent::BuyOrderRemoved(row.id));
        });
    }

    // Event tables are insert-only by design: a row is delivered and not
    // retained, which is exactly the lifetime "play this once" wants.
    macro_rules! mirror_event {
        ($table:ident, $variant:ident) => {{
            let fired = tx.clone();
            conn.db().$table().on_insert(move |_ctx, row| {
                let _ = fired.send(RowEvent::$variant(row.clone()));
            });
        }};
    }

    mirror_event!(cast_ended, CastEnded);
    mirror_event!(spell_visual_effect, SpellVisualEffect);
    mirror_event!(player_message, PlayerMessage);
    mirror_event!(gather_yield, GatherYield);

    let projectile_removed = tx.clone();
    conn.db().projectile().on_delete(move |_ctx, row| {
        let _ = projectile_removed.send(RowEvent::ProjectileRemoved(row.id));
    });
    let aoe_removed = tx.clone();
    conn.db().aoe_region().on_delete(move |_ctx, row| {
        let _ = aoe_removed.send(RowEvent::AoeRegionRemoved(row.id));
    });

    let removed = tx.clone();
    conn.db().game_entity().on_delete(move |_ctx, row| {
        let _ = removed.send(RowEvent::EntityRemoved(row.entity_id));
    });
    let gather_removed = tx.clone();
    conn.db().gather_session().on_delete(move |_ctx, row| {
        let _ = gather_removed.send(RowEvent::GatherSessionRemoved(row.entity_id));
    });
    let craft_removed = tx.clone();
    conn.db().craft_session().on_delete(move |_ctx, row| {
        let _ = craft_removed.send(RowEvent::CraftSessionRemoved(row.entity_id));
    });
}

/// Processes whatever the server has sent since the last frame.
///
/// `frame_tick` is the non-blocking variant: it applies pending messages and
/// returns rather than owning a thread. That keeps every row callback on the
/// main thread and inside the Bevy frame, which is why [`drain_events`] can run
/// immediately after and see a consistent batch.
fn pump_connection(
    conn: Res<StdbConnection>,
    screen: Res<State<Screen>>,
    mut next_screen: ResMut<NextState<Screen>>,
    mut failure: ResMut<ConnectionFailure>,
) {
    let lost = match conn.conn.frame_tick() {
        Err(err) => {
            error!("SpacetimeDB frame_tick failed: {err}");
            Some(format!("Connessione persa: {err}"))
        }
        Ok(()) if !conn.conn.is_active() => Some("Connessione a SpacetimeDB chiusa".to_string()),
        Ok(()) => None,
    };
    let Some(message) = lost else {
        return;
    };
    if let Some(next) = screen_after_connection_loss(*screen.get()) {
        failure.0 = Some(message);
        next_screen.set(next);
    }
}

fn drain_events(
    conn: Res<StdbConnection>,
    mut state: ReplicationState,
    mut auth: AuthResources,
    mut commands: Commands,
    mut cast_progress: MessageWriter<SpellCastProgress>,
    mut cast_ended: MessageWriter<SpellCastEnded>,
    mut visual_effects: MessageWriter<SpellVisualEffect>,
    mut cooldowns: MessageWriter<SpellCooldownState>,
    mut feed: PlayerFacingMessages,
    mut failure: ResMut<ConnectionFailure>,
    mut next_screen: ResMut<NextState<Screen>>,
) {
    let local_identity = conn.identity();

    while let Ok(event) = conn.events.try_recv() {
        match event {
            RowEvent::Entity(row) => {
                let entity_id = row.entity_id;
                let owner = row.owner_character_id;
                state.pending.entities.insert(entity_id, row.clone());
                if owner.is_some_and(|character_id| {
                    state.pending.offline_players.contains(&character_id)
                }) {
                    continue;
                }

                let is_new = !state.map.by_entity_id.contains_key(&entity_id);
                apply_entity(
                    &mut commands,
                    &mut state.map,
                    &row,
                    &state.local,
                    &state.pending,
                );
                replay_entity(&mut commands, &state.map, &mut state.pending, entity_id);
                if is_new {
                    if let Some(character_id) = owner {
                        replay_character(
                            &mut commands,
                            &state.map,
                            &state.pending,
                            character_id,
                            state.local.character_id,
                        );
                    }
                }
            }
            RowEvent::EntityRemoved(entity_id) => {
                if let Some(entity) = state.map.by_entity_id.remove(&entity_id) {
                    commands.entity(entity).despawn();
                }
                // Everything keyed by this entity goes with it — including the
                // per-character rows, which are keyed by `character_id` and so
                // were outliving the character they belonged to.
                let owners: Vec<Uuid> = state
                    .map
                    .entity_of_character
                    .iter()
                    .filter(|(_, id)| **id == entity_id)
                    .map(|(character_id, _)| *character_id)
                    .collect();
                for character_id in owners {
                    state.map.entity_of_character.remove(&character_id);
                    state.pending.inventory.remove(&character_id);
                    state.pending.equipment.remove(&character_id);
                    state.pending.hotbar.remove(&character_id);
                    state.pending.known_ancient_language.remove(&character_id);
                    state.pending.players.remove(&character_id);
                    state.roster.characters.remove(&character_id);
                    if state.local.character_id == Some(character_id) {
                        state.local.character_id = None;
                    }
                }
                state.pending.entities.remove(&entity_id);
                state.pending.stats.remove(&entity_id);
                state.pending.boss_state.remove(&entity_id);
                state
                    .pending
                    .active_status
                    .retain(|_, row| row.entity_id != entity_id);
                state.pending.applied.remove(&entity_id);
                state
                    .pending
                    .crowd_control
                    .retain(|_, cc| cc.entity_id != entity_id);
                state
                    .pending
                    .stat_modifier
                    .retain(|_, row| row.entity_id != entity_id);
                state
                    .pending
                    .periodic_effect
                    .retain(|_, row| row.entity_id != entity_id);
            }
            RowEvent::Stats(row) => {
                let entity_id = row.entity_id;
                state.pending.stats.insert(entity_id, row);
                replay_entity(&mut commands, &state.map, &mut state.pending, entity_id);
            }
            RowEvent::Player(row) => {
                state
                    .map
                    .entity_of_character
                    .insert(row.character_id, row.entity_id);
                if state.local.character_id == Some(row.character_id) {
                    conn.subscribe_owned_rows(row.character_id, Some(row.entity_id));
                    state.local.subscribed_character = Some(row.character_id);
                }

                // Cached regardless of account, then filtered back out in
                // `recompute_roster` — this row can arrive before the
                // `Session` row that reveals which account is ours (initial
                // subscription snapshots have no ordering guarantee), and
                // without the full cache that character would never make it
                // into the roster: nothing re-scans `player` rows once
                // `Session` finally does resolve the account id.
                state.pending.players.insert(row.character_id, row.clone());
                recompute_roster(&state.pending, &state.local, &mut state.roster);

                if row.online {
                    state.pending.offline_players.remove(&row.character_id);
                    if let Some(entity_row) = state.pending.entities.get(&row.entity_id).cloned() {
                        apply_entity(
                            &mut commands,
                            &mut state.map,
                            &entity_row,
                            &state.local,
                            &state.pending,
                        );
                        replay_entity(&mut commands, &state.map, &mut state.pending, row.entity_id);
                    }
                    replay_character(
                        &mut commands,
                        &state.map,
                        &state.pending,
                        row.character_id,
                        state.local.character_id,
                    );
                } else {
                    state.pending.offline_players.insert(row.character_id);
                    if let Some(entity) = state.map.by_entity_id.remove(&row.entity_id) {
                        commands.entity(entity).despawn();
                    }
                }
            }
            RowEvent::Wallet(row) => {
                if state.local.character_id == Some(row.character_id) {
                    state.gold.amount = row.gold;
                }
            }
            RowEvent::Npc(row) => {
                if let Some(entity) = state.map.get(row.entity_id) {
                    commands.entity(entity).insert(NpcKind {
                        kind_id: row.kind_id.clone(),
                    });
                    if let Some(market_id) = row.market_id.clone() {
                        commands.entity(entity).insert(NpcMarket { market_id });
                    }
                }
                state.pending.npcs.insert(row.entity_id, row);
            }
            RowEvent::ResourceNode(row) => {
                if let Some(entity) = state.map.get(row.entity_id) {
                    commands.entity(entity).insert(harvestable_from(&row));
                }
                state.pending.resource_nodes.insert(row.entity_id, row);
            }
            RowEvent::GatherSession(row) => {
                if let Some(entity) = state.map.get(row.entity_id) {
                    commands.entity(entity).insert(active_gather_from(&row));
                }
                state.pending.gather_sessions.insert(row.entity_id, row);
            }
            RowEvent::GatherSessionRemoved(entity_id) => {
                if let Some(entity) = state.map.get(entity_id) {
                    commands.entity(entity).remove::<ActiveGather>();
                }
                state.pending.gather_sessions.remove(&entity_id);
            }
            RowEvent::CraftSession(row) => {
                if let Some(entity) = state.map.get(row.entity_id) {
                    commands.entity(entity).insert(active_craft_from(&row));
                }
                state.pending.craft_sessions.insert(row.entity_id, row);
            }
            RowEvent::CraftSessionRemoved(entity_id) => {
                if let Some(entity) = state.map.get(entity_id) {
                    commands.entity(entity).remove::<ActiveCraft>();
                }
                state.pending.craft_sessions.remove(&entity_id);
            }
            RowEvent::GatherYield(row) => {
                debug!(
                    "gathered {} {} extra={} depleted={}",
                    row.amount, row.item_id, row.extra, row.node_depleted
                );
                emit_gather_yield_cue(&row, &state.map, &state.pending, &mut feed);
            }
            RowEvent::SellOrder(row) => {
                state.markets.orders.insert(row.id, row);
            }
            RowEvent::SellOrderRemoved(id) => {
                state.markets.orders.remove(&id);
            }
            RowEvent::BuyOrder(row) => {
                state.bids.orders.insert(row.id, row);
            }
            RowEvent::BuyOrderRemoved(id) => {
                state.bids.orders.remove(&id);
            }
            RowEvent::Inventory(row) => {
                let character_id = row.character_id;
                state.pending.inventory.insert(character_id, row);
                replay_character(
                    &mut commands,
                    &state.map,
                    &state.pending,
                    character_id,
                    state.local.character_id,
                );
            }
            RowEvent::Equipment(row) => {
                let character_id = row.character_id;
                state.pending.equipment.insert(character_id, row);
                replay_character(
                    &mut commands,
                    &state.map,
                    &state.pending,
                    character_id,
                    state.local.character_id,
                );
            }
            RowEvent::Hotbar(row) => {
                let character_id = row.character_id;
                state.pending.hotbar.insert(character_id, row);
                replay_character(
                    &mut commands,
                    &state.map,
                    &state.pending,
                    character_id,
                    state.local.character_id,
                );
            }
            RowEvent::KnownAncientLanguage(row) => {
                let character_id = row.character_id;
                state
                    .pending
                    .known_ancient_language
                    .insert(character_id, row);
                if state.local.character_id == Some(character_id) {
                    replay_character(
                        &mut commands,
                        &state.map,
                        &state.pending,
                        character_id,
                        state.local.character_id,
                    );
                }
            }
            RowEvent::Session(row) => {
                // `session` is public (see `tables::Session`'s doc comment),
                // so this connection receives every connected player's row —
                // filter to ours by `identity`, the one column every client
                // can compare against its own connection.
                if Some(row.identity) != local_identity {
                    continue;
                }
                state.local.account_id = Some(row.account_id);
                if state.local.character_id != row.character_id {
                    state.gold.amount = 0;
                }
                state.local.character_id = row.character_id;
                // Catches every `player` row that already arrived before
                // this `Session` row told us which account they should be
                // filtered against — see the comment on `RowEvent::Player`.
                recompute_roster(&state.pending, &state.local, &mut state.roster);

                // The `game_entity`/`player` rows for this character may well
                // have already arrived before this `Session` row confirmed
                // which character it belongs to — apply the marker now rather
                // than waiting for the next unrelated update to those rows.
                if let Some(character_id) = row.character_id {
                    let entity_id = state.map.entity_of_character.get(&character_id).copied();
                    conn.subscribe_owned_rows(character_id, entity_id);
                    state.local.subscribed_character = Some(character_id);
                    if let Some(entity) = entity_id.and_then(|id| state.map.get(id)) {
                        commands.entity(entity).insert(LocalPlayer);
                    }
                }
            }
            RowEvent::CastState(row) => {
                cast_progress.write(cast_progress_from(&row));
                if let Some(entity) = state.map.get(row.entity_id) {
                    commands
                        .entity(entity)
                        .insert(ActiveCastLock(movement_lock_from_cast(row.kind)));
                }
            }
            RowEvent::CastEnded(row) => {
                cast_ended.write(SpellCastEnded {
                    caster_network_id: row.entity_id,
                    spell_id: row.spell_id,
                    completed: !row.interrupted,
                });
                if let Some(entity) = state.map.get(row.entity_id) {
                    commands.entity(entity).remove::<ActiveCastLock>();
                }
            }
            RowEvent::SpellVisualEffect(row) => {
                visual_effects.write(SpellVisualEffect {
                    spell_id: row.spell_id,
                    start: to_vec3(&row.start),
                    end: to_vec3(&row.end),
                });
            }
            RowEvent::BossState(row) => {
                let entity_id = row.entity_id;
                state.pending.boss_state.insert(entity_id, row);
                replay_entity(&mut commands, &state.map, &mut state.pending, entity_id);
            }
            RowEvent::ActiveStatus(row) => {
                let entity_id = row.entity_id;
                state.pending.active_status.insert(row.id, row);
                replay_entity(&mut commands, &state.map, &mut state.pending, entity_id);
            }
            RowEvent::ActiveStatusRemoved(row) => {
                state.pending.active_status.remove(&row.id);
                replay_entity(&mut commands, &state.map, &mut state.pending, row.entity_id);
            }
            RowEvent::CrowdControl(row) => {
                let entity_id = row.entity_id;
                state.pending.crowd_control.insert(row.id, row);
                replay_entity(&mut commands, &state.map, &mut state.pending, entity_id);
            }
            RowEvent::CrowdControlRemoved(row) => {
                state.pending.crowd_control.remove(&row.id);
                replay_entity(&mut commands, &state.map, &mut state.pending, row.entity_id);
            }
            RowEvent::StatModifier(row) => {
                let entity_id = row.entity_id;
                state.pending.stat_modifier.insert(row.id, row);
                replay_entity(&mut commands, &state.map, &mut state.pending, entity_id);
            }
            RowEvent::StatModifierRemoved(row) => {
                state.pending.stat_modifier.remove(&row.id);
                replay_entity(&mut commands, &state.map, &mut state.pending, row.entity_id);
            }
            RowEvent::PeriodicEffect(row) => {
                let entity_id = row.entity_id;
                state.pending.periodic_effect.insert(row.id, row);
                replay_entity(&mut commands, &state.map, &mut state.pending, entity_id);
            }
            RowEvent::PeriodicEffectRemoved(row) => {
                state.pending.periodic_effect.remove(&row.id);
                replay_entity(&mut commands, &state.map, &mut state.pending, row.entity_id);
            }
            RowEvent::Cooldown(row) => {
                cooldowns.write(SpellCooldownState {
                    entity_id: row.entity_id,
                    ability_id: row.ability_id,
                    remaining_seconds: (row.duration_seconds - row.elapsed_seconds).max(0.0),
                    duration_seconds: row.duration_seconds,
                });
            }
            RowEvent::CooldownRemoved(row) => {
                // The row is gone, so the ability is ready: a zero remainder is
                // how the HUD is told to clear the overlay.
                cooldowns.write(SpellCooldownState {
                    entity_id: row.entity_id,
                    ability_id: row.ability_id,
                    remaining_seconds: 0.0,
                    duration_seconds: row.duration_seconds,
                });
            }
            RowEvent::Projectile(row) => {
                apply_projectile(&mut commands, &mut state.map, &row);
            }
            RowEvent::ProjectileRemoved(id) => {
                if let Some(entity) = state.map.projectiles.remove(&id) {
                    commands.entity(entity).despawn();
                }
            }
            RowEvent::AoeRegion(row) => {
                apply_aoe_region(&mut commands, &mut state.map, &row);
            }
            RowEvent::AoeRegionRemoved(id) => {
                if let Some(entity) = state.map.aoes.remove(&id) {
                    commands.entity(entity).despawn();
                }
            }
            RowEvent::PlayerMessage(row) => {
                // `target` of `None` is a broadcast. A targeted message only
                // reaches this client if the server addressed it here, but the
                // table is public, so the check is made rather than assumed.
                // Player chat lives only in the chat panel; routing it
                // through the notice toast too made every message render
                // twice in the same screen corner (chat.rs + notices/systems.rs).
                if row.target.is_none() || row.target == local_identity {
                    feed.chat_lines.write(ChatLine { text: row.text });
                }
            }
            RowEvent::ReducerRejected(message) => {
                feed.notices.write(ServerNotice::error(message));
            }
            RowEvent::JoinRejected(message) => {
                failure.0 = Some(message);
                next_screen.set(Screen::MainMenu);
            }
            RowEvent::AuthAccepted => {
                auth.state.0 = AuthStatus::Authenticated;
                auth.failure.0 = None;
            }
            RowEvent::AuthRejected(message) => {
                auth.state.0 = AuthStatus::Rejected;
                auth.failure.0 = Some(message);
            }
        }
    }
}

fn replay_entity(
    commands: &mut Commands,
    map: &StdbEntityMap,
    pending: &mut PendingRows,
    entity_id: u64,
) {
    let Some(entity) = map.get(entity_id) else {
        return;
    };

    if let Some(row) = pending.stats.get(&entity_id) {
        apply_stats(commands, entity, row);
    }
    if let Some(row) = pending.boss_state.get(&entity_id) {
        apply_boss_state(commands, entity, row);
    }
    if let Some(row) = pending.resource_nodes.get(&entity_id) {
        commands.entity(entity).insert(harvestable_from(row));
    }
    if let Some(row) = pending.gather_sessions.get(&entity_id) {
        commands.entity(entity).insert(active_gather_from(row));
    }
    if let Some(row) = pending.craft_sessions.get(&entity_id) {
        commands.entity(entity).insert(active_craft_from(row));
    }
    apply_effects(commands, entity, entity_id, pending);
}

fn harvestable_from(row: &ResourceNode) -> Harvestable {
    Harvestable {
        placement_id: row.placement_id.clone(),
        kind_id: row.kind_id.clone(),
        current_pieces: row.current_pieces,
    }
}

fn active_gather_from(row: &GatherSession) -> ActiveGather {
    ActiveGather {
        node_entity_id: row.node_entity_id,
        elapsed_seconds: row.elapsed_seconds,
        required_seconds: row.required_seconds,
    }
}

fn active_craft_from(row: &CraftSession) -> ActiveCraft {
    ActiveCraft {
        npc_entity_id: row.npc_entity_id,
        item_id: ItemId::new(row.item_id.clone()),
        quantity: row.quantity,
        elapsed_seconds: row.elapsed_seconds,
        required_seconds: row.required_seconds,
    }
}

/// Rewrites `CrowdControlState` and `ActiveStatModifiers` — but only when the
/// rows behind them actually changed.
///
/// Both components are derived from a set of rows rather than from a single
/// one, so there is nothing to update in place: they are rebuilt whole. The
/// guard is what keeps that from being a per-tick write on every entity in the
/// world, since this runs from `replay_entity` and `replay_entity` runs on
/// every `game_entity` update.
fn apply_effects(
    commands: &mut Commands,
    entity: Entity,
    entity_id: u64,
    pending: &mut PendingRows,
) {
    let active_status = active_statuses_for(entity_id, pending);
    let status_signature = status_signature_for(entity_id, pending);
    let crowd_control = crowd_control_state_for(entity_id, pending);
    let modifier_signature = modifier_signature_for(entity_id, pending);
    let next = AppliedEffects {
        active_status,
        crowd_control,
        status_signature,
        modifier_signature,
    };

    if pending.applied.get(&entity_id) == Some(&next) {
        return;
    }

    commands.entity(entity).insert((
        next.active_status.clone(),
        next.crowd_control.clone(),
        stat_modifiers_for(entity_id, pending),
    ));
    pending.applied.insert(entity_id, next);
}

fn replay_character(
    commands: &mut Commands,
    map: &StdbEntityMap,
    pending: &PendingRows,
    character_id: Uuid,
    local_character_id: Option<Uuid>,
) {
    let Some(entity) = entity_for(map, character_id) else {
        return;
    };

    if let Some(row) = pending.inventory.get(&character_id) {
        commands
            .entity(entity)
            .insert_if_neq(inventory_from(&row.slots));
    }
    if let Some(row) = pending.equipment.get(&character_id) {
        commands
            .entity(entity)
            .insert_if_neq(equipment_from(&row.slots));
    }
    if local_character_id == Some(character_id) {
        if let Some(row) = pending.known_ancient_language.get(&character_id) {
            commands
                .entity(entity)
                .insert(known_ancient_language_from(row));
        }
    }
}

fn entity_for(map: &StdbEntityMap, character_id: Uuid) -> Option<Entity> {
    map.entity_of_character
        .get(&character_id)
        .and_then(|id| map.get(*id))
}

fn apply_stats(commands: &mut Commands, entity: Entity, row: &EntityStats) {
    commands.entity(entity).insert((
        vital_from_entity_stats(row),
        CombatStats {
            armor: row.stats.armor,
            attack_power: row.stats.attack_power,
            threat_generation: row.stats.threat_generation,
        },
        MovementStats {
            speed: row.stats.movement_speed,
        },
        GatheringStats {
            speed: row.stats.gathering_speed,
            bonus: row.stats.gathering_bonus,
        },
    ));
}

fn vital_from_entity_stats(row: &EntityStats) -> VitalStats {
    let stats = &row.stats;
    VitalStats {
        current_health: stats.current_health,
        max_health: stats.max_health,
        current_mana: row.current_mana,
        max_mana: stats.max_mana,
        mana_regeneration: stats.mana_regeneration,
    }
}

fn known_ancient_language_from(row: &KnownAncientLanguageTable) -> KnownAncientLanguage {
    KnownAncientLanguage {
        root_words: row
            .root_words
            .iter()
            .cloned()
            .map(bevymmo_gameplay::abilities::RootWordId::new)
            .collect(),
        ancient_words: row
            .ancient_words
            .iter()
            .cloned()
            .map(AncientWordId::new)
            .collect(),
        base_abilities: row
            .base_abilities
            .iter()
            .cloned()
            .map(bevymmo_gameplay::abilities::AbilityId::new)
            .collect(),
    }
}

fn movement_lock_from_cast(kind: CastKindRow) -> MovementLock {
    match kind {
        CastKindRow::Instant => MovementLock::None,
        CastKindRow::CastTime => MovementLock::CastTime,
        CastKindRow::Channeling => MovementLock::Channel,
    }
}

fn cast_progress_from(row: &CastState) -> SpellCastProgress {
    SpellCastProgress {
        caster_network_id: row.entity_id,
        spell_id: row.spell_id.clone(),
        kind: match row.kind {
            CastKindRow::Instant | CastKindRow::CastTime => 0,
            CastKindRow::Channeling => 1,
        },
        elapsed_seconds: row.elapsed_seconds,
        required_seconds: row.required_seconds,
    }
}

fn apply_boss_state(commands: &mut Commands, entity: Entity, row: &BossState) {
    commands.entity(entity).insert((
        BossArena {
            center: to_vec3(&row.arena_center),
            radius: row.arena_radius,
            is_engaged: row.is_engaged,
        },
        boss_phase(row.phase),
    ));
}

fn boss_phase(phase: BossPhaseRow) -> BossPhase {
    match phase {
        BossPhaseRow::Idle => BossPhase::Dormant,
        BossPhaseRow::PhaseOne => BossPhase::Ground,
        BossPhaseRow::PhaseTwo => BossPhase::Aerial,
        BossPhaseRow::Enraged => BossPhase::Berserk,
    }
}

fn active_statuses_for(entity_id: u64, pending: &PendingRows) -> ActiveStatuses {
    let mut statuses: Vec<_> = pending
        .active_status
        .values()
        .filter(|row| row.entity_id == entity_id)
        .map(|row| ActiveStatusSnapshot {
            instance_id: row.id,
            status_id: row.status_id.clone(),
            source: row.source.map(EntityId::new),
            stacks: row.stacks,
            potency: row.potency,
            remaining_seconds: row.remaining_seconds,
            total_seconds: row.total_seconds,
        })
        .collect();
    statuses.sort_by_key(|status| status.instance_id);
    ActiveStatuses { statuses }
}

fn status_signature_for(entity_id: u64, pending: &PendingRows) -> Vec<(u64, u32)> {
    status_identity_signature(
        pending
            .active_status
            .values()
            .filter(|row| row.entity_id == entity_id)
            .map(|row| (row.id, row.stacks)),
    )
}

/// Identity of an entity's status set: instance id + stacks, not remaining time.
pub(crate) fn status_identity_signature(
    rows: impl IntoIterator<Item = (u64, u16)>,
) -> Vec<(u64, u32)> {
    let mut signature: Vec<_> = rows
        .into_iter()
        .map(|(id, stacks)| (id, u32::from(stacks)))
        .collect();
    signature.sort_unstable();
    signature
}

/// Collects one entity's crowd control into the component the UI queries.
/// `Root`, `Silence` and `Slow` are dropped rather than approximated: the
/// domain's `CrowdControlKind` knows only `Stun`, and inventing a mapping here
/// would put a bar on screen that no gating rule agrees with. Nothing emits
/// them today, so the branch is a guard against a future module change landing
/// silently, not a live gap.
fn crowd_control_state_for(entity_id: u64, pending: &PendingRows) -> CrowdControlState {
    let effects = pending
        .crowd_control
        .values()
        .filter(|row| row.entity_id == entity_id)
        .filter_map(|row| {
            let kind = match row.kind {
                CrowdControlKindRow::Stun => CrowdControlKind::Stun,
                CrowdControlKindRow::Root => CrowdControlKind::Root,
                CrowdControlKindRow::Silence => CrowdControlKind::Silence,
                CrowdControlKindRow::Slow => {
                    debug!(
                        "omitting Slow CrowdControl row: entity={entity_id} (modeled as a stat modifier)"
                    );
                    return None;
                }
            };
            Some(ActiveCrowdControl {
                kind,
                remaining_seconds: row.remaining_seconds,
                total_seconds: row.total_seconds,
            })
        })
        .collect();
    CrowdControlState { effects }
}

/// The rows behind one entity's `ActiveStatModifiers`, as a comparable key.
///
/// Durations are compared by their bit pattern because `f32` has no `Eq`, and
/// an exact-equality test is the right one here: the value either came through
/// unchanged from the last row event or it did not.
fn modifier_signature_for(entity_id: u64, pending: &PendingRows) -> Vec<(u64, u32)> {
    let mut signature: Vec<(u64, u32)> = pending
        .stat_modifier
        .values()
        .filter(|row| row.entity_id == entity_id)
        .map(|row| (row.id, 0))
        .chain(
            pending
                .periodic_effect
                .values()
                .filter(|row| row.entity_id == entity_id)
                // Periodic ids share the key space with modifier ids here, so
                // they are offset to keep the two apart.
                .map(|row| (row.id ^ (1 << 63), 0)),
        )
        .collect();
    signature.sort_unstable();
    signature
}

/// Rebuilds one entity's buff and debuff list from the two tables that feed it.
///
/// `stat_modifier` and `periodic_effect` are separate rows on the server — a
/// modifier changes what a stat *is*, a periodic effect changes health on a
/// schedule — but the domain component the UI reads holds both, as variants of
/// `ModifierEffectInstance`. One row becomes one instance with one effect;
/// nothing here needs to merge them, because the server already refreshes
/// rather than stacks.
fn stat_modifiers_for(entity_id: u64, pending: &PendingRows) -> ActiveStatModifiers {
    let stat_effects = pending
        .stat_modifier
        .values()
        .filter(|row| row.entity_id == entity_id)
        .map(|row| StatModifierInstance {
            id: StatModifierId(row.id),
            source: row.source.map(EntityId::new),
            effects: vec![ModifierEffectInstance::Stat {
                field: stat_field_from(&row.field),
                operation: if row.is_multiplicative {
                    ModifierOp::Multiply
                } else {
                    ModifierOp::Add
                },
                value: row.amount,
            }],
            remaining_seconds: row.remaining_seconds,
            kind: match row.kind {
                ModifierKindRow::Buff => ModifierKind::Buff,
                ModifierKindRow::Debuff => ModifierKind::Debuff,
            },
        });

    let periodic_effects = pending
        .periodic_effect
        .values()
        .filter(|row| row.entity_id == entity_id)
        .map(|row| StatModifierInstance {
            // Offset to keep the two id spaces from colliding, matching
            // `modifier_signature_for`.
            id: StatModifierId(row.id ^ (1 << 63)),
            source: row.source.map(EntityId::new),
            // The table stores one signed number; the domain wants the sign as
            // the variant and the magnitude as the value.
            effects: vec![if row.amount_per_tick >= 0.0 {
                ModifierEffectInstance::HealOverTime {
                    amount_per_tick: row.amount_per_tick,
                    tick_interval: row.tick_interval_seconds,
                    time_since_last_tick: row.since_last_tick,
                }
            } else {
                ModifierEffectInstance::DamageOverTime {
                    amount_per_tick: -row.amount_per_tick,
                    tick_interval: row.tick_interval_seconds,
                    time_since_last_tick: row.since_last_tick,
                }
            }],
            remaining_seconds: Some(row.remaining_seconds),
            kind: if row.amount_per_tick >= 0.0 {
                ModifierKind::Buff
            } else {
                ModifierKind::Debuff
            },
        });

    ActiveStatModifiers {
        modifiers: stat_effects.chain(periodic_effects).collect(),
    }
}

/// Parses the module's `StatField` debug name back into the enum.
///
/// The module stores the name rather than an ordinal so `spacetime sql` stays
/// readable, which makes this the inverse of its `stat_field_name`. An
/// unrecognised name means the two have drifted; `Speed` is the least harmful
/// landing spot and the warning says what happened.
fn stat_field_from(name: &str) -> StatField {
    match name {
        "Speed" => StatField::Speed,
        "Armor" => StatField::Armor,
        "AttackPower" => StatField::AttackPower,
        "MaxHealth" => StatField::MaxHealth,
        "MaxMana" => StatField::MaxMana,
        "ManaRegeneration" => StatField::ManaRegeneration,
        "GatheringSpeed" => StatField::GatheringSpeed,
        "GatheringBonus" => StatField::GatheringBonus,
        other => {
            warn!("unknown stat field {other:?} from the module; treating it as Speed");
            StatField::Speed
        }
    }
}

/// Spawns or updates the Bevy entity mirroring one `projectile` row.
///
/// Projectiles are not `game_entity` rows, so nothing else mirrors them — which
/// is why every spell that fires one had a visual effect at the muzzle and
/// nothing in between. `ProjectileVisual` is what tells the renderer to draw the
/// small emissive cube rather than a character model.
fn apply_projectile(commands: &mut Commands, map: &mut StdbEntityMap, row: &Projectile) {
    let position = to_vec3(&row.position);
    let flight = ProjectileFlight {
        speed: row.speed,
        target_entity: row.target_entity,
        target_position: row.target_position.as_ref().map(to_vec3),
    };

    match map.projectiles.get(&row.id).copied() {
        Some(entity) => {
            commands.entity(entity).insert((Position(position), flight));
        }
        None => {
            let entity = commands
                .spawn((
                    Position(position),
                    flight,
                    ProjectileVisual {
                        spell_id: row.spell_id.clone(),
                    },
                    EntityColor(Color::srgb(0.2, 0.7, 1.0)),
                ))
                .id();
            map.projectiles.insert(row.id, entity);
        }
    }
}

fn apply_aoe_region(commands: &mut Commands, map: &mut StdbEntityMap, row: &AoeRegion) {
    let zone = AoeZone {
        radius: row.radius,
        remaining_seconds: row.remaining_seconds,
        pending_delay_seconds: row.pending_delay_seconds,
        spell_id: row.spell_id.clone(),
        cone_angle_deg: match row.shape {
            super::module_bindings::AoeShapeRow::Cone => Some(row.angle_deg),
            super::module_bindings::AoeShapeRow::Circle => None,
        },
        direction: to_vec3(&row.direction),
        caster: row.caster,
    };
    let position = Position(to_vec3(&row.center));
    match map.aoes.get(&row.id).copied() {
        Some(entity) => {
            commands.entity(entity).insert((position, zone));
        }
        None => {
            let entity = commands.spawn((position, zone)).id();
            map.aoes.insert(row.id, entity);
        }
    }
}

/// Spawns or updates the Bevy entity mirroring one `game_entity` row.
fn apply_entity(
    commands: &mut Commands,
    map: &mut StdbEntityMap,
    row: &EntityRow,
    local: &LocalCharacter,
    pending: &PendingRows,
) {
    let authoritative = StdbAuthoritative {
        position: to_vec3(&row.position),
        move_target: row.move_target.as_ref().map(to_vec3),
        speed: row.speed,
    };

    let existing = map.by_entity_id.get(&row.entity_id).copied();
    let entity = match existing {
        Some(entity) => entity,
        None => {
            let entity = commands
                .spawn((
                    GameEntity,
                    NetworkEntityId(row.entity_id),
                    // Seeded from the authoritative position so a character does
                    // not visibly glide in from the origin on its first frame.
                    Position(authoritative.position),
                    PlayerName(row.display_name.clone()),
                ))
                .id();
            map.by_entity_id.insert(row.entity_id, entity);
            if let Some(character_id) = row.owner_character_id {
                // Recorded here as well as from the `player` table, because the
                // two rows can arrive in either order.
                map.entity_of_character.insert(character_id, row.entity_id);
            }
            debug!(
                "mirrored {:?} {} as entity {} at {}",
                row.kind, row.display_name, row.entity_id, authoritative.position
            );
            entity
        }
    };

    let mut cmd = commands.entity(entity);
    cmd.insert((
        authoritative,
        entity_color(&row.color),
        LookDirection(to_vec3(&row.look)),
        entity_kind(row.kind),
        entity_state(row.state),
    ));
    if matches!(row.kind, EntityKindRow::Boss) {
        cmd.insert(Boss);
    }
    if matches!(row.kind, EntityKindRow::Player) && local.is(row.owner_character_id) {
        cmd.insert(LocalPlayer);
    }
    if let Some(npc) = pending.npcs.get(&row.entity_id) {
        cmd.insert(NpcKind {
            kind_id: npc.kind_id.clone(),
        });
        if let Some(market_id) = npc.market_id.clone() {
            cmd.insert(NpcMarket { market_id });
        }
    }
    if let Some(node) = pending.resource_nodes.get(&row.entity_id) {
        cmd.insert(harvestable_from(node));
    }
}

fn entity_color(color: &ColorRow) -> EntityColor {
    EntityColor(Color::srgba(
        color.red,
        color.green,
        color.blue,
        color.alpha,
    ))
}

/// Maps the module's entity kinds onto the presentation's.
///
/// The module distinguishes `Enemy`/`Boss`/`Dummy` because the simulation
/// treats them differently; the client only cares whether something is hostile,
/// which is why the two enums are not the same shape.
fn entity_kind(kind: EntityKindRow) -> EntityKind {
    match kind {
        EntityKindRow::Player => EntityKind::Player,
        EntityKindRow::Npc => EntityKind::Friendly,
        EntityKindRow::AllyDummy => EntityKind::Ally,
        EntityKindRow::Dummy => EntityKind::Neutral,
        EntityKindRow::Enemy | EntityKindRow::Boss => EntityKind::Hostile,
        EntityKindRow::ResourceNode => EntityKind::Resource,
    }
}

fn entity_state(state: EntityStateRow) -> EntityState {
    match state {
        EntityStateRow::Idle => EntityState::Idle,
        EntityStateRow::Moving => EntityState::Moving,
        EntityStateRow::Dead => EntityState::Dead,
    }
}

fn inventory_from(slots: &[Option<ItemInstanceRow>]) -> Inventory {
    let mut inventory = Inventory::default();
    for (slot, row) in inventory.slots.iter_mut().zip(slots) {
        *slot = row.as_ref().map(item_instance_from);
    }
    inventory
}

fn equipment_from(slots: &[Option<ItemInstanceRow>]) -> Equipment {
    use bevymmo_gameplay::items::EquipSlot;
    // Same order the module writes them in; see `rows::EQUIP_SLOTS`.
    const ORDER: [EquipSlot; 10] = [
        EquipSlot::Bag,
        EquipSlot::Helmet,
        EquipSlot::Cape,
        EquipSlot::Weapon,
        EquipSlot::Armor,
        EquipSlot::Offhand,
        EquipSlot::Potion,
        EquipSlot::Shoes,
        EquipSlot::Food,
        EquipSlot::Mount,
    ];
    let mut equipment = Equipment::default();
    for (slot, row) in ORDER.iter().zip(slots) {
        *equipment.get_mut(*slot) = row.as_ref().map(item_instance_from);
    }
    equipment
}

fn item_instance_from(row: &ItemInstanceRow) -> bevymmo_gameplay::items::instance::ItemInstance {
    use bevymmo_gameplay::abilities::inscription::{
        ArmorInscription, SecondaryWord, SlotInscription, WeaponInscription,
    };
    use bevymmo_gameplay::abilities::root_word::RootWordId;
    use bevymmo_gameplay::abilities::weapon_abilities::AbilitySelection;
    use bevymmo_gameplay::abilities::{AbilityId, AncientWordId};
    use bevymmo_gameplay::items::instance::{ItemInstance, ItemInstanceId};
    use bevymmo_gameplay::items::registry::ItemId;

    let secondary_word = |s: &super::module_bindings::SecondaryWordRow| SecondaryWord {
        word_id: AncientWordId::new(s.word_id.clone()),
        intensity: s.intensity,
    };

    let slot_inscription = |s: &super::module_bindings::SlotInscriptionRow| SlotInscription {
        secondary_words: s.secondary_words.iter().map(secondary_word).collect(),
    };

    ItemInstance {
        instance_id: ItemInstanceId(row.instance_id),
        item_id: ItemId::new(row.item_id.clone()),
        quantity: row.quantity.max(1),
        ability_selection: AbilitySelection {
            primary: row.ability_selection.primary.clone().map(AbilityId::new),
            secondary: row.ability_selection.secondary.clone().map(AbilityId::new),
            ultimate: row.ability_selection.ultimate.clone().map(AbilityId::new),
        },
        root_inscription: row.root_inscription.as_ref().map(|w| WeaponInscription {
            root_word: w.root_word.clone().map(RootWordId::new),
            primary: slot_inscription(&w.primary),
            secondary: slot_inscription(&w.secondary),
            ultimate: slot_inscription(&w.ultimate),
        }),
        armor_inscription: row.armor_inscription.as_ref().map(|a| ArmorInscription {
            root_word: a.root_word.clone().map(RootWordId::new),
            secondary_words: a.secondary_words.iter().map(secondary_word).collect(),
        }),
    }
}

/// Turns a login-form submission into a `register`/`login` call.
///
/// `AuthState` moves to `Authenticating` the moment the request is sent, and
/// stays there until [`drain_events`] sees the reducer's own answer
/// (`AuthAccepted`/`AuthRejected`) — unlike `join`, this does not move
/// optimistically to the success state, so a slow round trip shows as
/// genuinely pending rather than as a lie the UI has to walk back.
fn auth_on_request(
    conn: Res<StdbConnection>,
    mut request: ResMut<AuthRequest>,
    mut auth_state: ResMut<AuthState>,
    mut auth_failure: ResMut<AuthFailure>,
) {
    let Some(intent) = request.0.take() else {
        return;
    };

    auth_state.0 = AuthStatus::Authenticating;
    auth_failure.0 = None;

    let result = match intent {
        AuthIntent::Register { email, password } => {
            conn.reducers()
                .register_then(email, password, conn.report_auth_outcome())
        }
        AuthIntent::Login { email, password } => {
            conn.reducers()
                .login_then(email, password, conn.report_auth_outcome())
        }
    };
    if let Err(err) = result {
        auth_state.0 = AuthStatus::Rejected;
        auth_failure.0 = Some(err.to_string());
    }
}

/// Turns a character-select "delete" press into a `delete_character` call.
///
/// Fire-and-forget beyond the rejection path: on success the row deletions
/// (and the `game_entity` deletion in particular) reach the client as
/// ordinary `EntityRemoved`/roster-cleanup events through [`drain_events`],
/// the same as any other character disappearing — there is no separate
/// "delete succeeded" signal to wait for.
fn delete_character_on_request(
    conn: Res<StdbConnection>,
    mut request: ResMut<DeleteCharacterRequest>,
) {
    let Some(character_id) = request.0.take() else {
        return;
    };
    if let Err(err) = conn.reducers().delete_character_then(
        character_id,
        conn.report_rejection("Impossibile eliminare il personaggio"),
    ) {
        error!("delete_character failed to send: {err}");
    }
}

/// Turns the main menu's "connect as <name>" into a `join` call, and handles
/// leaving a character / logging out of an account.
///
/// Reuses `ConnectionRequest`, the same resource the lightyear path consumed, so
/// the menu does not need to know which transport is mounted.
///
/// Unlike the version this replaced, neither `LeaveCharacter` nor
/// `LogoutAccount` disconnects: `leave` and `logout` are ordinary reducer
/// calls on the connection that is already open, the same as `join`. There is
/// nothing to wait for a grace period on — see the removed `finish_logout`
/// commit for the disconnect/reconnect dance this used to require back when
/// an `Identity` had no account behind it to keep authenticating as.
/// If Play is pressed without a live socket, try `connect` once more instead
/// of leaving the player on Connecting forever.
fn retry_connect_on_play(
    mut commands: Commands,
    mut request: ResMut<ConnectionRequest>,
    mut next_screen: ResMut<NextState<Screen>>,
    mut failure: ResMut<ConnectionFailure>,
    conn: Option<Res<StdbConnection>>,
    config: Res<StdbConnectionConfig>,
) {
    if conn.is_some() {
        return;
    }
    if !matches!(request.0, Some(ConnectionIntent::Connect { .. })) {
        return;
    }
    match connect(&config.uri, &config.module) {
        Ok((connection, party_events)) => {
            commands.insert_resource(connection);
            commands.insert_resource(party_events);
        }
        Err(err) => {
            request.0 = None;
            failure.0 = Some(format!("Impossibile connettersi: {err}"));
            next_screen.set(Screen::MainMenu);
        }
    }
}

fn join_on_request(
    conn: Res<StdbConnection>,
    mut request: ResMut<ConnectionRequest>,
    mut next_screen: ResMut<NextState<Screen>>,
    mut failure: ResMut<ConnectionFailure>,
    mut commands: Commands,
    mut state: ReplicationState,
    mut auth: AuthResources,
) {
    let Some(intent) = request.0.take() else {
        return;
    };

    match intent {
        ConnectionIntent::Connect { player_name } => {
            match conn
                .reducers()
                .join_then(player_name.clone(), conn.report_join_rejection())
            {
                Ok(()) => {
                    info!("joining SpacetimeDB as {player_name}");
                    // Optimistic: the reducer is authoritative and may still reject the
                    // name, in which case `player` never gains a row and the character
                    // never appears.
                    next_screen.set(Screen::InGame);
                }
                Err(err) => {
                    error!("join failed: {err}");
                    failure.0 = Some(format!("Impossibile connettersi: {err}"));
                }
            }
        }
        ConnectionIntent::LeaveCharacter => {
            // Fire-and-forget, same as `Shutdown`'s: `leave` is a no-op if no
            // character was active, and there is nothing meaningful to
            // report back — the character-select screen just repopulates
            // from the `player` rows this already-open connection keeps
            // receiving.
            if let Err(err) = conn.reducers().leave() {
                error!("leave failed to send: {err}");
            }
            next_screen.set(Screen::MainMenu);
        }
        ConnectionIntent::LogoutAccount => {
            if let Err(err) = conn.reducers().logout() {
                error!("logout failed to send: {err}");
            }
            // Optimistic: unlike `login`/`register` (see `auth_on_request`),
            // `logout` only ever clears the caller's own `Session` and has
            // no rejection path worth waiting on.
            clear_replicated_state(
                &mut commands,
                &mut state.map,
                &mut state.pending,
                &mut state.local,
                &mut state.roster,
                &mut state.gold,
                &mut state.markets,
                &mut state.bids,
            );
            auth.state.0 = AuthStatus::LoggedOut;
            auth.failure.0 = None;
            next_screen.set(Screen::MainMenu);
        }
        ConnectionIntent::Disconnect => {}
        // Handled by `begin_shutdown`, which runs in `PreUpdate` and takes the
        // request before this system ever sees it.
        ConnectionIntent::Shutdown => {}
    }
}

/// Drops all rows and Bevy entities mirrored from the previous connection.
///
/// A new identity receives its own subscription snapshot. Retaining this state
/// would leave the old character marked as [`LocalPlayer`], making both the
/// camera and input target multiple entities after logging out.
fn clear_replicated_state(
    commands: &mut Commands,
    map: &mut StdbEntityMap,
    pending: &mut PendingRows,
    local: &mut LocalCharacter,
    roster: &mut CharacterRoster,
    gold: &mut LocalGold,
    markets: &mut MarketOrderBook,
    bids: &mut MarketBuyBook,
) {
    for entity in map
        .by_entity_id
        .drain()
        .map(|(_, entity)| entity)
        .chain(map.projectiles.drain().map(|(_, entity)| entity))
    {
        commands.entity(entity).despawn();
    }
    map.entity_of_character.clear();
    *pending = PendingRows::default();
    *local = LocalCharacter::default();
    roster.characters.clear();
    gold.amount = 0;
    markets.orders.clear();
    bids.orders.clear();
}

/// Tells the server the client is still here.
fn send_heartbeat(conn: Res<StdbConnection>, time: Res<Time>, mut elapsed: Local<f32>) {
    *elapsed += time.delta_secs();
    if *elapsed < HEARTBEAT_INTERVAL {
        return;
    }
    *elapsed = 0.0;
    // Fails harmlessly before `join`: there is no character to mark present yet.
    let _ = conn.reducers().heartbeat();
}

/// `Some` once a graceful shutdown has been requested; the value is the
/// elapsed grace period, ticked by [`finish_shutdown`].
#[derive(Resource, Default)]
struct ShuttingDown(Option<f32>);

/// Starts a graceful shutdown: queues a disconnect and lets [`finish_shutdown`]
/// exit once it has actually gone out.
///
/// Two things ask for this, and both used to skip the disconnect entirely.
/// The window's close button fires [`WindowCloseRequested`], which by default
/// tears the process down with no chance to say goodbye — `bins/game`
/// disables that default exit precisely so this system can run first. The
/// main menu's "Exit" button used to call `AppExit` directly for the same
/// reason. Runs regardless of whether [`StdbConnection`] exists yet, so a
/// close during the initial connection attempt still exits promptly.
fn begin_shutdown(
    mut closed: MessageReader<WindowCloseRequested>,
    mut request: ResMut<ConnectionRequest>,
    mut shutting_down: ResMut<ShuttingDown>,
    conn: Option<Res<StdbConnection>>,
    config: Res<StdbConnectionConfig>,
) {
    let window_close_requested = closed.read().count() > 0;
    let exit_requested = matches!(request.0, Some(ConnectionIntent::Shutdown));
    if !window_close_requested && !exit_requested {
        return;
    }
    if exit_requested {
        request.0 = None;
    }
    if shutting_down.0.is_some() {
        // Already draining (e.g. the window close button was mashed).
        return;
    }
    if let Some(conn) = conn {
        // No explicit `leave` here: `client_disconnected` on the module
        // already marks the active character offline and clears the
        // session the instant this socket actually closes — see
        // `reducers::lifecycle::client_disconnected`. An earlier version of
        // this function called `leave` first, back when `leave` deleted the
        // character; that made closing the game window permanently destroy
        // whatever character was being played.
        if let Err(err) = conn.conn.disconnect() {
            warn!("could not disconnect during shutdown: {err}");
        }
    }
    // Forget the cached identity: the next launch should start fresh rather
    // than silently resuming whatever character this machine last used.
    forget_cached_credentials(&config.uri, &config.module);
    shutting_down.0 = Some(0.0);
}

/// Lets the disconnect queued by [`begin_shutdown`] actually reach the socket
/// — via the normal `frame_tick` pump — before the process exits.
fn finish_shutdown(
    mut shutting_down: ResMut<ShuttingDown>,
    conn: Option<Res<StdbConnection>>,
    time: Res<Time>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(elapsed) = shutting_down.0.as_mut() else {
        return;
    };
    *elapsed += time.delta_secs();
    let drained = conn.map(|c| !c.conn.is_active()).unwrap_or(true);
    if drained || *elapsed >= SHUTDOWN_GRACE_SECONDS {
        exit.write(AppExit::Success);
    }
}

/// Held right mouse button sets the destination, as it always has.
///
/// Reads [`MoveTarget`] instead of re-resolving the click itself:
/// `crate::player_movement::select_move_target` already casts the same
/// camera ray to the same ground every frame the button is held, to drive
/// the click-feedback rings. This used to be two independent copies of that
/// raycast (this one and `select_move_target`'s) computing the same point,
/// with `MoveTarget` written but never read by anything — now there is one
/// raycast and one source of truth. `.after(select_move_target)` keeps this
/// system reading the value `select_move_target` wrote earlier in the same
/// frame, not one frame stale.
fn send_move_commands(
    conn: Res<StdbConnection>,
    time: Res<Time>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    move_target: Res<MoveTarget>,
    local_player: Query<(Option<&ActiveCastLock>, Option<&CrowdControlState>), With<LocalPlayer>>,
    mut cooldown: Local<f32>,
) {
    let Some(mouse) = mouse else {
        return;
    };
    if !mouse.pressed(MouseButton::Right) {
        *cooldown = 0.0;
        return;
    }
    if let Ok((lock, cc)) = local_player.single() {
        let lock = lock.map(|lock| lock.0).unwrap_or(MovementLock::None);
        let cc_blocks = cc.is_some_and(|state| state.blocks_movement());
        if !movement_intent_allowed(lock, cc_blocks) {
            return;
        }
    }

    let just_pressed = mouse.just_pressed(MouseButton::Right);
    *cooldown -= time.delta_secs();
    // The first press goes out immediately; while held, the destination is
    // resent at a fixed rate so the character follows the pointer without one
    // reducer call per frame.
    if !just_pressed && *cooldown > 0.0 {
        return;
    }
    *cooldown = MOVE_COMMAND_INTERVAL;

    let Some(point) = move_target.0 else {
        return;
    };

    let _ = super::commands::stop_gather(&conn);
    if let Err(err) = conn.reducers().move_to(point.x, point.y, point.z) {
        error!("move_to failed: {err}");
    }
}

/// Advances every entity towards its destination, then eases the result back
/// towards what the server last said.
///
/// Runs for remote entities as much as the local one: at ~18 Hz, interpolating
/// other characters is what makes them walk instead of teleport between updates.
///
/// The step uses the *same* [`step_on_terrain`] the module's tick uses, over
/// the same manifest, so the predicted position obeys the same walls, ledges
/// and step-height rules the authoritative simulation does. Reconciliation is
/// a correction for timing skew, not a substitute for collision: while the
/// server holds a character against a parapet it keeps the move target set
/// (so the character can slide along the wall over the following ticks), and a
/// collisionless local step would therefore push into that wall every frame
/// forever, settling at `speed / RECONCILE_RATE` metres of penetration and
/// walking the character visibly off the map's raised ground.
///
/// [`step_on_terrain`]: crate::movement::step_on_terrain
fn predict_and_reconcile(
    time: Res<Time>,
    surfaces: Res<ClientSurfaceQuery>,
    collision: Res<ClientCollision>,
    pending_move: Res<MoveTarget>,
    freeze: Res<LocalMovementFreeze>,
    aim: Option<Res<AbilityAim>>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    mut query: Query<(
        &mut Position,
        &mut LookDirection,
        &StdbAuthoritative,
        Option<&LocalPlayer>,
        Option<&ActiveCastLock>,
        Option<&CrowdControlState>,
    )>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    // Both halves are published together by the presentation layer, and the
    // map is not loaded at the menu — until it is, fall back to the plain
    // straight-line step so characters still animate instead of freezing.
    let terrain = match (surfaces.0.as_ref(), collision.grid.as_ref()) {
        (Some(surfaces), Some(grid)) if !surfaces.is_empty() => Some((surfaces, grid)),
        _ => None,
    };

    let right_mouse_held = mouse
        .as_ref()
        .is_some_and(|buttons| buttons.pressed(MouseButton::Right));
    let now = time.elapsed_secs();
    let local_frozen = freeze.is_active(now);
    let aiming = aim.is_some_and(|aim| aim.is_active());

    for (mut position, mut look, authoritative, local, lock, cc) in &mut query {
        let dest = if local.is_some() {
            predicted_move_dest(
                pending_move.0,
                authoritative.move_target,
                lock.map(|lock| lock.0).unwrap_or(MovementLock::None),
                right_mouse_held,
                cc.is_some_and(|state| state.blocks_movement()),
                local_frozen,
            )
        } else {
            authoritative.move_target
        };

        match reconcile_offset(
            position.0,
            authoritative.position,
            dest,
            authoritative.speed,
        ) {
            Reconcile::Leave => {}
            Reconcile::Snap => position.0 = authoritative.position,
            Reconcile::Ease => {
                let error = authoritative.position - position.0;
                position.0 += error * (1.0 - (-RECONCILE_RATE * dt).exp());
            }
        }

        if let Some(target) = dest {
            position.0 = match terrain {
                Some((surfaces, grid)) => step_predicted_on_terrain(
                    position.0,
                    target,
                    authoritative.speed * dt,
                    surfaces,
                    grid,
                    collision.max_step_height,
                    collision.collision_radius,
                ),
                None => match movement::step_towards(position.0, target, authoritative.speed, dt) {
                    Step::Moving(p) | Step::Arrived(p) => p,
                },
            };
            if !(local.is_some() && aiming) {
                if let Some(direction) = movement::look_direction(position.0, target) {
                    look.0 = direction;
                }
            }
        }
    }
}

/// One terrain-aware prediction step, mirroring `sim::movement::step`.
///
/// `Blocked` deliberately leaves the position untouched rather than sliding
/// the character on: the module already tried both slide axes for this step,
/// and its answer arrives through `authoritative.position` a tick later.
fn step_predicted_on_terrain(
    position: Vec3,
    target: Vec3,
    max_travel: f32,
    surfaces: &SurfaceQuery,
    grid: &CollisionGrid,
    max_step_height: f32,
    collision_radius: f32,
) -> Vec3 {
    let mut position = position;
    snap_to_ground(&mut position, surfaces, max_step_height);

    match step_on_terrain(
        position,
        target.x,
        target.z,
        max_travel,
        surfaces,
        grid,
        max_step_height,
        collision_radius,
    ) {
        TerrainStep::Moved(next) | TerrainStep::Arrived(next) => next,
        TerrainStep::Blocked | TerrainStep::NoSurface => position,
    }
}

fn predict_projectiles(
    time: Res<Time>,
    mut projectiles: Query<(&mut Position, &ProjectileFlight), With<ProjectileVisual>>,
    targets: Query<(&NetworkEntityId, &Position), Without<ProjectileVisual>>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    for (mut position, flight) in &mut projectiles {
        let destination = match flight.target_entity {
            Some(id) => targets
                .iter()
                .find(|(network_id, _)| network_id.0 == id)
                .map(|(_, pos)| pos.0)
                .or(flight.target_position),
            None => flight.target_position,
        };
        let Some(destination) = destination else {
            continue;
        };
        let offset = destination - position.0;
        let distance = offset.length();
        if distance <= f32::EPSILON {
            continue;
        }
        let step = (flight.speed * dt).min(distance);
        position.0 += offset / distance * step;
    }
}

fn to_vec3(v: &Vec3Row) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

fn item_display_or_id(items: Option<&ItemRegistry>, item_id: &str) -> String {
    items
        .and_then(|items| items.get(&ItemId::new(item_id.to_string())))
        .map(|item| item.display_name().to_string())
        .unwrap_or_else(|| item_id.to_string())
}

fn gather_yield_label(amount: u32, extra: u32, item_label: &str) -> (String, Color) {
    if extra > 0 {
        (format!("+{amount} {item_label} Bonus!"), GATHER_BONUS_GOLD)
    } else {
        (format!("+{amount} {item_label}"), GATHER_AMBER)
    }
}

fn gather_yield_world_position(
    is_local_gatherer: bool,
    local_position: Option<Vec3>,
    node_position: Option<Vec3>,
) -> Option<Vec3> {
    if is_local_gatherer {
        local_position
            .map(|position| position + Vec3::Y * 2.0)
            .or(node_position)
    } else {
        node_position
    }
}

fn emit_gather_yield_cue(
    row: &GatherYieldEvent,
    map: &StdbEntityMap,
    pending: &PendingRows,
    feed: &mut PlayerFacingMessages,
) {
    let local = feed.local_player.iter().next();
    let is_local_gatherer = local.is_some_and(|(entity, _)| map.get(row.entity_id) == Some(entity));
    let local_position = local.map(|(_, position)| position.0);
    let node_position = map
        .get(row.node_entity_id)
        .and_then(|entity| feed.positions.get(entity).ok().map(|position| position.0))
        .or_else(|| {
            pending
                .entities
                .get(&row.node_entity_id)
                .map(|entity| to_vec3(&entity.position))
        });
    let Some(world_position) =
        gather_yield_world_position(is_local_gatherer, local_position, node_position)
    else {
        return;
    };

    let label = item_display_or_id(feed.items.as_deref(), &row.item_id);
    let (text, color) = gather_yield_label(row.amount, row.extra, &label);
    feed.world_text
        .write(WorldTextCue::new(world_position, text).with_color(color));
}

// ---------------------------------------------------------------------------
// Party roster
// ---------------------------------------------------------------------------
//
// `/party list` and bare `/party` (`crates/presentation/src/ui/chat.rs`)
// render entirely from already-subscribed `party`/`party_member` rows — no
// reducer call, per `plans/party-system.md`. This section is deliberately
// self-contained: its own channel, its own resource, its own drain system,
// registered independently of `register_callbacks`/`drain_events` above, so
// adding party support cannot perturb any already-shipped replication path.
// It reuses `session`'s *existing* subscription (no new query needed) via a
// second, independent listener, to learn the caller's own `character_id` —
// the one thing `party_member` alone cannot answer.

use super::module_bindings::party_member_table::PartyMemberTableAccess;
use super::module_bindings::party_table::PartyTableAccess;
use super::module_bindings::{PartyMemberRow, PartyRow};

enum PartyRowEvent {
    Member(PartyMemberRow),
    MemberRemoved(PartyMemberRow),
    PartyChanged(PartyRow),
    PartyRemoved(PartyRow),
    /// A `session` row, forwarded regardless of whose it is — `local_identity`
    /// is only known inside [`drain_party_events`], the same reason
    /// `RowEvent::Session` is handled that way above.
    SessionSeen(Session),
    /// A `player` row, forwarded so `/party list` can show display names
    /// instead of bare character ids, without depending on the private
    /// `PendingRows` cache above.
    PlayerSeen(Player),
}

#[derive(Resource)]
struct PartyEvents(Receiver<PartyRowEvent>);

/// One character in the caller's own party.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartyMemberView {
    pub character_id: Uuid,
    pub display_name: String,
    pub is_leader: bool,
}

/// The caller's own party, built entirely from subscribed `party`/
/// `party_member`/`player` rows — never populated by calling a reducer.
#[derive(Resource, Default)]
pub struct PartyRoster {
    local_character_id: Option<Uuid>,
    parties: HashMap<u64, PartyRow>,
    members: HashMap<Uuid, PartyMemberRow>,
    /// Display names for *any* character seen in a `player` row, not just the
    /// caller's own — `/party list` needs to name every member, not only the
    /// caller.
    display_names: HashMap<Uuid, String>,
}

impl PartyRoster {
    /// The caller's own party roster, if they are currently in one. `None`
    /// means "not in a party" — `/party list`/bare `/party` render that as
    /// "You are not in a party."
    pub fn my_party(&self) -> Option<Vec<PartyMemberView>> {
        let character_id = self.local_character_id?;
        let membership = self.members.get(&character_id)?;
        let leader = self.parties.get(&membership.party_id).map(|row| row.leader);
        let mut members: Vec<PartyMemberView> = self
            .members
            .values()
            .filter(|row| row.party_id == membership.party_id)
            .map(|row| PartyMemberView {
                character_id: row.character_id,
                display_name: self
                    .display_names
                    .get(&row.character_id)
                    .cloned()
                    .unwrap_or_else(|| format!("character #{}", row.character_id)),
                is_leader: Some(row.character_id) == leader,
            })
            .collect();
        members.sort_by_key(|member| member.character_id);
        Some(members)
    }
}

/// Registers the party subsystem's own row callbacks. Kept separate from
/// [`register_callbacks`] so a mistake here cannot touch any table that
/// subsystem already mirrors.
fn register_party_callbacks(conn: &DbConnection, tx: Sender<PartyRowEvent>) {
    let member_inserted = tx.clone();
    conn.db().party_member().on_insert(move |_ctx, row| {
        let _ = member_inserted.send(PartyRowEvent::Member(row.clone()));
    });
    let member_updated = tx.clone();
    conn.db().party_member().on_update(move |_ctx, _old, new| {
        let _ = member_updated.send(PartyRowEvent::Member(new.clone()));
    });
    let member_deleted = tx.clone();
    conn.db().party_member().on_delete(move |_ctx, row| {
        let _ = member_deleted.send(PartyRowEvent::MemberRemoved(row.clone()));
    });

    let party_inserted = tx.clone();
    conn.db().party().on_insert(move |_ctx, row| {
        let _ = party_inserted.send(PartyRowEvent::PartyChanged(row.clone()));
    });
    let party_updated = tx.clone();
    conn.db().party().on_update(move |_ctx, _old, new| {
        let _ = party_updated.send(PartyRowEvent::PartyChanged(new.clone()));
    });
    let party_deleted = tx.clone();
    conn.db().party().on_delete(move |_ctx, row| {
        let _ = party_deleted.send(PartyRowEvent::PartyRemoved(row.clone()));
    });

    let session_inserted = tx.clone();
    conn.db().session().on_insert(move |_ctx, row| {
        let _ = session_inserted.send(PartyRowEvent::SessionSeen(row.clone()));
    });
    let session_updated = tx.clone();
    conn.db().session().on_update(move |_ctx, _old, new| {
        let _ = session_updated.send(PartyRowEvent::SessionSeen(new.clone()));
    });

    let player_inserted = tx.clone();
    conn.db().player().on_insert(move |_ctx, row| {
        let _ = player_inserted.send(PartyRowEvent::PlayerSeen(row.clone()));
    });
    let player_updated = tx;
    conn.db().player().on_update(move |_ctx, _old, new| {
        let _ = player_updated.send(PartyRowEvent::PlayerSeen(new.clone()));
    });
}

fn drain_party_events(
    conn: Res<StdbConnection>,
    events: Res<PartyEvents>,
    mut roster: ResMut<PartyRoster>,
) {
    let local_identity = conn.identity();
    while let Ok(event) = events.0.try_recv() {
        match event {
            PartyRowEvent::Member(row) => {
                roster.members.insert(row.character_id, row);
            }
            PartyRowEvent::MemberRemoved(row) => {
                roster.members.remove(&row.character_id);
            }
            PartyRowEvent::PartyChanged(row) => {
                roster.parties.insert(row.party_id, row);
            }
            PartyRowEvent::PartyRemoved(row) => {
                roster.parties.remove(&row.party_id);
            }
            PartyRowEvent::SessionSeen(row) => {
                if Some(row.identity) == local_identity {
                    roster.local_character_id = row.character_id;
                }
            }
            PartyRowEvent::PlayerSeen(row) => {
                roster
                    .display_names
                    .insert(row.character_id, row.display_name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdb::module_bindings::{CastSourceRow, StatsRow};

    #[test]
    fn entity_stats_copy_current_mana_into_vital_stats() {
        let row = EntityStats {
            entity_id: 1,
            stats: StatsRow {
                current_health: 80.0,
                max_health: 100.0,
                max_mana: 50.0,
                mana_regeneration: 5.0,
                armor: 10.0,
                movement_speed: 0.15,
                attack_power: 12.0,
                threat_generation: 1.0,
                gathering_speed: 0.0,
                gathering_bonus: 0.0,
            },
            current_mana: 17.0,
        };
        let vital = vital_from_entity_stats(&row);
        assert_eq!(vital.current_health, 80.0);
        assert_eq!(vital.max_health, 100.0);
        assert_eq!(vital.current_mana, 17.0);
        assert_eq!(vital.max_mana, 50.0);
        assert_eq!(vital.mana_regeneration, 5.0);
    }

    #[test]
    fn status_signature_ignores_remaining_time() {
        let ticking = status_identity_signature([(7, 1), (3, 2)]);
        let later = status_identity_signature([(3, 2), (7, 1)]);
        assert_eq!(ticking, later);
        let stacked = status_identity_signature([(7, 2), (3, 2)]);
        assert_ne!(ticking, stacked);
    }

    #[test]
    fn color_row_becomes_entity_color() {
        let row = ColorRow {
            red: 0.2,
            green: 0.4,
            blue: 0.6,
            alpha: 0.8,
        };

        assert_eq!(
            entity_color(&row),
            EntityColor(Color::srgba(0.2, 0.4, 0.6, 0.8))
        );
    }

    #[test]
    fn known_language_row_becomes_domain_component() {
        let row = KnownAncientLanguageTable {
            character_id: Uuid::NIL,
            root_words: vec!["damage".to_string()],
            ancient_words: vec!["echo".to_string()],
            base_abilities: vec!["arcane_orb".to_string()],
        };

        let language = known_ancient_language_from(&row);

        assert!(language
            .root_words
            .contains(&bevymmo_gameplay::abilities::RootWordId::new("damage")));
        assert!(language.ancient_words.contains(&AncientWordId::new("echo")));
        assert!(language
            .base_abilities
            .contains(&bevymmo_gameplay::abilities::AbilityId::new("arcane_orb")));
    }

    #[test]
    fn cast_state_becomes_legacy_cast_progress() {
        let row = CastState {
            entity_id: 42,
            spell_id: "ray_of_light".to_string(),
            kind: CastKindRow::Channeling,
            source: CastSourceRow::Spell, // Legacy spell
            elapsed_seconds: 1.5,
            required_seconds: 3.0,
            start_position: Vec3Row {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            target_position: None,
            target_entity: None,
            channel_tick_accumulator: 0.0,
            tick_interval_seconds: 0.25,
            channel_movement_interrupts: true, // Standard interrupt-on-move
        };

        assert_eq!(
            cast_progress_from(&row),
            SpellCastProgress {
                caster_network_id: 42,
                spell_id: "ray_of_light".to_string(),
                kind: 1,
                elapsed_seconds: 1.5,
                required_seconds: 3.0,
            }
        );
    }

    #[test]
    fn boss_phases_match_existing_presentation_contract() {
        assert_eq!(boss_phase(BossPhaseRow::Idle), BossPhase::Dormant);
        assert_eq!(boss_phase(BossPhaseRow::PhaseOne), BossPhase::Ground);
        assert_eq!(boss_phase(BossPhaseRow::PhaseTwo), BossPhase::Aerial);
        assert_eq!(boss_phase(BossPhaseRow::Enraged), BossPhase::Berserk);
    }

    fn crowd_control_row(
        id: u64,
        entity_id: u64,
        kind: CrowdControlKindRow,
        remaining_seconds: f32,
        total_seconds: f32,
    ) -> CrowdControl {
        CrowdControl {
            id,
            entity_id,
            source: None,
            kind,
            remaining_seconds,
            total_seconds,
        }
    }

    #[test]
    fn crowd_control_projects_stun_and_root() {
        let mut pending = PendingRows::default();
        pending.crowd_control.insert(
            1,
            crowd_control_row(1, 7, CrowdControlKindRow::Stun, 1.5, 2.0),
        );
        pending.crowd_control.insert(
            2,
            crowd_control_row(2, 7, CrowdControlKindRow::Root, 1.0, 1.0),
        );

        let state = crowd_control_state_for(7, &pending);

        assert_eq!(state.effects.len(), 2);
        assert!(state
            .effects
            .iter()
            .any(|e| e.kind == CrowdControlKind::Stun));
        assert!(state
            .effects
            .iter()
            .any(|e| e.kind == CrowdControlKind::Root));
        let stun = state
            .effects
            .iter()
            .find(|e| e.kind == CrowdControlKind::Stun)
            .expect("stun");
        assert_eq!(stun.remaining_seconds, 1.5);
        assert_eq!(stun.total_seconds, 2.0);
    }

    #[test]
    fn unchanged_effects_are_not_rewritten() {
        let mut pending = PendingRows::default();
        pending.crowd_control.insert(
            1,
            crowd_control_row(1, 7, CrowdControlKindRow::Stun, 1.5, 2.0),
        );

        let first = AppliedEffects {
            active_status: active_statuses_for(7, &pending),
            crowd_control: crowd_control_state_for(7, &pending),
            status_signature: status_signature_for(7, &pending),
            modifier_signature: modifier_signature_for(7, &pending),
        };
        pending.applied.insert(7, first);

        let unchanged = AppliedEffects {
            active_status: active_statuses_for(7, &pending),
            crowd_control: crowd_control_state_for(7, &pending),
            status_signature: status_signature_for(7, &pending),
            modifier_signature: modifier_signature_for(7, &pending),
        };
        assert_eq!(pending.applied.get(&7), Some(&unchanged));

        pending.crowd_control.insert(
            1,
            crowd_control_row(1, 7, CrowdControlKindRow::Stun, 1.0, 2.0),
        );
        let ticked = AppliedEffects {
            active_status: active_statuses_for(7, &pending),
            crowd_control: crowd_control_state_for(7, &pending),
            status_signature: status_signature_for(7, &pending),
            modifier_signature: modifier_signature_for(7, &pending),
        };
        assert_ne!(pending.applied.get(&7), Some(&ticked));
    }

    #[test]
    fn periodic_effects_become_over_time_modifiers() {
        let mut pending = PendingRows::default();
        pending.periodic_effect.insert(
            3,
            PeriodicEffect {
                id: 3,
                entity_id: 7,
                source: Some(9),
                amount_per_tick: -4.0,
                tick_interval_seconds: 0.5,
                origin_status_instance_id: None,
                since_last_tick: 0.1,
                remaining_seconds: 6.0,
            },
        );

        let modifiers = stat_modifiers_for(7, &pending);

        assert_eq!(modifiers.modifiers.len(), 1);
        let instance = &modifiers.modifiers[0];
        assert_eq!(instance.source, Some(EntityId::new(9)));
        assert_eq!(instance.kind, ModifierKind::Debuff);
        // The table stores one signed number; the domain wants the sign as the
        // variant and the magnitude as the value.
        assert_eq!(
            instance.effects,
            vec![ModifierEffectInstance::DamageOverTime {
                amount_per_tick: 4.0,
                tick_interval: 0.5,
                time_since_last_tick: 0.1,
            }]
        );
    }

    #[test]
    fn gather_yield_without_extra_is_amber_amount_and_name() {
        let (text, color) = gather_yield_label(2, 0, "Wood");
        assert_eq!(text, "+2 Wood");
        assert_eq!(color, GATHER_AMBER);
    }

    #[test]
    fn gather_yield_with_extra_is_gold_bonus() {
        let (text, color) = gather_yield_label(3, 1, "wood");
        assert_eq!(text, "+3 wood Bonus!");
        assert_eq!(color, GATHER_BONUS_GOLD);
    }

    #[test]
    fn gather_yield_anchor_prefers_local_player_then_node() {
        let local = Vec3::new(1.0, 0.0, 2.0);
        let node = Vec3::new(4.0, 1.0, 4.0);
        assert_eq!(
            gather_yield_world_position(true, Some(local), Some(node)),
            Some(local + Vec3::Y * 2.0)
        );
        assert_eq!(
            gather_yield_world_position(true, None, Some(node)),
            Some(node)
        );
        assert_eq!(
            gather_yield_world_position(false, Some(local), Some(node)),
            Some(node)
        );
        assert_eq!(gather_yield_world_position(false, Some(local), None), None);
    }

    #[test]
    fn item_label_falls_back_to_id_when_registry_misses() {
        assert_eq!(item_display_or_id(None, "copper_ore"), "copper_ore");
    }
}
