use bevy::prelude::*;

use crate::assets::{BossDragonAssets, CreatureAssets, PlayerAssets, WeaponAssets};
use crate::game_state::Screen;
use bevymmo_gameplay::entity::boss::components::Boss;
use bevymmo_gameplay::entity::components::EntityKind;
use bevymmo_client::loot::LootBagMarker;
use bevymmo_gameplay::gathering::Harvestable;
use bevymmo_gameplay::items::components::Equipment;
use bevymmo_gameplay::placeables::{AssetHint, KindId, PlaceableRegistry};
use bevymmo_network::network::protocol::*;
use bevymmo_network::world_components::AoeZone;
use std::collections::HashMap;

#[derive(Resource)]
pub struct RendererAssets {
    projectile_mesh: Handle<Mesh>,
    projectile_material: Handle<StandardMaterial>,
    fallback_mesh_small: Handle<Mesh>,
    color_materials: HashMap<[u32; 3], Handle<StandardMaterial>>,
}

impl RendererAssets {
    fn get_or_create_color_material(
        &mut self,
        materials: &mut Assets<StandardMaterial>,
        color: Color,
    ) -> Handle<StandardMaterial> {
        let [r, g, b, _] = color.to_srgba().to_f32_array();
        let key = [r.to_bits(), g.to_bits(), b.to_bits()];
        self.color_materials
            .entry(key)
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: color,
                    ..default()
                })
            })
            .clone()
    }
}

fn init_renderer_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(RendererAssets {
        projectile_mesh: meshes.add(Cuboid::new(0.45, 0.45, 0.45)),
        projectile_material: materials.add(StandardMaterial {
            base_color: Color::WHITE,
            emissive: LinearRgba::rgb(0.1, 0.7, 1.0),
            ..default()
        }),
        fallback_mesh_small: meshes.add(Cuboid::new(2.0, 2.0, 2.0)),
        color_materials: HashMap::new(),
    });
}

#[derive(Component)]
pub struct RenderedEntity;

/// Ordering contract for everything that turns simulated state into pixels.
///
/// The three stages read each other's output within a single frame, and until
/// they were ordered explicitly Bevy was free to interleave them: the camera
/// followed last frame's player transform, and the screen-space UI projected
/// through last frame's camera. Both errors are proportional to how fast the
/// camera is moving, which is why they only showed up while walking — the
/// character glided but the world and the nameplates shook around it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderSync {
    /// Smooths every replicated `Position` into the entity's `Transform`.
    Transforms,
    /// Places the game camera on the smoothed player transform.
    Camera,
    /// Projects world anchors into viewport space for the screen-space UI.
    Project,
}

/// Rebuilds a camera's global transform from this frame's local `Transform`.
///
/// `GlobalTransform` is only recomputed in `PostUpdate`, so any system reading
/// it during `Update` sees where the camera was *last* frame. For a camera that
/// is glued to the player that is a full frame of player movement, and every
/// screen-space overlay projected through it swims against the character it is
/// anchored to.
///
/// The game camera is a direct child of `GameSceneRoot`, whose transform is the
/// identity (asserted by `scene_root_transform_is_identity`), so its local
/// transform *is* its global transform and this conversion is exact.
pub fn camera_view(transform: &Transform) -> GlobalTransform {
    GlobalTransform::from(*transform)
}

/// Marks the scene root of a player model whose imported root node needs to be
/// anchored to the replicated gameplay position.
#[derive(Component)]
struct PlayerModelRoot;

/// Marks an entity whose instantiated glTF scene still carries the layout
/// offset the asset was exported with, and whose own `Transform` is therefore
/// the position the model should be drawn at. See [`anchor_scene_layout`].
///
/// Deliberately not put on the map's own GLB (`MapSceneVisual` in
/// [`crate::world`]): that scene's root nodes are the map, and their offsets
/// *are* where the cliffs and ramps belong.
#[derive(Component)]
pub(crate) struct AnchorSceneLayout;

/// Set once an [`AnchorSceneLayout`] scene has been put back on its position,
/// so the walk runs once per model instead of every frame.
#[derive(Component)]
struct SceneLayoutAnchored;

#[derive(Component)]
struct EquippedWeaponVisual;

fn sword_hold_transform() -> Transform {
    Transform {
        translation: Vec3::new(0.38, 0.9, 0.18),
        rotation: Quat::from_rotation_x(-1.05) * Quat::from_rotation_y(0.35),
        scale: Vec3::splat(0.35),
    }
}

/// Prevents re-normalizing the imported node once its scene is instantiated.
#[derive(Component)]
struct PlayerModelAnchored;

// The current player.glb is authored in game-world units (unlike the old
// oversized animated asset), so it must not be scaled down by 0.035.
const PLAYER_SCENE_SCALE: f32 = 1.0;
const BOSS_DRAGON_SCENE_SCALE: f32 = 0.12;
const CREATURE_SCENE_SCALE: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VisualPrefab {
    Player,
    Dragon,
    Goblin,
    Merchant,
    Cube,
}

