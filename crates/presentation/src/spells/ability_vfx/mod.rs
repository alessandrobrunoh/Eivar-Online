//! Geometric VFX registry and dispatcher for alpha abilities.
//!
//! Each sword ability gets a dedicated spawn function
//! that produces a **distinct geometric manifestation** using Bevy primitive
//! meshes/materials. The registry maps `AbilityId` → spawn fn; the dispatcher
//! in `mod.rs` consults it before falling back to the legacy geometry-based
//! selector in `ability_effects`.

use bevy::asset::RenderAssetUsages;
use bevy::color::Color;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use bevymmo_gameplay::abilities::{AbilityGeometry, BaseAbility};
use bevymmo_network::network::protocol::SpellVisualEffect;

use crate::spells::effects::SpellVisual;

// ---------------------------------------------------------------------------
// Individual ability modules
// ---------------------------------------------------------------------------
pub mod lifecycle;

pub mod blade_storm;
pub mod cleave;
pub mod lunge;

// ---------------------------------------------------------------------------
// Registry types
// ---------------------------------------------------------------------------

/// Geometry taken from the same `BaseAbility` the aim preview reads, so the
/// spawned mesh covers the telegraph instead of a hardcoded prop size.
#[derive(Clone, Copy, Debug)]
pub struct AbilityVfxSpec {
    pub start: Vec3,
    pub end: Vec3,
    pub radius: f32,
    pub cone_angle_deg: Option<f32>,
    pub warning_seconds: f32,
}

impl AbilityVfxSpec {
    pub fn from_ability(effect: &SpellVisualEffect, ability: &dyn BaseAbility) -> Self {
        let params = ability.base_params();
        let cone_angle_deg = match ability.geometry() {
            AbilityGeometry::Cone { angle_deg, .. } => Some(angle_deg),
            _ => None,
        };
        Self {
            start: effect.start,
            end: effect.end,
            radius: ability.impact_radius(&params),
            cone_angle_deg,
            warning_seconds: ability.impact_delay(),
        }
    }

    pub fn from_effect(effect: &SpellVisualEffect) -> Self {
        Self {
            start: effect.start,
            end: effect.end,
            radius: effect.end.distance(effect.start).max(0.5),
            cone_angle_deg: None,
            warning_seconds: 0.0,
        }
    }

    /// Horizontal aim axis. Cone visuals point this way; projectiles travel it.
    pub fn direction(&self) -> Vec3 {
        let offset = Vec3::new(self.end.x - self.start.x, 0.0, self.end.z - self.start.z);
        if offset.length_squared() > 0.0001 {
            offset.normalize()
        } else {
            Vec3::Z
        }
    }

    /// Ground point the preview would have drawn as the impact origin.
    pub fn impact(&self) -> Vec3 {
        if self.cone_angle_deg.is_some() {
            self.start
        } else {
            self.end
        }
    }

    /// Delayed area hits already spawn a replicated `aoe_region` mesh. Drawing
    /// a second cone from `SpellVisualEffect` sits slightly off the hitbox
    /// (client look vs the server's stored direction).
    pub fn footprint_drawn_by_aoe_region(&self) -> bool {
        self.warning_seconds > 0.05
    }
}

/// Signature every ability-VFX spawn function must satisfy.
pub type AbilityVfxFn =
    fn(&mut Commands, &mut Assets<Mesh>, &mut Assets<StandardMaterial>, &AbilityVfxSpec);

/// Runtime registry mapping ability ID → VFX spawn function.
#[derive(Resource, Default, Debug)]
pub struct AbilityVfxRegistry {
    map: std::collections::HashMap<&'static str, AbilityVfxFn>,
}

impl AbilityVfxRegistry {
    /// Register a spawn function for the given ability ID.
    pub fn register(&mut self, id: &'static str, fn_: AbilityVfxFn) {
        self.map.insert(id, fn_);
    }

    /// Look up the spawn function for an ability ID.
    pub fn get(&self, id: &str) -> Option<AbilityVfxFn> {
        self.map.get(id).copied()
    }

