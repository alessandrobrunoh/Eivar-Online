//! Point-to-point movement, shared by both sides of the wire.
//!
//! The server advances characters by calling [`step_on_terrain`] on its tick;
//! the client calls the *same function* between server updates to predict where
//! its own character is going. That sharing is the point: with lightyear gone
//! the client no longer gets prediction for free, and two hand-written
//! implementations of "walk towards a point" would disagree in exactly the way
//! that makes a character rubber-band — or, when only one of them consults the
//! collision grid, walk visibly through walls until reconciliation catches up.
//!
//! Flat stepping ([`step_towards`]) remains the fallback for a client that has
//! no map loaded yet. Terrain stepping takes the world query and collision grid
//! from its caller, so this crate stays independent of Bevy, filesystems, and
//! storage.

use glam::Vec3;

/// Distance below which a character counts as having arrived.
///
/// Matches the threshold the Bevy server used, so the two agree on when
/// movement stops rather than leaving a character twitching on the spot.
pub const ARRIVAL_EPSILON: f32 = 0.001;

/// Outcome of advancing a character for one time step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// Still en route; the character is at this position.
    Moving(Vec3),
    /// Reached the target this step, and should stop.
    Arrived(Vec3),
}

/// Advances `position` towards `target` for `dt` seconds.
///
/// `speed` is in units per **second**. Note that the Bevy server stored speed as
/// units per *tick* at a fixed 60 Hz (`effective_speed.min(distance)`), which
/// only worked because the tick rate never varied. SpacetimeDB's scheduler does
/// not guarantee a fixed cadence — the interval is measured from the end of the
/// previous run, so a nominal 50 ms tick was measured at ~56 ms — hence the
/// explicit `dt` here. Converting an old value: `per_second = per_tick * 60.0`.
pub fn step_towards(position: Vec3, target: Vec3, speed: f32, dt: f32) -> Step {
    let offset = target - position;
    let distance = offset.length();

    if distance <= ARRIVAL_EPSILON {
        return Step::Arrived(target);
    }

    let travel = speed * dt;
    if travel >= distance {
        return Step::Arrived(target);
    }

    Step::Moving(position + offset / distance * travel)
}

/// Why a `move_to` request should be accepted or refused.
///
/// CastTime and Channeling still accept a destination so movement can cancel
/// the wind-up or an InterruptOnMove channel — the tick, not the reducer,
/// ends that cast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovementLock {
    None,
    CastTime,
    Channel,
}

/// Whether the player may issue a new destination.
///
/// `cc_blocks` covers Stun/Root. CastTime and Channel are allowed so a
/// click can interrupt.
pub fn movement_intent_allowed(lock: MovementLock, cc_blocks: bool) -> bool {
    if cc_blocks {
        return false;
    }
    match lock {
        MovementLock::None | MovementLock::Channel | MovementLock::CastTime => true,
    }
}

/// Destination the local client should step towards this frame.
///
/// A crowd-control block returns `None` immediately, even when the server
/// dest has not cleared yet.
///
/// `planted` is true on the frames after a CastTime / Channeling start,
/// before leftover dest has replicated as cleared. A new right-click still
/// walks so the player can interrupt; leftover dest is ignored.
///
/// While unlocked and the player is click-moving, prefer the pending click.
/// Otherwise follow the server so a cancelled dest is not resumed.
pub fn predicted_move_dest(
    pending: Option<Vec3>,
    authoritative: Option<Vec3>,
    lock: MovementLock,
    right_mouse_held: bool,
    cc_blocks: bool,
    planted: bool,
) -> Option<Vec3> {
    if !movement_intent_allowed(lock, cc_blocks) {
        return None;
    }
    if planted && !right_mouse_held {
        return None;
    }
    if right_mouse_held {
        pending.or(if planted { None } else { authoritative })
    } else {
        authoritative
    }
}

/// How far predicted position may lead the last server pose, in seconds of
/// travel, before an idle reconcile starts pulling it back.
///
/// One 50 ms tick plus a little RTT slack: a rooted or arrived character is
/// routinely this far ahead of the last replicated pose, and hauling that
/// gap in reads as walking backwards.
pub const RECONCILE_SLACK_SECONDS: f32 = 0.1;

