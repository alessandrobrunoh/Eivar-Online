//! Ground markers for replicated `aoe_region` rows.

use bevy::prelude::*;
use bevymmo_client::user_settings::{GameSettingsResource, SettingToggle};
use bevymmo_gameplay::abilities::{AbilityGeometry, AbilityId, BaseAbilityRegistry};
use bevymmo_gameplay::entity::components::EntityKind;
use bevymmo_network::world_components::{AoeZone, NetworkEntityId, Position};

use crate::spells::ability_vfx::{ground_sector_mesh, ground_yaw_towards, vfx_glow};
use crate::spells::effects::SpellVisual;

pub fn spawn_aoe_meshes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    abilities: Res<BaseAbilityRegistry>,
    added: Query<(Entity, &Position, &AoeZone), (Added<AoeZone>, Without<Mesh3d>)>,
) {
    for (entity, position, zone) in &added {
        let warning = zone.pending_delay_seconds > 0.0;
        // Same aqua as the aim preview so the hitbox does not look like a
        // second, differently-coloured ability.
        let color = if warning {
            Color::srgb(0.2, 0.95, 0.95)
        } else {
            Color::srgb(0.45, 0.85, 1.0)
        };
        let cone = cone_draw(zone, &abilities);
        let (mesh, transform) = if let Some((angle, direction)) = cone {
            let mesh = meshes.add(ground_sector_mesh(zone.radius.max(0.1), angle));
            let transform = Transform::from_translation(position.0 + Vec3::Y * 0.04)
                .with_rotation(ground_yaw_towards(direction));
            (mesh, transform)
        } else {
            (
                meshes.add(Cylinder::new(zone.radius.max(0.1), 0.08)),
                Transform::from_translation(position.0 + Vec3::Y * 0.04),
            )
        };
        commands.entity(entity).insert((
            Mesh3d(mesh),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: color.with_alpha(0.4),
                emissive: vfx_glow(color, 1.5),
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                double_sided: true,
                cull_mode: None,
                ..default()
            })),
            transform,
            SpellVisual,
            Visibility::Inherited,
        ));
    }
}

/// True when a hostile caster's ground marker should not be drawn.
pub(crate) fn enemy_preview_hidden(
    show_enemy_previews: bool,
    caster_kind: Option<EntityKind>,
) -> bool {
    !show_enemy_previews && matches!(caster_kind, Some(EntityKind::Hostile))
}

/// Hides or shows ground markers from the gameplay toggle.
///
/// Meshes always spawn so turning the setting back on reveals zones that
/// already exist; this system only drives [`Visibility`].
pub fn apply_aoe_preview_visibility(
    settings: Res<GameSettingsResource>,
    casters: Query<(&NetworkEntityId, &EntityKind)>,
    mut zones: Query<(&AoeZone, &mut Visibility)>,
) {
    let show_enemy = settings.0.toggle(SettingToggle::ShowEnemyAbilityPreviews);
    for (zone, mut visibility) in &mut zones {
        let kind = casters
            .iter()
            .find_map(|(id, kind)| (id.0 == zone.caster).then_some(*kind));
        *visibility = if enemy_preview_hidden(show_enemy, kind) {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }
}

fn cone_draw(zone: &AoeZone, abilities: &BaseAbilityRegistry) -> Option<(f32, Vec3)> {
    if let Some(angle) = zone.cone_angle_deg.filter(|angle| *angle > 1.0) {
        return Some((angle, zone.direction));
    }
    let ability = abilities.get(&AbilityId::new(zone.spell_id.clone()))?;
    match ability.geometry() {
        AbilityGeometry::Cone { angle_deg, .. } => Some((angle_deg, zone.direction)),
        _ => None,
    }
}

pub fn pulse_aoe_meshes(time: Res<Time>, mut zones: Query<(&AoeZone, &mut Transform)>) {
    let t = time.elapsed_secs();
    for (zone, mut transform) in &mut zones {
        let pulse = if zone.pending_delay_seconds > 0.05 {
            0.9 + 0.08 * (t * 8.0).sin()
        } else {
            1.0
        };
        transform.scale = Vec3::new(pulse, 1.0, pulse);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleave_draws_as_a_cone_even_if_the_row_omits_the_angle() {
        let abilities = bevymmo_content::ability_definitions::default_base_abilities();
        let zone = AoeZone {
            radius: 5.0,
            remaining_seconds: 0.15,
            pending_delay_seconds: 0.0,
            spell_id: "cleave".into(),
            cone_angle_deg: None,
            direction: Vec3::Z,
            caster: 0,
        };
        let (angle, dir) = cone_draw(&zone, &abilities).expect("cone");
        assert!((angle - 85.0).abs() < f32::EPSILON);
        assert_eq!(dir, Vec3::Z);
    }

    #[test]
    fn only_hostile_previews_hide_when_the_toggle_is_off() {
        assert!(enemy_preview_hidden(false, Some(EntityKind::Hostile)));
        assert!(!enemy_preview_hidden(true, Some(EntityKind::Hostile)));
        assert!(!enemy_preview_hidden(false, Some(EntityKind::Player)));
        assert!(!enemy_preview_hidden(false, Some(EntityKind::Ally)));
        assert!(!enemy_preview_hidden(false, None));
    }

    fn zone(caster: u64) -> AoeZone {
        AoeZone {
            radius: 3.0,
            remaining_seconds: 1.0,
            pending_delay_seconds: 0.4,
            spell_id: "meteorite".into(),
            cone_angle_deg: None,
            direction: Vec3::Z,
            caster,
        }
    }

    #[test]
    fn visibility_system_hides_hostile_zones_and_keeps_player_zones() {
        use bevymmo_client::user_settings::GameSettings;

        let mut app = App::new();
        let mut settings = GameSettings::default();
        settings.set_toggle(SettingToggle::ShowEnemyAbilityPreviews, false);
        app.insert_resource(GameSettingsResource(settings));
        app.add_systems(Update, apply_aoe_preview_visibility);

        app.world_mut()
            .spawn((NetworkEntityId(1), EntityKind::Hostile));
        app.world_mut()
            .spawn((NetworkEntityId(2), EntityKind::Player));
        let hostile = app.world_mut().spawn((zone(1), Visibility::Inherited)).id();
        let player = app.world_mut().spawn((zone(2), Visibility::Inherited)).id();

        app.update();

        assert_eq!(
            *app.world().get::<Visibility>(hostile).unwrap(),
            Visibility::Hidden
        );
        assert_eq!(
            *app.world().get::<Visibility>(player).unwrap(),
            Visibility::Inherited
        );

        app.world_mut()
            .resource_mut::<GameSettingsResource>()
            .0
            .set_toggle(SettingToggle::ShowEnemyAbilityPreviews, true);
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(hostile).unwrap(),
            Visibility::Inherited
        );
    }
}
