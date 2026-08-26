//! World loot bags: replicated state, click-to-open, interact range.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};

use bevymmo_gameplay::entity::components::EntityState;
use bevymmo_gameplay::gathering::in_interact_range;
use bevymmo_gameplay::items::instance::ItemInstance;
use bevymmo_gameplay::loot::LOOT_INTERACT_RANGE;
use bevymmo_network::world_components::Position;

use crate::app_state::in_gameplay;
use crate::local_player::LocalPlayer;
use crate::pointer::{hud_wants_pointer, PointerOnHud};

/// Click volume around a sack, metres from the bag origin to the ray.
const LOOT_PICK_RADIUS: f32 = 1.4;

/// One bag currently sitting in the world.
#[derive(Clone, Debug)]
pub struct LootBagView {
    pub id: u64,
    pub position: Vec3,
    pub gold: u64,
    pub slots: Vec<(u8, ItemInstance)>,
}

impl LootBagView {
    pub fn slot(&self, index: u8) -> Option<&ItemInstance> {
        self.slots
            .iter()
            .find(|(slot, _)| *slot == index)
            .map(|(_, item)| item)
    }
}

/// Every bag the client currently knows about.
#[derive(Resource, Default, Debug)]
pub struct WorldLoot {
    pub bags: HashMap<u64, LootBagView>,
}

/// Marker on the Bevy entity that stands in for a replicated bag.
#[derive(Component, Debug, Clone, Copy)]
pub struct LootBagMarker {
    pub bag_id: u64,
}

/// Which bag the loot UI should show. `None` is closed.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct OpenLootBag(pub Option<u64>);

/// World click → open a bag that is in range.
pub struct LootPlugin;

impl Plugin for LootPlugin {
    fn build(&self, app: &mut App) {
        crate::pointer::PointerPlugin::ensure(app);
        app.init_resource::<WorldLoot>();
        app.init_resource::<OpenLootBag>();
        app.add_systems(
            Update,
            (hover_loot_cursor, open_loot_on_click).run_if(in_gameplay),
        );
    }
}

fn open_loot_on_click(
    mouse: Res<ButtonInput<MouseButton>>,
    pointer_on_hud: Res<PointerOnHud>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &Transform), With<Camera3d>>,
    bags: Query<(&LootBagMarker, &Position)>,
    player: Query<(&Position, Option<&EntityState>), With<LocalPlayer>>,
    mut open: ResMut<OpenLootBag>,
) {
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    if hud_wants_pointer(&pointer_on_hud) {
        return;
    }
    let Some(ray) = cursor_ray(&windows, &cameras) else {
        return;
    };
    let Ok((player_pos, state)) = player.single() else {
        return;
    };
    if state.is_some_and(|state| state.is_dead()) {
        return;
    }
    let Some(bag_id) = pick_bag(ray, bags.iter().map(|(marker, pos)| (marker.bag_id, pos.0)))
    else {
        return;
    };
    let Some(bag) = bags.iter().find(|(marker, _)| marker.bag_id == bag_id) else {
        return;
    };
    if !in_interact_range(
        player_pos.0.x,
        player_pos.0.z,
        bag.1 .0.x,
        bag.1 .0.z,
        LOOT_INTERACT_RANGE,
    ) {
        return;
    }
    open.0 = Some(bag_id);
}

fn hover_loot_cursor(
    mut commands: Commands,
    windows: Query<(Entity, &Window, Option<&CursorIcon>), With<PrimaryWindow>>,
    cameras: Query<(&Camera, &Transform), With<Camera3d>>,
    pointer_on_hud: Res<PointerOnHud>,
    bags: Query<(&LootBagMarker, &Position)>,
    player: Query<&Position, With<LocalPlayer>>,
) {
    let Ok((window_entity, window, current)) = windows.single() else {
        return;
    };
    if hud_wants_pointer(&pointer_on_hud) {
        return;
    }
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Some((camera, transform)) = cameras.iter().next() else {
        return;
    };
    let view = GlobalTransform::from(*transform);
    let Ok(ray) = camera.viewport_to_world(&view, cursor) else {
        return;
    };
    let hit = pick_bag(ray, bags.iter().map(|(marker, pos)| (marker.bag_id, pos.0)));
    let in_range = hit.and_then(|bag_id| {
        let player_pos = player.single().ok()?;
        let bag = bags.iter().find(|(marker, _)| marker.bag_id == bag_id)?;
        in_interact_range(
            player_pos.0.x,
            player_pos.0.z,
            bag.1 .0.x,
            bag.1 .0.z,
            LOOT_INTERACT_RANGE,
        )
        .then_some(())
    });
    if in_range.is_some() {
        let icon = CursorIcon::from(SystemCursorIcon::Pointer);
        if current != Some(&icon) {
            commands.entity(window_entity).insert(icon);
        }
    }
}

fn pick_bag(ray: Ray3d, bags: impl Iterator<Item = (u64, Vec3)>) -> Option<u64> {
    let mut best: Option<(u64, f32)> = None;
    for (id, position) in bags {
        let to_point = position - ray.origin;
        let t = to_point.dot(*ray.direction).max(0.0);
        let closest = ray.origin + *ray.direction * t;
        let distance = position.distance(closest);
        if distance > LOOT_PICK_RADIUS {
            continue;
        }
        if best.is_none_or(|(_, best_t)| t < best_t) {
            best = Some((id, t));
        }
    }
    best.map(|(id, _)| id)
}

fn cursor_ray(
    windows: &Query<&Window, With<PrimaryWindow>>,
    cameras: &Query<(&Camera, &Transform), With<Camera3d>>,
) -> Option<Ray3d> {
    let window = windows.single().ok()?;
    let cursor_pos = window.cursor_position()?;
    let (camera, transform) = cameras.iter().next()?;
    let view = GlobalTransform::from(*transform);
    camera.viewport_to_world(&view, cursor_pos).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_prefers_the_closer_bag_along_the_ray() {
        let ray = Ray3d::new(Vec3::new(0.0, 1.0, 0.0), Dir3::new(Vec3::Z).unwrap());
        let near = (1, Vec3::new(0.0, 1.0, 2.0));
        let far = (2, Vec3::new(0.0, 1.0, 8.0));
        assert_eq!(pick_bag(ray, [near, far].into_iter()), Some(1));
    }

    #[test]
    fn pick_ignores_bags_beside_the_ray() {
        let ray = Ray3d::new(Vec3::ZERO, Dir3::new(Vec3::Z).unwrap());
        let beside = (3, Vec3::new(10.0, 0.0, 2.0));
        assert_eq!(pick_bag(ray, [beside].into_iter()), None);
    }
}