/// Beyond this much error, stop easing and teleport. Covers a genuine
/// desync — a teleport, a respawn, a long stall — where smoothing would
/// send the character gliding across the map.
pub const PREDICTION_SNAP_DISTANCE: f32 = 5.0;

/// What to do with the gap between predicted and authoritative position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconcile {
    /// Leave the predicted pose alone this frame.
    Leave,
    /// Ease toward the last server pose.
    Ease,
    /// Snap onto the last server pose.
    Snap,
}

/// Whether a predicted pose should be corrected toward the last server pose.
///
/// While a destination is live the predicted step owns the motion; pulling
/// toward a stale pose fights it. Once the dest is gone, a gap inside
/// [`RECONCILE_SLACK_SECONDS`] of travel is ordinary prediction lead and
/// must not walk the character backwards — that is the hitch at the start
/// of a rooted cast.
pub fn reconcile_offset(
    predicted: Vec3,
    authoritative: Vec3,
    dest: Option<Vec3>,
    speed: f32,
) -> Reconcile {
    let drift = predicted.distance(authoritative);
    if drift > PREDICTION_SNAP_DISTANCE {
        return Reconcile::Snap;
    }
    if dest.is_some() {
        return Reconcile::Leave;
    }
    let slack = speed.max(0.0) * RECONCILE_SLACK_SECONDS;
    if drift <= slack {
        Reconcile::Leave
    } else {
        Reconcile::Ease
    }
}

/// Whether a newly aimed cast should turn the character to face its target.
///
/// Instant casts while walking must not: the next movement tick (and the
/// client's predicted look) would immediately overwrite it, which reads as
/// a twitch toward the spell and back onto the path. CastTime and Channeling
/// stop leftover dest then face, even though a later click can walk and
/// interrupt.
pub fn should_face_cast_target(moving: bool, lock: MovementLock) -> bool {
    if !moving {
        return true;
    }
    matches!(lock, MovementLock::CastTime | MovementLock::Channel)
}

/// Horizontal facing implied by moving from `position` to `target`.
///
/// Returns `None` when the two are vertically aligned, in which case the caller
/// should keep the previous facing rather than snapping to an arbitrary one.
pub fn look_direction(position: Vec3, target: Vec3) -> Option<Vec3> {
    let flat = Vec3::new(target.x - position.x, 0.0, target.z - position.z);
    if flat.length() <= ARRIVAL_EPSILON {
        return None;
    }
    Some(flat.normalize_or_zero())
}

/// Outcome of a terrain-aware movement step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TerrainStep {
    /// Entity reached its target; carry the resolved on-ground position.
    Arrived(Vec3),
    /// Entity moved one step toward the target.
    Moved(Vec3),
    /// Step was rejected by terrain or a blocker.
    Blocked,
    /// Target is not on a walkable surface.
    NoSurface,
}

/// Permissive vertical budget for spawn, persisted-position, and teleport recovery.
pub const SNAP_STEP_BUDGET: f32 = 5.0;

/// Fallback collision radius when a map does not declare `player_radius`.
///
/// Matches [`bevymmo_world::WorldMetrics::default`]. Tests that reconstruct
/// map_02's parapets still pass an explicit 0.45 so their contact point stays
/// the one they assert.
pub const DEFAULT_STEP_RADIUS: f32 = 0.35;

/// Historical probe radius used by the wall-contact tests.
#[cfg(test)]
const STEP_COLLISION_RADIUS: f32 = 0.45;

/// Snaps an entity onto the highest reachable ground surface at its X/Z point.
///
/// When the entity is stranded below terrain, this deliberately falls back to
/// the highest surface so spawn and persisted-position recovery cannot leave it
/// permanently unable to move.
pub fn snap_to_ground(
    position: &mut Vec3,
    surface_query: &crate::world::SurfaceQuery,
    max_step_height: f32,
) -> bool {
    let contact = surface_query
        .ground_at_reachable(position.x, position.z, position.y, max_step_height)
        .or_else(|| surface_query.ground_at(position.x, position.z));
    let Some(contact) = contact else {
        return false;
    };

    if (contact.height - position.y).abs() <= ARRIVAL_EPSILON {
        return false;
    }
    position.y = contact.height;
    true
}

