//! Every table in the module.
//!
//! Kept in one file on purpose: the schema is the contract the reducers and the
//! client bindings are both written against, and splitting it across files makes
//! it harder to see at a glance what the world is made of.
//!
//! # Persistent versus runtime
//!
//! In SpacetimeDB *everything* persists — there is no distinction the database
//! enforces. The distinction below is one the module has to keep for itself:
//!
//! - **Persistent**: `account`, `api_key`, `player`, `player_stats`, `hotbar`,
//!   `inventory`, `equipment`, `known_glyphs`, `character_wallet`,
//!   `account_economy`, `market`, `market_sell_order`, `market_buy_order`,
//!   `prop_override`, `resource_node`. These outlive a session and must
//!   survive a republish.
//! - **Runtime**: `game_entity`, `entity_stats`, `cast_state`, `cooldown`,
//!   `projectile`, `aoe_region`, `crowd_control`, `threat`, `stat_modifier`,
//!   `gather_session`, `craft_session`. Conceptually these die with the session, so `init`
//!   clears and re-seeds them — otherwise a republish inherits yesterday's
//!   projectiles mid-flight.
//!
//! # Spatial queries
//!
//! There is no ECS to query, so "every entity near this point" is an index
//! scan. `game_entity` carries a `cell_x`/`cell_z` grid index for exactly that:
//! a linear scan per mob per tick does not survive contact with a populated map.

use spacetimedb::{table, Identity, SpacetimeType, Timestamp, Uuid};

use crate::rows::{EffectPayloadRow, HotbarRow, ItemInstanceRow, StatsRow, Vec3Row};

/// Side of a spatial grid cell, in world units.
///
/// Sized so that the widest AoE and the boss aggro radius each touch only a
/// handful of cells: too small and every query walks many cells, too large and
/// each cell degenerates into the linear scan the grid exists to avoid.
pub const GRID_CELL_SIZE: f32 = 16.0;

/// Which grid cell a world position falls in.
pub fn grid_cell(position: Vec3Row) -> (i32, i32) {
    (
        (position.x / GRID_CELL_SIZE).floor() as i32,
        (position.z / GRID_CELL_SIZE).floor() as i32,
    )
}

// ---------------------------------------------------------------------------
// Persistent: accounts
// ---------------------------------------------------------------------------

/// What an account is allowed to do. Everyone starts as `Player`; promotion to
/// `Admin` is itself a protected, audited reducer call (Slice 3), never a
/// value a client can set on itself.
#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleRow {
    Player,
    Admin,
}

/// A permanent login: one email, one password hash, up to
/// [`crate::MAX_CHARACTERS_PER_ACCOUNT`] characters (see `Player::account_id`).
///
/// Deliberately holds no gameplay state of its own — that stays on `Player`
/// rows, keyed by `account_id` — so an account is exactly the credential plus
/// the role, and nothing about it needs to change when a character does.
///
/// Deliberately **not** `public`: SpacetimeDB 2.8.1's row-level security
/// filters are unimplemented (`client_visibility_filter` is marked
/// unenforced), so `public` here would let any connected client subscribe to
/// `SELECT * FROM account` and read every email and password hash in the
/// database. Reducers can still read and write this table freely — visibility
/// only gates client-side subscriptions — so nothing about `reducers::account`
/// needs to change; the client simply never sees this table directly.
#[table(accessor = account)]
pub struct Account {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    /// Lowercased, trimmed uniqueness key. See `reducers::account::normalize_email`.
    #[unique]
    pub normalized_email: String,
    /// As the player typed it, for their own profile display.
    pub email: String,
    /// Argon2id PHC string (algorithm, salt and hash together) — never the
    /// plaintext, and never a fast unsalted digest. See `reducers::account`.
    pub password_hash: String,
    pub role: RoleRow,
    pub created_at: Timestamp,
}

