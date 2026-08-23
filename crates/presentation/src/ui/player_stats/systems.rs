use bevy::prelude::*;
use bevymmo_client::local_player::LocalPlayer;
use bevymmo_client::stdb::LocalGold;

use bevymmo_client::movement::effective_movement_speed;
use bevymmo_client::network::types::ClientConnectionConfig;
use bevymmo_gameplay::stats::components::{
    CombatStats, GatheringStats, MovementStats, ShieldStats, VitalStats,
};
use bevymmo_gameplay::stats::modifiers::ActiveStatModifiers;
use bevymmo_network::network::protocol::PlayerId;

use crate::game_state::Screen;

use crate::ui::text::spawn_text;
use crate::ui::theme::UiTheme;

use super::plugin::{PlayerStatsText, PlayerStatsUi};

const PANEL_OFFSET: f32 = 16.0;

pub fn setup_player_stats(mut commands: Commands, theme: Res<UiTheme>) {
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(PANEL_OFFSET),
                right: Val::Px(PANEL_OFFSET),
                padding: UiRect::all(Val::Px(12.0)),
                ..default()
            },
            BackgroundColor(theme.panel_bg),
            PlayerStatsUi,
        ))
        .id();

    let text = spawn_text(
        &mut commands,
        root,
        "Waiting for Player stats...",
        theme.hp_font_size,
        theme.text_color,
    );
    commands.entity(text).insert(PlayerStatsText);
}

pub fn update_player_stats(
    screen: Res<State<Screen>>,
    client_config: Option<Res<ClientConnectionConfig>>,

    player_query: Query<(
        &MovementStats,
        &CombatStats,
        &VitalStats,
        Option<&ShieldStats>,
        Option<&GatheringStats>,
        Option<&PlayerId>,
        Has<LocalPlayer>,
        Option<&ActiveStatModifiers>,
    )>,
    mut root_query: Query<&mut Node, With<PlayerStatsUi>>,
    mut text_query: Query<&mut Text, With<PlayerStatsText>>,
    mut last_text: Local<String>,
    gold: Option<Res<LocalGold>>,
) {
    let Ok(mut root) = root_query.single_mut() else {
        return;
    };

    if *screen.get() != Screen::InGame {
        root.display = Display::None;
        return;
    }

    root.display = Display::Flex;
    let local_client_id = client_config.map(|config| config.client_id);
    let Some((movement, combat, vital, shield, gathering, _, _, modifiers)) = player_query
        .iter()
        .find(|(_, _, _, _, _, _, controlled, _)| *controlled)
        .or_else(|| {
            player_query.iter().find(|(_, _, _, _, _, player_id, _, _)| {
                player_id.is_some_and(|id| {
                    local_client_id.is_some_and(|client_id| id.0.to_bits() == client_id)
                })
            })
        })
    else {
        return;
    };
    let Ok(mut text) = text_query.single_mut() else {
        return;
    };

    let gold_amount = gold.map(|g| g.amount).unwrap_or(0);
    let shield = shield.copied().unwrap_or_default();
    let gathering = gathering.copied().unwrap_or_default();
    let new_text = format_stats(movement, combat, vital, &shield, &gathering, modifiers, gold_amount);
    if *last_text != new_text {
        text.0 = new_text.clone();
        *last_text = new_text;
    }
}

fn format_stats(
    movement: &MovementStats,
    combat: &CombatStats,
    vital: &VitalStats,
    shield: &ShieldStats,
    gathering: &GatheringStats,
    modifiers: Option<&ActiveStatModifiers>,
    gold: u64,
) -> String {
    let move_speed = displayed_movement_speed(movement.speed, modifiers);
    format!(
        "HP: {}/{}\nShield: {}/{}\nMana: {}/{}\nMana Regen: {:.1}/s\nArmor: {} ({}% reduction)\nAttack Power: {}\nMove Speed: {:.2}\nGather Speed: {}\nGather Bonus: {}%\nGold: {}",
        format_value(vital.current_health),
        format_value(vital.max_health),
        format_value(shield.current),
        format_value(shield.max),
        format_value(vital.current_mana),
        format_value(vital.max_mana),
        vital.mana_regeneration,
        format_value(combat.armor),
        combat.armor_damage_reduction() * 100.0,
        format_value(combat.attack_power),
        move_speed,
        format_value(gathering.speed),
        format_value(gathering.bonus * 100.0),
        gold,
    )
}

fn displayed_movement_speed(base_speed: f32, modifiers: Option<&ActiveStatModifiers>) -> f32 {
    effective_movement_speed(base_speed, modifiers)
}

fn format_value(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}
