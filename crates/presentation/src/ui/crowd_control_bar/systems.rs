//! Screen-space crowd control bar systems.

use bevy::prelude::*;
use std::collections::HashMap;

use bevymmo_gameplay::crowd_control::{ActiveCrowdControl, CrowdControlKind, CrowdControlState};
use bevymmo_network::network::protocol::Position;

use crate::ui::bar::{get_or_spawn_root, spawn_bar};
use crate::ui::crowd_control_bar::components::{
    CrowdControlBarParts, CrowdControlBarRoot, ScreenCrowdControlBar,
};
use crate::ui::theme::UiTheme;

const BAR_WIDTH: f32 = 90.0;
const BAR_HEIGHT: f32 = 12.0;
const BAR_OFFSET: Vec3 = Vec3::new(0.0, 3.1, 0.0);

/// Spawns/despends one screen-space bar per entity with blocking crowd control.
///
/// Only renders a bar when the entity has a non-empty CrowdControlState
/// (Stun, Root, or Silence). This system reacts to lifecycle changes,
/// while positioning and content updates are handled separately.
///
/// # Example
/// ```rust,ignore
/// app.add_systems(Update, sync_screen_cc_bars);
/// ```
pub fn sync_screen_cc_bars(
    mut commands: Commands,
    theme: Res<UiTheme>,
    root_query: Query<Entity, With<CrowdControlBarRoot>>,
    bars: Query<(Entity, &ScreenCrowdControlBar)>,
    cc_query: Query<(Entity, &CrowdControlState, Option<&Position>)>,
) {
    // Collect entities with blocking CC into a HashMap for efficient lookup
    let mut blocking_entities: HashMap<Entity, ActiveCrowdControl> = HashMap::new();

    for (entity, cc_state, maybe_position) in cc_query.iter() {
        // Skip entities without a Position component
        let _position = match maybe_position {
            Some(pos) => pos,
            None => continue,
        };

        if !cc_state.is_empty() {
            if let Some(active_cc) = cc_state.longest() {
                blocking_entities.insert(entity, active_cc.clone());
            }
        }
    }

    // Get or spawn the root UI node
    let root = get_or_spawn_root(&mut commands, &root_query);

    // Remove bars for entities that no longer have blocking CC
    for (bar_entity, bar) in bars.iter() {
        if !blocking_entities.contains_key(&bar.target_entity) {
            commands.entity(bar_entity).despawn();
        }
    }

    // Spawn new bars for entities with blocking CC that don't have a bar yet
    for (&target_entity, active_cc) in blocking_entities.iter() {
        // Skip if this entity already has a bar
        if bars
            .iter()
            .any(|(_, bar)| bar.target_entity == target_entity)
        {
            continue;
        }

        spawn_screen_cc_bar(&mut commands, root, target_entity, active_cc, &theme);
    }
}

/// Projects bars above their target entities and updates fill/label values.
///
/// The bar drains from 100% → 0% as the crowd control effect expires.
/// Only processes entities that have both a CrowdControlState and a Position.
///
/// # Example
/// ```rust,ignore
/// app.add_systems(Update, update_screen_cc_bars);
/// ```
pub fn update_screen_cc_bars(
    camera_query: Query<(&Camera, &Transform), With<Camera3d>>,
    cc_query: Query<(Entity, &CrowdControlState, &Position, Option<&Transform>), Without<Camera3d>>,
    mut bar_query: Query<(&ScreenCrowdControlBar, &mut Node, &mut CrowdControlBarParts)>,
    mut fill_query: Query<&mut Node, Without<ScreenCrowdControlBar>>,
    mut text_query: Query<&mut Text>,
    ui_scale: Res<UiScale>,
) {
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };
    let camera_transform = crate::renderer::camera_view(camera_transform);

    let scale_factor = ui_scale.0;

    for (bar, mut node, mut parts) in bar_query.iter_mut() {
        // Get the target entity's CC state and position
        let Some((_, cc_state, target_position, rendered)) = cc_query
            .iter()
            .find(|(entity, _, _, _)| *entity == bar.target_entity)
        else {
            set_bar_display(&mut node, &mut parts, Display::None);
            continue;
        };

        if cc_state.is_empty() {
            set_bar_display(&mut node, &mut parts, Display::None);
            continue;
        }

        let Some(active_cc) = cc_state.longest() else {
            set_bar_display(&mut node, &mut parts, Display::None);
            continue;
        };

        // Project world position to screen space, anchored to the rendered
        // transform so the bar tracks the mesh rather than the fixed-step
        // `Position` a tick ahead of it.
        let anchor = rendered.map(|t| t.translation).unwrap_or(target_position.0);
        let world_pos = anchor + BAR_OFFSET;
        let Ok(viewport_pos) = camera.world_to_viewport(&camera_transform, world_pos) else {
            set_bar_display(&mut node, &mut parts, Display::None);
            continue;
        };

        let scaled_viewport_pos = viewport_pos / scale_factor;
        // Update bar position and content
        set_bar_position(&mut node, &mut parts, scaled_viewport_pos);
        update_bar_content(active_cc, &mut parts, &mut fill_query, &mut text_query);
    }
}