pub(crate) fn visual_prefab(kind: Option<EntityKind>, is_boss: bool) -> VisualPrefab {
    match kind {
        Some(EntityKind::Player) => VisualPrefab::Player,
        Some(EntityKind::Hostile) if is_boss => VisualPrefab::Dragon,
        Some(EntityKind::Hostile) => VisualPrefab::Goblin,
        Some(EntityKind::Friendly) => VisualPrefab::Merchant,
        Some(EntityKind::Ally) | Some(EntityKind::Neutral) | Some(EntityKind::Resource) | None => {
            VisualPrefab::Cube
        }
    }
}

pub struct RendererPlugin;

impl Plugin for RendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init_renderer_assets);
        // Note: there is deliberately no `Add<Position>` observer here.
        // An observer and `spawn_entity_meshes` used to run the same branching
        // logic side by side. The observer fired before `PlayerAssets` /
        // `BossDragonAssets` finished loading, silently dropped those entities
        // (its `if let Some(assets)` arms have no `else`), and bypassed the
        // material cache — so whichever path won the race decided whether a
        // material was leaked per entity. `spawn_entity_meshes` is the retry
        // loop that already handled the not-yet-loaded case correctly, so it is
        // the single source of truth.
        app.configure_sets(
            Update,
            (
                RenderSync::Transforms,
                RenderSync::Camera,
                RenderSync::Project,
            )
                .chain(),
        );
        app.add_systems(
            Update,
            (
                spawn_entity_meshes,
                upgrade_harvestable_visual,
                sync_equipped_weapon,
                sync_transforms,
                anchor_player_model,
                anchor_scene_layout,
                update_colors,
                crate::harvest::update_harvestable_fill,
            )
                .chain()
                .in_set(RenderSync::Transforms)
                .run_if(in_state(Screen::InGame)),
        )
        .add_systems(
            Update,
            cleanup_entity_render.run_if(not(in_state(Screen::InGame))),
        );
    }
}

/// Gives every replicated entity its local render components.
///
/// Runs every frame rather than as an `Add<Position>` observer on purpose: an
/// entity can be replicated before its glTF collection has finished loading,
/// and this retries until the assets exist.
fn spawn_entity_meshes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    player_assets: Option<Res<PlayerAssets>>,
    dragon_assets: Option<Res<BossDragonAssets>>,
    creature_assets: Option<Res<CreatureAssets>>,
    mut renderer_assets: Option<ResMut<RendererAssets>>,
    placeables: Option<Res<PlaceableRegistry>>,
    asset_server: Option<Res<AssetServer>>,
    entities: Query<
        (
            Entity,
            &Position,
            &EntityColor,
            Option<&EntityKind>,
            Option<&ProjectileVisual>,
            Option<&Boss>,
            Option<&Harvestable>,
            Option<&LootBagMarker>,
        ),
        Without<RenderedEntity>,
    >,
) {
    for (entity, position, color, kind, projectile_visual, boss, harvestable, loot_bag) in
        entities.iter()
    {
        if loot_bag.is_some() {
            let mesh = meshes.add(Cuboid::new(0.7, 0.55, 0.7));
            let material = materials.add(StandardMaterial {
                base_color: Color::srgb(0.42, 0.28, 0.14),
                perceptual_roughness: 0.9,
                ..default()
            });
            commands.entity(entity).insert((
                Mesh3d(mesh),
                MeshMaterial3d(material),
                Transform::from_translation(position.0 + Vec3::Y * 0.28),
                RenderedEntity,
            ));
            continue;
        }
        if let (Some(harvestable), Some(placeables), Some(asset_server)) =
            (harvestable, placeables.as_ref(), asset_server.as_ref())
        {
            if let Some((handle, transform)) =
                harvestable_scene(harvestable, placeables, asset_server, position.0)
            {
                commands.entity(entity).insert((
                    WorldAssetRoot(handle),
                    transform,
                    AnchorSceneLayout,
                    RenderedEntity,
                ));
                continue;
            }
        }
        // Resource nodes have no creature prefab. Wait for `Harvestable` so we
        // do not lock in a fallback cube before the replicated row arrives.
        if matches!(kind, Some(EntityKind::Resource)) && harvestable.is_none() {
            continue;
        }
        let is_projectile = projectile_visual.is_some();
        if is_projectile {
            let (mesh, material) = if let Some(ra) = renderer_assets.as_ref() {
                (ra.projectile_mesh.clone(), ra.projectile_material.clone())
            } else {
                (
                    meshes.add(Cuboid::new(0.45, 0.45, 0.45)),
                    materials.add(StandardMaterial {
                        base_color: color.0,
                        emissive: LinearRgba::rgb(0.1, 0.7, 1.0),
                        ..default()
                    }),
                )
            };
            commands.entity(entity).insert((
                Mesh3d(mesh),
                MeshMaterial3d(material),
                Transform::from_translation(position.0),
                RenderedEntity,
            ));
        } else {
            match visual_prefab(kind.copied(), boss.is_some()) {
                VisualPrefab::Player => {
                    if let Some(assets) = player_assets.as_ref() {
                        commands.entity(entity).insert((
                            WorldAssetRoot(assets.scene.clone()),
                            Transform::from_translation(position.0)
                                .with_scale(Vec3::splat(PLAYER_SCENE_SCALE)),
                            PlayerModelRoot,
                            RenderedEntity,
                        ));
                    }
                }
                VisualPrefab::Dragon => {
                    if let Some(assets) = dragon_assets.as_ref() {
                        commands.entity(entity).insert((
                            WorldAssetRoot(assets.scene.clone()),
                            Transform::from_translation(position.0)
                                .with_scale(Vec3::splat(BOSS_DRAGON_SCENE_SCALE)),
                            RenderedEntity,
                        ));
                    }
                }
                VisualPrefab::Goblin => {
                    if let Some(assets) = creature_assets.as_ref() {
                        commands.entity(entity).insert((
                            WorldAssetRoot(assets.goblin.clone()),
                            Transform::from_translation(position.0)
                                .with_scale(Vec3::splat(CREATURE_SCENE_SCALE)),
                            RenderedEntity,
                        ));
                    }
                }
                VisualPrefab::Merchant => {
                    if let Some(assets) = creature_assets.as_ref() {
                        commands.entity(entity).insert((
                            WorldAssetRoot(assets.merchant.clone()),
                            Transform::from_translation(position.0)
                                .with_scale(Vec3::splat(CREATURE_SCENE_SCALE)),
                            RenderedEntity,
                        ));
                    }
                }
                VisualPrefab::Cube => {
                    let (mesh, material) = if let Some(ra) = renderer_assets.as_mut() {
                        (
                            ra.fallback_mesh_small.clone(),
                            ra.get_or_create_color_material(&mut materials, color.0),
                        )
                    } else {
                        (
                            meshes.add(Cuboid::new(2.0, 2.0, 2.0)),
                            materials.add(StandardMaterial {
                                base_color: color.0,
                                ..default()
                            }),
                        )
                    };
                    commands.entity(entity).insert((
                        Mesh3d(mesh),
                        MeshMaterial3d(material),
                        Transform::from_translation(position.0),
                        RenderedEntity,
                    ));
                }
            }
        }
    }
}

