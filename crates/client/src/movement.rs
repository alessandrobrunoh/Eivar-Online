//! Shared pure helpers for point-and-click movement.
//!
//! Contains the canonical movement-speed computation, the cast-blocking
//! policy, and the shared move-towards-target stepping used by both the
//! authoritative server system and the client-side prediction system.

use bevy::prelude::{
    Camera, Camera3d, GlobalTransform, Mut, Query, Ray3d, Resource, Transform, Vec3, Window, With,
};
use bevy::window::PrimaryWindow;

use bevymmo_gameplay::entity::components::EntityState;
use bevymmo_gameplay::spells::{CastKind, CastProgress};
use bevymmo_gameplay::stats::events::ModifierOp;
use bevymmo_gameplay::stats::events::StatField;
use bevymmo_gameplay::stats::modifiers::ActiveStatModifiers;
use bevymmo_gameplay::stats::modifiers::StatModifierInstance;
use bevymmo_network::network::protocol::{Inputs, LookDirection, Position};
use bevymmo_world::{CollisionGrid, SurfaceQuery};

/// Distance (in world units) under which a move command is considered satisfied.
pub const ARRIVAL_DISTANCE: f32 = 0.05;

/// Local pending click target shared between click selection, input buffering,
/// and the predicted/authoritative move systems.
///
/// Pure data: lives in `shared` so both `bevymmo_server` and `bevymmo_client`
/// can use it without creating a cross-crate dependency.
#[derive(Resource, Default)]
pub struct MoveTarget(pub Option<Vec3>);

/// Optimistic plant applied the frame a CastTime / Channeling is sent,
/// before leftover dest replicates as cleared.
///
/// Without this the client keeps walking toward the last server dest for
/// ~100 ms and interrupts its own wind-up. The freeze expires on its own
/// so a rejected reducer cannot leave the character planted. A new
/// right-click still walks (and interrupts) because prediction only
/// ignores stale dest while the freeze is active.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct LocalMovementFreeze {
    until: f32,
}

impl LocalMovementFreeze {
    /// How long the optimistic root lasts if `cast_state` never arrives.
    pub const DURATION: f32 = 0.3;

    /// Start (or refresh) the freeze at `now` seconds of app time.
    pub fn arm(&mut self, now: f32) {
        self.until = now + Self::DURATION;
    }

    pub fn is_active(&self, now: f32) -> bool {
        now < self.until
    }

    pub fn clear(&mut self) {
        self.until = 0.0;
    }
}

/// Client-side surface query data for height-aware click-to-move.
///
/// Populated by the presentation layer from loaded map data, consumed by the
/// client click-to-move system to resolve mouse clicks onto proper terrain heights.
/// Lives in `shared` so both client and presentation can access it without
/// cross-crate dependencies.
#[derive(Resource, Default)]
pub struct ClientSurfaceQuery(pub Option<SurfaceQuery>);

/// Client-side blocker grid and step budget, the companion to
/// [`ClientSurfaceQuery`].
///
/// Populated by the presentation layer from the same loaded manifest, and
/// consumed by the SpacetimeDB prediction system so the locally simulated
/// position obeys the *same* walls the authoritative module does. Without it
/// the client walks its rendered character straight through a parapet and off
/// a ledge, and only the reconcile pull drags it back — which is a drift of
/// `speed / RECONCILE_RATE` metres of visible wall penetration, not a
/// correction anybody wants to see.
///
/// Lives here rather than next to `ClientWorldMap` because `bevymmo_client`
/// cannot depend on `bevymmo_presentation` (the dependency runs the other
/// way), exactly like [`ClientSurfaceQuery`].
#[derive(Resource, Default)]
pub struct ClientCollision {
    pub grid: Option<CollisionGrid>,
    pub max_step_height: f32,
    pub collision_radius: f32,
}

/// Calculates movement speed after active stat modifiers.
///
/// This is shared by gameplay and the stats UI so the value displayed to the
/// player matches the speed used by gameplay.
pub fn effective_movement_speed(base_speed: f32, modifiers: Option<&ActiveStatModifiers>) -> f32 {
    let Some(active) = modifiers else {
        return base_speed;
    };
    effective_value(StatField::Speed, base_speed, &active.modifiers)
}

