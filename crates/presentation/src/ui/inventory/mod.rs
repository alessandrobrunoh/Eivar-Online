//! Inventory UI plugin definition and state.

pub mod components;
pub mod detail;
pub mod drag;
mod equipment_section;
mod inscription_editor;
mod inventory_section;
mod stack;
pub mod systems;
pub mod weapon_detail;

use bevy::prelude::*;
use bevymmo_gameplay::{
    abilities::AbilitySlot,
    items::{
        components::{Equipment, Inventory},
        registry::{ItemId, ItemRegistry},
    },
};

use crate::{
    game_state::{in_gameplay, not_typing},
    ui::{
        card::{
            builder::{CardBuilder, CardFrameAssets},
            components::{CardKind, CardPositioning},
        },
        theme::UiTheme,
    },
};
use components::{InventorySelection, InventorySlotImages};
pub use drag::ItemDragState;
use equipment_section::spawn_equipment_section;
use inventory_section::spawn_inventory_section;

const INVENTORY_CARD_WIDTH: f32 = 448.0;
const INNER_CONTENT_PADDING: f32 = 12.0;
const SECTION_GAP: f32 = 8.0;
const SLOT_EMPTY_PATH: &str = "ui/hud/slot_empty_01.png";
const SLOT_ACTIVE_PATH: &str = "ui/hud/slot_active.png";

pub(crate) fn load_item_icon(
    asset_server: &AssetServer,
    registry: &ItemRegistry,
    item_id: &ItemId,
) -> Option<Handle<Image>> {
    registry
        .get(item_id)
        .and_then(|item| item.icon())
        .map(|path| asset_server.load(path.to_string()))
}

/// Root marker for the main inventory card.
#[derive(Component, Debug)]
pub struct InventoryCard;

/// Global state resource for the Inventory UI.
#[derive(Resource, Default)]
pub struct InventoryUiState {
    pub is_open: bool,
    pub selected: Option<InventorySelection>,
}

#[derive(Resource)]
pub(super) struct ItemDetailUiState {
    pub active_slot: AbilitySlot,
    pub scroll: f32,
    /// 0 means "use the default half-stack amount for the current pile".
    pub split_amount: u32,
}

impl Default for ItemDetailUiState {
    fn default() -> Self {
        Self {
            active_slot: AbilitySlot::Primary,
            scroll: 0.0,
            split_amount: 0,
        }
    }
}

pub(super) fn spawn_inventory_window(
    commands: &mut Commands,
    theme: &UiTheme,
    registry: &ItemRegistry,
    inventory: &Inventory,
    equipment: &Equipment,
    asset_server: &AssetServer,
) {
    let inventory = inventory.clone();
    let equipment = equipment.clone();
    let slot_images = InventorySlotImages {
        empty: asset_server.load(SLOT_EMPTY_PATH),
        active: asset_server.load(SLOT_ACTIVE_PATH),
    };

    let card = CardBuilder::new(CardKind::Inventory, "Inventory")
        .frame(CardFrameAssets::load(asset_server))
        .headerless()
        .width(Val::Px(INVENTORY_CARD_WIDTH))
        .height(Val::Auto)
        .positioning(CardPositioning::Right)
        .exclusive()
        .with_body(move |body| {
            body.spawn(Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                min_height: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(INNER_CONTENT_PADDING)),
                row_gap: Val::Px(SECTION_GAP),
                ..default()
            })
            .with_children(|main| {
                spawn_equipment_section(
                    main,
                    theme,
                    &equipment,
                    registry,
                    &slot_images,
                    asset_server,
                );

                main.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Px(1.0),
                        flex_shrink: 0.0,
                        ..default()
                    },
                    BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.16)),
                ));

                spawn_inventory_section(
                    main,
                    theme,
                    &inventory,
                    registry,
                    &slot_images,
                    asset_server,
                );
            });
        })
        .spawn(commands, theme);

    commands.entity(card).insert(InventoryCard);
}

pub struct InventoryUiPlugin;

impl Plugin for InventoryUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InventoryUiState>();
        app.init_resource::<ItemDetailUiState>();
        app.init_resource::<ItemDragState>();
        app.add_systems(
            Update,
            (
                // Only the *toggle* is gated by typing focus — the window,
                // once open, must keep rendering/dragging normally even if
                // the player also opens chat, so the gate does not apply to
                // the rest of this chain.
                systems::toggle_inventory.run_if(not_typing),
                systems::update_inventory_ui,
                systems::handle_inventory_interactions,
                systems::handle_stack_controls,
                systems::focus_split_amount,
                systems::unfocus_split_when_chat_focused,
                systems::edit_split_amount,
                systems::update_split_amount_display,
                systems::defocus_split_amount_on_world_click,
                inscription_editor::handle_item_editor_tabs,
                inscription_editor::update_item_editor_tabs,
                inscription_editor::handle_item_editor_choices,
                detail::refresh_item_detail_on_equipment_change,
                drag::start_item_drag,
                drag::update_item_drag,
                drag::end_item_drag,
                drag::inspect_clicked_item,
                drag::handle_destroy_dialog,
            )
                .chain()
                .run_if(in_gameplay),
        );
    }
}