    /// Number of registered abilities (for testing / diagnostics).
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Population – called once during plugin setup
// ---------------------------------------------------------------------------

/// Fill the registry with the sword-ability entries.
pub fn populate_registry(registry: &mut AbilityVfxRegistry) {
    registry.register("cleave", cleave::spawn);
    registry.register("lunge", lunge::spawn);
    registry.register("blade_storm", blade_storm::spawn);
}

// ---------------------------------------------------------------------------
// Animation system – ticks all lifecycle components each frame
// ---------------------------------------------------------------------------

/// Animate every ability-VFX entity that carries a lifecycle component.
pub fn animate_lifecycle(
    time: Res<Time>,
    mut commands: Commands,
    mut queries: ParamSet<(
        Query<(Entity, &mut Transform, &mut lifecycle::VfxExpandFade)>,
        Query<(Entity, &mut Transform, &mut lifecycle::VfxPulseRing)>,
        Query<(Entity, &mut lifecycle::VfxLifetime)>,
        Query<(Entity, &mut Transform, &mut lifecycle::VfxFall)>,
        Query<(Entity, &mut Transform, &mut lifecycle::VfxSpinExpand)>,
        Query<(Entity, &mut Transform, &mut lifecycle::VfxOscillate)>,
    )>,
) {
    let delta = time.delta_secs();

    for (entity, mut transform, mut comp) in queries.p0().iter_mut() {
        if comp.tick(delta, &mut transform) {
            commands.entity(entity).despawn();
        }
    }
    for (entity, mut transform, mut comp) in queries.p1().iter_mut() {
        if comp.tick(delta, &mut transform) {
            commands.entity(entity).despawn();
        }
    }
    for (entity, mut comp) in queries.p2().iter_mut() {
        if comp.tick(delta) {
            commands.entity(entity).despawn();
        }
    }
    for (entity, mut transform, mut comp) in queries.p3().iter_mut() {
        if comp.tick(delta, &mut transform) {
            commands.entity(entity).despawn();
        }
    }
    for (entity, mut transform, mut comp) in queries.p4().iter_mut() {
        if comp.tick(delta, &mut transform) {
            commands.entity(entity).despawn();
        }
    }
    for (entity, mut transform, mut comp) in queries.p5().iter_mut() {
        if comp.tick(delta, &mut transform) {
            commands.entity(entity).despawn();
        }
    }
}

// ---------------------------------------------------------------------------
// Common helpers – reused across ability modules
// ---------------------------------------------------------------------------

/// Yaw-only rotation that maps a ground sector (opens along local −Z) onto
/// `direction`. Unlike [`Transform::looking_to`], this never tilts the mesh
/// off the XZ plane, so the cone stays a floor decal like the aim preview.
pub fn ground_yaw_towards(direction: Vec3) -> Quat {
    let dir = Vec3::new(direction.x, 0.0, direction.z).normalize_or_zero();
    if dir == Vec3::ZERO {
        Quat::IDENTITY
    } else {
        Quat::from_rotation_arc(Vec3::NEG_Z, dir)
    }
}

fn vfx_ground_material(
    materials: &mut Assets<StandardMaterial>,
    color: Color,
    alpha: f32,
    emissive_strength: f32,
) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: color.with_alpha(alpha),
        emissive: vfx_glow(color, emissive_strength),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        ..default()
    })
}

/// Emissive glow from a base colour.
pub fn vfx_glow(color: Color, strength: f32) -> LinearRgba {
    let rgba = color.to_linear();
    LinearRgba::rgb(
        rgba.red * strength,
        rgba.green * strength,
        rgba.blue * strength,
    )
}

/// Standard emissive-blend material used by most VFX meshes.
pub fn vfx_material(
    materials: &mut Assets<StandardMaterial>,
    color: Color,
    alpha: f32,
    emissive_strength: f32,
) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: color.with_alpha(alpha),
        emissive: vfx_glow(color, emissive_strength),
        alpha_mode: AlphaMode::Blend,
        ..default()
    })
}

/// Spawn a sphere mesh entity with [`SpellVisual`] marker + user-supplied lifecycle component.
///
/// This is the workhorse helper for "burst / expand / fade" style effects.
pub fn spawn_sphere<T: Component>(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    center: Vec3,
    radius: f32,
    color: Color,
    alpha: f32,
    emissive: f32,
    scale: Vec3,
    lifecycle: T,
) {
    let mesh = meshes.add(Sphere::new(radius));
    let mat = vfx_material(materials, color, alpha, emissive);
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_translation(center).with_scale(scale),
        SpellVisual,
        lifecycle,
    ));
}