/// Binds one live connection to the account that authenticated it, and to
/// whichever character (if any) that connection is currently playing.
///
/// Rows here are ephemeral — deleted on `client_disconnected` and on
/// `logout` — unlike `Account`/`Player`, which outlive every session. The
/// SpacetimeDB `Identity` is a per-connection credential, not a login: this is
/// the only path a reducer has from `ctx.sender()` to an account, and from
/// there to a character.
///
/// Unlike [`Account`], this table **is** `public` — it holds no credential,
/// only bookkeeping (`account_id`, `character_id`) that is already derivable
/// from the public `player` table (`Player::account_id`) by anyone who knows
/// one of the account's characters. Making `Session` public is what lets the
/// owning client learn its *own* `account_id` — the one thing it cannot
/// derive from `player` before it has a character yet — and from there build
/// its character roster, by filtering `player` rows client-side to the row
/// whose `identity` matches its own connection.
#[table(accessor = session, public)]
pub struct Session {
    #[primary_key]
    pub identity: Identity,
    #[index(btree)]
    pub account_id: u64,
    /// `None` until `join` selects or creates a character for this session.
    #[index(btree)]
    pub character_id: Option<Uuid>,
    pub authenticated_at: Timestamp,
}

/// A long-lived HTTP credential for one [`Account`]. Used by the gateway so a
/// bot can read the owner's profile, wallet and stats without a browser cookie.
///
/// Deliberately **not** `public`, for the same reason as [`Account`]: a public
/// table would let any connected client subscribe and read every key hash.
/// The owning client sees metadata (name, prefix, dates — never the hash)
/// through the `my_api_keys` view, which filters by the caller's session.
///
/// The plaintext secret is never stored. The gateway mints it, the reducer
/// hashes it with SHA-256, and the HTTP create response is the only time the
/// caller sees the secret.
#[table(accessor = api_key)]
pub struct ApiKey {
    #[primary_key]
    pub id: Uuid,
    #[index(btree)]
    pub account_id: u64,
    /// SHA-256 hex of the full secret (`eiv_` + 64 hex chars). Unique so
    /// `authenticate_api_key` is a point lookup, not a scan.
    #[unique]
    pub key_hash: String,
    pub name: String,
    /// First 12 characters of the secret (`eiv_` + 8 hex), shown in the UI
    /// so the owner can tell keys apart after the secret is gone.
    pub prefix: String,
    pub created_at: Timestamp,
    pub last_used_at: Option<Timestamp>,
}

/// What the `my_api_keys` view returns: everything an owner may see, and
/// nothing they must not (the hash). Named fields because `SpacetimeType`
/// panics on tuple structs.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct ApiKeyMeta {
    pub id: Uuid,
    pub name: String,
    pub prefix: String,
    pub created_at: Timestamp,
    pub last_used_at: Option<Timestamp>,
}

// ---------------------------------------------------------------------------
// Persistent: the character
// ---------------------------------------------------------------------------

/// A player character, owned by an [`Account`].
///
/// Up to [`crate::MAX_CHARACTERS_PER_ACCOUNT`] rows share an `account_id`.
/// `character_id` is stable regardless of which connection, if any, is
/// currently playing the character — see `Session` for that binding — which is
/// what makes more than one character per account possible: an `Identity` can
/// be only one `Session.character_id` at a time, but a `Player` row does not
/// stop existing just because nobody is connected as it right now.
#[table(accessor = player, public)]
pub struct Player {
    /// Random UUID (v4, from `ReducerContext::new_uuid_v4`), minted once at
    /// `join` and never reused. Not `#[auto_inc]`: sequential ids leak how
    /// many characters exist and invite enumeration; a UUID is safe to show
    /// to other clients, which is exactly what the public `player` table and
    /// the gateway's `/public/accounts/*` do.
    #[primary_key]
    pub character_id: Uuid,
    #[index(btree)]
    pub account_id: u64,
    #[unique]
    pub normalized_name: String,
    pub display_name: String,
    /// The character's entity in `game_entity`. Combat, spells and AI all work
    /// on entity ids, so the character has one like everything else.
    #[unique]
    pub entity_id: u64,
    /// Whether a connection is currently playing this character. Distinct
    /// from row existence: the character outlives the session.
    pub online: bool,
    pub last_seen: Timestamp,
}

