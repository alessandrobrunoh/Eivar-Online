//! The map: what exists in it, and where.
//!
//! # How the map gets in here
//!
//! A WASM module has no filesystem, so the loader the native server used
//! (`bevymmo_presentation::map_loader`) cannot run. Instead `build.rs` parses every
//! `assets/maps/*.world.json` on the host and re-emits it as postcard under
//! `OUT_DIR`; the generated table below is a list of `include_bytes!`, decoded
//! once into a `OnceLock` the first time anything asks for a map.
//!
//! The format was chosen for one file. `map_02.world.json` is 3.9 MB of which
//! ~95% is a single array of 130 321 heightfield floats in decimal text; the
//! other three maps are ~21 KB each. Measured, all four maps together:
//!
//! | encoding                           |     bytes |
//! |------------------------------------|-----------|
//! | JSON as authored (`include_str!`)  | 4 152 959 |
//! | bincode 2 `standard`, `f32` heights|   557 251 |
//! | postcard, `f32` heights            |   557 246 |
//! | postcard, `u16`-quantised heights  |   290 110 |
//!
//! 14.3x smaller than the JSON, and the module no longer runs a JSON parser at
//! `init`. Round-tripping the quantised form reproduces every non-height field
//! exactly and every height to within 0.15 mm. See `build.rs` for why postcard
//! rather than bincode — it is not the size — and why the quantisation cannot
//! be felt in play.
//!
//! # What seeding costs
//!
//! Decoding expands the quantised samples back to `f32` — 521 KB of linear
//! memory for map_02 — and `SurfaceQuery` keeps its own copy of the surfaces,
//! so roughly 1 MB resident per large map. That is the price of reusing the
//! domain's ground-resolution code instead of reimplementing bilinear sampling
//! over a packed array, and it is negligible next to a module's memory budget.

use std::collections::HashMap;
use std::sync::OnceLock;

use bevymmo_domain::content::placeables::register_all;
use bevymmo_domain::placeables::{InteractionKind, PlaceableRegistry};
use bevymmo_domain::world::{CollisionGrid, GroundContact, MapManifest, Prop, SurfaceQuery};
use spacetimedb::{reducer, ReducerContext, Table};

use crate::rows::{StatsRow, Vec3Row};
use crate::sim::gathering::far_future;
use crate::tables::{
    boss_state, enemy_ai, entity_stats, game_entity, grid_cell, npc, player, prop_override,
    resource_node, BossPhaseRow, BossState, ColorRow, EnemyAi, EntityKindRow, EntityStateRow,
    EntityStats, GameEntity, Npc, PropOverride, ResourceNode,
};

// `EMBEDDED_MAPS: &[(&str, &[u8])]`, one entry per authored map.
include!(concat!(env!("OUT_DIR"), "/maps.rs"));

/// The map the world is seeded from and players spawn on.
///
/// Mirrors `bevymmo_shared::paths::DEFAULT_MAP_ID`, which the module cannot
/// depend on (that crate is Bevy-facing and reads from disk). Every other map is
/// embedded too, so switching this constant is the whole change.
pub const DEFAULT_MAP_ID: &str = "map_02";

/// Converts the legacy per-tick movement rate in the placeable configs to the
/// per-second rate `game_entity.speed` stores.
///
/// `MovementStats::speed` was authored against the Bevy server's fixed 60 Hz
/// `FixedUpdate`, so a goblin's `0.08` means 0.08 units per tick. The module's
/// tick interval varies, so speeds are stored per second — the same conversion
/// `DEFAULT_SPEED_PER_SECOND` applies to players.
const LEGACY_TICKS_PER_SECOND: f32 = 60.0;

/// One quantised heightfield, as `build.rs` wrote it.
///
/// See the alias of the same name there for why the samples are bytes.
type EncodedHeightfield = (u32, f32, f32, Vec<u8>);

/// The contents of one embedded `.bin`: a manifest whose heightfields have been
/// emptied, plus those heights quantised.
///
/// Postcard is not self-describing — this type *is* the wire format. It must
/// stay structurally identical to `EncodedMap` in `build.rs`, or a decode
/// silently produces nonsense instead of an error.
type EncodedMap = (MapManifest, Vec<EncodedHeightfield>);

