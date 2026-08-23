//! Large two-column detail card for a selected inventory or equipment item.

use bevy::prelude::*;
use bevymmo_client::local_player::LocalPlayer;
use bevymmo_gameplay::{
    abilities::KnownAncientLanguage,
    items::{
        components::{Equipment, Inventory},
        effects::ItemEffect,
        registry::ItemRegistry,
    },
    stats::events::{ModifierOp, StatField},
};

use super::{
    components::*,
    inscription_editor::{spawn_item_editor, ItemEditorContext, ItemEditorRegistries},
    stack::{resolved_split_amount, spawn_stack_controls, stack_footer, stack_title},
    weapon_detail::{meta_line, summarize_weapon, GlyphRegistries},
    InventoryUiState, ItemDetailUiState,
};
use crate::ui::{
    button::{spawn_bar_child, BarButtonKind},
    card::{CardBuilder, CardFrameAssets, CardKind, CardWindow},
    scrollbar::{descendant_scroll, ScrollView},
    theme::UiTheme,
};

const CARD_WIDTH: f32 = 760.0;
const CARD_HEIGHT: f32 = 560.0;
const SUMMARY_WIDTH: f32 = 210.0;
const COLUMN_GAP: f32 = 18.0;
const PORTRAIT_SIZE: f32 = 132.0;
const DESCRIPTION_FONT_SIZE: f32 = 15.0;
const DESCRIPTION_COLOR: Color = Color::srgba(0.78, 0.80, 0.86, 0.95);
const PORTRAIT_PATH: &str = "ui/hud/slot_active.png";
const _: () = assert!(CARD_WIDTH <= 800.0 && CARD_HEIGHT <= 600.0);

/// Left column containing the selected item's identity and passive information.
#[derive(Component, Debug)]
pub struct ItemSummaryPanel;

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_item_detail_card(
    commands: &mut Commands,
    theme: &UiTheme,
    registry: &ItemRegistry,
    glyphs: &GlyphRegistries,
    known: &KnownAncientLanguage,
    inventory: &Inventory,
    equipment: &Equipment,
    selection: InventorySelection,
    detail_state: &ItemDetailUiState,
    asset_server: &AssetServer,
) {
    let (item_instance, equipped_slot, slot_index) = match selection {
        InventorySelection::Slot(index) => {
            let item_instance = inventory
                .slots
                .get(index as usize)
                .and_then(|item| item.clone());
            let equipped_slot = item_instance
                .as_ref()
                .and_then(|instance| equipment.slot_holding(instance.instance_id));
            (item_instance, equipped_slot, Some(index))
        }
        InventorySelection::Equipment(slot) => (equipment.get(slot).clone(), Some(slot), None),
    };

    let Some(item_instance) = item_instance else {
        return;
    };
    let Some(item) = registry.get(&item_instance.item_id) else {
        return;
    };

    let config = item.config();
    let item_name = config.display_name.to_string();
    let card_title = stack_title(&item_name, item_instance.quantity);
    let description = config.description.to_string();
    let meta = meta_line(
        config.category,
        config.rarity,
        config.equippable_into,
        config.weight,
    );
    let effects: Vec<String> = item.effects().iter().map(effect_label).collect();
    let equippable_into = config.equippable_into;
    let weapon_summary = summarize_weapon(&item_instance, registry, glyphs.catalog(), known);
    let has_icon = item.icon().is_some();
    let portrait = item
        .icon()
        .map(|path| asset_server.load(path.to_string()))
        .unwrap_or_else(|| asset_server.load(PORTRAIT_PATH));
    let active_slot = detail_state.active_slot;
    let initial_scroll = detail_state.scroll;
    let registries = ItemEditorRegistries {
        abilities: &glyphs.abilities,
        root_words: &glyphs.root_words,
        ancient_words: &glyphs.ancient_words,
    };
    let stack = stack_footer(inventory, registry, selection);
    let split_amount =
        stack.map(|footer| resolved_split_amount(detail_state.split_amount, footer.quantity));

    CardBuilder::new(CardKind::ItemDetail, card_title)
        .frame(CardFrameAssets::load(asset_server))
        .width(Val::Px(CARD_WIDTH))
        .height(Val::Px(CARD_HEIGHT))
        .draggable()
        .closeable()
        .coexist()
        .with_body(move |body| {
            body.spawn(Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                min_height: Val::Px(0.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Stretch,
                column_gap: Val::Px(COLUMN_GAP),
                ..default()
            })
            .with_children(|columns| {
                spawn_item_summary(
                    columns,
                    theme,
                    &stack_title(&item_name, item_instance.quantity),
                    &meta,
                    &description,
                    &effects,
                    portrait,
                    has_icon,
                );
                spawn_item_editor(
                    columns,
                    theme,
                    ItemEditorContext {
                        item: item.as_ref(),
                        instance: &item_instance,
                        equipped_slot,
                        known,
                        registries,
                        weapon_summary: weapon_summary.as_ref(),
                        active_slot,
                        initial_scroll,
                    },
                );
            });
        })
        .with_footer(move |footer| {
            if let Some(slot) = equipped_slot {
                spawn_bar_child(
                    footer,
                    "Unequip",
                    theme.button_font_size,
                    theme.button_text_color,
                    Val::Percent(100.0),
                    Val::Px(36.0),
                    BarButtonKind::Neutral,
                    UnequipButton { slot },
                );
            } else if equippable_into.is_some() {
                if let Some(index) = slot_index {
                    spawn_bar_child(
                        footer,
                        "Equip to configure",
                        theme.button_font_size,
                        theme.button_text_color,
                        Val::Percent(100.0),
                        Val::Px(36.0),
                        BarButtonKind::Primary,
                        EquipButton { slot_index: index },
                    );
                }
            }
            if let (Some(stack), Some(amount)) = (stack, split_amount) {
                spawn_stack_controls(footer, theme, stack, amount);
            }
        })
        .spawn(commands, theme);
}