/// Spawn a horizontal cylinder (disc / ring) at ground level.
pub fn spawn_disc<T: Component>(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    center: Vec3,
    radius: f32,
    height: f32,
    color: Color,
    alpha: f32,
    emissive: f32,
    scale: Vec3,
    lifecycle: T,
) {
    let mesh = meshes.add(Cylinder::new(radius, height));
    let mat = vfx_material(materials, color, alpha, emissive);
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_translation(center + Vec3::Y * 0.02).with_scale(scale),
        SpellVisual,
        lifecycle,
    ));
}

/// Spawn a box (cuboid) mesh – useful for blade / shockwave shapes.
pub fn spawn_box<T: Component>(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    center: Vec3,
    size: Vec3,
    color: Color,
    alpha: f32,
    emissive: f32,
    lifecycle: T,
) {
    let mesh = meshes.add(Cuboid::from_size(size));
    let mat = vfx_material(materials, color, alpha, emissive);
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_translation(center),
        SpellVisual,
        lifecycle,
    ));
}

/// Spawn a capsule mesh – good for lunges, rushes, elongated strikes.
pub fn spawn_capsule<T: Component>(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    center: Vec3,
    radius: f32,
    length: f32,
    color: Color,
    alpha: f32,
    emissive: f32,
    lifecycle: T,
) {
    let mesh = meshes.add(Capsule3d::new(radius, length));
    let mat = vfx_material(materials, color, alpha, emissive);
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_translation(center),
        SpellVisual,
        lifecycle,
    ));
}

/// Spawn a torus (ring) mesh – for orbital / domain effects.
pub fn spawn_torus<T: Component>(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    center: Vec3,
    ring_radius: f32,
    tube_radius: f32,
    color: Color,
    alpha: f32,
    emissive: f32,
    lifecycle: T,
) {
    let mesh = meshes.add(Torus::new(ring_radius, tube_radius));
    let mat = vfx_material(materials, color, alpha, emissive);
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_translation(center),
        SpellVisual,
        lifecycle,
    ));
}

/// Spawn a cone mesh – for directional AoE visualisation (ground slam wave, etc.).
pub fn spawn_cone<T: Component>(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    center: Vec3,
    radius: f32,
    height: f32,
    color: Color,
    alpha: f32,
    emissive: f32,
    lifecycle: T,
) {
    let mesh = meshes.add(Cone { radius, height });
    let mat = vfx_material(materials, color, alpha, emissive);
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_translation(center),
        SpellVisual,
        lifecycle,
    ));
}

/// Spawn a tetrahedron (sharp, angular) – for piercing / kinetic effects.
pub fn spawn_tetra<T: Component>(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    center: Vec3,
    size: f32,
    color: Color,
    alpha: f32,
    emissive: f32,
    lifecycle: T,
) {
    // Bevy doesn't have Tetrahedron primitive; use a small sharp box as proxy
    // or compose from custom vertices. For now we use a scaled box to represent
    // a sharp kinetic shape.
    let mesh = meshes.add(Cuboid::from_size(Vec3::splat(size)));
    let mat = vfx_material(materials, color, alpha, emissive);
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_translation(center),
        SpellVisual,
        lifecycle,
    ));
}