fn harvestable_scene(
    harvestable: &Harvestable,
    placeables: &PlaceableRegistry,
    asset_server: &AssetServer,
    position: Vec3,
) -> Option<(Handle<WorldAsset>, Transform)> {
    let definition = placeables
        .resources
        .get(&KindId::new(harvestable.kind_id.clone()))?;
    let AssetHint::Scene(path) = definition.asset_hint() else {
        return None;
    };
    let handle = asset_server.load::<WorldAsset>(format!("{path}#Scene0"));
    let scale = Vec3::from_array(definition.defaults().transform.scale);
    Some((
        handle,
        Transform::from_translation(position).with_scale(scale),
    ))
}

/// Replaces the cube fallback once `Harvestable` arrives on an already-rendered entity.
fn upgrade_harvestable_visual(
    mut commands: Commands,
    placeables: Option<Res<PlaceableRegistry>>,
    asset_server: Option<Res<AssetServer>>,
    cubes: Query<
        (Entity, &Harvestable, &Position),
        (With<RenderedEntity>, With<Mesh3d>, Without<WorldAssetRoot>),
    >,
) {
    let (Some(placeables), Some(asset_server)) = (placeables, asset_server) else {
        return;
    };
    for (entity, harvestable, position) in &cubes {
        let Some((handle, transform)) =
            harvestable_scene(harvestable, &placeables, &asset_server, position.0)
        else {
            continue;
        };
        commands
            .entity(entity)
            .remove::<Mesh3d>()
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .insert((WorldAssetRoot(handle), transform, AnchorSceneLayout));
    }
}

fn sync_equipped_weapon(
    mut commands: Commands,
    weapons: Option<Res<WeaponAssets>>,
    players: Query<(Entity, &Equipment, Option<&Children>), With<PlayerModelRoot>>,
    visuals: Query<(), With<EquippedWeaponVisual>>,
) {
    let Some(weapons) = weapons else {
        return;
    };
    for (entity, equipment, children) in &players {
        let wants_sword = equipment
            .weapon
            .as_ref()
            .is_some_and(|item| item.item_id.as_str() == "sword");
        let existing = children.and_then(|kids| kids.iter().find(|child| visuals.contains(*child)));
        match (wants_sword, existing) {
            (true, None) => {
                commands.entity(entity).with_children(|parent| {
                    parent.spawn((
                        WorldAssetRoot(weapons.sword.clone()),
                        sword_hold_transform(),
                        EquippedWeaponVisual,
                    ));
                });
            }
            (false, Some(child)) => {
                commands.entity(child).despawn();
            }
            _ => {}
        }
    }
}