/// Gold of one character. Not shared across an account's other characters.
///
/// Created at `join` with `gold = 0`. `Account` itself holds no Gold — that
/// table is credentials, and Crystals (later) get their own account-scoped
/// table rather than a column here.
#[table(accessor = character_wallet, public)]
pub struct CharacterWallet {
    #[primary_key]
    pub character_id: Uuid,
    pub gold: u64,
}

/// Account-wide economy knobs. Public the way `player` is: it is not a
/// credential. `fee_bps` starts at
/// [`bevymmo_domain::economy::DEFAULT_ACCOUNT_FEE_BPS`] and a future
/// subscription reducer writes `0` without touching each market's own fee.
#[table(accessor = account_economy, public)]
pub struct AccountEconomy {
    #[primary_key]
    pub account_id: u64,
    pub fee_bps: u16,
}

/// Isolated player market. Seeded in `init`; not cleared as runtime state.
#[table(accessor = market, public)]
pub struct Market {
    #[primary_key]
    pub id: String,
    pub display_name: String,
    pub fee_bps: u16,
}

/// Runtime NPC profile. Re-seeded with `game_entity` because entity ids
/// change every `init`. `market_id` is `Some` only for market NPCs.
#[table(accessor = npc, public)]
pub struct Npc {
    #[primary_key]
    pub entity_id: u64,
    pub kind_id: String,
    pub market_id: Option<String>,
}

/// Runtime enemy profile. Re-seeded with `game_entity` because entity ids
/// change every `init`. `kind_id` is the placeable catalog key (`mob_goblin`).
#[table(accessor = enemy_ai, public)]
pub struct EnemyAi {
    #[primary_key]
    pub entity_id: u64,
    pub kind_id: String,
}

/// One listed item instance, escrowed out of the seller's inventory.
#[table(
    accessor = market_sell_order,
    public,
    index(accessor = by_market_item, btree(columns = [market_id, item_id]))
)]
pub struct MarketSellOrder {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub market_id: String,
    #[index(btree)]
    pub seller_character_id: Uuid,
    pub item_id: String,
    pub item: ItemInstanceRow,
    pub price_gold: u64,
    pub created_at: Timestamp,
}

/// One bid: `price_gold` is Gold escrowed from the buyer until fill or cancel.
#[table(
    accessor = market_buy_order,
    public,
    index(accessor = by_market_item, btree(columns = [market_id, item_id]))
)]
pub struct MarketBuyOrder {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub market_id: String,
    #[index(btree)]
    pub buyer_character_id: Uuid,
    pub item_id: String,
    pub price_gold: u64,
    pub created_at: Timestamp,
}

/// Base stats, without equipment bonuses. See [`StatsRow`].
#[table(accessor = player_stats, public)]
pub struct PlayerStats {
    #[primary_key]
    pub character_id: Uuid,
    pub stats: StatsRow,
}

#[table(accessor = hotbar, public)]
pub struct Hotbar {
    #[primary_key]
    pub character_id: Uuid,
    pub slots: HotbarRow,
}

#[table(accessor = inventory, public)]
pub struct InventoryTable {
    #[primary_key]
    pub character_id: Uuid,
    pub slots: Vec<Option<ItemInstanceRow>>,
}

#[table(accessor = equipment, public)]
pub struct EquipmentTable {
    #[primary_key]
    pub character_id: Uuid,
    /// Ten slots in `rows::EQUIP_SLOTS` order.
    pub slots: Vec<Option<ItemInstanceRow>>,
}

/// New vocabulary for Root Words and universal Ancient Words. This table is
/// additive to `KnownGlyphsTable` so existing characters remain readable while
/// the migration is in progress.
#[table(accessor = known_ancient_language, public)]
pub struct KnownAncientLanguageTable {
    #[primary_key]
    pub character_id: Uuid,
    pub root_words: Vec<String>,
    pub ancient_words: Vec<String>,
    pub base_abilities: Vec<String>,
}

/// A player's resonance (XP and level) with an Ancient Word.
///
/// Keyed by auto-increment ID; the natural key `(character_id, root_word_id)`
/// is enforced unique so that a character has at most one row per word.
#[table(
    accessor = resonance,
    public,
    index(accessor = character_root_word, btree(columns = [character_id, root_word_id]))
)]
#[derive(Clone)]
pub struct Resonance {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_id: Uuid,
    pub root_word_id: String,
    pub xp: u64,
    pub level: u32,
}