/// An authored map, decoded and indexed for the queries the simulation makes.
pub struct MapData {
    /// The manifest exactly as authored, heightfields restored.
    pub manifest: MapManifest,
    /// Ground height and slope resolution over the walkable surfaces.
    pub surfaces: SurfaceQuery,
    /// Broad-phase index over the manifest's blockers and colliding props.
    pub collision: CollisionGrid,
}

static MAPS: OnceLock<HashMap<String, MapData>> = OnceLock::new();
static PLACEABLES: OnceLock<PlaceableRegistry> = OnceLock::new();

/// Every embedded map, decoded on first use.
///
/// A map that fails to decode is logged and skipped rather than panicking: one
/// broken map should not take the whole module down at `init`, and `build.rs`
/// already refuses to emit a manifest it could not parse and validate.
fn maps() -> &'static HashMap<String, MapData> {
    MAPS.get_or_init(|| {
        let mut maps = HashMap::with_capacity(EMBEDDED_MAPS.len());
        for (map_id, bytes) in EMBEDDED_MAPS {
            match decode(bytes) {
                Ok(manifest) => {
                    maps.insert(
                        (*map_id).to_string(),
                        MapData {
                            surfaces: SurfaceQuery::from_manifest(&manifest),
                            collision: CollisionGrid::build(&manifest),
                            manifest,
                        },
                    );
                }
                Err(error) => log::error!("map {map_id} failed to decode: {error}"),
            }
        }
        maps
    })
}

/// The placeable catalogue, behind a `OnceLock` because building it allocates an
/// `Arc` per kind and every caller wants the same one.
///
/// The definitions themselves are `bevymmo_domain`'s — the same ones the editor
/// palette and the client asset resolver use. Nothing about a goblin is
/// restated here.
pub(crate) fn placeables() -> &'static PlaceableRegistry {
    PLACEABLES.get_or_init(|| {
        let mut registry = PlaceableRegistry::default();
        register_all(&mut registry);
        registry
    })
}

/// Catalog config for a spawned enemy, keyed by the placeable `kind_id`.
pub(crate) fn enemy_config_for(kind_id: &str) -> Option<bevymmo_domain::placeables::EnemyConfig> {
    let id = bevymmo_domain::placeables::KindId::new(kind_id.to_string());
    placeables()
        .enemies
        .get(&id)
        .map(|definition| definition.enemy_config())
}

/// Catalog config for a spawned boss, keyed by the placeable `kind_id`.
pub(crate) fn boss_config_for(kind_id: &str) -> Option<bevymmo_domain::placeables::BossConfig> {
    let id = bevymmo_domain::placeables::KindId::new(kind_id.to_string());
    placeables()
        .bosses
        .get(&id)
        .map(|definition| definition.boss_config())
}

/// Authored corpse timer for an enemy or boss catalog kind.
///
/// Looks in both submaps because `#[enemy(type = Boss)]` registers only under
/// bosses, while still implementing [`bevymmo_domain::placeables::EnemyPlaceable`].
pub(crate) fn respawn_seconds_for(kind_id: &str) -> Option<f32> {
    let id = bevymmo_domain::placeables::KindId::new(kind_id.to_string());
    let registry = placeables();
    registry
        .enemies
        .get(&id)
        .map(|definition| definition.enemy_config().respawn_seconds)
        .or_else(|| {
            registry
                .bosses
                .get(&id)
                .map(|definition| definition.enemy_config().respawn_seconds)
        })
}

/// Reverses `build.rs`'s encoding.
fn decode(bytes: &[u8]) -> Result<MapManifest, String> {
    let (mut manifest, heightfields): EncodedMap =
        postcard::from_bytes(bytes).map_err(|error| error.to_string())?;

    for (index, min, step, samples) in heightfields {
        let heightfield = manifest
            .surfaces
            .get_mut(index as usize)
            .and_then(|surface| surface.heightfield.as_mut())
            .ok_or_else(|| format!("heightfield names surface {index}, which has none"))?;
        heightfield.heights = samples
            .chunks_exact(2)
            .map(|pair| min + step * f32::from(u16::from_le_bytes([pair[0], pair[1]])))
            .collect();
    }

    Ok(manifest)
}

