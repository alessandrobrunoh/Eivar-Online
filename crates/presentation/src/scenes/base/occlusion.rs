//! Camera occlusion handling for the isometric game camera.
//!
//! Anything from the map scene that comes between the camera and the locally
//! controlled player fades to [`OCCLUDED_ALPHA`] instead of vanishing, so the
//! player keeps reading what is actually in front of them.
//!
//! **What can fade.** Every mesh node instantiated from the map GLB, except:
//! - reserved world-format nodes (`WALKABLE_*`, `TRAVERSAL_*`, `__bevymmo`) —
//!   the terrain must never go translucent;
//! - nodes whose name ends in `_Base` — the trunk/floor half of a prop, which
//!   stays put while its `_Top` fades.
//!
//! Visible `BLOCKING_*` nodes **do** fade: blocker meshes are the arena's
//! walls, pillars and cover, and an opaque wall between the camera and the
//! player is exactly what the fade exists for. When blockers were purely
//! invisible collision volumes excluding them was harmless; on the arena map
//! it made every wall read as solid.
//!
//! Scoping by scene ancestry rather than by name is what keeps the player's own
//! model and the generated terrain mesh out of the set: both are spawned
//! outside the map scene root.
//!
//! **Per-entity materials.** glTF materials are shared by every instance that
//! uses them — map_02's 232 rocks share a handful — so fading one would fade
//! them all. Each occluder gets a clone of its material when tagged and only
//! ever writes to that clone.
//!
//! The work is split in two systems on purpose: [`update_camera_occlusion`]
//! decides *whether* each occluder is in the way (pure geometry, no timing) and
//! [`animate_occluder_fade`] walks the alpha toward that decision.

use bevy::camera::primitives::Aabb;
use bevy::prelude::*;
use bevy::reflect::Reflect;
use bevymmo_client::local_player::LocalPlayer;

use bevymmo_network::network::protocol::Position;

use super::systems::GameCamera;
use crate::world::{MapPropVisual, MapSceneVisual};

/// Suffix marking the half of a prop that must never fade: trunks, floors,
/// anything that would look wrong left floating.
pub const OCCLUDER_BASE_SUFFIX: &str = "_Base";

/// Node-name prefixes owned by the world format that must never fade: the
/// terrain surface and metadata scaffolding. `BLOCKING_*` is deliberately
/// absent — visible blocker meshes (arena walls, cover, pillars) are props
/// like any other and ghost when they block the view.
const RESERVED_NODE_PREFIXES: [&str; 3] = ["WALKABLE_", "TRAVERSAL_", "__bevymmo"];

/// Alpha an occluder fades down to.
///
/// Not zero on purpose: a ghosted rock still tells the player a rock is there,
/// which a vanished one does not. Kept low because occluders stack — a copse
/// puts four or five faded canopies on the same sight line, and at 0.25 each
/// they still add up to an opaque wall.
pub const OCCLUDED_ALPHA: f32 = 0.10;

/// Alpha units per second while fading — the full range in about 0.15 s.
const FADE_RATE_PER_SEC: f32 = 5.0;

/// World-space slack added around an occluder's box before testing it.
///
/// The test uses a line but the character is roughly 0.7 m wide, so without
/// margin a box edge grazing that line stays opaque while still clipping a
/// shoulder.
const OCCLUSION_MARGIN_M: f32 = 0.6;

/// Height above the player's feet that the occlusion line aims at.
///
/// `Position` sits on the ground; aiming at the torso catches a canopy directly
/// overhead and ignores a low rock the character's head clears.
const PLAYER_FOCUS_HEIGHT_M: f32 = 1.2;

/// Marks a map-scene node as something that may fade when it blocks the view.
#[derive(Component, Reflect, Clone, Copy, Default)]
#[reflect(Component)]
pub struct Occludable;

/// Marks a node [`tag_occludables`] already examined and rejected, so the scan
/// does not reconsider it every frame.
#[derive(Component, Reflect, Clone, Copy, Default)]
#[reflect(Component)]
pub struct NotOccludable;

