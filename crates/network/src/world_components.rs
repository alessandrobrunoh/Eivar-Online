//! The components that describe where things are and what they look like.
//!
//! These lived in `network::protocol` because lightyear replicated them, which
//! made a set of perfectly ordinary components look like networking. They are
//! not: `Position` is where something is, and the presentation layer reads it
//! the same way regardless of what filled it in.
//!
//! Extracting them is what lets `network::protocol` — and with it the lightyear
//! dependency — be deleted. The SpacetimeDB bridge writes these from `game_entity`
//! rows; nothing else changes.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Position of any game entity: player, enemy, NPC, projectile.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect, Deref, DerefMut)]
pub struct Position(pub Vec3);

/// Horizontal direction the entity is facing.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect, Deref, DerefMut)]
pub struct LookDirection(pub Vec3);

impl Default for LookDirection {
    fn default() -> Self {
        Self(Vec3::Z)
    }
}

/// Tint applied to an entity's generic mesh.
#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct EntityColor(pub bevy::color::Color);

/// The server-side id of the entity this one mirrors.
///
/// Distinct from Bevy's `Entity`, which is local and unstable: this is the value
/// a client sends back to name a target. It is `game_entity.entity_id`.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, Reflect)]
pub struct NetworkEntityId(pub u64);

/// Homing or ground-targeted projectile, predicted between server ticks.
#[derive(Component, Clone, Debug)]
pub struct ProjectileFlight {
    pub speed: f32,
    pub target_entity: Option<u64>,
    pub target_position: Option<Vec3>,
}

/// Authoritative ground AoE region, drawn locally for its remaining lifetime.
#[derive(Component, Clone, Debug)]
pub struct AoeZone {
    pub radius: f32,
    pub remaining_seconds: f32,
    pub pending_delay_seconds: f32,
    pub spell_id: String,
    /// `None` = circle. Degrees when the region is a frontal cone.
    pub cone_angle_deg: Option<f32>,
    pub direction: Vec3,
    /// `game_entity.entity_id` of the caster. Used to hide hostile telegraphs.
    pub caster: u64,
}

/// Marks a spell projectile so the renderer draws it as one rather than as a
/// generic entity mesh.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProjectileVisual {
    pub spell_id: String,
}