// ---------------------------------------------------------------------------
// Queries the rest of the module can ask about the world
// ---------------------------------------------------------------------------

/// One decoded map, or `None` if no such map is embedded.
pub fn map(map_id: &str) -> Option<&'static MapData> {
    maps().get(map_id)
}

/// The map the world runs on.
pub fn default_map() -> Option<&'static MapData> {
    map(DEFAULT_MAP_ID)
}

/// Ground height and surface normal under a point on the default map.
///
/// `None` when the point is over no walkable surface — off the edge of the
/// world, or in a hole. Callers deciding whether a move is legal want exactly
/// that distinction, so it is not flattened to a height of zero here.
pub fn ground_at(x: f32, z: f32) -> Option<GroundContact> {
    default_map().and_then(|map| map.surfaces.ground_at(x, z))
}

/// Ground height under a point on the default map.
pub fn ground_height(x: f32, z: f32) -> Option<f32> {
    ground_at(x, z).map(|contact| contact.height)
}

/// Whether a cylinder of `radius` at `point` overlaps a blocker on the default
/// map.
pub fn is_blocked(point: Vec3Row, radius: f32) -> bool {
    default_map().is_some_and(|map| {
        map.collision
            .is_blocked([point.x, point.y, point.z], radius)
    })
}

/// Lifts a position onto the ground when it would otherwise be underneath it.
///
/// Deliberately one-directional. Authored placements are trusted — the dragon
/// sits 28 cm above the terrain on purpose, and a flying or perched creature
/// must stay where the designer put it — but nothing should start buried, which
/// is what a placement authored at y = 0 would be on a map whose terrain rises
/// to 20 m.
pub fn lift_to_ground(position: Vec3Row) -> Vec3Row {
    match ground_height(position.x, position.z) {
        Some(height) if height > position.y => Vec3Row {
            y: height,
            ..position
        },
        _ => position,
    }
}

// ---------------------------------------------------------------------------
// Seeding
// ---------------------------------------------------------------------------

/// Populates the world. Called once from `init`, and again by
/// [`gm_reseed_world`].
///
/// Every entity comes from the manifest: there is no hardcoded mob list. The
/// dispatch mirrors the Bevy server's `spawn_placeables_on_map_load` — look the
/// prop's kind up in the registry's typed submaps, and let whichever submap
/// holds it decide what to build.
pub fn seed(ctx: &ReducerContext) {
    let Some(map) = default_map() else {
        log::error!("default map {DEFAULT_MAP_ID} is not embedded; world left empty");
        return;
    };

    // Only the props are cloned, not the whole manifest: the overrides cannot
    // touch surfaces or blockers, and map_02's heightfield is half a megabyte.
    let mut props = map.manifest.props.clone();
    apply_prop_overrides(ctx, DEFAULT_MAP_ID, &mut props);

    let registry = placeables();
    let (mut enemies, mut bosses, mut npcs, mut resources, mut unbound) =
        (0u32, 0u32, 0u32, 0u32, 0u32);

    for prop in &props {
        if let Some(definition) = registry.dummies.get(&prop.kind) {
            let kind = if definition.is_ally() {
                EntityKindRow::AllyDummy
            } else {
                EntityKindRow::Dummy
            };
            spawn_creature(
                ctx,
                prop,
                kind,
                definition.display_name(),
                StatsRow::from(&definition.dummy_stats()),
            );
        } else if let Some(definition) = registry.enemies.get(&prop.kind) {
            let config = definition.enemy_config();
            let entity = spawn_creature(
                ctx,
                prop,
                EntityKindRow::Enemy,
                definition.display_name(),
                StatsRow::from(&config.stats),
            );
            ctx.db.enemy_ai().insert(EnemyAi {
                entity_id: entity.entity_id,
                kind_id: prop.kind.as_str().to_string(),
            });
            enemies += 1;
        } else if let Some(definition) = registry.bosses.get(&prop.kind) {
            let config = definition.boss_config();
            let entity = spawn_creature(
                ctx,
                prop,
                EntityKindRow::Boss,
                definition.display_name(),
                StatsRow::from(&config.stats),
            );
            ctx.db.enemy_ai().insert(EnemyAi {
                entity_id: entity.entity_id,
                kind_id: prop.kind.as_str().to_string(),
            });
            ctx.db.boss_state().insert(BossState {
                entity_id: entity.entity_id,
                kind_id: prop.kind.as_str().to_string(),
                // `BossPhase::Dormant` in the domain: the encounter has not
                // started, and the AI ignores the boss until a player crosses
                // the arena ring.
                phase: BossPhaseRow::Idle,
                arena_center: entity.spawn_point,
                arena_radius: config.arena_radius,
                is_engaged: false,
                engaged_seconds: 0.0,
                rotation_cursor: 0,
            });
            bosses += 1;
        } else if let Some(definition) = registry.npcs.get(&prop.kind) {
            // No `entity_stats` row: NPCs are not combatants, and the Bevy
            // server likewise gave them a marker and a transform only.
            let entity = spawn_entity(
                ctx,
                prop,
                EntityKindRow::Npc,
                definition.display_name(),
                0.0,
            );
            let market_id = match definition.interaction() {
                InteractionKind::Market { market_id } => Some(market_id),
                _ => None,
            };
            ctx.db.npc().insert(Npc {
                entity_id: entity.entity_id,
                kind_id: prop.kind.as_str().to_string(),
                market_id,
            });
            npcs += 1;
        } else if let Some(definition) = registry.resources.get(&prop.kind) {
            let entity = spawn_entity(
                ctx,
                prop,
                EntityKindRow::ResourceNode,
                definition.display_name(),
                0.0,
            );
            upsert_resource_node(ctx, prop, entity.entity_id, definition.resource_config());
            resources += 1;
        } else if registry.player_spawns.contains_key(&prop.kind) {
            // Read on demand by `player_spawn_point`, not turned into an entity.
        } else if registry.props.contains_key(&prop.kind) {
            // Static scenery. It reaches the simulation through
            // `MapData::collision`, never as a row.
        } else {
            // Triggers and interactables are registered kinds with no table
            // to live in yet, as are kinds the registry does not know at all.
            unbound += 1;
        }
    }

    log::info!(
        "seeded {DEFAULT_MAP_ID}: {enemies} enemies, {bosses} bosses, {npcs} npcs, \
         {resources} resource nodes, {unbound} placements with no runtime binding"
    );
}

