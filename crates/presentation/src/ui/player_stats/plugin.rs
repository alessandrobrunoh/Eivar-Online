//! Pannello con le statistiche del Player locale.

use bevy::prelude::*;

use super::systems::{setup_player_stats, update_player_stats};

/// Marker del nodo root del pannello stats.
#[derive(Component)]
pub struct PlayerStatsUi;

/// Marker del testo aggiornato dal sistema stats.
#[derive(Component)]
pub struct PlayerStatsText;

pub struct PlayerStatsPlugin;

impl Plugin for PlayerStatsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_player_stats);
        app.add_systems(Update, update_player_stats);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_state::{init_screen_states, Screen};
    use crate::ui::theme::UiTheme;
    use bevymmo_client::local_player::LocalPlayer;
    use bevymmo_client::stdb::LocalGold;
    use bevymmo_gameplay::stats::components::{
        CombatStats, MovementStats, ShieldStats, VitalStats,
    };

    fn test_app() -> App {
        let mut app = App::new();
        app.init_resource::<UiTheme>();
        init_screen_states(&mut app);
        app.add_plugins(PlayerStatsPlugin);
        app.insert_state(Screen::InGame);
        app
    }

    fn panel_text(app: &mut App) -> String {
        let text_entity = app
            .world_mut()
            .query_filtered::<Entity, With<PlayerStatsText>>()
            .single(app.world())
            .expect("stats text");
        app.world()
            .entity(text_entity)
            .get::<Text>()
            .expect("Text component")
            .0
            .clone()
    }

    #[test]
    fn shows_local_player_stats_in_the_top_right_panel() {
        let mut app = test_app();
        app.world_mut().spawn((
            LocalPlayer,
            MovementStats { speed: 0.15 },
            CombatStats {
                attack_power: 10.0,
                armor: 100.0,
                threat_generation: 1.0,
            },
            ShieldStats {
                current: 1000.0,
                max: 1000.0,
            },
            VitalStats {
                current_health: 100.0,
                max_health: 100.0,
                current_mana: 80.0,
                max_mana: 80.0,
                mana_regeneration: 4.0,
            },
        ));
        app.update();

        let root = app
            .world_mut()
            .query_filtered::<&Node, With<PlayerStatsUi>>()
            .single(app.world())
            .expect("stats root");

        assert_eq!(root.position_type, PositionType::Absolute);
        assert_eq!(root.right, Val::Px(16.0));
        assert_eq!(root.top, Val::Px(16.0));
        assert_eq!(
            panel_text(&mut app),
            "HP: 100/100\nShield: 1000/1000\nMana: 80/80\nMana Regen: 4.0/s\nArmor: 100 (50% reduction)\nAttack Power: 10\nMove Speed: 0.15\nGather Speed: 0\nGather Bonus: 0%\nGold: 0"
        );
    }

    #[test]
    fn updates_when_local_player_stats_change_and_hides_outside_gameplay() {
        let mut app = test_app();
        let player = app
            .world_mut()
            .spawn((
                LocalPlayer,
                MovementStats { speed: 0.15 },
                CombatStats {
                    attack_power: 10.0,
                    armor: 0.0,
                    threat_generation: 1.0,
                },
                VitalStats {
                    current_health: 100.0,
                    max_health: 100.0,
                    current_mana: 80.0,
                    max_mana: 80.0,
                    mana_regeneration: 4.0,
                },
            ))
            .id();
        app.update();

        app.world_mut()
            .entity_mut(player)
            .get_mut::<VitalStats>()
            .unwrap()
            .max_mana = 120.0;
        app.insert_state(Screen::MainMenu);
        app.update();

        let root = app
            .world_mut()
            .query_filtered::<&Node, With<PlayerStatsUi>>()
            .single(app.world())
            .expect("stats root");
        assert_eq!(root.display, Display::None);

        app.insert_state(Screen::InGame);
        app.update();
        assert_eq!(
            panel_text(&mut app),
            "HP: 100/100\nShield: 0/0\nMana: 80/120\nMana Regen: 4.0/s\nArmor: 0 (0% reduction)\nAttack Power: 10\nMove Speed: 0.15\nGather Speed: 0\nGather Bonus: 0%\nGold: 0"
        );
    }

    #[test]
    fn shows_local_character_gold() {
        let mut app = test_app();
        app.insert_resource(LocalGold { amount: 150 });
        app.world_mut().spawn((
            LocalPlayer,
            MovementStats { speed: 0.15 },
            CombatStats {
                attack_power: 10.0,
                armor: 0.0,
                threat_generation: 1.0,
            },
            VitalStats {
                current_health: 100.0,
                max_health: 100.0,
                current_mana: 80.0,
                max_mana: 80.0,
                mana_regeneration: 4.0,
            },
        ));
        app.update();
        assert_eq!(
            panel_text(&mut app),
            "HP: 100/100\nShield: 0/0\nMana: 80/80\nMana Regen: 4.0/s\nArmor: 0 (0% reduction)\nAttack Power: 10\nMove Speed: 0.15\nGather Speed: 0\nGather Bonus: 0%\nGold: 150"
        );
    }
}