fn effective_value(field: StatField, base: f32, modifiers: &[StatModifierInstance]) -> f32 {
    let mut result = base;
    let mut override_value: Option<f32> = None;

    for modifier in modifiers {
        for effect in &modifier.effects {
            if let bevymmo_gameplay::stats::modifiers::ModifierEffectInstance::Stat {
                field: effect_field,
                operation,
                value,
            } = effect
            {
                if *effect_field != field {
                    continue;
                }
                match operation {
                    ModifierOp::Add => result += value,
                    ModifierOp::Multiply => result *= value,
                    ModifierOp::Override => override_value = Some(*value),
                }
            }
        }
    }

    override_value.unwrap_or(result)
}

/// Returns true when a cast state must freeze point-and-click movement.
pub fn should_block_movement_for_cast(cast: Option<&CastProgress>) -> bool {
    let Some(cast) = cast else {
        return false;
    };
    match cast.kind {
        CastKind::CastTime => true,
        CastKind::Channeling => {
            cast.channel_movement
                == bevymmo_gameplay::spells::ChannelMovementPolicy::InterruptOnMove
        }
        CastKind::Instant => false,
    }
}

/// Steps a single entity towards its current move target.
///
/// Shared by the authoritative server system (`bevymmo_server::player_movement`)
/// and the client prediction system (`bevymmo_presentation::player_movement`)
/// so both sides advance movement with identical math.
///
/// Returns early if the entity is dead; clears state to `Idle` when the input
/// is not a `MoveTo` or the entity has reached the target.
pub fn move_towards_target(
    mut position: Mut<Position>,
    mut look_direction: Mut<LookDirection>,
    input: &Inputs,
    speed: f32,
    mut state: Mut<EntityState>,
) {
    if state.is_dead() {
        return;
    }

    let Inputs::MoveTo(target) = input else {
        *state = EntityState::Idle;
        return;
    };

    let offset = *target - position.0;
    let distance = offset.length();
    if distance > 0.001 {
        look_direction.0 = (offset / distance).normalize_or_zero();
    }
    if distance <= ARRIVAL_DISTANCE {
        position.0 = *target;
        *state = EntityState::Idle;
        return;
    }

    position.0 += offset / distance * speed.min(distance);
    *state = EntityState::Moving;
}

// The authoritative module and Bevy clients use the same pure terrain rules.
pub use bevymmo_gameplay::movement::{
    snap_to_ground, step_on_terrain, TerrainStep, SNAP_STEP_BUDGET,
};

// ==================== RAY-TO-SURFACE RESOLUTION ====================

