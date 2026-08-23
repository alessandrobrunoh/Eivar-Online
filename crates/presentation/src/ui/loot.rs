//! Loot bag window: a compact, scrollable slot grid for a sack on the ground.

use bevy::prelude::*;
use bevy::ui::UiScale;
use bevy::window::PrimaryWindow;
use bevymmo_client::local_player::LocalPlayer;
use bevymmo_client::stdb::{commands as stdb_commands, OpenLootBag, StdbConnection, WorldLoot};
use bevymmo_gameplay::entity::components::EntityState;
use bevymmo_gameplay::gathering::in_interact_range;
use bevymmo_gameplay::items::instance::ItemInstance;
use bevymmo_gameplay::items::registry::ItemRegistry;
use bevymmo_gameplay::loot::LOOT_INTERACT_RANGE;
use bevymmo_network::world_components::Position;

use crate::game_state::in_gameplay;
use crate::ui::button::{spawn_bar_child, BarButtonKind};
use crate::ui::card::builder::{framed_chrome_height, CardBuilder, CardFrameAssets};
use crate::ui::card::components::{CardKind, CardWindow};
use crate::ui::inventory::load_item_icon;
use crate::ui::scale::window_to_ui_px;
use crate::ui::scrollbar::{descendant_scroll, spawn_scroll_view_scrolled, ScrollView};
use crate::ui::theme::UiTheme;

/// Wide enough for the four-slot grid plus the scrollbar track once the frame's
/// 64 px inset comes off each side. The old 320 px left 192 px of content for a
/// 250 px grid, so the right-hand column was clipped away.
const LOOT_CARD_WIDTH: f32 = 448.0;
const SLOT_SIZE: f32 = 58.0;
const SLOT_GAP: f32 = 6.0;
const GRID_COLUMNS: u16 = 4;
const GRID_PADDING_Y: f32 = 4.0;
const GOLD_ROW_HEIGHT: f32 = 34.0;
const GOLD_ROW_GAP: f32 = 10.0;
const FOOTER_BUTTON_HEIGHT: f32 = 34.0;
const FOOTER_BUTTON_WIDTH: f32 = 168.0;
/// Height reserved for the "bag is empty" line when there is nothing to show.
const EMPTY_BODY_HEIGHT: f32 = 44.0;
/// Share of the window the card may occupy. The default window is 800x600, so
/// a card built from a full 30-slot corpse bag would otherwise run off both
/// ends of the screen.
const MAX_VIEWPORT_FRACTION: f32 = 0.72;
/// Even on a tall screen the grid stops growing here and starts scrolling: a
/// bag is a transfer window, not a second inventory.
const MAX_VISIBLE_ROWS: u16 = 5;
const SLOT_ACTIVE_PATH: &str = "ui/hud/slot_active.png";
/// Slots sit slightly dimmed so hovering one reads as a highlight. Tinting
/// *up* instead would need values above 1.0, which clip on a non-HDR target.
const SLOT_TINT_IDLE: Color = Color::srgb(0.80, 0.80, 0.80);
const SLOT_TINT_HOVER: Color = Color::WHITE;
const SLOT_TINT_PRESSED: Color = Color::srgb(0.60, 0.60, 0.60);

#[derive(Component, Debug)]
struct LootCard {
    bag_id: u64,
}

#[derive(Component, Debug, Clone, Copy)]
struct LootSlotButton {
    bag_id: u64,
    index: u8,
}

#[derive(Component, Debug, Clone, Copy)]
struct LootTakeGoldButton {
    bag_id: u64,
}

#[derive(Component, Debug, Clone, Copy)]
struct LootTakeAllButton {
    bag_id: u64,
}

pub struct LootUiPlugin;

impl Plugin for LootUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                close_loot_when_far_or_dead,
                sync_loot_window,
                clear_open_loot_if_card_gone,
                handle_loot_clicks,
                highlight_hovered_loot_slot,
            )
                .chain()
                .run_if(in_gameplay),
        );
    }
}

/// Rows of slots the body may show before the grid starts scrolling.
fn visible_rows(total_rows: u16, available: f32) -> u16 {
    let fits = ((available + SLOT_GAP) / (SLOT_SIZE + SLOT_GAP)).floor();
    let cap = if fits >= 1.0 {
        (fits as u16).clamp(1, MAX_VISIBLE_ROWS)
    } else {
        1
    };
    total_rows.clamp(1, cap)
}

fn grid_height(rows: u16) -> f32 {
    let rows = rows.max(1) as f32;
    rows * SLOT_SIZE + (rows - 1.0) * SLOT_GAP + GRID_PADDING_Y * 2.0
}