/// Per-occluder fade state, plus the private material clone it writes to.
#[derive(Component, Reflect, Clone)]
#[reflect(Component)]
pub struct OccluderFade {
    /// Shared glTF material. Restored when the node is fully opaque so
    /// identical rocks can instance again.
    pub shared: Handle<StandardMaterial>,
    /// Material currently written: `shared` while opaque, a private clone
    /// while fading.
    pub material: Handle<StandardMaterial>,
    /// Alpha the occluder is heading toward: [`OCCLUDED_ALPHA`] or its
    /// authored opacity.
    pub target: f32,
    /// Alpha currently written to [`Self::material`].
    pub current: f32,
    /// The material's authored alpha, restored when nothing is in the way.
    pub opaque_alpha: f32,
}

/// Whether a glTF node name denotes a prop that may fade.
fn is_occludable_name(name: &str) -> bool {
    if RESERVED_NODE_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        return false;
    }
    !strip_duplicate_suffix(name).ends_with(OCCLUDER_BASE_SUFFIX)
}

/// Drops a trailing Blender duplicate index (`.001`), leaving other dotted
/// tails alone.
///
/// Blender appends `.001`, `.002`, … to every duplicate and the suffix survives
/// the glTF export, so suffix tests have to strip it first. Before this was
/// handled, a plain `ends_with("_Top")` matched only 5 of map_02's 73 canopies.
fn strip_duplicate_suffix(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => name,
    }
}

type UnexaminedNodes<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Name,
        &'static MeshMaterial3d<StandardMaterial>,
    ),
    (With<Mesh3d>, Without<Occludable>, Without<NotOccludable>),
>;

/// Tags map-scene props as [`Occludable`] and gives each one a private material.
///
/// Scans unexamined mesh nodes rather than reacting to `Added<Name>`: scene
/// instantiation sets a node's name and its parent in the same frame but not
/// necessarily before this system observes it, and one missed `Added` would
/// leave a prop permanently opaque. Every entity is marked one way or the
/// other, so the scan settles to nothing once the scene has loaded.
///
/// Two kinds of map entity qualify:
/// - mesh nodes that are **descendants** of a [`MapSceneVisual`] root (GLB
///   scenes, either the whole-map scene or per-prop GLB scenes);
/// - entities that carry [`MapPropVisual`] directly (placeholder cuboids
///   spawned for props that have no authored GLB asset — these are root
///   entities themselves, not children of anything, so the ancestry walk
///   alone would never find them).
pub fn tag_occludables(
    mut commands: Commands,
    unexamined: UnexaminedNodes,
    parents: Query<&ChildOf>,
    map_roots: Query<Entity, With<MapSceneVisual>>,
    prop_visuals: Query<Entity, With<MapPropVisual>>,
    materials: Res<Assets<StandardMaterial>>,
) {
    for (entity, name, material) in &unexamined {
        let belongs_to_map = prop_visuals.contains(entity)
            || map_roots
                .iter()
                .any(|root| is_descendant_of(entity, root, &parents));

        if !belongs_to_map || !is_occludable_name(name.as_str()) {
            if !belongs_to_map {
                debug!(
                    "Occlusion: {name:?} ({entity:?}) does NOT belong to map — marking NotOccludable"
                );
            }
            commands.entity(entity).insert(NotOccludable);
            continue;
        }

        let Some(source) = materials.get(&material.0).cloned() else {
            // Asset still streaming in: leave the entity unexamined so a later
            // frame picks it up.
            continue;
        };

        let opaque_alpha = source.base_color.alpha();
        debug!("Occlusion: tagged {name:?} ({entity:?}) as Occludable (alpha={opaque_alpha})");

        commands.entity(entity).insert((
            Occludable,
            OccluderFade {
                shared: material.0.clone(),
                material: material.0.clone(),
                target: opaque_alpha,
                current: opaque_alpha,
                opaque_alpha,
            },
        ));
    }
}

fn is_descendant_of(entity: Entity, root: Entity, parents: &Query<&ChildOf>) -> bool {
    let mut current = entity;
    while let Ok(parent) = parents.get(current) {
        if parent.0 == root {
            return true;
        }
        current = parent.0;
    }
    false
}