/// Distance beyond which a position change is a teleport, not movement.
///
/// Comfortably above one tick of travel (a fast player covers ~0.3 m per tick)
/// and well below a respawn or a knockback, which must snap rather than glide
/// across the map.
const TELEPORT_SNAP_DISTANCE: f32 = 5.0;

/// How quickly the rendered transform closes the *residual* gap to `Position`.
///
/// This used to be the only term, at 25/s, and it was the source of the judder.
/// A pure exponential follow derives its whole velocity from the outstanding
/// error, and that error is a sawtooth: `Position` advances on the fixed
/// schedule while this runs on the render schedule, so at 60 Hz each against
/// the other some frames step twice, some not at all. Turning that sawtooth
/// into velocity made the character's on-screen speed swing by a third every
/// frame — and because the camera is glued to the character, the entire world
/// swung with it.
///
/// With the feed-forward term below carrying the motion, this only has to bleed
/// off leftover error (rollback corrections, a missed tick), so it is
/// deliberately slow: a low rate filters the per-tick quantisation of
/// `Position` out of the rendered velocity instead of amplifying it.
const RENDER_FOLLOW_RATE: f32 = 12.0;

/// How quickly the velocity estimate tracks the measured `Position` delta.
///
/// Fast enough to pick up a direction change within a couple of frames, slow
/// enough that a single double-stepped frame does not spike the feed-forward.
const VELOCITY_FOLLOW_RATE: f32 = 20.0;

/// How far behind the simulation the rendered transform is deliberately held.
///
/// Feed-forward alone would put the render position level with the newest
/// `Position`, which means it spends half of every tick *ahead* of a value that
/// has not been updated yet, and gets pulled back when it is. Holding ~2 ticks
/// of slack keeps the render position interpolating between known states
/// instead of extrapolating past them. At 30 ms the delay is far below what is
/// perceptible in a click-to-move game, and it is constant, so it reads as
/// weight rather than lag.
const RENDER_DELAY: f32 = 0.03;

/// Seconds of an unchanged `Position` after which the feed-forward is dropped.
///
/// Must be comfortably longer than one fixed tick: at 60 Hz render and 60 Hz
/// simulation, frames that happen to run no tick at all are routine and must
/// not be mistaken for the entity having stopped.
///
/// This only decides when the feed-forward is *dropped*; it is not what stops
/// the transform overshooting an arrival. That is the clamp in
/// [`sync_transforms`], which does not depend on detecting a stop at all.
const VELOCITY_IDLE_TIMEOUT: f32 = 0.15;

/// Upper bound on a single velocity sample, in world units per second.
///
/// A rollback that rewrites `Position` by a metre inside one frame reads as a
/// huge instantaneous velocity. Feeding that into the extrapolation would fling
/// the transform away from the entity, so outlier samples are dropped and left
/// to the correction term instead.
const MAX_ESTIMATED_SPEED: f32 = 40.0;

/// How quickly the rendered facing catches up to `LookDirection`, per second.
///
/// Facing used to snap, so every click that changed direction spun the model
/// within a single frame.
const ROTATION_FOLLOW_RATE: f32 = 16.0;

/// Per-entity state backing the render smoothing in [`sync_transforms`].
#[derive(Component)]
pub struct RenderSmoothing {
    /// Last `Position` value observed, in world units.
    last_target: Vec3,
    /// Wall-clock seconds elapsed since `last_target` last changed.
    since_change: f32,
    /// Estimated velocity of `Position`, in world units per second.
    velocity: Vec3,
}

impl RenderSmoothing {
    fn new(target: Vec3) -> Self {
        Self {
            last_target: target,
            since_change: 0.0,
            velocity: Vec3::ZERO,
        }
    }
}