fn upsert_resource_node(
    ctx: &ReducerContext,
    prop: &Prop,
    entity_id: u64,
    config: bevymmo_domain::placeables::ResourceConfig,
) {
    let placement_id = prop.id.clone();
    if let Some(existing) = ctx.db.resource_node().placement_id().find(&placement_id) {
        let next_regen_at = if existing.current_pieces >= config.max_pieces {
            far_future()
        } else {
            existing.next_regen_at
        };
        ctx.db.resource_node().placement_id().update(ResourceNode {
            entity_id,
            kind_id: prop.kind.as_str().to_string(),
            next_regen_at,
            ..existing
        });
        return;
    }
    ctx.db.resource_node().insert(ResourceNode {
        placement_id,
        entity_id,
        kind_id: prop.kind.as_str().to_string(),
        current_pieces: config.max_pieces,
        last_regen_at: ctx.timestamp,
        next_regen_at: far_future(),
    });
}

/// Where a newly created character appears.
///
/// Spawn points are `player_spawn` placeables in the manifest. When a map has
/// several, characters are dealt round-robin by how many already exist, which
/// spreads a login rush without needing the RNG. When a map has none — map_02
/// does not — the centre of its bounds is used, lifted onto the terrain: the
/// origin of map_02 sits under 4.9 m of hillside, so the old `Vec3::ZERO`
/// buried every character that joined.
pub fn player_spawn_point(ctx: &ReducerContext) -> Vec3Row {
    let Some(map) = default_map() else {
        return Vec3Row::default();
    };

    let mut props = map.manifest.props.clone();
    apply_prop_overrides(ctx, DEFAULT_MAP_ID, &mut props);

    let registry = placeables();
    let spawns: Vec<Vec3Row> = props
        .iter()
        .filter(|prop| registry.player_spawns.contains_key(&prop.kind))
        .map(translation)
        .collect();

    if spawns.is_empty() {
        let bounds = map.manifest.bounds;
        let x = (bounds.min_x + bounds.max_x) * 0.5;
        let z = (bounds.min_z + bounds.max_z) * 0.5;
        log::warn!(
            "{DEFAULT_MAP_ID} has no player_spawn placeable; spawning at its centre instead"
        );
        // Not `lift_to_ground`: the ground height *is* the answer here, and a
        // centre with no walkable surface under it leaves zero as the only
        // guess left.
        return Vec3Row {
            x,
            y: ground_height(x, z).unwrap_or(0.0),
            z,
        };
    }

    lift_to_ground(spawns[ctx.db.player().count() as usize % spawns.len()])
}