/// Vertical space the grid may use, given what the chrome and the gold row
/// have already claimed out of the card's viewport budget.
fn grid_budget(viewport_height: f32, gold_block: f32) -> f32 {
    let chrome = framed_chrome_height(FOOTER_BUTTON_HEIGHT);
    (viewport_height * MAX_VIEWPORT_FRACTION - chrome - gold_block).max(SLOT_SIZE)
        - GRID_PADDING_Y * 2.0
}

#[allow(clippy::too_many_arguments)]
fn sync_loot_window(
    mut commands: Commands,
    open: Res<OpenLootBag>,
    loot: Res<WorldLoot>,
    theme: Res<UiTheme>,
    registry: Res<ItemRegistry>,
    asset_server: Res<AssetServer>,
    ui_scale: Res<UiScale>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    existing: Query<(Entity, &LootCard)>,
    card_windows: Query<(Entity, &CardWindow)>,
    children: Query<&Children>,
    scroll_views: Query<&ScrollView>,
) {
    let wanted = open.0.filter(|id| loot.bags.contains_key(id));
    let current = existing
        .iter()
        .next()
        .map(|(entity, card)| (entity, card.bag_id));
    let viewport_height = primary_window
        .single()
        .map(|window| window_to_ui_px(Vec2::new(0.0, window.height()), &ui_scale).y)
        .unwrap_or(600.0);

    if current.map(|(_, id)| id) == wanted {
        if wanted.is_some() && (open.is_changed() || loot.is_changed()) {
            // The card is rebuilt wholesale whenever a row changes, so the
            // grid's offset has to be carried across: without it, taking the
            // last item of a long bag snaps the list back to the top.
            let scroll = current
                .map(|(entity, _)| descendant_scroll(entity, &children, &scroll_views))
                .unwrap_or(0.0);
            if let Some((entity, _)) = current {
                commands.entity(entity).despawn();
            }
            if let Some(bag_id) = wanted {
                spawn_loot_card(
                    &mut commands,
                    &theme,
                    &registry,
                    &asset_server,
                    viewport_height,
                    scroll,
                    bag_id,
                    loot.bags.get(&bag_id).expect("filtered"),
                );
            }
        }
        return;
    }

    for (entity, window) in card_windows.iter() {
        if window.kind == CardKind::Loot {
            commands.entity(entity).despawn();
        }
    }
    if let Some(bag_id) = wanted {
        spawn_loot_card(
            &mut commands,
            &theme,
            &registry,
            &asset_server,
            viewport_height,
            0.0,
            bag_id,
            loot.bags.get(&bag_id).expect("filtered"),
        );
    }
}

fn close_loot_when_far_or_dead(
    mut open: ResMut<OpenLootBag>,
    loot: Res<WorldLoot>,
    player: Query<(&Position, Option<&EntityState>), With<LocalPlayer>>,
) {
    let Some(bag_id) = open.0 else {
        return;
    };
    let Ok((player_pos, state)) = player.single() else {
        open.0 = None;
        return;
    };
    if state.is_some_and(|state| state.is_dead()) {
        open.0 = None;
        return;
    }
    let Some(bag) = loot.bags.get(&bag_id) else {
        open.0 = None;
        return;
    };
    if !in_interact_range(
        player_pos.0.x,
        player_pos.0.z,
        bag.position.x,
        bag.position.z,
        LOOT_INTERACT_RANGE,
    ) {
        open.0 = None;
    }
}

fn clear_open_loot_if_card_gone(cards: Query<(), With<LootCard>>, mut open: ResMut<OpenLootBag>) {
    if open.is_changed() {
        return;
    }
    if open.0.is_some() && cards.is_empty() {
        open.0 = None;
    }
}

fn handle_loot_clicks(
    slots: Query<(&Interaction, &LootSlotButton), Changed<Interaction>>,
    gold: Query<(&Interaction, &LootTakeGoldButton), Changed<Interaction>>,
    all: Query<(&Interaction, &LootTakeAllButton), Changed<Interaction>>,
    conn: Option<Res<StdbConnection>>,
) {
    let Some(conn) = conn else {
        return;
    };
    for (interaction, button) in slots.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let _ = stdb_commands::loot_take(&conn, button.bag_id, button.index);
    }
    for (interaction, button) in gold.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let _ = stdb_commands::loot_take_gold(&conn, button.bag_id);
    }
    for (interaction, button) in all.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let _ = stdb_commands::loot_take_all(&conn, button.bag_id);
    }
}