fn sync_transforms(
    time: Res<Time>,
    mut commands: Commands,
    mut entities: Query<
        (
            Entity,
            &Position,
            Option<&LookDirection>,
            &mut Transform,
            Option<&mut RenderSmoothing>,
        ),
        Without<AoeZone>,
    >,
) {
    let delta = time.delta_secs();
    if delta <= 0.0 {
        return;
    }

    // Frame-rate independent exponential decay: the fraction of the remaining
    // gap closed this frame depends only on elapsed time, so the motion looks
    // identical at 60 and 240 Hz.
    let position_blend = 1.0 - (-RENDER_FOLLOW_RATE * delta).exp();
    let velocity_blend = 1.0 - (-VELOCITY_FOLLOW_RATE * delta).exp();
    let rotation_blend = 1.0 - (-ROTATION_FOLLOW_RATE * delta).exp();

    for (entity, position, look_direction, mut transform, smoothing) in entities.iter_mut() {
        let target = position.0;

        let Some(mut smoothing) = smoothing else {
            // First frame for this entity: start level with the simulation
            // rather than gliding in from wherever the mesh was spawned.
            transform.translation = target;
            apply_look(&mut transform, look_direction, 1.0);
            commands.entity(entity).insert(RenderSmoothing::new(target));
            continue;
        };

        // Re-estimate velocity only when `Position` actually moves, dividing by
        // the wall-clock span the delta covers rather than by one frame. That is
        // what keeps the estimate correct on frames that ran two fixed ticks, or
        // none.
        smoothing.since_change += delta;
        if target != smoothing.last_target {
            let sample = (target - smoothing.last_target) / smoothing.since_change;
            if sample.length_squared() <= MAX_ESTIMATED_SPEED * MAX_ESTIMATED_SPEED {
                smoothing.velocity = smoothing.velocity.lerp(sample, velocity_blend);
            }
            smoothing.last_target = target;
            smoothing.since_change = 0.0;
        } else if smoothing.since_change > VELOCITY_IDLE_TIMEOUT {
            smoothing.velocity = Vec3::ZERO;
        }

        if transform.translation.distance(target) > TELEPORT_SNAP_DISTANCE {
            // Respawn, knockback, teleport: jump, and forget a velocity that
            // describes a path the entity never travelled.
            transform.translation = target;
            smoothing.velocity = Vec3::ZERO;
        } else {
            // Carry the transform at the simulation's own velocity, then nudge
            // it toward the delayed goal. The first term supplies the motion
            // (constant, hence smooth); the second only removes drift.
            let goal = target - smoothing.velocity * RENDER_DELAY;
            let carried = clamp_to_target(
                transform.translation + smoothing.velocity * delta,
                target,
                smoothing.velocity,
            );
            transform.translation = carried.lerp(goal, position_blend.clamp(0.0, 1.0));
        }

        apply_look(&mut transform, look_direction, rotation_blend);
    }
}

/// Stops the feed-forward carrying `carried` past `target` along `velocity`.
///
/// The feed-forward has to keep moving on frames where `Position` did not
/// change, or the character steps rather than walks — but on arrival *every*
/// subsequent frame is such a frame, and nothing in the extrapolation itself
/// knows the walk ended. Left alone it sails on at the last known speed until
/// the velocity estimate times out, then gets dragged back by the correction
/// term: the click lands, the character drifts a little past the spot, and
/// snaps back onto it.
///
/// The simulation is the authority on where the character stopped, and it
/// never overshoots its own target (see `movement::step_towards`). So the
/// render is free to lag it, but never to lead it: this removes any component
/// of the extrapolation that has crossed `target` in the direction of travel,
/// leaving motion before arrival untouched — during a walk the transform is
/// deliberately held `RENDER_DELAY` behind, so the clamp never engages.
///
/// Only the along-velocity component is clamped; sideways offset is left to
/// the correction term, so a mid-walk direction change still eases round
/// instead of cornering.
fn clamp_to_target(carried: Vec3, target: Vec3, velocity: Vec3) -> Vec3 {
    let direction = velocity.normalize_or_zero();
    let overshoot = (carried - target).dot(direction);
    if overshoot > 0.0 {
        carried - direction * overshoot
    } else {
        carried
    }
}

/// Turns the entity toward `look_direction`, blending by `blend` (1.0 = snap).
fn apply_look(transform: &mut Transform, look_direction: Option<&LookDirection>, blend: f32) {
    let Some(look_direction) = look_direction else {
        return;
    };
    let direction = Vec3::new(look_direction.x, 0.0, look_direction.z);
    if direction.length_squared() <= 0.001 {
        return;
    }
    let facing = Transform::default()
        .looking_to(direction.normalize(), Vec3::Y)
        .rotation;
    transform.rotation = transform.rotation.slerp(facing, blend.clamp(0.0, 1.0));
}

/// Removes horizontal offsets embedded in the instantiated player scene while
/// preserving its authored vertical placement. Bevy may place an intermediate
/// scene entity between `WorldAssetRoot` and `Node0`, so inspect the full parent
/// chain instead of assuming a direct child.
fn anchor_player_model(
    mut commands: Commands,
    roots: Query<Entity, With<PlayerModelRoot>>,
    parents: Query<&ChildOf>,
    mut scene_nodes: Query<
        (Entity, &mut Transform),
        (Without<PlayerModelRoot>, Without<PlayerModelAnchored>),
    >,
) {
    for (entity, mut transform) in &mut scene_nodes {
        if roots
            .iter()
            .any(|root| is_descendant_of(entity, root, &parents))
        {
            transform.translation.x = 0.0;
            transform.translation.z = 0.0;
            commands.entity(entity).insert(PlayerModelAnchored);
        }
    }
}