/// Folds the GM overrides for `map_id` into a copy of its props.
///
/// Same semantics as the Bevy server's `placeables::persistence::apply_overrides`:
/// removals win outright, and the surviving props take whichever of position,
/// yaw and scale the override supplies. An override naming a prop the manifest
/// does not have is skipped — it is a stale row from an older revision of the
/// map, and inventing a prop from it would be worse than ignoring it.
fn apply_prop_overrides(ctx: &ReducerContext, map_id: &str, props: &mut Vec<Prop>) {
    let overrides: Vec<PropOverride> = ctx.db.prop_override().map_id().filter(map_id).collect();
    if overrides.is_empty() {
        return;
    }

    props.retain(|prop| {
        !overrides
            .iter()
            .any(|row| row.removed && row.prop_id == prop.id)
    });

    for row in overrides.iter().filter(|row| !row.removed) {
        let Some(prop) = props.iter_mut().find(|prop| prop.id == row.prop_id) else {
            log::warn!(
                "prop override for {map_id}/{} names no prop in the manifest; skipped",
                row.prop_id
            );
            continue;
        };
        if let Some(position) = row.position {
            prop.transform.translation = [position.x, position.y, position.z];
        }
        if let Some(yaw) = row.rotation_y {
            prop.transform.rotation_deg[1] = yaw;
        }
        if let Some(scale) = row.scale {
            prop.transform.scale = [scale.x, scale.y, scale.z];
        }
    }
}

/// Spawns harvestable placements missing from a live database.
///
/// `seed` runs only on an empty DB (and GM reseed). A publish onto an existing
/// world therefore never creates `resource_oak_tree` — the client does not
/// draw resource kinds from the map GLB, so the tree is simply absent.
pub fn ensure_resource_nodes(ctx: &ReducerContext) -> bool {
    let Some(map) = default_map() else {
        return false;
    };
    let registry = placeables();
    for prop in &map.manifest.props {
        let Some(definition) = registry.resources.get(&prop.kind) else {
            continue;
        };
        if let Some(existing) = ctx.db.resource_node().placement_id().find(&prop.id) {
            if ctx
                .db
                .game_entity()
                .entity_id()
                .find(&existing.entity_id)
                .is_some()
            {
                continue;
            }
        }
        let entity = spawn_entity(
            ctx,
            prop,
            EntityKindRow::ResourceNode,
            definition.display_name(),
            0.0,
        );
        upsert_resource_node(ctx, prop, entity.entity_id, definition.resource_config());
        log::info!(
            "seeded resource {} ({}) at entity {}",
            prop.id,
            prop.kind.as_str(),
            entity.entity_id
        );
    }
    true
}

/// Spawns NPC placements missing from a live database.
///
/// `seed` only runs on an empty DB. A new crafter added to the map would
/// otherwise never appear until a reset.
pub fn ensure_npcs(ctx: &ReducerContext) -> bool {
    let Some(map) = default_map() else {
        return false;
    };
    let registry = placeables();
    for prop in &map.manifest.props {
        let Some(definition) = registry.npcs.get(&prop.kind) else {
            continue;
        };
        let kind = prop.kind.as_str();
        if ctx.db.npc().iter().any(|row| row.kind_id == kind) {
            continue;
        }
        let entity = spawn_entity(
            ctx,
            prop,
            EntityKindRow::Npc,
            definition.display_name(),
            0.0,
        );
        let market_id = match definition.interaction() {
            InteractionKind::Market { market_id } => Some(market_id),
            _ => None,
        };
        ctx.db.npc().insert(Npc {
            entity_id: entity.entity_id,
            kind_id: kind.to_string(),
            market_id,
        });
        log::info!(
            "seeded npc {} ({}) at entity {}",
            prop.id,
            kind,
            entity.entity_id
        );
    }
    true
}