// ---------------------------------------------------------------------------
// Persistent: parties
// ---------------------------------------------------------------------------
//
// Keyed on `character_id`, not `Identity`. A party is a relationship between
// *characters* — the persistent things a player builds up gear and resonance
// on — not between connections. A player can reconnect under a fresh
// `Identity` (a new browser tab, a dropped socket) and keep the same
// character; the party must follow the character through that, not evaporate
// because the old `Identity` disconnected. See `reducers::lifecycle::caller_character`
// for how a reducer resolves "which character is calling", and `Session` for
// how a character's *current* `Identity`, if any, is found when a party
// notification needs somewhere to go.

/// One party of up to [`crate::MAX_PARTY_SIZE`] characters.
///
/// `leader` is `#[unique]`: [`PartyMemberRow`] guarantees a character is
/// never in more than one party at a time, and the leader is always also a
/// member (see `reducers::parties`), so a character can never lead two
/// parties simultaneously — the uniqueness constraint documents that
/// invariant rather than merely hoping for it.
#[table(accessor = party, public)]
pub struct PartyRow {
    #[primary_key]
    #[auto_inc]
    pub party_id: u64,
    #[unique]
    pub leader: Uuid,
    pub created_at: Timestamp,
}

/// One character's membership in one party.
///
/// Keyed by `character_id` rather than an auto-inc id: that primary key *is*
/// the "one party per character at a time" guarantee, the same way
/// `Player::entity_id` being `#[unique]` guarantees one entity per character.
/// No separate uniqueness check is needed anywhere in `reducers::parties`.
#[table(
    accessor = party_member,
    public,
    index(accessor = by_party, btree(columns = [party_id]))
)]
pub struct PartyMemberRow {
    #[primary_key]
    pub character_id: Uuid,
    pub party_id: u64,
    /// Used to break ties when a leader leaves: the longest-tenured
    /// remaining member is promoted (see `reducers::parties::party_leave`).
    pub joined_at: Timestamp,
}

/// Which direction a pending [`PartyRequestRow`] runs.
///
/// Both kinds are resolved identically by `party_accept`/`party_decline`:
/// whoever is named in `recipient` is the one who must act. The kind exists
/// only so `party_accept` knows who joins the party on acceptance — the
/// invitee for an `Invite`, the original joiner (not the accepting leader)
/// for a `JoinRequest`.
#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartyRequestKind {
    /// The party's leader invited someone outside it.
    Invite,
    /// An outsider asked to join a named leader's party.
    JoinRequest,
}

/// A pending invite or join request, waiting on `recipient` to accept or
/// decline.
///
/// At most one live request may connect any two characters — enforced in
/// `reducers::parties`, not by the schema — so `party_accept`/`party_decline`
/// resolving "whichever pending request exists between the sender and the
/// named character" is never ambiguous.
#[table(
    accessor = party_request,
    public,
    index(accessor = by_recipient, btree(columns = [recipient]))
)]
pub struct PartyRequestRow {
    #[primary_key]
    #[auto_inc]
    pub request_id: u64,
    pub party_id: u64,
    pub kind: PartyRequestKind,
    pub initiator: Uuid,
    /// Whoever must `/party accept` or `/party decline` this request.
    pub recipient: Uuid,
    pub created_at: Timestamp,
}

/// GM edits to a map's props: moved, retinted or removed.
///
/// Overlaid on the manifest at seed time. The Postgres version of this table had
/// no writer at all — the upsert was dead code — so this is the first time the
/// overrides can actually be produced as well as consumed.
#[table(accessor = prop_override, public)]
pub struct PropOverride {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[index(btree)]
    pub map_id: String,
    pub prop_id: String,
    pub position: Option<Vec3Row>,
    pub rotation_y: Option<f32>,
    pub scale: Option<Vec3Row>,
    pub removed: bool,
}