/// Extruded ground wedge: apex at the origin, opening along local −Z.
///
/// Thickness matches the circle AoE cylinders so the cone is visible from
/// the same camera angles; a paper-thin fan is back-face culled and reads
/// as "nothing", leaving only a circular burst at the caster's feet.
pub fn ground_sector_mesh(radius: f32, angle_deg: f32) -> Mesh {
    let radius = radius.max(0.1);
    let angle = angle_deg.clamp(1.0, 359.0);
    let half = angle.to_radians() * 0.5;
    let steps = ((angle / 8.0).ceil() as usize).clamp(4, 32);
    let thick = 0.1;
    let arc = steps + 1;
    let mut positions = Vec::with_capacity(2 * (1 + arc));
    let mut normals = Vec::with_capacity(2 * (1 + arc));
    let mut uvs = Vec::with_capacity(2 * (1 + arc));

    let push_fan = |y: f32,
                    ny: f32,
                    positions: &mut Vec<[f32; 3]>,
                    normals: &mut Vec<[f32; 3]>,
                    uvs: &mut Vec<[f32; 2]>| {
        positions.push([0.0, y, 0.0]);
        normals.push([0.0, ny, 0.0]);
        uvs.push([0.5, 0.5]);
        for i in 0..arc {
            let t = i as f32 / steps as f32;
            let a = -half + half * 2.0 * t;
            let x = a.sin() * radius;
            let z = -a.cos() * radius;
            positions.push([x, y, z]);
            normals.push([0.0, ny, 0.0]);
            uvs.push([0.5 + 0.5 * (x / radius), 0.5 + 0.5 * (z / radius)]);
        }
    };
    push_fan(0.0, -1.0, &mut positions, &mut normals, &mut uvs);
    push_fan(thick, 1.0, &mut positions, &mut normals, &mut uvs);

    let bottom_apex = 0u32;
    let top_apex = (1 + arc) as u32;
    let mut indices = Vec::new();
    for i in 1..arc as u32 {
        // Bottom, winding so −Y is out.
        indices.extend([bottom_apex, i + 1, i]);
        // Top, winding so +Y is out.
        indices.extend([top_apex, top_apex + i, top_apex + i + 1]);
    }
    // Radial walls and the outer arc.
    let bottom_arc = 1u32;
    let top_arc = top_apex + 1;
    indices.extend([
        bottom_apex,
        bottom_arc,
        top_arc,
        bottom_apex,
        top_arc,
        top_apex,
    ]);
    let last = (arc - 1) as u32;
    indices.extend([
        bottom_apex,
        top_apex,
        top_arc + last,
        bottom_apex,
        top_arc + last,
        bottom_arc + last,
    ]);
    for i in 0..last {
        let b0 = bottom_arc + i;
        let b1 = b0 + 1;
        let t0 = top_arc + i;
        let t1 = t0 + 1;
        indices.extend([b0, b1, t1, b0, t1, t0]);
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

/// Places a ground cone whose filled sector matches the aim-preview gizmos.
pub fn spawn_ground_cone<T: Component>(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    apex: Vec3,
    direction: Vec3,
    radius: f32,
    angle_deg: f32,
    color: Color,
    lifecycle: T,
) {
    let mesh = meshes.add(ground_sector_mesh(radius, angle_deg));
    let mat = vfx_ground_material(materials, color, 0.45, 2.4);
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_translation(apex + Vec3::Y * 0.03)
            .with_rotation(ground_yaw_towards(direction)),
        SpellVisual,
        lifecycle,
    ));
}

/// Ground marker that covers the same circle or cone the aim preview drew.
pub fn spawn_matching_footprint(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    spec: &AbilityVfxSpec,
    color: Color,
) {
    if spec.radius <= 0.05 {
        return;
    }
    if spec.footprint_drawn_by_aoe_region() {
        return;
    }
    let warning = spec.warning_seconds.max(0.0);
    if let Some(angle) = spec.cone_angle_deg {
        spawn_ground_cone(
            commands,
            meshes,
            materials,
            spec.impact(),
            spec.direction(),
            spec.radius,
            angle,
            color,
            lifecycle::VfxLifetime::new((warning + 0.35).max(0.45)),
        );
        return;
    }
    if warning > 0.05 {
        spawn_disc(
            commands,
            meshes,
            materials,
            spec.impact(),
            spec.radius,
            0.08,
            color,
            0.4,
            2.2,
            Vec3::ONE,
            lifecycle::VfxPulseRing::new(warning, 0.3),
        );
    } else {
        spawn_disc(
            commands,
            meshes,
            materials,
            spec.impact(),
            spec.radius,
            0.08,
            color,
            0.4,
            2.2,
            Vec3::ONE,
            lifecycle::VfxLifetime::new(0.45),
        );
    }
}

/// Colour palette per weapon family (used as base; each ability tweaks it).
///
/// Only `SWORD` has a weapon behind it today. The other three are authored
/// values kept next to it so the family that ships next inherits a considered
/// colour rather than whatever the first VFX author picks.
#[allow(dead_code)]
mod palette {
    use bevy::color::Color;