/// Spawns the allied training dummy if this database was seeded before that
/// placement existed. `seed` only runs on empty DBs and GM reseed, so a live
/// world would otherwise keep the old dummy-only roster after publish.
pub fn ensure_ally_dummy(ctx: &ReducerContext) -> bool {
    if ctx
        .db
        .game_entity()
        .iter()
        .any(|entity| entity.kind == EntityKindRow::AllyDummy)
    {
        return true;
    }
    let Some(map) = default_map() else {
        return false;
    };
    let registry = placeables();
    let Some(definition) = registry.dummies.values().find(|dummy| dummy.is_ally()) else {
        return false;
    };
    let wanted = definition.id();
    let Some(prop) = map
        .manifest
        .props
        .iter()
        .find(|prop| prop.kind.as_str() == wanted.as_str())
    else {
        return false;
    };
    spawn_creature(
        ctx,
        prop,
        EntityKindRow::AllyDummy,
        definition.display_name(),
        StatsRow::from(&definition.dummy_stats()),
    );
    true
}

/// Inserts a `game_entity` for a placement, and its `entity_stats`.
fn spawn_creature(
    ctx: &ReducerContext,
    prop: &Prop,
    kind: EntityKindRow,
    display_name: &str,
    stats: StatsRow,
) -> GameEntity {
    let entity = spawn_entity(
        ctx,
        prop,
        kind,
        display_name,
        stats.movement_speed * LEGACY_TICKS_PER_SECOND,
    );
    ctx.db.entity_stats().insert(EntityStats {
        entity_id: entity.entity_id,
        stats,
        current_mana: stats.max_mana,
    });
    crate::sim::combat::record_base_stats(entity.entity_id, stats);
    entity
}

/// Inserts a `game_entity` for a placement, spatial index and spawn point
/// included.
fn spawn_entity(
    ctx: &ReducerContext,
    prop: &Prop,
    kind: EntityKindRow,
    display_name: &str,
    speed: f32,
) -> GameEntity {
    let position = lift_to_ground(translation(prop));
    let (cell_x, cell_z) = grid_cell(position);
    ctx.db.game_entity().insert(GameEntity {
        entity_id: 0,
        kind,
        owner_character_id: None,
        display_name: display_name.to_string(),
        color: ColorRow::for_kind(kind),
        position,
        look: authored_facing(prop),
        move_target: None,
        speed,
        state: EntityStateRow::Idle,
        cell_x,
        cell_z,
        // Enemies return here when they lose their target, so it is the
        // authored placement rather than wherever the fight ends.
        spawn_point: position,
        // Counted down only once the entity is dead; set on death rather than
        // here, so a live entity carries no timer.
        respawn_in_seconds: None,
    })
}

fn translation(prop: &Prop) -> Vec3Row {
    let [x, y, z] = prop.transform.translation;
    Vec3Row { x, y, z }
}

/// The direction a placement was authored facing.
///
/// `TransformData::rotation_deg` is YXZ with yaw on Y, and a yaw of zero means
/// +Z — the same forward the join handler gives a new character.
fn authored_facing(prop: &Prop) -> Vec3Row {
    let yaw = prop.transform.rotation_deg[1].to_radians();
    Vec3Row {
        x: yaw.sin(),
        y: 0.0,
        z: yaw.cos(),
    }
}

// ---------------------------------------------------------------------------
// GM surface
// ---------------------------------------------------------------------------

/// Identities allowed to edit the world, baked in when the module is published.
///
/// The module cannot work out who the database owner is: `sender_auth` only
/// describes the caller, and `database_identity` is the *database's* identity,
/// which no human holds — checking against it locks the GM reducers to nobody.
/// The schema has no role table either, so until one exists the list arrives at
/// publish time:
///
/// ```sh
/// BEVYMMO_GM_IDENTITIES=c200e8d5…,c200a1b2… spacetime publish bevymmo
/// ```
///
/// Unset means nobody, deliberately: a world-editing reducer that defaults open
/// is worse than one that has to be configured.
const GM_IDENTITIES: Option<&str> = option_env!("BEVYMMO_GM_IDENTITIES");