/// Resolves a camera ray to a ground position on walkable surfaces.
///
/// Samples points along the ray from the camera and returns the first point
/// whose X/Z coordinates resolve to a valid ground contact via `SurfaceQuery`.
/// This enables height-aware click-to-move without relying on visual mesh raycasts.
///
/// # Arguments
/// * `ray_origin` - The camera position in world space
/// * `ray_direction` - Normalized direction vector from camera through cursor
/// * `surface_query` - Surface query data for terrain height resolution
/// * `max_distance` - Maximum distance to sample along the ray (default 100.0)
/// * `step_size` - Distance between sample points along the ray (default 1.0)
///
/// # Returns
/// * `Some(Vec3)` - First valid ground position on the ray
/// * `None` - No valid ground position found within max_distance
///
/// # Example
/// ```ignore
/// let target = resolve_ray_to_ground(
///     camera_pos,
///     ray_dir,
///     &surface_query,
///     100.0,
///     1.0
/// );
/// ```
pub fn resolve_ray_to_ground(
    ray_origin: Vec3,
    ray_direction: Vec3,
    surface_query: &SurfaceQuery,
    max_distance: f32,
    step_size: f32,
) -> Option<Vec3> {
    if surface_query.is_empty() {
        // Fallback to Y=0 plane when no surface data is available
        let plane_normal = Vec3::Y;
        let plane_d = 0.0; // Y = 0 plane

        // Ray-plane intersection: t = -(normal · origin + d) / (normal · direction)
        let denominator = plane_normal.dot(ray_direction);
        if denominator.abs() < 1e-6 {
            return None; // Ray is parallel to plane
        }

        let t = -(plane_normal.dot(ray_origin) + plane_d) / denominator;
        if t < 0.0 || t > max_distance {
            return None; // Intersection is behind camera or too far
        }

        let intersection = ray_origin + ray_direction * t;
        return Some(Vec3::new(intersection.x, 0.0, intersection.z));
    }

    let normalized_direction = ray_direction.normalize_or_zero();
    if normalized_direction == Vec3::ZERO {
        return None;
    }

    // Find where the camera ray actually crosses the terrain. Merely returning
    // the first sample whose X/Z is inside the map selects a point near the
    // camera on hills, instead of the point under the cursor.
    let num_steps = (max_distance / step_size).ceil() as i32;
    let mut previous: Option<(f32, f32)> = None;

    for step in 0..=num_steps {
        let t = (step as f32 * step_size).min(max_distance);
        let sample_point = ray_origin + normalized_direction * t;
        let Some(ground_contact) = surface_query.surface_contact_at(sample_point.x, sample_point.z)
        else {
            previous = None;
            continue;
        };

        let signed_distance = sample_point.y - ground_contact.height;
        let is_crossing = match previous {
            Some((_prev_t, prev_dist)) => prev_dist >= 0.0 && signed_distance <= 0.0,
            None => signed_distance <= 0.0 && ray_origin.y >= ground_contact.height,
        };

        if is_crossing {
            // Refine the crossing so coarse ray steps do not move the click
            // target noticeably on steep terrain.
            let mut low = previous
                .map(|(pt, _)| pt)
                .unwrap_or((t - step_size).max(0.0));
            let mut high = t;
            for _ in 0..8 {
                let middle = (low + high) * 0.5;
                let point = ray_origin + normalized_direction * middle;
                let Some(contact) = surface_query.surface_contact_at(point.x, point.z) else {
                    low = middle;
                    continue;
                };
                if point.y - contact.height > 0.0 {
                    low = middle;
                } else {
                    high = middle;
                }
            }

            let point = ray_origin + normalized_direction * high;
            let contact = surface_query.surface_contact_at(point.x, point.z)?;
            return Some(Vec3::new(point.x, contact.height, point.z));
        }

        previous = Some((t, signed_distance));
    }

    None
}

/// Casts a ray from the primary window's cursor through the first active
/// `Camera3d`.
///
/// Returns `None` if there is no primary window, the cursor is outside it,
/// no 3D camera exists, or the viewport-to-world projection fails (e.g. a
/// zero-size viewport) — the same set of checks every mouse-click system in
/// this crate needs before it can do anything else with the click.
pub fn cursor_ray(
    windows: &Query<&Window, With<PrimaryWindow>>,
    cameras: &Query<(&Camera, &Transform), With<Camera3d>>,
) -> Option<Ray3d> {
    let window = windows.single().ok()?;
    let cursor_position = window.cursor_position()?;
    let (camera, camera_transform) = cameras.iter().next()?;
    let view = GlobalTransform::from(*camera_transform);
    camera.viewport_to_world(&view, cursor_position).ok()
}

/// Intersection with the Y=0 plane, or `None` when the ray is parallel
/// or the hit is behind the camera / past `max_distance`.
pub fn intersect_y0_plane(origin: Vec3, direction: Vec3, max_distance: f32) -> Option<Vec3> {
    if direction.y.abs() < 1e-6 {
        return None;
    }
    let t = -origin.y / direction.y;
    if !t.is_finite() || t < 0.0 || t > max_distance {
        return None;
    }
    Some(origin + direction * t)
}