// ---------------------------------------------------------------------------
// Runtime: the simulation
// ---------------------------------------------------------------------------

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKindRow {
    Player,
    Enemy,
    Boss,
    Dummy,
    Npc,
    /// Training dummy that receives heals the way a party member would.
    AllyDummy,
    /// Harvestable world node. Not a combatant.
    ResourceNode,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityStateRow {
    Idle,
    Moving,
    Dead,
}

/// RGBA tint stored independently of Bevy so it can cross the database boundary.
#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct ColorRow {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub alpha: f32,
}

impl ColorRow {
    pub const fn for_kind(kind: EntityKindRow) -> Self {
        match kind {
            EntityKindRow::Player => Self::srgb(0.2, 0.8, 0.2),
            EntityKindRow::Enemy => Self::srgb(0.8, 0.2, 0.2),
            EntityKindRow::Boss => Self::srgb(0.55, 0.05, 0.05),
            EntityKindRow::Dummy => Self::srgb(0.7, 0.1, 0.1),
            EntityKindRow::AllyDummy => Self::srgb(0.2, 0.75, 0.35),
            EntityKindRow::Npc => Self::srgb(0.5, 0.5, 0.5),
            EntityKindRow::ResourceNode => Self::srgb(0.45, 0.7, 0.3),
        }
    }

    const fn srgb(red: f32, green: f32, blue: f32) -> Self {
        Self {
            red,
            green,
            blue,
            alpha: 1.0,
        }
    }
}

/// Anything that occupies a position and can be hit: players, enemies, bosses,
/// training dummies.
///
/// One table rather than one per kind, because every query that matters —
/// "what is near this point", "what can this spell hit" — is kind-agnostic.
#[table(
    accessor = game_entity,
    public,
    index(accessor = cell, btree(columns = [cell_x, cell_z]))
)]
pub struct GameEntity {
    #[primary_key]
    #[auto_inc]
    pub entity_id: u64,
    pub kind: EntityKindRow,
    /// Set for player characters, so a reducer can map this entity back to
    /// the character that owns it (see `Player::character_id`).
    #[index(btree)]
    pub owner_character_id: Option<Uuid>,
    pub display_name: String,
    pub color: ColorRow,
    pub position: Vec3Row,
    pub look: Vec3Row,
    pub move_target: Option<Vec3Row>,
    /// Movement rate in units per **second**. The Bevy server stored units per
    /// tick at a fixed 60 Hz, which only worked because the tick never varied.
    pub speed: f32,
    pub state: EntityStateRow,
    /// Spatial index; kept in sync with `position` by whoever writes it.
    pub cell_x: i32,
    pub cell_z: i32,
    /// Where this entity respawns, and where enemies return to when they lose
    /// their target.
    pub spawn_point: Vec3Row,
    /// Seconds until this corpse gets back up, for anything that respawns on a
    /// timer. `None` for players, who respawn when they ask to, and for
    /// anything meant to stay dead.
    pub respawn_in_seconds: Option<f32>,
}

/// Effective stats for anything that can fight, players included.
///
/// Player rows here are *derived*: base stats from `player_stats` plus
/// equipment bonuses plus active modifiers. `player_stats` is what persists.
#[table(accessor = entity_stats, public)]
pub struct EntityStats {
    #[primary_key]
    pub entity_id: u64,
    pub stats: StatsRow,
    pub current_mana: f32,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastKindRow {
    Instant,
    CastTime,
    Channeling,
}

/// Which system started this cast — determines how `advance_casts` resolves
/// and fires the effect.
#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastSourceRow {
    /// Legacy spell from the hotbar (`cast_spell` reducer).
    Spell,
    /// Weapon ability (`cast_weapon` reducer).
    Weapon,
    /// Primary ability from the equipped helmet.
    Helmet,
    /// Primary ability from the equipped chestplate.
    Armor,
    /// Primary ability from the equipped boots/shoes.
    Shoes,
    /// Catalog `BaseAbility` fired by AI (enemy/boss/NPC kit). No equipment.
    Catalog,
}