/// Advances an entity toward an X/Z target across walkable terrain.
///
/// `max_travel` is the horizontal distance available for this simulation step;
/// callers with per-second speed should pass `speed * dt`. Height is always
/// resolved from the authoritative surface query, never from the target.
///
/// Long steps are split into chunks of `collision_radius` so a probe cannot
/// tunnel through a blocker thinner than the character's footprint. A player
/// at 9 u/s with a stalled tick clamped to 0.25 s can ask for 2.25 m — more
/// than map_02's 0.6 m parapets — and without the split would walk off the
/// podium. At the nominal 50 ms tick the budget equals one radius, so the
/// common path pays no extra queries.
pub fn step_on_terrain(
    current: Vec3,
    target_x: f32,
    target_z: f32,
    max_travel: f32,
    surface_query: &crate::world::SurfaceQuery,
    collision_grid: &crate::world::CollisionGrid,
    max_step_height: f32,
    collision_radius: f32,
) -> TerrainStep {
    let collision_radius = collision_radius.max(0.05);
    let mut position = current;
    let mut remaining = max_travel.max(0.0);
    let mut moved = false;

    loop {
        let budget = remaining.min(collision_radius);
        let outcome = probe_terrain_step(
            position,
            target_x,
            target_z,
            budget,
            surface_query,
            collision_grid,
            max_step_height,
            collision_radius,
        );

        let next = match outcome {
            // Arriving ends the step wherever it happens.
            TerrainStep::Arrived(next) => return TerrainStep::Arrived(next),
            TerrainStep::Moved(next) => next,
            // Running into a wall (or off the map) part-way through is still
            // progress for the chunks already taken.
            blocked => {
                return if moved {
                    TerrainStep::Moved(position)
                } else {
                    blocked
                }
            }
        };

        let advanced = ((next.x - position.x).powi(2) + (next.z - position.z).powi(2)).sqrt();
        position = next;
        moved = true;
        remaining -= budget;

        // A blocker recovery walks *backwards*, further than the budget it was
        // given; continuing to probe forward would only shove the character
        // back into the blocker it was just pushed out of. A zero-length step
        // cannot make progress either, and would spin here forever.
        if advanced > budget + ARRIVAL_EPSILON || advanced <= ARRIVAL_EPSILON {
            return TerrainStep::Moved(position);
        }
        if remaining <= ARRIVAL_EPSILON {
            return TerrainStep::Moved(position);
        }
    }
}

/// One collision probe of at most [`MAX_PROBE_TRAVEL`], the body
/// [`step_on_terrain`] repeats.
fn probe_terrain_step(
    current: Vec3,
    target_x: f32,
    target_z: f32,
    max_travel: f32,
    surface_query: &crate::world::SurfaceQuery,
    collision_grid: &crate::world::CollisionGrid,
    max_step_height: f32,
    collision_radius: f32,
) -> TerrainStep {
    let target_contact = match surface_query.ground_at(target_x, target_z) {
        Some(contact) => contact,
        None => return TerrainStep::NoSurface,
    };

    let dx = target_x - current.x;
    let dz = target_z - current.z;
    let horizontal_distance = (dx * dx + dz * dz).sqrt();
    if horizontal_distance <= ARRIVAL_EPSILON {
        return TerrainStep::Arrived(Vec3::new(target_x, target_contact.height, target_z));
    }

    let travel = max_travel.max(0.0).min(horizontal_distance);
    if travel <= 0.0 {
        return TerrainStep::Blocked;
    }
    let nx = dx / horizontal_distance;
    let nz = dz / horizontal_distance;

    // Recover from a stale or externally authored position that is already
    // inside a blocker. This keeps the character visibly outside the wall
    // instead of merely preventing further movement while embedded.
    if collision_grid.is_blocked([current.x, current.y, current.z], collision_radius) {
        if let Some(position) = recover_from_blocker(
            current,
            nx,
            nz,
            surface_query,
            collision_grid,
            max_step_height,
            collision_radius,
        ) {
            return TerrainStep::Moved(position);
        }
        return TerrainStep::Blocked;
    }

    if let Some(position) = advance_to_contact(
        current,
        nx,
        nz,
        travel,
        surface_query,
        collision_grid,
        max_step_height,
        collision_radius,
    ) {
        return TerrainStep::Moved(position);
    }

    // Slide: give up the axis the wall refuses and keep the other. Each axis
    // gets only the *projection* of the step onto it, never the whole budget.
    // Spending the full travel sideways is what made a character shove into a
    // wall dead-on oscillate along it forever: with the target straight ahead
    // the sideways component is a rounding error, yet the slide moved a full
    // step in whichever direction that error happened to point, overshot, and
    // flipped the sign — every tick, for as long as the button was held. It is
    // also simply the right speed: approaching a wall at a shallow angle should
    // scrub off the blocked component, not convert it into free sideways
    // travel.
    let (first, second) = if nx.abs() >= nz.abs() {
        ((nx, 0.0), (0.0, nz))
    } else {
        ((0.0, nz), (nx, 0.0))
    };
    for (step_x, step_z) in [first, second] {
        if let Some(position) = advance_to_contact(
            current,
            step_x,
            step_z,
            travel * step_x.hypot(step_z),
            surface_query,
            collision_grid,
            max_step_height,
            collision_radius,
        ) {
            return TerrainStep::Moved(position);
        }
    }

    TerrainStep::Blocked
}