/// Slots are plain `ImageNode`s rather than bar buttons, so they get their
/// hover state from a tint here instead of from `update_button_visuals`.
fn highlight_hovered_loot_slot(
    mut slots: Query<(&Interaction, &mut ImageNode), (Changed<Interaction>, With<LootSlotButton>)>,
) {
    for (interaction, mut image) in slots.iter_mut() {
        image.color = match interaction {
            Interaction::Pressed => SLOT_TINT_PRESSED,
            Interaction::Hovered => SLOT_TINT_HOVER,
            Interaction::None => SLOT_TINT_IDLE,
        };
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_loot_card(
    commands: &mut Commands,
    theme: &UiTheme,
    registry: &ItemRegistry,
    asset_server: &AssetServer,
    viewport_height: f32,
    initial_scroll: f32,
    bag_id: u64,
    bag: &bevymmo_client::stdb::LootBagView,
) {
    let slot_image: Handle<Image> = asset_server.load(SLOT_ACTIVE_PATH);
    let slots = bag.slots.clone();
    let gold = bag.gold;
    let text_color = theme.text_color;
    let muted_color = theme.muted_text_color;
    let font_size = theme.button_font_size * 0.55;

    let gold_block = if gold > 0 {
        GOLD_ROW_HEIGHT + GOLD_ROW_GAP
    } else {
        0.0
    };
    let body_height = if slots.is_empty() {
        EMPTY_BODY_HEIGHT
    } else {
        let rows = (slots.len() as u16).div_ceil(GRID_COLUMNS);
        grid_height(visible_rows(rows, grid_budget(viewport_height, gold_block)))
    };
    let card_height = framed_chrome_height(FOOTER_BUTTON_HEIGHT) + gold_block + body_height;

    let card = CardBuilder::new(CardKind::Loot, "Loot")
        .frame(CardFrameAssets::load(asset_server))
        .width(Val::Px(LOOT_CARD_WIDTH))
        // A definite height is what keeps the card centred and bounded: given
        // `Val::Auto` the builder pins it to both viewport edges instead, and
        // it stretches to the full screen height.
        .height(Val::Px(card_height))
        .closeable()
        .exclusive()
        .with_body(move |body| {
            if gold > 0 {
                body.spawn(Node {
                    width: Val::Percent(100.0),
                    margin: UiRect::bottom(Val::Px(GOLD_ROW_GAP)),
                    flex_shrink: 0.0,
                    ..default()
                })
                .with_children(|row| {
                    spawn_bar_child(
                        row,
                        format!("Take {gold} gold"),
                        16.0,
                        text_color,
                        Val::Percent(100.0),
                        Val::Px(GOLD_ROW_HEIGHT),
                        BarButtonKind::Neutral,
                        LootTakeGoldButton { bag_id },
                    );
                });
            }

            if slots.is_empty() {
                body.spawn(Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(EMPTY_BODY_HEIGHT),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                })
                .with_children(|row| {
                    row.spawn((
                        Text::new("Nothing left in the bag."),
                        TextFont {
                            font_size: FontSize::Px(15.0),
                            ..default()
                        },
                        TextColor(muted_color),
                    ));
                });
                return;
            }

            let section = body
                .spawn(Node {
                    width: Val::Percent(100.0),
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    min_height: Val::Px(0.0),
                    flex_direction: FlexDirection::Column,
                    ..default()
                })
                .id();
            let mut commands = body.commands();
            spawn_scroll_view_scrolled(
                &mut commands,
                section,
                theme,
                initial_scroll,
                move |commands| {
                    commands
                        .spawn(Node::default())
                        .with_children(|content| {
                            content
                                .spawn(Node {
                                    width: Val::Percent(100.0),
                                    display: Display::Grid,
                                    grid_template_columns: RepeatedGridTrack::px(
                                        GRID_COLUMNS,
                                        SLOT_SIZE,
                                    ),
                                    // Auto rows, not a fixed count: the grid is
                                    // taller than its viewport now and the
                                    // scroll view is what bounds it.
                                    grid_auto_rows: vec![GridTrack::px(SLOT_SIZE)],
                                    row_gap: Val::Px(SLOT_GAP),
                                    column_gap: Val::Px(SLOT_GAP),
                                    justify_content: JustifyContent::Center,
                                    padding: UiRect::vertical(Val::Px(GRID_PADDING_Y)),
                                    ..default()
                                })
                                .with_children(|grid| {
                                    for (index, instance) in slots {
                                        spawn_loot_slot(
                                            grid,
                                            registry,
                                            asset_server,
                                            &slot_image,
                                            text_color,
                                            font_size,
                                            bag_id,
                                            index,
                                            &instance,
                                        );
                                    }
                                });
                        })
                        .id()
                },
            );
        })
        .with_footer(move |footer| {
            spawn_bar_child(
                footer,
                "Take All",
                16.0,
                text_color,
                Val::Px(FOOTER_BUTTON_WIDTH),
                Val::Px(FOOTER_BUTTON_HEIGHT),
                BarButtonKind::Primary,
                LootTakeAllButton { bag_id },
            );
        })
        .spawn(commands, theme);

    commands.entity(card).insert(LootCard { bag_id });
}

#[allow(clippy::too_many_arguments)]
fn spawn_loot_slot(
    parent: &mut ChildSpawnerCommands,
    registry: &ItemRegistry,
    asset_server: &AssetServer,
    slot_image: &Handle<Image>,
    text_color: Color,
    font_size: f32,
    bag_id: u64,
    index: u8,
    instance: &ItemInstance,
) {
    let icon = load_item_icon(asset_server, registry, &instance.item_id);
    let label = if icon.is_some() {
        if instance.quantity > 1 {
            instance.quantity.to_string()
        } else {
            String::new()
        }
    } else {
        let display = registry
            .get(&instance.item_id)
            .map(|item| item.display_name().to_string())
            .unwrap_or_else(|| instance.item_id.as_str().to_string());
        if instance.quantity > 1 {
            format!("{display} x{}", instance.quantity)
        } else {
            display
        }
    };
    let icon_visible = icon.is_some();

    parent
        .spawn((
            Button,
            Node {
                width: Val::Px(SLOT_SIZE),
                height: Val::Px(SLOT_SIZE),
                position_type: PositionType::Relative,
                justify_content: JustifyContent::Center,
                // The stack count rides the bottom edge of the icon instead of
                // sitting across its middle, where a centred label landed.
                align_items: AlignItems::End,
                padding: UiRect::all(Val::Px(3.0)),
                overflow: Overflow::clip(),
                ..default()
            },
            ImageNode::new(slot_image.clone())
                .with_mode(NodeImageMode::Stretch)
                .with_color(SLOT_TINT_IDLE),
            LootSlotButton { bag_id, index },
        ))
        .with_children(|button| {
            button.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(6.0),
                    right: Val::Px(6.0),
                    top: Val::Px(6.0),
                    bottom: Val::Px(6.0),
                    ..default()
                },
                ImageNode {
                    image: icon.unwrap_or_default(),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                if icon_visible {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                },
            ));
            button.spawn((
                Text::new(label),
                TextFont {
                    font_size: FontSize::Px(font_size),
                    ..default()
                },
                TextColor(text_color),
                TextLayout::justify(Justify::Center),
            ));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_bag_shows_every_row() {
        assert_eq!(visible_rows(1, 400.0), 1);
        assert_eq!(visible_rows(3, 400.0), 3);
    }

    #[test]
    fn a_long_bag_stops_at_the_row_cap() {
        assert_eq!(visible_rows(20, 4000.0), MAX_VISIBLE_ROWS);
    }

    #[test]
    fn a_short_window_wins_over_the_row_cap() {
        // Room for two rows only: 2 * 58 + 6 = 122.
        assert_eq!(visible_rows(20, 122.0), 2);
    }

    #[test]
    fn a_cramped_window_still_shows_one_row() {
        assert_eq!(visible_rows(20, 0.0), 1);
        assert_eq!(visible_rows(20, -80.0), 1);
    }

    #[test]
    fn grid_height_counts_gaps_between_rows_only() {
        let pad = GRID_PADDING_Y * 2.0;
        assert_eq!(grid_height(1), SLOT_SIZE + pad);
        assert_eq!(grid_height(3), 3.0 * SLOT_SIZE + 2.0 * SLOT_GAP + pad);
    }

    #[test]
    fn a_full_corpse_bag_stays_inside_the_default_window() {
        let viewport = 600.0;
        let gold_block = GOLD_ROW_HEIGHT + GOLD_ROW_GAP;
        // Thirty inventory slots plus equipment: eight rows of four.
        let rows = visible_rows(8, grid_budget(viewport, gold_block));
        let height = framed_chrome_height(FOOTER_BUTTON_HEIGHT) + gold_block + grid_height(rows);
        assert!(
            height <= viewport,
            "card is {height} px tall on a {viewport} px window"
        );
    }
}