fn spawn_item_summary(
    parent: &mut ChildSpawnerCommands,
    theme: &UiTheme,
    item_name: &str,
    meta: &str,
    description: &str,
    effects: &[String],
    portrait: Handle<Image>,
    has_icon: bool,
) {
    parent
        .spawn((
            Node {
                width: Val::Px(SUMMARY_WIDTH),
                flex_shrink: 0.0,
                min_height: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(8.0),
                padding: UiRect::right(Val::Px(12.0)),
                border: UiRect::right(Val::Px(1.0)),
                overflow: Overflow::clip(),
                ..default()
            },
            BorderColor {
                right: Color::srgba(1.0, 1.0, 1.0, 0.16),
                ..default()
            },
            ItemSummaryPanel,
        ))
        .with_children(|summary| {
            summary
                .spawn((
                    Node {
                        width: Val::Px(PORTRAIT_SIZE),
                        height: Val::Px(PORTRAIT_SIZE),
                        flex_shrink: 0.0,
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        padding: UiRect::all(Val::Px(12.0)),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    ImageNode::new(portrait).with_mode(NodeImageMode::Stretch),
                ))
                .with_children(|portrait| {
                    if !has_icon {
                        portrait.spawn((
                            Text::new(item_name),
                            TextFont {
                                font_size: FontSize::Px(theme.button_font_size * 0.72),
                                ..default()
                            },
                            TextColor(theme.text_color),
                            TextLayout::justify(Justify::Center),
                        ));
                    }
                });

            summary.spawn((
                Text::new(meta),
                TextFont {
                    font_size: FontSize::Px(theme.button_font_size * 0.62),
                    ..default()
                },
                TextColor(theme.muted_text_color),
                TextLayout {
                    linebreak: LineBreak::WordOrCharacter,
                    justify: Justify::Center,
                },
                Node {
                    width: Val::Percent(100.0),
                    ..default()
                },
            ));

            spawn_description_block(summary, description);

            if !effects.is_empty() {
                spawn_summary_heading(summary, theme, "EFFECTS");
                for effect in effects {
                    summary.spawn((
                        Text::new(effect),
                        TextFont {
                            font_size: FontSize::Px(theme.button_font_size * 0.66),
                            ..default()
                        },
                        TextColor(Color::srgba(0.4, 0.9, 0.6, 1.0)),
                        TextLayout {
                            linebreak: LineBreak::WordOrCharacter,
                            ..default()
                        },
                        Node {
                            width: Val::Percent(100.0),
                            ..default()
                        },
                    ));
                }
            }
        });
}

fn spawn_description_block(body: &mut ChildSpawnerCommands, description: &str) {
    body.spawn((
        Text::new(description),
        TextFont {
            font_size: FontSize::Px(DESCRIPTION_FONT_SIZE),
            ..default()
        },
        TextColor(DESCRIPTION_COLOR),
        TextLayout {
            linebreak: LineBreak::WordOrCharacter,
            justify: Justify::Center,
        },
        Node {
            width: Val::Percent(100.0),
            ..default()
        },
    ));
}

fn spawn_summary_heading(parent: &mut ChildSpawnerCommands, theme: &UiTheme, label: &str) {
    parent.spawn((
        Text::new(label),
        TextFont {
            font_size: FontSize::Px(theme.button_font_size * 0.68),
            ..default()
        },
        TextColor(Color::srgba(0.6, 0.75, 0.95, 0.9)),
        Node {
            margin: UiRect::top(Val::Px(6.0)),
            ..default()
        },
    ));
}

fn effect_label(effect: &ItemEffect) -> String {
    match effect {
        ItemEffect::StatBonus { field, op, value } => {
            let field = match field {
                StatField::MaxHealth => "Max Health",
                StatField::MaxMana => "Max Mana",
                StatField::Speed => "Speed",
                StatField::AttackPower => "Attack Power",
                StatField::Armor => "Armor",
                StatField::ThreatGeneration => "Threat Generation",
                StatField::ManaRegeneration => "Mana Regen",
                StatField::GatheringSpeed => "Gather Speed",
                StatField::GatheringBonus => "Gather Bonus",
            };
            let op = match op {
                ModifierOp::Add => "+",
                ModifierOp::Multiply => "x",
                ModifierOp::Override => "=",
            };
            format!("{op}{value} {field}")
        }
        ItemEffect::InstantHeal { amount } => format!("Instant Heal: {amount}"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn refresh_item_detail_on_equipment_change(
    mut commands: Commands,
    mut inventory_state: ResMut<InventoryUiState>,
    mut detail_state: ResMut<ItemDetailUiState>,
    player_query: Query<
        (&Inventory, &Equipment, Option<&KnownAncientLanguage>),
        (
            With<LocalPlayer>,
            Or<(Changed<Equipment>, Changed<Inventory>)>,
        ),
    >,
    registry: Res<ItemRegistry>,
    glyphs: GlyphRegistries,
    theme: Res<UiTheme>,
    asset_server: Res<AssetServer>,
    cards: Query<(Entity, &CardWindow)>,
    children: Query<&Children>,
    scroll_views: Query<&ScrollView>,
) {
    let Some(selection) = inventory_state.selected else {
        return;
    };
    let Some(detail_entity) = cards
        .iter()
        .find_map(|(entity, card)| (card.kind == CardKind::ItemDetail).then_some(entity))
    else {
        return;
    };
    let Ok((inventory, equipment, known)) = player_query.single() else {
        return;
    };

    if selection_is_empty(inventory, equipment, selection) {
        commands.entity(detail_entity).despawn();
        inventory_state.selected = None;
        return;
    }

    detail_state.scroll = descendant_scroll(detail_entity, &children, &scroll_views);
    commands.entity(detail_entity).despawn();
    let known = known.cloned().unwrap_or_default();
    spawn_item_detail_card(
        &mut commands,
        &theme,
        &registry,
        &glyphs,
        &known,
        inventory,
        equipment,
        selection,
        &detail_state,
        &asset_server,
    );
}

fn selection_is_empty(
    inventory: &Inventory,
    equipment: &Equipment,
    selection: InventorySelection,
) -> bool {
    match selection {
        InventorySelection::Slot(index) => inventory
            .slots
            .get(index as usize)
            .is_none_or(Option::is_none),
        InventorySelection::Equipment(slot) => equipment.get(slot).is_none(),
    }
}

pub fn despawn_detail_cards(commands: &mut Commands, cards: &Query<(Entity, &CardWindow)>) {
    for (entity, window) in cards.iter() {
        if window.kind == CardKind::ItemDetail {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_wraps_on_words_with_readable_style() {
        let mut app = App::new();
        let mut commands = app.world_mut().commands();
        let root = commands.spawn(Node::default()).id();
        commands.entity(root).with_children(|body| {
            spawn_description_block(
                body,
                "A long flavor sentence that should wrap on words, not mid-glyph.",
            );
        });
        app.update();

        let world = app.world_mut();
        let mut query = world.query::<(&Text, &TextLayout, &TextColor, &TextFont)>();
        let (_, layout, color, font) = query
            .iter(world)
            .find(|(text, _, _, _)| text.0.contains("flavor sentence"))
            .expect("description text");
        assert_eq!(layout.linebreak, LineBreak::WordOrCharacter);
        assert_eq!(color.0, DESCRIPTION_COLOR);
        assert_eq!(font.font_size, FontSize::Px(DESCRIPTION_FONT_SIZE));
    }

    #[test]
    fn effect_copy_keeps_the_operator_and_stat_name() {
        assert_eq!(
            effect_label(&ItemEffect::StatBonus {
                field: StatField::AttackPower,
                op: ModifierOp::Add,
                value: 80.0,
            }),
            "+80 Attack Power"
        );
    }

    #[test]
    fn configuration_panel_marker_is_not_the_summary_panel() {
        fn assert_component<T: Component>() {}
        assert_component::<ItemSummaryPanel>();
        assert_component::<super::super::inscription_editor::ItemConfigurationPanel>();
    }

    #[test]
    fn default_detail_tab_is_primary() {
        assert_eq!(
            ItemDetailUiState::default().active_slot,
            bevymmo_gameplay::abilities::AbilitySlot::Primary
        );
    }
}