fn recover_from_blocker(
    current: Vec3,
    direction_x: f32,
    direction_z: f32,
    surface_query: &crate::world::SurfaceQuery,
    collision_grid: &crate::world::CollisionGrid,
    max_step_height: f32,
    collision_radius: f32,
) -> Option<Vec3> {
    let length = (direction_x * direction_x + direction_z * direction_z).sqrt();
    if length <= ARRIVAL_EPSILON {
        return None;
    }

    // Walk opposite the requested direction in small increments until the
    // circle footprint is clear. The bound is deliberately short: it repairs
    // penetration caused by a stale position without teleporting a player
    // across a room.
    for distance in (1..=16).map(|step| step as f32 * 0.1) {
        let x = current.x - direction_x / length * distance;
        let z = current.z - direction_z / length * distance;
        let contact = surface_query.ground_at_reachable(x, z, current.y, max_step_height)?;
        let candidate = Vec3::new(x, contact.height, z);
        if !collision_grid.is_blocked([candidate.x, candidate.y, candidate.z], collision_radius) {
            return Some(candidate);
        }
    }
    None
}

/// Bisection steps spent looking for the wall after a full step is refused.
///
/// Each halving doubles the precision of the contact point; six of them put a
/// server tick's 0.45 m budget inside 7 mm, well under what the render
/// smoothing can show.
const CONTACT_REFINEMENTS: u32 = 6;

/// Advances as far along `(direction_x, direction_z)` as the terrain and the
/// blockers allow, up to `travel`.
///
/// Taking the whole step or nothing is what made a character *judder* against a
/// wall. Client and server run this with different budgets — a render frame's
/// 0.15 m against a tick's 0.45 m — so each stopped at whatever multiple of its
/// own step size last fitted, up to 0.3 m apart. Reconciliation then pulled the
/// predicted position back off the wall, the next frame's step walked it
/// forward again, and the character shook between the two at frame rate.
///
/// Stopping at the contact point instead makes the answer independent of the
/// budget: both sides converge on the same place, the error goes to zero, and
/// there is nothing left for reconciliation to fight. It also just looks right
/// — the character stands against the wall rather than a step short of it.
///
/// Returns `None` when even a hair of movement is refused, so the caller can
/// fall through to its slide axes and ultimately report `Blocked` (which is
/// what keeps a character standing still out of its walk animation).
fn advance_to_contact(
    current: Vec3,
    direction_x: f32,
    direction_z: f32,
    travel: f32,
    surface_query: &crate::world::SurfaceQuery,
    collision_grid: &crate::world::CollisionGrid,
    max_step_height: f32,
    collision_radius: f32,
) -> Option<Vec3> {
    if let Some(position) = try_terrain_step(
        current,
        direction_x,
        direction_z,
        travel,
        surface_query,
        collision_grid,
        max_step_height,
        collision_radius,
    ) {
        return Some(position);
    }

    let mut admissible = 0.0;
    let mut refused = travel;
    let mut contact = None;
    for _ in 0..CONTACT_REFINEMENTS {
        let candidate_travel = 0.5 * (admissible + refused);
        match try_terrain_step(
            current,
            direction_x,
            direction_z,
            candidate_travel,
            surface_query,
            collision_grid,
            max_step_height,
            collision_radius,
        ) {
            Some(position) => {
                contact = Some(position);
                admissible = candidate_travel;
            }
            None => refused = candidate_travel,
        }
    }

    // Already touching: report the refusal so the caller tries to slide instead
    // of inching forward by a rounding error every tick.
    contact.filter(|_| admissible > ARRIVAL_EPSILON)
}