/// Puts an instantiated scene back onto the position its entity was spawned at.
///
/// The `models/new/*.glb` models were exported from one Blender scene laid out
/// in a grid, and each kept that layout in its glTF root node: `tree_oak_medium`
/// carries `(95, 0, 20)`, `rock_medium` `(110, 0, -20)`. Instantiated under the
/// entity that positions them, those offsets draw the model tens of metres from
/// the thing it represents — the harvestable oak was drawn 95 m east of the
/// `resource_node` the gather click tests against, so clicking the tree on
/// screen did nothing. Static map props from the same export are off by the
/// same amount, and their colliders stay at the authored placement.
///
/// Only the outermost offset node of each branch is zeroed: parts placed
/// relative to it (a canopy sitting over a trunk) must keep their own offsets.
/// The vertical offset is kept for the same reason `anchor_player_model` keeps
/// it — a model authored to sit above its origin is doing that on purpose.
fn anchor_scene_layout(
    mut commands: Commands,
    roots: Query<Entity, (With<AnchorSceneLayout>, Without<SceneLayoutAnchored>)>,
    children: Query<&Children>,
    mut transforms: Query<&mut Transform>,
) {
    for root in roots.iter() {
        // The scene spawner attaches the whole hierarchy at once, so no
        // children yet means the glTF is still loading: retry next frame.
        let Ok(kids) = children.get(root) else {
            continue;
        };
        let mut stack: Vec<Entity> = kids.iter().collect();
        while let Some(entity) = stack.pop() {
            let mut corrected = false;
            if let Ok(mut transform) = transforms.get_mut(entity) {
                if transform.translation.x.abs() > 1e-4 || transform.translation.z.abs() > 1e-4 {
                    transform.translation.x = 0.0;
                    transform.translation.z = 0.0;
                    corrected = true;
                }
            }
            if corrected {
                continue;
            }
            if let Ok(kids) = children.get(entity) {
                stack.extend(kids.iter());
            }
        }
        commands.entity(root).insert(SceneLayoutAnchored);
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

/// Whether a new `EntityColor` should change the assigned material handle.
#[cfg(test)]
pub(crate) fn color_material_needs_swap(current: Color, next: Color) -> bool {
    current != next
}

fn update_colors(
    mut renderer_assets: Option<ResMut<RendererAssets>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut entities: Query<
        (&EntityColor, &mut MeshMaterial3d<StandardMaterial>),
        (Changed<EntityColor>, Without<ProjectileVisual>),
    >,
) {
    let Some(assets) = renderer_assets.as_mut() else {
        return;
    };
    for (color, mut handle) in &mut entities {
        let next = assets.get_or_create_color_material(&mut materials, color.0);
        if handle.0 != next {
            *handle = MeshMaterial3d(next);
        }
    }
}

fn cleanup_entity_render(
    mut commands: Commands,
    entities: Query<(Entity, Option<&Children>), With<RenderedEntity>>,
) {
    for (entity, children) in entities.iter() {
        if let Some(children) = children {
            for child in children.iter() {
                commands.entity(child).despawn();
            }
        }
        commands
            .entity(entity)
            .remove::<RenderedEntity>()
            .remove::<Mesh3d>()
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .remove::<WorldAssetRoot>()
            .remove::<RenderSmoothing>()
            .remove::<PlayerModelRoot>()
            .remove::<PlayerModelAnchored>()
            .remove::<AnchorSceneLayout>()
            .remove::<SceneLayoutAnchored>()
            .remove::<Transform>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// One render frame at 60 Hz.
    const FRAME: Duration = Duration::from_nanos(16_666_667);

    /// One fixed tick of travel for a player at the default
    /// `MovementStats::speed` (0.15 world units per tick).
    const TICK_STEP: f32 = 0.15;

    /// `tree_oak_medium.glb` keeps the (95, 0, 20) place it had in the Blender
    /// scene every `models/new` prop was exported from. Left alone, the oak is
    /// drawn 95 m from the `resource_node` the gather click tests against, so
    /// the tree on screen is not the tree you can harvest.
    #[test]
    fn imported_scene_offset_does_not_move_the_harvestable() {
        let mut app = App::new();
        app.add_systems(Update, anchor_scene_layout);

        let root = app
            .world_mut()
            .spawn((AnchorSceneLayout, Transform::from_xyz(-48.0, 0.0, 4.0)))
            .id();
        let offset_node = app
            .world_mut()
            .spawn(Transform::from_xyz(95.0, 0.0, 20.0))
            .id();
        // The canopy sits above the trunk in the same node: a part placed
        // relative to the corrected root keeps its own offset.
        let canopy = app
            .world_mut()
            .spawn(Transform::from_xyz(0.0, 7.5, 0.0))
            .id();
        app.world_mut().entity_mut(offset_node).add_child(canopy);
        app.world_mut().entity_mut(root).add_child(offset_node);

        app.update();

        let node = app.world().entity(offset_node).get::<Transform>().unwrap();
        assert_eq!(
            node.translation,
            Vec3::ZERO,
            "layout offset must be dropped"
        );
        let canopy = app.world().entity(canopy).get::<Transform>().unwrap();
        assert_eq!(
            canopy.translation,
            Vec3::new(0.0, 7.5, 0.0),
            "parts under the corrected node keep their authored placement"
        );
        assert!(
            app.world().entity(root).contains::<SceneLayoutAnchored>(),
            "an anchored node is not walked again"
        );
    }

    /// Bevy may insert its own entity between the marked root and the glTF's
    /// root node, so the correction cannot assume a direct child.
    #[test]
    fn offset_is_found_below_an_intermediate_scene_entity() {
        let mut app = App::new();
        app.add_systems(Update, anchor_scene_layout);

        let root = app
            .world_mut()
            .spawn((AnchorSceneLayout, Transform::from_xyz(4.0, 0.0, -2.0)))
            .id();
        let intermediate = app.world_mut().spawn(Transform::default()).id();
        let offset_node = app
            .world_mut()
            .spawn(Transform::from_xyz(110.0, 0.0, -20.0))
            .id();
        app.world_mut()
            .entity_mut(intermediate)
            .add_child(offset_node);
        app.world_mut().entity_mut(root).add_child(intermediate);

        app.update();

        assert_eq!(
            app.world()
                .entity(offset_node)
                .get::<Transform>()
                .unwrap()
                .translation,
            Vec3::ZERO
        );
    }

    fn smoothing_app() -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.add_systems(Update, sync_transforms);
        let entity = app
            .world_mut()
            .spawn((Position(Vec3::ZERO), Transform::default()))
            .id();
        // First update snaps to the simulation and installs `RenderSmoothing`.
        app.update();
        (app, entity)
    }

    /// Advances one render frame during which the fixed schedule ran `ticks`
    /// times, and returns the resulting rendered X.
    fn frame(app: &mut App, entity: Entity, ticks: u32) -> f32 {
        app.world_mut().resource_mut::<Time>().advance_by(FRAME);
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<Position>()
            .expect("position")
            .0
            .x += TICK_STEP * ticks as f32;
        app.update();
        app.world()
            .entity(entity)
            .get::<Transform>()
            .unwrap()
            .translation
            .x
    }

    /// The regression this smoothing exists for.
    ///
    /// `Position` advances on the fixed schedule while the transform is written
    /// on the render schedule, so at 60 Hz against 60 Hz the tick lands
    /// unevenly: some frames carry two steps, some none. The previous follow
    /// derived its entire velocity from the outstanding error and so replayed
    /// that pattern as on-screen speed — and because the camera is glued to the
    /// player, the whole world juddered with it.
    ///
    /// What matters is not that the rendered position is accurate, but that its
    /// *derivative* is steady, so this asserts on the spread of the per-frame
    /// displacement.
    #[test]
    fn rendered_velocity_stays_steady_across_uneven_ticks() {
        let (mut app, entity) = smoothing_app();
        // Two ticks arriving in one frame, then a frame with none: the worst
        // realistic phase drift between two 60 Hz schedules.
        let pattern = [1, 2, 0, 1, 1, 2, 0, 1];

        // Let the velocity estimate settle before measuring.
        let mut previous = 0.0;
        for step in 0..48 {
            previous = frame(&mut app, entity, pattern[step % pattern.len()]);
        }

        let mut deltas = Vec::new();
        for step in 48..96 {
            let current = frame(&mut app, entity, pattern[step % pattern.len()]);
            deltas.push(current - previous);
            previous = current;
        }

        let mean = deltas.iter().sum::<f32>() / deltas.len() as f32;
        assert!(
            (mean - TICK_STEP).abs() < 0.01,
            "rendered motion must average the simulated speed, got {mean} vs {TICK_STEP}"
        );

        let worst = deltas
            .iter()
            .map(|delta| (delta - mean).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst < mean * 0.25,
            "per-frame displacement swung by {worst} around a mean of {mean}; \
             that swing is the judder this smoothing exists to remove"
        );
    }

    /// With the feed-forward carrying the motion, the transform must not settle
    /// at a fixed distance behind the simulation the way a pure error-driven
    /// follow does — otherwise the character visibly trails its own hitbox.
    #[test]
    fn rendered_position_does_not_lag_far_behind_a_steady_walk() {
        let (mut app, entity) = smoothing_app();
        for _ in 0..120 {
            frame(&mut app, entity, 1);
        }

        let simulated = app.world().entity(entity).get::<Position>().unwrap().0.x;
        let rendered = app
            .world()
            .entity(entity)
            .get::<Transform>()
            .unwrap()
            .translation
            .x;
        // One tick of slack is the deliberate render delay; anything approaching
        // the ~2 ticks the old follow settled at reads as input lag.
        assert!(
            simulated - rendered < TICK_STEP * 3.0,
            "rendered {rendered} trails simulated {simulated} by more than three ticks"
        );
        assert!(
            rendered <= simulated,
            "the render must never run ahead of the simulation"
        );
    }

    /// A stopped entity must land exactly on its simulated position: the
    /// feed-forward has to bleed away rather than park the transform short of
    /// the target forever.
    #[test]
    fn rendered_position_converges_exactly_once_movement_stops() {
        let (mut app, entity) = smoothing_app();
        for _ in 0..60 {
            frame(&mut app, entity, 1);
        }
        for _ in 0..60 {
            frame(&mut app, entity, 0);
        }

        let simulated = app.world().entity(entity).get::<Position>().unwrap().0.x;
        let rendered = app
            .world()
            .entity(entity)
            .get::<Transform>()
            .unwrap()
            .translation
            .x;
        assert!(
            (simulated - rendered).abs() < 0.001,
            "rendered {rendered} never settled onto simulated {simulated}"
        );
    }

    /// The click-to-move "recoil": the character reaches the clicked point,
    /// drifts a little past it, then snaps back onto it.
    ///
    /// The feed-forward keeps extrapolating at the last known walking speed on
    /// every frame that `Position` does not change — and once the walk ends,
    /// that is every frame. So the transform used to sail past the arrival
    /// point for as long as the velocity estimate survived, and the correction
    /// term then hauled it back.
    ///
    /// Overshoot and reversal are asserted separately because either alone is
    /// the visible bug: leading the simulation is what the player sees as the
    /// character stepping past the click, reversing is the snap back.
    #[test]
    fn arrival_never_overshoots_or_reverses() {
        let (mut app, entity) = smoothing_app();
        for _ in 0..60 {
            frame(&mut app, entity, 1);
        }

        let arrival = app.world().entity(entity).get::<Position>().unwrap().0.x;

        // Long enough to cover the whole velocity-idle timeout, which is how
        // far the stale feed-forward used to carry the transform.
        let mut previous = f32::NEG_INFINITY;
        for step in 0..60 {
            let rendered = frame(&mut app, entity, 0);
            assert!(
                rendered <= arrival + 1e-4,
                "frame {step}: rendered {rendered} ran past the arrival point {arrival}"
            );
            assert!(
                rendered >= previous - 1e-4,
                "frame {step}: rendered position went backwards, {previous} -> {rendered}"
            );
            previous = rendered;
        }

        assert!(
            (previous - arrival).abs() < 0.001,
            "rendered {previous} never settled onto the arrival point {arrival}"
        );
    }

    /// The clamp must not engage mid-walk: it exists only to stop the
    /// extrapolation leading the simulation, and a walking character is
    /// deliberately held `RENDER_DELAY` behind, so motion must be unaffected.
    #[test]
    fn clamp_leaves_a_walk_in_progress_alone() {
        let velocity = Vec3::new(9.0, 0.0, 0.0);
        let target = Vec3::new(5.0, 0.0, 0.0);
        // Trailing the target, as during a steady walk.
        let carried = Vec3::new(4.8, 0.0, 0.0);
        assert_eq!(clamp_to_target(carried, target, velocity), carried);
    }

    /// Only the along-velocity component is removed, so a character easing
    /// back onto its path after a direction change still corners smoothly.
    #[test]
    fn clamp_removes_only_the_along_velocity_overshoot() {
        let velocity = Vec3::new(9.0, 0.0, 0.0);
        let target = Vec3::ZERO;
        let carried = Vec3::new(0.5, 0.0, 0.3);
        let clamped = clamp_to_target(carried, target, velocity);
        assert!((clamped.x - 0.0).abs() < 1e-6, "overshoot not removed");
        assert!((clamped.z - 0.3).abs() < 1e-6, "sideways offset was eaten");
    }

    /// Respawn, knockback and teleport must cut, not glide across the map.
    #[test]
    fn a_teleport_snaps_instead_of_gliding() {
        let (mut app, entity) = smoothing_app();
        app.world_mut().resource_mut::<Time>().advance_by(FRAME);
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<Position>()
            .unwrap()
            .0 = Vec3::new(TELEPORT_SNAP_DISTANCE * 4.0, 0.0, 0.0);
        app.update();

        let rendered = app
            .world()
            .entity(entity)
            .get::<Transform>()
            .unwrap()
            .translation
            .x;
        assert_eq!(rendered, TELEPORT_SNAP_DISTANCE * 4.0);
    }

    #[test]
    fn identical_colors_do_not_need_a_material_swap() {
        let green = Color::srgb(0.2, 0.8, 0.2);
        assert!(!color_material_needs_swap(green, green));
        assert!(color_material_needs_swap(green, Color::srgb(0.8, 0.2, 0.2)));
    }

    #[test]
    fn hostile_without_boss_is_a_goblin_not_the_dragon() {
        assert_eq!(
            visual_prefab(Some(EntityKind::Hostile), false),
            VisualPrefab::Goblin
        );
        assert_eq!(
            visual_prefab(Some(EntityKind::Hostile), true),
            VisualPrefab::Dragon
        );
        assert_eq!(
            visual_prefab(Some(EntityKind::Friendly), false),
            VisualPrefab::Merchant
        );
        assert_eq!(
            visual_prefab(Some(EntityKind::Neutral), false),
            VisualPrefab::Cube
        );
        assert_eq!(
            visual_prefab(Some(EntityKind::Ally), false),
            VisualPrefab::Cube
        );
        assert_eq!(
            visual_prefab(Some(EntityKind::Player), true),
            VisualPrefab::Player
        );
    }
}