/// Resolves the cursor's camera ray to a world-space ground point for
/// click-to-move.
///
/// Prefers the terrain surface (via [`resolve_ray_to_ground`]) when
/// `surface_query` has one loaded; otherwise falls back to the horizontal
/// plane at Y=0. The server ignores the client-sent Y and resolves X/Z
/// authoritatively against its own collision data, so this only needs to
/// land *roughly* under the cursor, not exactly on terrain.
///
/// This is the single implementation behind what used to be two
/// independent copies of "read the click, ray-cast from the camera,
/// resolve to ground" (one driving the click-feedback rings, one driving
/// the actual move command sent to the server) plus a third, partial copy
/// of just the camera-ray step in the targeting system.
pub fn resolve_click_to_ground(
    windows: &Query<&Window, With<PrimaryWindow>>,
    cameras: &Query<(&Camera, &Transform), With<Camera3d>>,
    surface_query: &ClientSurfaceQuery,
    max_distance: f32,
) -> Option<Vec3> {
    let ray = cursor_ray(windows, cameras)?;
    surface_query
        .0
        .as_ref()
        .and_then(|sq| resolve_ray_to_ground(ray.origin, *ray.direction, sq, max_distance, 0.5))
        .or_else(|| intersect_y0_plane(ray.origin, *ray.direction, max_distance))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevymmo_world::{
        CollisionGrid, HeightfieldData, MapBounds, MapManifest, SurfaceBounds, SurfaceKind,
        SurfaceQuery, WalkableSurface, WorldMetrics,
    };

    fn create_test_surface_query() -> (SurfaceQuery, MapManifest) {
        let manifest = MapManifest {
            version: 2,
            map_id: "test_movement".to_string(),
            display_name: "Test Movement".to_string(),
            bounds: MapBounds {
                min_x: -20.0,
                max_x: 20.0,
                min_z: -20.0,
                max_z: 20.0,
            },
            terrain: Default::default(),
            props: vec![],
            world_metrics: Some(WorldMetrics::default()),
            surfaces: vec![WalkableSurface {
                id: "surface_flat".to_string(),
                kind: SurfaceKind::Flat,
                object: None,
                bounds: Some(SurfaceBounds {
                    min_x: -10.0,
                    max_x: 10.0,
                    min_z: -10.0,
                    max_z: 10.0,
                }),
                height: Some(2.0),
                min_height: None,
                max_height: None,
                grid_size: None,
                size: Some(20.0),
                purpose: Some("Test surface for movement".to_string()),
                heightfield: None,
                walkable_mesh: None,
                layer: None,
                max_slope_deg: None,
            }],
            traversals: vec![],
            blockers: vec![],
            test_route: vec![],
            test_checklist: vec![],
            mountain_switchback_test: None,
            distant_plateau_test: None,
        };

        (SurfaceQuery::from_manifest(&manifest), manifest)
    }

    #[test]
    fn snap_to_ground_lifts_an_entity_stranded_below_the_terrain() {
        let (query, _manifest) = create_test_surface_query();

        // Surface sits at y = 2.0; the entity is 9 m under it, as happens to a
        // player spawned at the default `Vec3::ZERO` on a map whose origin is
        // a hillside, or one whose persisted Y predates a terrain edit.
        let mut position = Vec3::new(0.0, -7.0, 0.0);
        assert!(
            snap_to_ground(&mut position, &query, 0.45),
            "an entity below every surface must be recovered, not left stuck"
        );
        assert_eq!(position.y, 2.0);
    }

    #[test]
    fn snap_to_ground_still_refuses_positions_with_no_surface_at_all() {
        let (query, _manifest) = create_test_surface_query();

        let mut position = Vec3::new(100.0, -7.0, 100.0);
        assert!(!snap_to_ground(&mut position, &query, 0.45));
        assert_eq!(position.y, -7.0);
    }

    // ==================== TERRAIN STEP TESTS ====================

    /// Builds a tiny world with a flat ground at y=0.0 and a ramp that rises
    /// from y=0.0 at x=5.0 to y=5.0 at x=10.0 (1 unit of Y per 1 unit of X).
    /// The ramp overlaps the ground in XZ bounds, which is exactly the
    /// rolling-hills-vs-mountain scenario from the bug report.
    fn create_ramp_world() -> (SurfaceQuery, CollisionGrid) {
        let bounds = SurfaceBounds {
            min_x: -10.0,
            max_x: 10.0,
            min_z: -10.0,
            max_z: 10.0,
        };
        // 5x5 ramp heightfield: 0 at x=-10, 5 at x=10 (linear).
        let res = 5u32;
        let stride = (res + 1) as usize;
        let mut heights = vec![0.0f32; stride * stride];
        for xi in 0..=res as usize {
            let h = xi as f32; // 0..5
            for zi in 0..=res as usize {
                heights[xi * stride + zi] = h;
            }
        }
        let ramp_hf = HeightfieldData::new(res, bounds, heights);
        let manifest = MapManifest {
            version: 2,
            map_id: "ramp".to_string(),
            display_name: "Ramp".to_string(),
            bounds: MapBounds {
                min_x: -10.0,
                max_x: 10.0,
                min_z: -10.0,
                max_z: 10.0,
            },
            terrain: Default::default(),
            props: vec![],
            world_metrics: Some(WorldMetrics::default()),
            surfaces: vec![WalkableSurface {
                id: "surface_ramp".to_string(),
                kind: SurfaceKind::Mesh,
                object: None,
                bounds: Some(bounds),
                height: None,
                min_height: None,
                max_height: None,
                grid_size: None,
                size: None,
                purpose: None,
                heightfield: Some(ramp_hf),
                walkable_mesh: None,
                layer: None,
                max_slope_deg: None,
            }],
            traversals: vec![],
            blockers: vec![],
            test_route: vec![],
            test_checklist: vec![],
            mountain_switchback_test: None,
            distant_plateau_test: None,
        };
        (
            SurfaceQuery::from_manifest(&manifest),
            CollisionGrid::build(&manifest),
        )
    }

    #[test]
    fn test_step_on_terrain_rejects_cliff_too_high() {
        // From y=0 a direct step toward a ramp point that is much higher than
        // max_step_height must NOT teleport the entity up: there is no
        // reachable surface at the candidate position within budget.
        let (query, grid) = create_ramp_world();

        // Player on the ground at y=0, near the steep top of the ramp where
        // the height delta per step exceeds the default 0.45 budget.
        let start = Vec3::new(8.0, 0.0, 0.0);
        // Step toward a point on the ramp at x=9 (height ≈ 4.5).
        let step = step_on_terrain(start, 9.0, 0.0, 1.0, &query, &grid, 0.45, 0.45);
        assert_eq!(step, TerrainStep::Blocked);
    }

    #[test]
    fn test_step_on_terrain_climbs_ramp_gradually() {
        // Walking from the bottom of the ramp upward one tick at a time, the
        // entity should ascend without ever skipping more than max_step_height
        // in a single tick. This is the canonical "player walks up the
        // switchback" scenario.
        let (query, grid) = create_ramp_world();

        let mut pos = Vec3::new(-9.0, 0.0, 0.0); // ramp base, y=0
        let target = Vec3::new(9.0, 5.0, 0.0); // ramp top
        let max_step_height = 0.45;
        let speed = 0.5;

        let mut prev_y = pos.y;
        for _ in 0..200 {
            match step_on_terrain(
                pos,
                target.x,
                target.z,
                speed,
                &query,
                &grid,
                max_step_height,
                0.45,
            ) {
                TerrainStep::Arrived(p) => {
                    pos = p;
                    break;
                }
                TerrainStep::Moved(p) => {
                    let dy = (p.y - prev_y).abs();
                    assert!(
                        dy <= max_step_height + 1e-5,
                        "single-tick vertical delta {} exceeded max_step_height {}",
                        dy,
                        max_step_height
                    );
                    prev_y = p.y;
                    pos = p;
                }
                TerrainStep::Blocked | TerrainStep::NoSurface => break,
            }
        }

        // The entity should have climbed significantly, proving the ramp was
        // followed instead of the entity being stuck on the ground.
        assert!(
            pos.y > 2.0,
            "entity should have climbed the ramp, ended at y={}",
            pos.y
        );
    }

    // ==================== RAY-TO-SURFACE TESTS ====================

    #[test]
    fn test_resolve_ray_to_ground_flat_surface() {
        let (query, _manifest) = create_test_surface_query();

        // Camera above the surface, looking down at the center
        let camera_pos = Vec3::new(0.0, 10.0, 5.0);
        let ray_dir = (Vec3::new(0.0, 2.0, 0.0) - camera_pos).normalize();

        let result = resolve_ray_to_ground(camera_pos, ray_dir, &query, 100.0, 0.5);

        assert!(result.is_some(), "Ray should hit the flat surface");
        let ground_pos = result.unwrap();
        assert_eq!(ground_pos.y, 2.0, "Height should match surface height");
        // X should be near 0 (camera is centered on X)
        assert!((ground_pos.x - 0.0).abs() < 1.0, "X should be near 0");
        // Z should be closer to 0 than to camera position (5.0), indicating we hit the surface
        assert!(ground_pos.z < 4.0, "Z should be less than camera Z (5.0)");
        // Z should be reasonably close to 0 (the target we were aiming at)
        assert!(ground_pos.z > -1.0, "Z should be greater than -1.0");
    }

    #[test]
    fn test_resolve_ray_to_ground_fallback_to_y0_plane() {
        // Create an empty surface query (no surface data)
        let empty_query = SurfaceQuery::from_manifest(&MapManifest {
            version: 2,
            map_id: "empty".to_string(),
            display_name: "Empty Map".to_string(),
            bounds: MapBounds {
                min_x: -10.0,
                max_x: 10.0,
                min_z: -10.0,
                max_z: 10.0,
            },
            terrain: Default::default(),
            props: vec![],
            world_metrics: None,
            surfaces: vec![], // No surfaces
            traversals: vec![],
            blockers: vec![],
            test_route: vec![],
            test_checklist: vec![],
            mountain_switchback_test: None,
            distant_plateau_test: None,
        });

        // Camera looking down at the Y=0 plane
        let camera_pos = Vec3::new(0.0, 10.0, 5.0);
        let ray_dir = (Vec3::new(0.0, 0.0, 0.0) - camera_pos).normalize();

        let result = resolve_ray_to_ground(camera_pos, ray_dir, &empty_query, 100.0, 0.5);

        assert!(result.is_some(), "Ray should hit Y=0 plane as fallback");
        let ground_pos = result.unwrap();
        assert_eq!(ground_pos.y, 0.0, "Height should be 0.0 (fallback plane)");
    }

    #[test]
    fn test_resolve_ray_to_ground_no_hit() {
        let (query, _manifest) = create_test_surface_query();

        // Camera looking away from the surface
        let camera_pos = Vec3::new(0.0, 10.0, 5.0);
        let ray_dir = Vec3::new(0.0, 1.0, 0.0); // Looking straight up

        let result = resolve_ray_to_ground(camera_pos, ray_dir, &query, 100.0, 0.5);

        assert!(
            result.is_none(),
            "Ray looking up should not hit any surface"
        );
    }

    #[test]
    fn test_resolve_ray_to_ground_parallel_to_plane() {
        let empty_query = SurfaceQuery::from_manifest(&MapManifest {
            version: 2,
            map_id: "empty".to_string(),
            display_name: "Empty Map".to_string(),
            bounds: MapBounds {
                min_x: -10.0,
                max_x: 10.0,
                min_z: -10.0,
                max_z: 10.0,
            },
            terrain: Default::default(),
            props: vec![],
            world_metrics: None,
            surfaces: vec![],
            traversals: vec![],
            blockers: vec![],
            test_route: vec![],
            test_checklist: vec![],
            mountain_switchback_test: None,
            distant_plateau_test: None,
        });

        // Ray parallel to Y=0 plane
        let camera_pos = Vec3::new(0.0, 0.0, 0.0);
        let ray_dir = Vec3::new(1.0, 0.0, 0.0); // Horizontal ray

        let result = resolve_ray_to_ground(camera_pos, ray_dir, &empty_query, 100.0, 0.5);

        assert!(
            result.is_none(),
            "Horizontal ray should not intersect plane"
        );
    }

    #[test]
    fn y0_plane_rejects_a_horizontal_ray() {
        assert_eq!(
            intersect_y0_plane(Vec3::new(0.0, 10.0, 0.0), Vec3::X, 100.0),
            None
        );
    }

    #[test]
    fn y0_plane_hits_looking_down() {
        let hit = intersect_y0_plane(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 100.0);
        assert_eq!(hit, Some(Vec3::ZERO));
    }

    #[test]
    fn test_resolve_ray_to_ground_zero_direction() {
        let (query, _manifest) = create_test_surface_query();

        let camera_pos = Vec3::new(0.0, 10.0, 5.0);
        let zero_dir = Vec3::ZERO;

        let result = resolve_ray_to_ground(camera_pos, zero_dir, &query, 100.0, 0.5);

        assert!(result.is_none(), "Zero direction should return None");
    }

    #[test]
    fn test_resolve_ray_to_ground_height_tolerance() {
        let (query, _manifest) = create_test_surface_query();

        // Camera positioned such that ray passes near but not through the surface
        let camera_pos = Vec3::new(0.0, 10.0, 5.0);
        let ray_dir = (Vec3::new(0.0, 2.5, 0.0) - camera_pos).normalize(); // Aiming slightly above surface

        let result = resolve_ray_to_ground(camera_pos, ray_dir, &query, 100.0, 0.5);

        // Should still find the surface within tolerance
        assert!(result.is_some(), "Ray should find surface within tolerance");
        let ground_pos = result.unwrap();
        assert_eq!(ground_pos.y, 2.0, "Height should be resolved to surface");
    }

    #[test]
    fn test_resolve_ray_to_ground_steep_mountain_slope() {
        let bounds = SurfaceBounds {
            min_x: 0.0,
            max_x: 10.0,
            min_z: 0.0,
            max_z: 10.0,
        };
        // 2x2 heightfield: rises from 0.0 to 20.0 over 10 units in X (slope > 60 deg)
        let heights = vec![0.0, 0.0, 20.0, 20.0];
        let hf = HeightfieldData::new(1, bounds, heights);
        let manifest = MapManifest {
            version: 2,
            map_id: "steep_mountain".to_string(),
            display_name: "Steep Mountain".to_string(),
            bounds: MapBounds {
                min_x: 0.0,
                max_x: 10.0,
                min_z: 0.0,
                max_z: 10.0,
            },
            terrain: Default::default(),
            props: vec![],
            world_metrics: Some(WorldMetrics {
                max_walkable_slope_deg: 45.0,
                ..Default::default()
            }),
            surfaces: vec![WalkableSurface {
                id: "steep_cliff".to_string(),
                kind: SurfaceKind::Mesh,
                object: None,
                bounds: Some(bounds),
                height: None,
                min_height: Some(0.0),
                max_height: Some(20.0),
                grid_size: None,
                size: None,
                purpose: None,
                heightfield: Some(hf),
                walkable_mesh: None,
                layer: None,
                max_slope_deg: Some(45.0),
            }],
            traversals: vec![],
            blockers: vec![],
            test_route: vec![],
            test_checklist: vec![],
            mountain_switchback_test: None,
            distant_plateau_test: None,
        };

        let query = SurfaceQuery::from_manifest(&manifest);

        // Camera high above at (5.0, 30.0, 5.0), aiming straight down at the steep mountain at x=5, z=5 (elevation = 10.0)
        let camera_pos = Vec3::new(5.0, 30.0, 5.0);
        let ray_dir = Vec3::new(0.0, -1.0, 0.0);

        let result = resolve_ray_to_ground(camera_pos, ray_dir, &query, 100.0, 0.5);
        assert!(result.is_some(), "Ray should hit steep mountain");
        let hit = result.unwrap();
        assert!(
            (hit.y - 10.0).abs() < 0.1,
            "Hit height should be ~10.0 on the mountain, got {}",
            hit.y
        );
    }

    #[test]
    fn local_freeze_is_active_until_it_expires() {
        let mut freeze = LocalMovementFreeze::default();
        assert!(!freeze.is_active(0.0));
        freeze.arm(1.0);
        assert!(freeze.is_active(1.0));
        assert!(freeze.is_active(1.0 + LocalMovementFreeze::DURATION - 0.001));
        assert!(!freeze.is_active(1.0 + LocalMovementFreeze::DURATION));
        freeze.clear();
        assert!(!freeze.is_active(1.0));
    }
}