/// Decides, per occluder, whether it currently stands between camera and player.
///
/// Tests the camera→player line against the occluder's box in **local space**:
/// the line is transformed by the inverse of the node's global transform and
/// clipped against the local `Aabb` with a slab test. The bounding-sphere
/// approximation this replaced was fine for a 3 m canopy but wildly over-eager
/// for map_02's boulders, whose 20×7×20 half-extents give a 29 m bounding
/// radius — they would have ghosted from thirty metres off to the side.
pub fn update_camera_occlusion(
    player_query: Query<&Position, With<LocalPlayer>>,
    camera_query: Query<&Transform, With<GameCamera>>,
    mut occluders: Query<(&GlobalTransform, Option<&Aabb>, &mut OccluderFade), With<Occludable>>,
) {
    let Ok(player_position) = player_query.single() else {
        return;
    };
    let Ok(camera_transform) = camera_query.single() else {
        return;
    };

    let camera_pos = camera_transform.translation;
    let focus_pos = player_position.0 + Vec3::Y * PLAYER_FOCUS_HEIGHT_M;
    if camera_pos.distance_squared(focus_pos) < f32::EPSILON {
        return;
    }

    for (transform, aabb, mut fade) in occluders.iter_mut() {
        let blocking = match aabb {
            Some(aabb) => {
                // `segment_hits_aabb` opens with a matrix inverse, which is the
                // expensive part and was being paid for every tagged prop on
                // the map every frame. Almost none of them are anywhere near
                // the camera-to-player segment, and a bounding-sphere reject
                // costs a few multiplies.
                near_segment(camera_pos, focus_pos, transform, aabb)
                    && segment_hits_aabb(camera_pos, focus_pos, transform, aabb)
            }
            // Bounds not computed yet: assume clear rather than ghosting a prop
            // that may be nowhere near the camera.
            None => false,
        };

        fade.target = if blocking {
            OCCLUDED_ALPHA
        } else {
            fade.opaque_alpha
        };
    }
}

/// Conservative bounding-sphere reject for [`segment_hits_aabb`].
///
/// Never rejects something the slab test would have accepted: the sphere is
/// the AABB's circumscribed sphere, scaled by the node's largest world axis
/// and widened by [`OCCLUSION_MARGIN_M`] the same way the slab test widens the
/// box. A false accept just means paying for the exact test, which is what
/// used to happen unconditionally.
fn near_segment(start: Vec3, end: Vec3, transform: &GlobalTransform, aabb: &Aabb) -> bool {
    // Must match what `segment_hits_aabb` actually tests against. That widens
    // each *local* half-extent by `OCCLUSION_MARGIN_M / scale`, so in world
    // space the box is `half_extents * scale + margin` per axis and the
    // circumscribed sphere is the length of that vector. Adding the margin to
    // the radius instead — the obvious-looking version — gives a sphere
    // smaller than the box it is supposed to contain, and rejects real hits on
    // the box's diagonals.
    let scale = transform.compute_transform().scale.abs();
    let world_half = Vec3::from(aabb.half_extents) * scale + Vec3::splat(OCCLUSION_MARGIN_M);
    let radius = world_half.length();
    let center = transform.transform_point(Vec3::from(aabb.center));

    let segment = end - start;
    let length_squared = segment.length_squared();
    // Degenerate segment: fall back to a plain distance check.
    if length_squared <= f32::EPSILON {
        return center.distance_squared(start) <= radius * radius;
    }
    // Closest point on the segment to the sphere centre, clamped to the ends.
    let t = ((center - start).dot(segment) / length_squared).clamp(0.0, 1.0);
    let closest = start + segment * t;
    closest.distance_squared(center) <= radius * radius
}