/// Despawns all crowd control bars when leaving gameplay.
///
/// # Example
/// ```rust,ignore
/// app.add_systems(Update, cleanup_screen_cc_bars);
/// ```
pub fn cleanup_screen_cc_bars(
    mut commands: Commands,
    roots: Query<Entity, With<CrowdControlBarRoot>>,
) {
    for root in roots.iter() {
        commands.entity(root).despawn();
    }
}

/// Spawns a new crowd control bar for a target entity.
fn spawn_screen_cc_bar(
    commands: &mut Commands,
    root: Entity,
    target_entity: Entity,
    active_cc: &ActiveCrowdControl,
    theme: &UiTheme,
) {
    let fill_color = match active_cc.kind {
        CrowdControlKind::Stun => Color::srgb(1.0, 0.55, 0.0),
        CrowdControlKind::Root => Color::srgb(0.45, 0.75, 0.35),
        CrowdControlKind::Silence => Color::srgb(0.55, 0.45, 0.9),
    };

    let bar_entity = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                display: Display::None,
                ..default()
            },
            ScreenCrowdControlBar { target_entity },
        ))
        .id();

    let (bar_body, fill_entity) = spawn_bar(
        commands,
        bar_entity,
        0.0,
        1.0,
        Vec2::new(BAR_WIDTH, BAR_HEIGHT),
        Color::srgba(0.0, 0.0, 0.0, 0.72),
        fill_color,
    );

    let label_entity = commands
        .spawn((
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(10.0),
                ..default()
            },
            TextColor(theme.text_color),
        ))
        .id();

    commands.entity(bar_body).add_child(label_entity);

    commands.entity(bar_entity).insert(CrowdControlBarParts {
        fill: fill_entity,
        label: label_entity,
        last_left: Val::Auto,
        last_top: Val::Auto,
        last_display: Display::None,
        last_fill_pct: -1.0,
        last_label: String::new(),
    });

    commands.entity(root).add_child(bar_entity);
}

/// Sets the absolute position of the bar on screen.
fn set_bar_position(node: &mut Node, parts: &mut CrowdControlBarParts, viewport_pos: Vec2) {
    let left = Val::Px(viewport_pos.x - BAR_WIDTH * 0.5);
    let top = Val::Px(viewport_pos.y - 48.0);

    if parts.last_left != left {
        node.left = left;
        parts.last_left = left;
    }
    if parts.last_top != top {
        node.top = top;
        parts.last_top = top;
    }
    set_bar_display(node, parts, Display::Flex);
}

/// Sets the display state, avoiding redundant updates.
fn set_bar_display(node: &mut Node, parts: &mut CrowdControlBarParts, display: Display) {
    if parts.last_display == display {
        return;
    }
    node.display = display;
    parts.last_display = display;
}

/// Updates the fill percentage and label text for a bar.
fn update_bar_content(
    active_cc: &ActiveCrowdControl,
    parts: &mut CrowdControlBarParts,
    fill_query: &mut Query<&mut Node, Without<ScreenCrowdControlBar>>,
    text_query: &mut Query<&mut Text>,
) {
    let fill_pct = cc_fill_pct(active_cc);
    if (parts.last_fill_pct - fill_pct).abs() > 0.25 {
        if let Ok(mut fill_node) = fill_query.get_mut(parts.fill) {
            fill_node.width = Val::Percent(fill_pct);
        }
        parts.last_fill_pct = fill_pct;
    }

    let label = cc_label(active_cc);
    if parts.last_label == label {
        return;
    }
    if let Ok(mut text) = text_query.get_mut(parts.label) {
        text.0 = label.clone();
    }
    parts.last_label = label;
}

/// Calculates the fill percentage for a crowd control effect.
///
/// The bar drains from 100% → 0% as the effect expires.
fn cc_fill_pct(active_cc: &ActiveCrowdControl) -> f32 {
    if active_cc.total_seconds <= 0.0 {
        return 100.0;
    }

    let remaining = active_cc.remaining_seconds.max(0.0);
    let progress = (remaining / active_cc.total_seconds).clamp(0.0, 1.0);
    progress * 100.0
}

/// Generates the label text for a crowd control effect.
fn cc_label(active_cc: &ActiveCrowdControl) -> String {
    let remaining = active_cc.remaining_seconds.max(0.0);
    match active_cc.kind {
        CrowdControlKind::Stun => format!("Stun {:.1}s", remaining),
        CrowdControlKind::Root => format!("Root {:.1}s", remaining),
        CrowdControlKind::Silence => format!("Silence {:.1}s", remaining),
    }
}