/// A cast in progress. At most one per caster: starting another cancels it.
#[table(accessor = cast_state, public)]
pub struct CastState {
    #[primary_key]
    pub entity_id: u64,
    /// Spell id or ability id — namespace shared by both sources.
    pub spell_id: String,
    pub kind: CastKindRow,
    /// Which system started this cast; determines how to resolve and fire.
    pub source: CastSourceRow,
    pub elapsed_seconds: f32,
    pub required_seconds: f32,
    /// Caster position when the cast began, to detect movement interrupts.
    pub start_position: Vec3Row,
    pub target_position: Option<Vec3Row>,
    pub target_entity: Option<u64>,
    pub channel_tick_accumulator: f32,
    pub tick_interval_seconds: f32,
    /// For Channeling casts only: whether movement cancels the channel.
    /// True for legacy spells with InterruptOnMove; reflects AbilityCastMode
    /// for weapon abilities. Ignored for Instant/CastTime (movement always
    /// interrupts CastTime).
    pub channel_movement_interrupts: bool,
}

/// One spell or ability on cooldown for one entity.
#[table(
    accessor = cooldown,
    public,
    index(accessor = owner_ability, btree(columns = [entity_id, ability_id]))
)]
pub struct Cooldown {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub entity_id: u64,
    /// Spell id or ability id — they share a namespace here because a cooldown
    /// is a cooldown regardless of what started it.
    pub ability_id: String,
    pub elapsed_seconds: f32,
    pub duration_seconds: f32,
}

#[table(accessor = projectile, public)]
pub struct Projectile {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub caster: u64,
    pub spell_id: String,
    pub position: Vec3Row,
    /// Homing projectiles chase an entity; the rest fly to a fixed point.
    pub target_entity: Option<u64>,
    pub target_position: Option<Vec3Row>,
    pub speed: f32,
    pub effects: Vec<EffectPayloadRow>,
    pub hit_radius: f32,
    pub remaining_seconds: f32,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AoeShapeRow {
    Circle,
    Cone,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AoeTargetingRow {
    /// Hits all entities in range.
    Everyone,
    /// Caster only (e.g. self-heal AoE).
    CasterOnly,
    /// Everyone except caster (e.g. Meteorite: caster is not damaged).
    ExcludeCaster,
}

#[table(accessor = aoe_region, public)]
pub struct AoeRegion {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub caster: u64,
    pub spell_id: String,
    pub center: Vec3Row,
    pub direction: Vec3Row,
    pub radius: f32,
    pub shape: AoeShapeRow,
    /// Total cone aperture in degrees. Unused for circles (`0.0`).
    pub angle_deg: f32,
    pub remaining_seconds: f32,
    /// Time before the region starts applying its effect. Meteorite's warning
    /// circle exists during this window without doing anything.
    pub pending_delay_seconds: f32,
    /// Entities already affected, for effects that apply once each.
    pub affected: Vec<u64>,
    /// Targeting policy for this region.
    pub targeting: AoeTargetingRow,
    pub effects: Vec<EffectPayloadRow>,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrowdControlKindRow {
    Stun,
    Root,
    Silence,
    Slow,
}

#[table(
    accessor = crowd_control,
    public,
    index(accessor = victim, btree(columns = [entity_id]))
)]
pub struct CrowdControl {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub entity_id: u64,
    /// Who applied it. The CC bar names the source, and a future immunity rule
    /// needs to tell two casters apart.
    pub source: Option<u64>,
    pub kind: CrowdControlKindRow,
    pub remaining_seconds: f32,
    /// The duration this effect started with. Without it the client cannot draw
    /// a fill ratio: it only ever sees the countdown, so the first frame it
    /// observes would always look like a full bar.
    pub total_seconds: f32,
}

/// Semantic status instance. Specialized runtime tables remain optimized child
/// state; this row gives them one stable owner and gives clients a unified view.
#[table(
    accessor = active_status,
    public,
    index(accessor = on_entity, btree(columns = [entity_id]))
)]
pub struct ActiveStatus {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub entity_id: u64,
    pub status_id: String,
    pub source: Option<u64>,
    pub stacks: u16,
    pub potency: f32,
    pub remaining_seconds: f32,
    pub total_seconds: f32,
    /// Present while the status is represented by the legacy CC table.
    pub control_kind: Option<CrowdControlKindRow>,
}