/// Whether the segment `start`→`end` intersects an entity's local-space box.
///
/// Standard slab test, run in local space so a rotated prop is tested against
/// its true box instead of a padded world-space one. [`OCCLUSION_MARGIN_M`] is
/// converted per-axis by the node's world scale before being applied.
fn segment_hits_aabb(start: Vec3, end: Vec3, transform: &GlobalTransform, aabb: &Aabb) -> bool {
    let inverse = transform.affine().inverse();
    let origin = inverse.transform_point3(start);
    let direction = inverse.transform_point3(end) - origin;

    let scale = transform.compute_transform().scale.abs();
    let margin = Vec3::new(
        axis_margin(scale.x),
        axis_margin(scale.y),
        axis_margin(scale.z),
    );

    let center = Vec3::from(aabb.center);
    let half = Vec3::from(aabb.half_extents) + margin;
    let min = center - half;
    let max = center + half;

    let mut enter = 0.0_f32;
    let mut exit = 1.0_f32;
    for axis in 0..3 {
        if direction[axis].abs() < 1e-6 {
            // Parallel to this slab: either always within it, or never.
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return false;
            }
            continue;
        }
        let inv_dir = 1.0 / direction[axis];
        let mut near = (min[axis] - origin[axis]) * inv_dir;
        let mut far = (max[axis] - origin[axis]) * inv_dir;
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        enter = enter.max(near);
        exit = exit.min(far);
        if enter > exit {
            return false;
        }
    }
    true
}

fn axis_margin(scale: f32) -> f32 {
    if scale > 1e-6 {
        OCCLUSION_MARGIN_M / scale
    } else {
        0.0
    }
}

