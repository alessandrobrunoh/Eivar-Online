//! Placeable definitions: the single source of truth for "what can be
//! placed in the world".
//!
//! The base trait [`PlaceableDefinition`] carries only data (id, name,
//! defaults, asset hint). Categories are expressed via **subtraits**
//! (`PropPlaceable`, `EnemyPlaceable`, ...) that extend the base trait
//! and add a config method. Implementing a subtrait IS the categorization:
//! the compiler enforces that every registered enemy has `enemy_config()`,
//! so dispatch is a typed HashMap lookup, not a runtime `match` on an enum.
//!
//! Each subtrait is object-safe: it returns concrete config DTOs (see
//! [`super::config`]), never `impl Bundle`. The spawn machinery is a free
//! function in the server crate that reads the DTO and builds the bundle —
//! mirroring the existing `spawn_entity::<T>()` pattern already used for
//! `Player` / `Enemy` / `Boss`.

use crate::world::{CollisionShape, TransformData};

use super::config::{BossConfig, EnemyConfig, InteractionKind, ResourceConfig, TriggerConfig};
use super::registry::KindId;

// -------------------------------------------------------------------------
// Asset hint + defaults
// -------------------------------------------------------------------------

/// Hint for the client binding about which visual to build for a kind.
///
/// Kept as a non-`Component` enum so the trait stays object-safe; the
/// client binding translates it into a `SceneRoot` / placeholder mesh at
/// build time.
#[derive(Debug, Clone)]
pub enum AssetHint {
    /// Render as a placeholder colored cuboid (editor + dev mode).
    Placeholder,
    /// Load a GLB scene at the given relative path (e.g. `"models/props/tree_oak.glb"`).
    Scene(&'static str),
    /// Marker-only placement: no visual mesh, just an icon in the editor.
    /// Used by invisible triggers and player spawn points.
    Invisible,
}

/// Default values written into the manifest when the user places the kind.
///
/// All fields can be overridden per-placement in the editor; these are the
/// starting point suggested by the definition (e.g. a tree defaults to a
/// trunk collision cylinder, a player spawn defaults to no collision).
#[derive(Debug, Clone)]
pub struct PlaceableDefaults {
    /// Initial transform (translation / rotation / scale).
    pub transform: TransformData,
    /// Optional tint multiplier (linear RGB 0..1). `None` leaves the
    /// material's base color untouched.
    pub tint: Option<[f32; 3]>,
    /// Optional server-side collision shape. `None` means walkable.
    pub collision: Option<CollisionShape>,
    /// Whether the prop blocks movement on the server collision grid.
    pub blocks_movement: bool,
}

impl Default for PlaceableDefaults {
    fn default() -> Self {
        Self {
            transform: TransformData::at(0.0, 0.0, 0.0),
            tint: None,
            collision: None,
            blocks_movement: false,
        }
    }
}

// -------------------------------------------------------------------------
// Base trait
// -------------------------------------------------------------------------

/// Single source of truth for a placeable kind's data.
///
/// Mirrors the existing `Spell` trait: a pure data contract, object-safe,
/// stored as `Arc<dyn PlaceableDefinition>` (or one of the category
/// subtraits) inside [`super::registry::PlaceableRegistry`].
///
/// Concrete kinds live in `crate::content::placeables` and implement
/// the base trait plus exactly one category subtrait.
pub trait PlaceableDefinition: Send + Sync + 'static {
    /// Stable identifier stored in the manifest (e.g. `"tree_oak"`).
    fn id(&self) -> KindId;

    /// Human-readable name shown in the editor palette and tooltips.
    fn display_name(&self) -> &'static str;

    /// Default transform, tint and collision applied on placement.
    fn defaults(&self) -> PlaceableDefaults {
        PlaceableDefaults::default()
    }

    /// Asset hint used by the client binding to build the visual.
    fn asset_hint(&self) -> AssetHint {
        AssetHint::Placeholder
    }

    /// Short description shown in the editor palette tooltip.
    fn description(&self) -> &'static str {
        ""
    }

    /// Emoji / glyph used by the editor palette for compact display.
    fn icon(&self) -> &'static str {
        "▢"
    }
}

// -------------------------------------------------------------------------
// Category subtraits
// -------------------------------------------------------------------------
//
// Implementing one of these IS the categorization. Adding a new `mob_orc`
// means: `impl PlaceableDefinition for OrcDefinition` + `impl EnemyPlaceable
// for OrcDefinition`. No enum variant, no match arm, no central registry
// edit beyond `register_enemy(Arc::new(OrcDefinition))`.
//
// Every subtrait extends `PlaceableDefinition`, so a registered enemy is
// also queryable as a base definition (the registry stores it under the
// typed subtrait, but the base methods are always available).

/// Static visual prop (tree, rock, house). No behavior beyond defaults.
///
/// Implementing this trait puts the kind in the editor's "Props" palette
/// group and tells the server it only contributes to the collision grid.
pub trait PropPlaceable: PlaceableDefinition {}

/// Player spawn marker. Invisible in-game; the server picks one of these
/// per connected client and instantiates the local player there.
pub trait PlayerSpawnPlaceable: PlaceableDefinition {}

/// Static training dummy: hittable, no AI, no spells.
pub trait DummyPlaceable: PlaceableDefinition {
    /// Stat profile (HP, armor). Movement and attack should be zero.
    fn dummy_stats(&self) -> crate::stats::components::StatsBundleData;

    /// Whether this dummy counts as an ally for healing (Life) and UI.
    /// The default dummy is a hostile sack of HP.
    fn is_ally(&self) -> bool {
        false
    }
}

/// Hostile or neutral AI creature (goblin, wolf, skeleton, ...).
///
/// The server binding calls `enemy_config()` to read the stats / kit /
/// aggro, then spawns the existing `Enemy` entity via
/// `spawn_entity::<Enemy>()` and overrides those components.
pub trait EnemyPlaceable: PlaceableDefinition {
    /// Per-archetype stats, catalog ability kit, acquire and leash radii.
    fn enemy_config(&self) -> EnemyConfig;
}

/// Boss entity (dragon, lich king, ...).
///
/// The server binding calls `boss_config()` to read the stats and spell
/// rotation, then spawns the existing `Boss` entity. The boss plugin reads
/// the `CreatureArchetype` tag to pick the right rotation.
pub trait BossPlaceable: EnemyPlaceable {
    /// Per-boss stats, spell rotation and arena radius.
    fn boss_config(&self) -> BossConfig;
}

/// Friendly or neutral interactable (merchant, quest giver).
pub trait NpcPlaceable: PlaceableDefinition {
    /// Interaction triggered when a player talks to the NPC.
    fn interaction(&self) -> InteractionKind;
}

/// Invisible gameplay zone (PvP, teleport, area trigger).
pub trait TriggerPlaceable: PlaceableDefinition {
    /// Shape, event and one-shot policy for this trigger kind.
    fn trigger_config(&self) -> TriggerConfig;
}

/// Harvestable node (ore vein, tree, herb).
pub trait ResourceNodePlaceable: PlaceableDefinition {
    /// Piece count, channel time, regen and yield for this resource kind.
    fn resource_config(&self) -> ResourceConfig;
}

/// One-shot interaction (door, lever, chest).
pub trait InteractablePlaceable: PlaceableDefinition {
    /// Interaction triggered when a player uses the object.
    fn interaction(&self) -> InteractionKind;
}