/// Rejects everyone who is not a configured game master.
pub(crate) fn require_gm(ctx: &ReducerContext) -> Result<(), String> {
    let sender = ctx.sender().to_hex().to_string();
    let allowed = GM_IDENTITIES.unwrap_or("").split(',').any(|entry| {
        // `0x`-prefixed is how `spacetime sql` prints identities, bare is how
        // `spacetime login show` does; accept whichever was pasted.
        let entry = entry.trim().trim_start_matches("0x");
        !entry.is_empty() && entry.eq_ignore_ascii_case(&sender)
    });
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "identity {sender} is not a game master; publish with \
             BEVYMMO_GM_IDENTITIES to grant it"
        ))
    }
}

/// Records a GM edit to one authored prop: moved, turned, resized or removed.
///
/// `prop_override` had no writer at all under Postgres — the read path existed
/// and nothing ever produced a row — so this is the first way to actually
/// produce one. Each field is `Option`: `None` means "leave what the manifest
/// says", which is why passing all `None` with `removed = false` is a no-op row
/// rather than a reset.
///
/// The edit lands in the world at the next seed; call [`gm_reseed_world`] to
/// apply it without republishing.
#[reducer]
pub fn gm_set_prop_override(
    ctx: &ReducerContext,
    map_id: String,
    prop_id: String,
    position: Option<Vec3Row>,
    rotation_y: Option<f32>,
    scale: Option<Vec3Row>,
    removed: bool,
) -> Result<(), String> {
    require_gm(ctx)?;

    let map = map(&map_id).ok_or_else(|| format!("no map {map_id:?} is embedded"))?;
    if !map.manifest.props.iter().any(|prop| prop.id == prop_id) {
        return Err(format!("map {map_id:?} has no prop {prop_id:?}"));
    }

    let existing = ctx
        .db
        .prop_override()
        .map_id()
        .filter(&map_id)
        .find(|row| row.prop_id == prop_id);

    match existing {
        // One row per prop: overrides are a statement of the prop's current
        // state, not a log of edits, so a second edit replaces the first.
        Some(row) => {
            ctx.db.prop_override().id().update(PropOverride {
                position,
                rotation_y,
                scale,
                removed,
                ..row
            });
        }
        None => {
            ctx.db.prop_override().insert(PropOverride {
                id: 0,
                map_id,
                prop_id,
                position,
                rotation_y,
                scale,
                removed,
            });
        }
    }
    Ok(())
}

/// Drops a GM edit, restoring the prop to whatever the manifest says.
#[reducer]
pub fn gm_clear_prop_override(
    ctx: &ReducerContext,
    map_id: String,
    prop_id: String,
) -> Result<(), String> {
    require_gm(ctx)?;

    let ids: Vec<u64> = ctx
        .db
        .prop_override()
        .map_id()
        .filter(&map_id)
        .filter(|row| row.prop_id == prop_id)
        .map(|row| row.id)
        .collect();
    if ids.is_empty() {
        return Err(format!("no override on {map_id:?}/{prop_id:?}"));
    }
    for id in ids {
        ctx.db.prop_override().id().delete(&id);
    }
    Ok(())
}

/// Rebuilds the seeded half of the world from the manifest plus the current
/// overrides.
///
/// Without this an override only takes effect on the next republish, since
/// `seed` otherwise runs only from `init`. Player characters are untouched:
/// only entities with no owner are cleared, the same rule
/// `clear_runtime_state` uses.
#[reducer]
pub fn gm_reseed_world(ctx: &ReducerContext) -> Result<(), String> {
    require_gm(ctx)?;

    // Collected first: the tables must not be iterated while being deleted from.
    let seeded: Vec<u64> = ctx
        .db
        .game_entity()
        .iter()
        .filter(|entity| entity.owner_character_id.is_none())
        .map(|entity| entity.entity_id)
        .collect();
    for entity_id in seeded {
        ctx.db.boss_state().entity_id().delete(&entity_id);
        ctx.db.enemy_ai().entity_id().delete(&entity_id);
        ctx.db.entity_stats().entity_id().delete(&entity_id);
        ctx.db.game_entity().entity_id().delete(&entity_id);
    }

    seed(ctx);
    Ok(())
}