/// Walks each occluder's alpha toward its target and writes it to the material.
///
/// `alpha_mode` flips to `Blend` only while the material is actually
/// translucent: leaving every prop in the transparency pass would cost sorting
/// work for the ~300 props that are opaque on any given frame.
pub fn animate_occluder_fade(
    mut commands: Commands,
    time: Res<Time>,
    mut occluders: Query<(Entity, &MeshMaterial3d<StandardMaterial>, &mut OccluderFade)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let step = FADE_RATE_PER_SEC * time.delta_secs();
    if step <= 0.0 {
        return;
    }

    for (entity, handle, mut fade) in occluders.iter_mut() {
        if fade.current == fade.target {
            continue;
        }

        let fading = fade.target < fade.opaque_alpha - f32::EPSILON;
        if fading && fade.material == fade.shared {
            // If the source has not streamed in yet there is nothing to clone,
            // and falling through would write the fade onto `shared` itself —
            // glTF materials are shared between instances, so every rock cut
            // from the same material would ghost along with this one. Skip the
            // frame instead; the asset lands and the fade starts a frame later.
            let Some(source) = materials.get(&fade.shared).cloned() else {
                continue;
            };
            let clone = materials.add(source);
            fade.material = clone.clone();
            commands.entity(entity).insert(MeshMaterial3d(clone));
        }

        let delta = (fade.target - fade.current).clamp(-step, step);
        let next = fade.current + delta;
        fade.current = if (fade.target - next).abs() < step * 0.5 {
            fade.target
        } else {
            next
        };
        let current = fade.current;

        let Some(mut material) = materials.get_mut(&fade.material) else {
            continue;
        };
        material.base_color.set_alpha(current);
        material.alpha_mode = if current >= fade.opaque_alpha {
            AlphaMode::Opaque
        } else {
            AlphaMode::Blend
        };

        if current >= fade.opaque_alpha && fade.material != fade.shared {
            fade.material = fade.shared.clone();
            if handle.0 != fade.shared {
                commands
                    .entity(entity)
                    .insert(MeshMaterial3d(fade.shared.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::math::Vec3A;

    fn test_world() -> World {
        let mut world = World::new();
        world.init_resource::<Assets<StandardMaterial>>();
        world
    }

    /// Spawns an occluder at `translation` with unit scale and the given
    /// local-space half extents.
    fn occluder_with_extents(world: &mut World, translation: Vec3, half: Vec3A) -> Entity {
        let material = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        world
            .spawn((
                Transform::from_translation(translation),
                GlobalTransform::from_translation(translation),
                Visibility::Inherited,
                MeshMaterial3d(material.clone()),
                Occludable,
                OccluderFade {
                    shared: material.clone(),
                    material: material.clone(),
                    target: 1.0,
                    current: 1.0,
                    opaque_alpha: 1.0,
                },
                Aabb {
                    center: Vec3A::ZERO,
                    half_extents: half,
                },
            ))
            .id()
    }

    fn occluder_entity(world: &mut World, translation: Vec3, half_extent: f32) -> Entity {
        occluder_with_extents(world, translation, Vec3A::splat(half_extent))
    }

    fn target_of(world: &World, entity: Entity) -> f32 {
        world
            .entity(entity)
            .get::<OccluderFade>()
            .expect("occluder has fade state")
            .target
    }

    /// Spawns a `GameCamera` at `camera` and a `LocalPlayer` player at `player`,
    /// then runs [`update_camera_occlusion`] once.
    fn run_occlusion(world: &mut World, camera: Vec3, player: Vec3) {
        world.spawn((
            GameCamera,
            Transform::from_translation(camera),
            Camera3d::default(),
        ));
        world.spawn((LocalPlayer, Position(player)));
        world
            .run_system_once(update_camera_occlusion)
            .expect("system runs");
    }

    #[test]
    fn occluder_between_camera_and_player_fades() {
        let mut world = test_world();
        let entity = occluder_entity(&mut world, Vec3::ZERO, 5.0);

        run_occlusion(
            &mut world,
            Vec3::new(0.0, 0.0, -10.0),
            Vec3::new(0.0, 0.0, 10.0),
        );

        assert_eq!(
            target_of(&world, entity),
            OCCLUDED_ALPHA,
            "an occluder on the segment must fade"
        );
    }

    #[test]
    fn occluder_off_axis_stays_opaque() {
        let mut world = test_world();
        let entity = occluder_entity(&mut world, Vec3::new(50.0, 0.0, 0.0), 1.0);

        run_occlusion(
            &mut world,
            Vec3::new(0.0, 0.0, -10.0),
            Vec3::new(0.0, 0.0, 10.0),
        );

        assert_eq!(target_of(&world, entity), 1.0);
    }

    #[test]
    fn occluder_behind_player_stays_opaque() {
        let mut world = test_world();
        // Past the player along the same axis: the segment stops short of it.
        let entity = occluder_entity(&mut world, Vec3::new(0.0, 0.0, 20.0), 1.0);

        run_occlusion(
            &mut world,
            Vec3::new(0.0, 0.0, -10.0),
            Vec3::new(0.0, 0.0, 10.0),
        );

        assert_eq!(target_of(&world, entity), 1.0);
    }

    #[test]
    fn canopy_over_the_player_fades() {
        let mut world = test_world();
        // Canopy centred 4 m above the player, as a tree is placed.
        let entity = occluder_entity(&mut world, Vec3::new(0.0, 4.0, 0.0), 5.0);

        run_occlusion(&mut world, Vec3::new(0.0, 25.0, 25.0), Vec3::ZERO);

        assert_eq!(
            target_of(&world, entity),
            OCCLUDED_ALPHA,
            "a canopy over the player must fade so the character stays readable"
        );
    }

    /// A boulder the size of map_02's `Rock_Large` must not ghost from far
    /// away. Its bounding sphere is ~29 m — the radius the old sphere test used
    /// as its hit threshold — while the box itself is only 20 m wide.
    #[test]
    fn large_boulder_clear_of_the_line_stays_opaque() {
        let mut world = test_world();
        let entity = occluder_with_extents(
            &mut world,
            Vec3::new(28.0, 7.0, 0.0),
            Vec3A::new(20.0, 7.0, 20.0),
        );

        run_occlusion(&mut world, Vec3::new(0.0, 45.0, -45.0), Vec3::ZERO);

        assert_eq!(
            target_of(&world, entity),
            1.0,
            "a boulder whose box clears the sight line must stay opaque"
        );
    }

    #[test]
    fn occlusion_noops_without_controlled_player() {
        let mut world = test_world();
        world.spawn((GameCamera, Transform::from_translation(Vec3::ZERO)));
        let entity = occluder_entity(&mut world, Vec3::ZERO, 1.0);

        world
            .run_system_once(update_camera_occlusion)
            .expect("system runs");

        assert_eq!(target_of(&world, entity), 1.0);
    }

    /// The cull in `update_camera_occlusion` is only sound if it never
    /// rejects a box the exact test would have accepted. Swept over a grid of
    /// positions rather than a couple of hand-picked ones, because a false
    /// reject shows up as a prop that stops fading at one specific angle.
    #[test]
    fn the_bounding_sphere_never_rejects_what_the_slab_test_accepts() {
        let camera = Vec3::new(0.0, 8.0, 10.0);
        let focus = Vec3::new(0.0, 1.2, 0.0);

        let mut checked = 0;
        let mut accepted = 0;
        for x in -12..=12 {
            for z in -12..=12 {
                for y in 0..=6 {
                    let translation =
                        Vec3::new(x as f32 * 0.75, y as f32 * 0.75, z as f32 * 0.75);
                    let transform = GlobalTransform::from(Transform::from_translation(translation));
                    let aabb = Aabb {
                        center: Vec3A::ZERO,
                        half_extents: Vec3A::new(0.5, 1.5, 0.5),
                    };
                    checked += 1;
                    let exact = segment_hits_aabb(camera, focus, &transform, &aabb);
                    if exact {
                        accepted += 1;
                        assert!(
                            near_segment(camera, focus, &transform, &aabb),
                            "cull rejected a box the slab test accepts at {translation:?}"
                        );
                    }
                }
            }
        }
        assert!(checked > 1000, "the sweep should be broad");
        assert!(accepted > 0, "the sweep must contain real hits to be meaningful");
    }

    /// ...and it has to actually reject something, or it is not a cull.
    #[test]
    fn the_bounding_sphere_rejects_distant_boxes() {
        let camera = Vec3::new(0.0, 8.0, 10.0);
        let focus = Vec3::new(0.0, 1.2, 0.0);
        let transform = GlobalTransform::from(Transform::from_translation(Vec3::new(
            120.0, 0.0, -95.0,
        )));
        let aabb = Aabb {
            center: Vec3A::ZERO,
            half_extents: Vec3A::new(0.5, 1.5, 0.5),
        };
        assert!(!near_segment(camera, focus, &transform, &aabb));
    }

    #[test]
    fn occludable_names_exclude_bases_and_reserved_nodes() {
        assert!(is_occludable_name("Template_Rock_Large_Rock_Large.020"));
        assert!(is_occludable_name(
            "Template_Tree_Pine_Large_Tree_Pine_Large_Top.018"
        ));
        assert!(!is_occludable_name(
            "Template_Tree_Pine_Large_Tree_Pine_Large_Base.018"
        ));
        assert!(!is_occludable_name("WALKABLE_map_02"));
        // Visible blocker walls (arena walls, cover) fade; only the invisible
        // world-format scaffolding above stays reserved.
        assert!(is_occludable_name("BLOCKING_Arena_Wall_North"));
        assert!(!is_occludable_name("__bevymmo_map_meta"));
        // A dotted tail that is not a duplicate index must not be stripped.
        assert!(is_occludable_name("Tree_Base.old"));
    }

    #[test]
    fn fade_walks_to_the_target_and_settles_exactly_on_it() {
        let mut world = test_world();
        let entity = occluder_entity(&mut world, Vec3::ZERO, 1.0);
        world
            .entity_mut(entity)
            .get_mut::<OccluderFade>()
            .expect("fade state")
            .target = OCCLUDED_ALPHA;

        let mut time = Time::<()>::default();
        time.advance_by(std::time::Duration::from_millis(50));
        world.insert_resource(time);

        for _ in 0..40 {
            world
                .run_system_once(animate_occluder_fade)
                .expect("system runs");
        }

        let fade = world.entity(entity).get::<OccluderFade>().expect("fade");
        assert_eq!(
            fade.current, OCCLUDED_ALPHA,
            "the fade must land exactly on the target, not creep near it"
        );
    }
}