fn try_terrain_step(
    current: Vec3,
    direction_x: f32,
    direction_z: f32,
    travel: f32,
    surface_query: &crate::world::SurfaceQuery,
    collision_grid: &crate::world::CollisionGrid,
    max_step_height: f32,
    collision_radius: f32,
) -> Option<Vec3> {
    let length = (direction_x * direction_x + direction_z * direction_z).sqrt();
    if length <= ARRIVAL_EPSILON {
        return None;
    }

    let next_x = current.x + direction_x / length * travel;
    let next_z = current.z + direction_z / length * travel;
    let contact = surface_query.ground_at_reachable(next_x, next_z, current.y, max_step_height)?;
    let candidate = Vec3::new(next_x, contact.height, next_z);

    (!collision_grid.is_blocked([candidate.x, candidate.y, candidate.z], collision_radius))
        .then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{
        BlockerData, BlockerKind, CollisionGrid, CollisionShape, MapBounds, MapManifest,
        SurfaceBounds, SurfaceKind, SurfaceQuery, TransformData, WalkableSurface, WorldMetrics,
    };

    /// A flat world with a thin wall across `x = 0`, mirroring map_02's
    /// 0.6 m arena parapets — the geometry a long step used to jump over.
    fn walled_world(height: f32) -> (SurfaceQuery, CollisionGrid) {
        let (surfaces, _) = flat_world(height);
        let manifest = MapManifest {
            version: 2,
            map_id: "wall_test".to_string(),
            display_name: "Wall Test".to_string(),
            bounds: MapBounds {
                min_x: -10.0,
                max_x: 10.0,
                min_z: -10.0,
                max_z: 10.0,
            },
            terrain: Default::default(),
            props: vec![],
            world_metrics: Some(WorldMetrics::default()),
            surfaces: vec![],
            traversals: vec![],
            blockers: vec![BlockerData {
                id: "wall".to_string(),
                kind: BlockerKind::Box,
                object: None,
                transform: Some(TransformData::at(0.0, height + 1.0, 0.0)),
                shape: Some(CollisionShape::Box {
                    half_extents: [0.3, 1.0, 10.0],
                }),
                blocks_movement: true,
            }],
            test_route: vec![],
            test_checklist: vec![],
            mountain_switchback_test: None,
            distant_plateau_test: None,
        };
        (surfaces, CollisionGrid::build(&manifest))
    }

    fn flat_world(height: f32) -> (SurfaceQuery, CollisionGrid) {
        let manifest = MapManifest {
            version: 2,
            map_id: "movement_test".to_string(),
            display_name: "Movement Test".to_string(),
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
                id: "ground".to_string(),
                kind: SurfaceKind::Flat,
                object: None,
                bounds: Some(SurfaceBounds {
                    min_x: -10.0,
                    max_x: 10.0,
                    min_z: -10.0,
                    max_z: 10.0,
                }),
                height: Some(height),
                min_height: None,
                max_height: None,
                grid_size: None,
                size: None,
                purpose: None,
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
        (
            SurfaceQuery::from_manifest(&manifest),
            CollisionGrid::build(&manifest),
        )
    }

    #[test]
    fn moves_along_the_line_to_the_target() {
        let step = step_towards(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), 2.0, 0.5);
        assert_eq!(step, Step::Moving(Vec3::new(1.0, 0.0, 0.0)));
    }

    #[test]
    fn never_overshoots_the_target() {
        // One second at 100 u/s covers far more than the 3 units available.
        let step = step_towards(Vec3::ZERO, Vec3::new(3.0, 0.0, 0.0), 100.0, 1.0);
        assert_eq!(step, Step::Arrived(Vec3::new(3.0, 0.0, 0.0)));
    }

    #[test]
    fn arrives_when_already_on_target() {
        let target = Vec3::new(4.0, 1.0, -2.0);
        assert_eq!(
            step_towards(target, target, 5.0, 0.05),
            Step::Arrived(target)
        );
    }

    #[test]
    fn a_longer_step_covers_proportionally_more_ground() {
        // The property that matters for prediction: splitting a step in two
        // must land in the same place as taking it whole, so a client ticking
        // at frame rate agrees with a server ticking at 20 Hz.
        let target = Vec3::new(10.0, 0.0, 0.0);
        let Step::Moving(once) = step_towards(Vec3::ZERO, target, 2.0, 1.0) else {
            panic!("expected to still be moving");
        };
        let Step::Moving(half) = step_towards(Vec3::ZERO, target, 2.0, 0.5) else {
            panic!("expected to still be moving");
        };
        let Step::Moving(twice) = step_towards(half, target, 2.0, 0.5) else {
            panic!("expected to still be moving");
        };
        assert!((once - twice).length() < 1e-5, "{once} vs {twice}");
    }

    #[test]
    fn look_direction_ignores_height() {
        let dir = look_direction(Vec3::ZERO, Vec3::new(0.0, 99.0, 5.0)).expect("has a facing");
        assert_eq!(dir, Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn look_direction_is_none_when_only_height_differs() {
        assert_eq!(look_direction(Vec3::ZERO, Vec3::new(0.0, 5.0, 0.0)), None);
    }

    #[test]
    fn terrain_step_snaps_then_follows_authoritative_ground_height() {
        let (surfaces, collision) = flat_world(2.0);
        let mut current = Vec3::new(0.0, 0.0, 0.0);
        assert!(snap_to_ground(&mut current, &surfaces, 0.45));

        assert_eq!(
            step_on_terrain(current, 2.0, 0.0, 0.5, &surfaces, &collision, 0.45, 0.45),
            TerrainStep::Moved(Vec3::new(0.5, 2.0, 0.0))
        );
    }

    #[test]
    fn a_long_step_cannot_tunnel_through_a_thin_wall() {
        let (surfaces, collision) = walled_world(0.0);
        let start = Vec3::new(-2.0, 0.0, 0.0);

        // 4.5 m in one step: what a 9 u/s character asks for when the module
        // clamps a stalled tick to its 0.25 s ceiling, ten times the wall's
        // thickness. Before sub-stepping this landed on the far side.
        let step = step_on_terrain(start, 8.0, 0.0, 4.5, &surfaces, &collision, 0.45, 0.45);
        let TerrainStep::Moved(position) = step else {
            panic!("expected the character to advance up to the wall, got {step:?}");
        };
        assert!(
            position.x < -STEP_COLLISION_RADIUS,
            "character tunnelled through the wall at x = 0, ending at {position}"
        );
    }

    #[test]
    fn sub_stepping_still_covers_the_whole_budget_in_the_open() {
        // Splitting a step must not shorten it: the same 4.5 m of travel with
        // no wall in the way has to land exactly where one step would.
        let (surfaces, collision) = flat_world(0.0);
        let step = step_on_terrain(Vec3::ZERO, 9.0, 0.0, 4.5, &surfaces, &collision, 0.45, 0.45);
        let TerrainStep::Moved(position) = step else {
            panic!("expected to still be moving, got {step:?}");
        };
        assert!(
            (position.x - 4.5).abs() < 1e-3,
            "expected to cover the full 4.5 units, reached {position}"
        );
    }

    /// Walks into the wall until nothing more fits, and reports where it stopped.
    fn walk_into_the_wall(travel: f32, surfaces: &SurfaceQuery, collision: &CollisionGrid) -> Vec3 {
        let mut position = Vec3::new(-2.0, 0.0, 0.0);
        for _ in 0..200 {
            match step_on_terrain(position, 8.0, 0.0, travel, surfaces, collision, 0.45, 0.45) {
                TerrainStep::Moved(next) | TerrainStep::Arrived(next) => position = next,
                _ => break,
            }
        }
        position
    }

    #[test]
    fn different_step_budgets_stop_at_the_same_contact_point() {
        // The judder the players saw: the client steps once per rendered frame
        // and the server once per tick, so without contact seeking each stopped
        // at whatever multiple of its own budget last fitted — up to 0.3 apart.
        // Reconciliation then dragged the predicted character back and forth
        // between the two every frame.
        let (surfaces, collision) = walled_world(0.0);

        let client = walk_into_the_wall(0.15, &surfaces, &collision);
        let server = walk_into_the_wall(0.45, &surfaces, &collision);

        assert!(
            (client.x - server.x).abs() < 0.01,
            "client stopped at {client}, server at {server}"
        );
        // Both against the wall's blocked band (half-extent 0.3 + radius 0.45),
        // not a step short of it.
        assert!(
            (client.x + 0.75).abs() < 0.01,
            "expected to stand against the wall, stopped at {client}"
        );
    }

    #[test]
    fn walking_straight_into_a_wall_does_not_slide_sideways() {
        // The slide axes exist to round a wall that is in the way, not to shove
        // a character along one it is facing head on. Spending the whole step
        // budget on the leftover axis used to send it skating left and right.
        let (surfaces, collision) = walled_world(0.0);
        let stopped = walk_into_the_wall(0.15, &surfaces, &collision);
        assert!(
            stopped.z.abs() < 0.01,
            "drifted sideways along the wall to {stopped}"
        );
    }

    #[test]
    fn terrain_step_rejects_targets_outside_walkable_surfaces() {
        let (surfaces, collision) = flat_world(0.0);
        assert_eq!(
            step_on_terrain(
                Vec3::ZERO,
                20.0,
                0.0,
                1.0,
                &surfaces,
                &collision,
                0.45,
                0.45
            ),
            TerrainStep::NoSurface
        );
    }

    #[test]
    fn cast_modes_do_not_block_movement_intent() {
        assert!(movement_intent_allowed(MovementLock::CastTime, false));
        assert!(movement_intent_allowed(MovementLock::None, false));
        assert!(movement_intent_allowed(MovementLock::Channel, false));
    }

    #[test]
    fn stun_blocks_even_when_not_casting() {
        assert!(!movement_intent_allowed(MovementLock::None, true));
        assert!(!movement_intent_allowed(MovementLock::Channel, true));
    }

    #[test]
    fn after_cast_a_stale_click_is_not_resumed() {
        let click = Some(Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(
            predicted_move_dest(click, None, MovementLock::None, false, false, false),
            None
        );
    }

    #[test]
    fn held_right_mouse_still_steers_when_unlocked() {
        let click = Some(Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(
            predicted_move_dest(click, None, MovementLock::None, true, false, false),
            click
        );
    }

    #[test]
    fn unlocked_follows_the_server_dest() {
        let server = Some(Vec3::new(3.0, 0.0, 1.0));
        assert_eq!(
            predicted_move_dest(None, server, MovementLock::None, false, false, false),
            server
        );
    }

    #[test]
    fn stun_drops_a_live_server_dest() {
        let click = Some(Vec3::new(10.0, 0.0, 0.0));
        let server = Some(Vec3::new(4.0, 0.0, 1.0));
        assert_eq!(
            predicted_move_dest(click, server, MovementLock::None, true, true, false),
            None
        );
    }

    #[test]
    fn cast_time_click_can_walk_to_interrupt() {
        let click = Some(Vec3::new(10.0, 0.0, 0.0));
        let server = Some(Vec3::new(4.0, 0.0, 1.0));
        assert_eq!(
            predicted_move_dest(click, server, MovementLock::CastTime, true, false, true),
            click
        );
    }

    #[test]
    fn planted_wind_up_ignores_leftover_dest() {
        let server = Some(Vec3::new(4.0, 0.0, 1.0));
        assert_eq!(
            predicted_move_dest(None, server, MovementLock::CastTime, false, false, true),
            None
        );
        assert_eq!(
            predicted_move_dest(None, server, MovementLock::Channel, false, false, true),
            None
        );
    }

    #[test]
    fn planted_wind_up_does_not_fall_back_to_leftover_dest_on_click() {
        let click = Some(Vec3::new(10.0, 0.0, 0.0));
        let server = Some(Vec3::new(4.0, 0.0, 1.0));
        assert_eq!(
            predicted_move_dest(None, server, MovementLock::Channel, true, false, true),
            None
        );
        assert_eq!(
            predicted_move_dest(click, server, MovementLock::Channel, true, false, true),
            click
        );
    }

    #[test]
    fn channel_lock_still_follows_the_server_dest() {
        let server = Some(Vec3::new(4.0, 0.0, 1.0));
        assert_eq!(
            predicted_move_dest(None, server, MovementLock::Channel, false, false, false),
            server
        );
    }

    #[test]
    fn crowd_control_drops_dest_even_when_unlocked() {
        let click = Some(Vec3::new(10.0, 0.0, 0.0));
        let server = Some(Vec3::new(4.0, 0.0, 1.0));
        assert_eq!(
            predicted_move_dest(click, server, MovementLock::None, true, true, false),
            None
        );
        assert_eq!(
            predicted_move_dest(click, server, MovementLock::Channel, true, true, false),
            None
        );
    }

    #[test]
    fn idle_prediction_lead_is_left_alone() {
        let predicted = Vec3::new(0.45, 0.0, 0.0);
        let authoritative = Vec3::ZERO;
        assert_eq!(
            reconcile_offset(predicted, authoritative, None, 9.0),
            Reconcile::Leave
        );
    }

    #[test]
    fn walking_prediction_is_not_pulled_toward_a_stale_pose() {
        let predicted = Vec3::new(2.0, 0.0, 0.0);
        let authoritative = Vec3::ZERO;
        let dest = Some(Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(
            reconcile_offset(predicted, authoritative, dest, 9.0),
            Reconcile::Leave
        );
    }

    #[test]
    fn idle_error_beyond_slack_eases() {
        let predicted = Vec3::new(2.0, 0.0, 0.0);
        assert_eq!(
            reconcile_offset(predicted, Vec3::ZERO, None, 9.0),
            Reconcile::Ease
        );
    }

    #[test]
    fn large_desync_snaps_even_while_walking() {
        let predicted = Vec3::new(PREDICTION_SNAP_DISTANCE + 0.1, 0.0, 0.0);
        let dest = Some(Vec3::new(20.0, 0.0, 0.0));
        assert_eq!(
            reconcile_offset(predicted, Vec3::ZERO, dest, 9.0),
            Reconcile::Snap
        );
    }

    #[test]
    fn zero_speed_has_no_idle_slack() {
        let predicted = Vec3::new(0.05, 0.0, 0.0);
        assert_eq!(
            reconcile_offset(predicted, Vec3::ZERO, None, 0.0),
            Reconcile::Ease
        );
    }

    #[test]
    fn slack_is_travel_time_not_a_fixed_distance() {
        // 0.5 m at 9 u/s is inside slack; the same gap at 2 u/s is not.
        let predicted = Vec3::new(0.5, 0.0, 0.0);
        assert_eq!(
            reconcile_offset(predicted, Vec3::ZERO, None, 9.0),
            Reconcile::Leave
        );
        assert_eq!(
            reconcile_offset(predicted, Vec3::ZERO, None, 2.0),
            Reconcile::Ease
        );
    }

    #[test]
    fn instant_while_moving_does_not_steal_walk_facing() {
        assert!(!should_face_cast_target(true, MovementLock::None));
    }

    #[test]
    fn rooted_or_standing_casts_still_face_the_target() {
        assert!(should_face_cast_target(true, MovementLock::CastTime));
        assert!(should_face_cast_target(true, MovementLock::Channel));
        assert!(should_face_cast_target(false, MovementLock::None));
    }
}