    pub const STAFF: Color = Color::srgb(0.65, 0.45, 1.0); // violet
    pub const BOW: Color = Color::srgb(0.3, 0.9, 0.5); // emerald
    pub const SWORD: Color = Color::srgb(1.0, 0.85, 0.2); // gold
    pub const HAMMER: Color = Color::srgb(1.0, 0.4, 0.15); // fire orange
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_sword_abilities() {
        let mut reg = AbilityVfxRegistry::default();
        populate_registry(&mut reg);
        assert_eq!(reg.len(), 3);
    }

    #[test]
    fn registry_lookup_succeeds_for_each_ability() {
        let mut reg = AbilityVfxRegistry::default();
        populate_registry(&mut reg);

        for id in ["cleave", "lunge", "blade_storm"] {
            assert!(reg.get(id).is_some(), "{id} should be registered");
        }
    }

    #[test]
    fn registry_returns_none_for_unknown() {
        let reg = AbilityVfxRegistry::default();
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn circle_spec_matches_the_preview_radius() {
        use bevymmo_content::ability_definitions::blade_storm::BladeStorm;
        use bevymmo_network::network::protocol::SpellVisualEffect;

        let effect = SpellVisualEffect {
            spell_id: "blade_storm".into(),
            start: Vec3::ZERO,
            end: Vec3::new(5.0, 0.0, 1.0),
        };
        let spec = AbilityVfxSpec::from_ability(&effect, &BladeStorm);
        assert!((spec.radius - 5.5).abs() < f32::EPSILON);
        assert!(spec.cone_angle_deg.is_none());
        assert_eq!(spec.impact(), effect.end);
    }

    #[test]
    fn cone_spec_keeps_the_preview_apex_and_angle() {
        use bevymmo_content::ability_definitions::cleave::Cleave;
        use bevymmo_network::network::protocol::SpellVisualEffect;

        let effect = SpellVisualEffect {
            spell_id: "cleave".into(),
            start: Vec3::ZERO,
            end: Vec3::Z * 5.0,
        };
        let spec = AbilityVfxSpec::from_ability(&effect, &Cleave);
        assert!((spec.radius - 5.0).abs() < f32::EPSILON);
        assert_eq!(spec.cone_angle_deg, Some(85.0));
        assert_eq!(spec.impact(), effect.start);
        assert!((spec.direction() - Vec3::Z).length() < 0.01);
    }

    #[test]
    fn sector_mesh_covers_an_arc() {
        let mesh = ground_sector_mesh(8.0, 55.0);
        let count = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .map(bevy::mesh::VertexAttributeValues::len)
            .unwrap_or(0);
        assert!(count >= 10, "extruded apex + arc, got {count}");
    }

    #[test]
    fn delayed_area_hits_leave_the_footprint_to_the_aoe_region() {
        use bevymmo_content::ability_definitions::cleave::Cleave;
        use bevymmo_gameplay::abilities::BaseAbility;
        use bevymmo_network::network::protocol::SpellVisualEffect;

        let effect = SpellVisualEffect {
            spell_id: String::new(),
            start: Vec3::ZERO,
            end: Vec3::Z,
        };
        let spec = AbilityVfxSpec::from_ability(&effect, &Cleave as &dyn BaseAbility);
        assert!(
            !spec.footprint_drawn_by_aoe_region(),
            "cleave is instant so VFX owns the footprint"
        );
    }

    #[test]
    fn yaw_maps_the_sector_axis_onto_look() {
        for dir in [
            Vec3::Z,
            Vec3::X,
            -Vec3::X,
            Vec3::new(1.0, 0.0, 1.0).normalize(),
        ] {
            let axis = ground_yaw_towards(dir) * Vec3::NEG_Z;
            let expected = Vec3::new(dir.x, 0.0, dir.z).normalize();
            assert!(
                (axis - expected).length() < 0.02,
                "look {dir:?} mapped to {axis:?}"
            );
        }
    }

    #[test]
    fn registry_all_fns_are_distinct_pointers() {
        let mut reg = AbilityVfxRegistry::default();
        populate_registry(&mut reg);

        // Collect all fn pointers and verify no duplicates (each ability has its own)
        let fns: Vec<_> = reg.map.values().collect();
        let unique: std::collections::HashSet<_> = fns.iter().copied().collect();
        assert_eq!(
            unique.len(),
            fns.len(),
            "each ability must have its own spawn fn"
        );
    }
}