/// Whether a modifier helps or hurts the entity carrying it.
///
/// Not inferable from the sign: `-0.3` on `Armor` is a debuff, the same number
/// on an incoming-damage field would be a buff. The caster states it, so the
/// row records it rather than guessing.
#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierKindRow {
    Buff,
    Debuff,
}

/// A timed stat modifier — a buff or a debuff.
#[table(
    accessor = stat_modifier,
    public,
    index(accessor = target, btree(columns = [entity_id]))
)]
pub struct StatModifier {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub entity_id: u64,
    /// Who applied it, so a buff can be attributed and a dispel can target one
    /// caster's work.
    pub source: Option<u64>,
    /// `StatField` as its debug name; the field set is small and stable, and a
    /// string keeps the table readable from `spacetime sql`.
    pub field: String,
    pub is_multiplicative: bool,
    pub amount: f32,
    pub kind: ModifierKindRow,
    pub origin_status_instance_id: Option<u64>,
    /// `None` means it lasts until something removes it.
    pub remaining_seconds: Option<f32>,
}

/// Damage or healing applied a little at a time: a poison, a regeneration.
///
/// Separate from `stat_modifier` rather than three more columns on it, because
/// the two have nothing in common beyond a duration. A modifier changes what a
/// stat *is* and is folded into the effective stats on every recompute; a
/// periodic effect changes health on a schedule and is not part of any stat at
/// all. Merging them meant every stat recompute walking rows it had to ignore.
#[table(
    accessor = periodic_effect,
    public,
    index(accessor = on_entity, btree(columns = [entity_id]))
)]
pub struct PeriodicEffect {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub entity_id: u64,
    /// Who applied it, so the damage earns them threat like any other.
    pub source: Option<u64>,
    /// Positive heals, negative hurts. One field rather than a flag because
    /// every consumer wants the signed number anyway.
    pub amount_per_tick: f32,
    pub tick_interval_seconds: f32,
    /// Status instance that owns this periodic schedule, when applicable.
    pub origin_status_instance_id: Option<u64>,
    /// Counts up to `tick_interval_seconds`, then fires and resets.
    pub since_last_tick: f32,
    pub remaining_seconds: f32,
}

/// How much a Table-policy combatant hates each of its attackers.
///
/// Used by bosses and by any enemy whose kit threat policy is Table
/// (Sticky also writes amount-1 rows to remember the current target).
/// Adding `threat_generation` / renaming this column requires
/// `./scripts/stdb.sh reset`.
#[table(
    accessor = threat,
    public,
    index(accessor = by_combatant, btree(columns = [combatant_entity]))
)]
pub struct Threat {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub combatant_entity: u64,
    pub target_entity: u64,
    pub amount: f32,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BossPhaseRow {
    Idle,
    PhaseOne,
    PhaseTwo,
    Enraged,
}

#[table(accessor = boss_state, public)]
pub struct BossState {
    #[primary_key]
    pub entity_id: u64,
    pub phase: BossPhaseRow,
    pub arena_center: Vec3Row,
    pub arena_radius: f32,
    pub is_engaged: bool,
    pub engaged_seconds: f32,
    /// Index into the boss's spell rotation.
    pub rotation_cursor: u32,
}

// ---------------------------------------------------------------------------
// Events: transient, broadcast, never read back
// ---------------------------------------------------------------------------
//
// SpacetimeDB 2.0's event tables replace the server-to-client messages the
// lightyear protocol carried. Rows are delivered to subscribers and not
// retained, which is exactly the lifetime a "play this effect once" message
// wants — and the reason global reducer callbacks were removed in 2.0.

/// A one-shot visual: a bolt, a burst, an impact.
#[table(accessor = spell_visual_effect, public, event)]
pub struct SpellVisualEffectEvent {
    pub spell_id: String,
    pub start: Vec3Row,
    pub end: Vec3Row,
}

/// Damage or healing applied, for floating combat text.
#[table(accessor = damage_event, public, event)]
pub struct DamageEventRow {
    pub target: u64,
    pub amount: f32,
    /// Negative damage is healing; kept explicit so the client does not have to
    /// infer it from the sign.
    pub is_healing: bool,
    pub killed: bool,
}

/// A cast finished or was interrupted.
#[table(accessor = cast_ended, public, event)]
pub struct CastEndedEvent {
    pub entity_id: u64,
    pub spell_id: String,
    pub interrupted: bool,
}

/// Persistent, server-only domain history for gateway/admin and inspect.
#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainEventKind {
    DamageDealt,
    EntityDied,
    PlayerDied,
    SpellCast,
}

#[table(
    accessor = domain_event,
    index(accessor = by_time, btree(columns = [occurred_at]))
)]
pub struct DomainEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub occurred_at: Timestamp,
    pub kind: DomainEventKind,
    pub actor_entity_id: Option<u64>,
    pub target_entity_id: Option<u64>,
    pub amount: Option<f32>,
    pub source_id: Option<String>,
    pub killer_entity_id: Option<u64>,
    pub payload: Option<String>,
}

/// Server-side switch and thresholds for raw domain event recording.
#[table(accessor = domain_event_config)]
pub struct DomainEventConfig {
    #[primary_key]
    pub id: u8,
    pub enabled: bool,
    pub damage_threshold: f32,
    pub retention_seconds: u64,
}

/// A line of text for one player, or for everyone when `target` is `None`.
#[table(accessor = player_message, public, event)]
pub struct PlayerMessageEvent {
    pub target: Option<Identity>,
    pub text: String,
}

/// One completed gather channel. Floating text / VFX on the client.
#[table(accessor = gather_yield, public, event)]
pub struct GatherYieldEvent {
    pub entity_id: u64,
    pub node_entity_id: u64,
    pub item_id: String,
    pub amount: u32,
    pub extra: u32,
    pub node_depleted: bool,
}

// ---------------------------------------------------------------------------
// Gathering
// ---------------------------------------------------------------------------

/// Persistent harvest state, keyed by the map placement id.
#[table(
    accessor = resource_node,
    public,
    index(accessor = next_regen, btree(columns = [next_regen_at]))
)]
#[derive(Clone)]
pub struct ResourceNode {
    #[primary_key]
    pub placement_id: String,
    #[unique]
    pub entity_id: u64,
    pub kind_id: String,
    pub current_pieces: u32,
    pub last_regen_at: Timestamp,
    pub next_regen_at: Timestamp,
}

/// One player gathering at a time. Runtime: cleared on init.
#[table(accessor = gather_session, public)]
pub struct GatherSession {
    #[primary_key]
    pub entity_id: u64,
    pub node_entity_id: u64,
    pub placement_id: String,
    pub elapsed_seconds: f32,
    pub required_seconds: f32,
    pub start_position: Vec3Row,
}

/// One player crafting at a time. Runtime: cleared on init.
#[table(accessor = craft_session, public)]
pub struct CraftSession {
    #[primary_key]
    pub entity_id: u64,
    pub npc_entity_id: u64,
    pub item_id: String,
    pub quantity: u32,
    pub elapsed_seconds: f32,
    pub required_seconds: f32,
    pub start_position: Vec3Row,
}

// ---------------------------------------------------------------------------
// Scheduling
// ---------------------------------------------------------------------------

/// Drives `game_tick`. One row, inserted by `init`.
#[table(accessor = tick_schedule, scheduled(crate::tick::game_tick))]
pub struct TickSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: spacetimedb::ScheduleAt,
}

/// Runs domain-event retention away from the simulation hot path.
#[table(
    accessor = domain_event_cleanup_schedule,
    scheduled(crate::sim::event_log::prune)
)]
pub struct DomainEventCleanupSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: spacetimedb::ScheduleAt,
}

/// The tick clock. One row, id 0.
///
/// `last_tick` is what gives the simulation a real `dt`. Assuming the nominal
/// interval would run the world slow: the scheduler measures the gap from the
/// *end* of the previous run, and a 50 ms nominal tick was measured at ~53-56 ms.
#[table(accessor = tick_stats, public)]
pub struct TickStats {
    #[primary_key]
    pub id: u32,
    pub ticks: u64,
    pub first_tick: Timestamp,
    pub last_tick: Timestamp,
}
