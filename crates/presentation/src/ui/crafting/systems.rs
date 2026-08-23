//! Click-to-open crafter list, recipe confirm, and start_craft.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevymmo_client::pointer::{hud_wants_pointer, PointerOnHud};
use bevymmo_client::stdb::{commands, NpcKind, StdbConnection};
use bevymmo_gameplay::entity::components::{EntityKind, GameEntity};
use bevymmo_gameplay::items::components::Inventory;
use bevymmo_gameplay::items::definition::ItemCategory;
use bevymmo_gameplay::items::registry::ItemRegistry;
use bevymmo_gameplay::items::Item;
use bevymmo_gameplay::placeables::PlaceableRegistry;
use bevymmo_network::network::protocol::Position;
use bevymmo_network::world_components::NetworkEntityId;

use crate::ui::card::components::CardPositioning;
use crate::ui::card::{
    ornate_bar_image, CardBuilder, CardFrameAssets, CardKind, ORNATE_BAR_CONFIRM_PATH,
    ORNATE_BAR_NEUTRAL_PATH,
};
use crate::ui::crafting::components::{
    CraftDialogCard, CraftListCard, CraftRecipeButton, CraftSubmitButton,
};
use crate::ui::crafting::crafter_categories;
use crate::ui::npc_sidebar::systems::{closest_friendly_hit, cursor_ray, EntityHit};
use crate::ui::theme::UiTheme;
use bevymmo_client::local_player::LocalPlayer;

const NPC_SELECT_RADIUS: f32 = 1.2;

pub fn crafter_npc_on_click(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    pointer_on_hud: Res<PointerOnHud>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &Transform), With<Camera3d>>,
    theme: Res<UiTheme>,
    item_registry: Res<ItemRegistry>,
    placeables: Option<Res<PlaceableRegistry>>,
    entity_query: Query<(Entity, &Position, &EntityKind, Option<&NpcKind>), With<GameEntity>>,
    name_query: Query<&bevymmo_gameplay::entity::components::PlayerName>,
    existing_list: Query<Entity, With<CraftListCard>>,
    existing_dialog: Query<Entity, With<CraftDialogCard>>,
    asset_server: Res<AssetServer>,
) {
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    if hud_wants_pointer(&pointer_on_hud) {
        return;
    }
    let Some(placeables) = placeables else {
        return;
    };
    let Some(ray) = cursor_ray(&windows, &cameras) else {
        return;
    };

    let mut hits: Vec<EntityHit> = Vec::new();
    let mut category_for = None;
    for (entity, position, kind, npc_kind) in entity_query.iter() {
        if *kind != EntityKind::Friendly {
            continue;
        }
        let Some(npc_kind) = npc_kind else {
            continue;
        };
        let Some(categories) = crafter_categories(&npc_kind.kind_id, &placeables) else {
            continue;
        };
        let distance = crate::ui::npc_sidebar::systems::point_to_ray_distance(
            position.0,
            ray.origin,
            *ray.direction,
        );
        if distance > NPC_SELECT_RADIUS {
            continue;
        }
        hits.push(EntityHit { entity, distance });
        category_for = Some((entity, categories));
    }

    let Some(target_entity) = closest_friendly_hit(&hits) else {
        return;
    };
    let categories = category_for
        .and_then(|(entity, categories)| (entity == target_entity).then_some(categories))
        .or_else(|| {
            entity_query
                .get(target_entity)
                .ok()
                .and_then(|(_, _, _, kind)| {
                    kind.and_then(|kind| crafter_categories(&kind.kind_id, &placeables))
                })
        });
    let Some(categories) = categories else {
        return;
    };

    for entity in existing_list.iter().chain(existing_dialog.iter()) {
        commands.entity(entity).despawn();
    }

    let npc_name = name_query
        .get(target_entity)
        .map(|name| name.0.clone())
        .unwrap_or_else(|_| "Fabbro".to_string());

    spawn_craft_list(
        &mut commands,
        &theme,
        &asset_server,
        &item_registry,
        target_entity,
        &npc_name,
        categories,
    );
}

fn spawn_craft_list(
    commands: &mut Commands,
    theme: &UiTheme,
    asset_server: &AssetServer,
    item_registry: &ItemRegistry,
    npc: Entity,
    npc_name: &str,
    categories: Vec<ItemCategory>,
) {
    let vendor_bar = asset_server.load(ORNATE_BAR_NEUTRAL_PATH);
    let recipes = item_registry.craftable_in_any(&categories);
    let card_entity = CardBuilder::new(CardKind::Generic, npc_name)
        .frame(CardFrameAssets::load(asset_server))
        .width(Val::Px(320.0))
        .height(Val::Px(360.0))
        .positioning(CardPositioning::Left)
        .closeable()
        .exclusive()
        .scrollable()
        .with_body(move |body| {
            let intro = if recipes.is_empty() {
                "Nessun oggetto craftabile."
            } else {
                "Oggetti craftabili"
            };
            body.spawn((
                Text::new(intro),
                TextFont {
                    font_size: FontSize::Px(16.0),
                    ..default()
                },
                TextColor(theme.text_color),
            ));
            for (_, item) in recipes {
                let item_id = item.id();
                let name = item.display_name().to_string();
                let rarity = format!("{:?}", item.config().rarity);
                body.spawn((
                    Button,
                    Node {
                        width: Val::Percent(100.0),
                        min_height: Val::Px(32.0),
                        margin: UiRect::vertical(Val::Px(2.0)),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    ornate_bar_image(vendor_bar.clone()),
                    CraftRecipeButton { npc, item_id },
                ))
                .with_children(|button| {
                    button.spawn((
                        Text::new(format!("{name}  {rarity}")),
                        TextFont {
                            font_size: FontSize::Px(theme.button_font_size),
                            ..default()
                        },
                        TextColor(theme.text_color),
                    ));
                });
            }
        })
        .spawn(commands, theme);

    commands.entity(card_entity).insert(CraftListCard { npc });
}

pub fn open_craft_dialog(
    mut commands: Commands,
    interactions: Query<(&Interaction, &CraftRecipeButton), Changed<Interaction>>,
    existing_dialog: Query<Entity, With<CraftDialogCard>>,
    theme: Res<UiTheme>,
    item_registry: Res<ItemRegistry>,
    inventory: Query<&Inventory, With<LocalPlayer>>,
    asset_server: Res<AssetServer>,
) {
    for (interaction, button) in interactions.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        for entity in existing_dialog.iter() {
            commands.entity(entity).despawn();
        }
        let Some(item) = item_registry.get(&button.item_id) else {
            continue;
        };
        let Some(recipe) = item.craft_recipe() else {
            continue;
        };
        let bag = inventory.single().ok();
        spawn_craft_dialog(
            &mut commands,
            &theme,
            &asset_server,
            &item_registry,
            button.npc,
            item.as_ref(),
            recipe,
            bag,
        );
    }
}

fn spawn_craft_dialog(
    commands: &mut Commands,
    theme: &UiTheme,
    asset_server: &AssetServer,
    item_registry: &ItemRegistry,
    npc: Entity,
    item: &dyn Item,
    recipe: &bevymmo_gameplay::items::CraftRecipe,
    bag: Option<&Inventory>,
) {
    let confirm_bar = asset_server.load(ORNATE_BAR_CONFIRM_PATH);
    let title = format!("Craft: {}", item.display_name());
    let item_id = item.id();
    let mut lines: Vec<String> = Vec::new();
    for ingredient in &recipe.ingredients {
        let name = item_registry
            .get(&ingredient.item_id)
            .map(|item| item.display_name().to_string())
            .unwrap_or_else(|| ingredient.item_id.as_str().to_string());
        let have = bag
            .map(|inventory| inventory.count_item(&ingredient.item_id))
            .unwrap_or(0);
        lines.push(format!("{name}  x{}   ({have})", ingredient.amount));
    }
    lines.push(format!("Tempo di channel   {:.1}s", recipe.channel_seconds));

    let card_entity = CardBuilder::new(CardKind::Generic, &title)
        .frame(CardFrameAssets::load(asset_server))
        .width(Val::Px(320.0))
        .height(Val::Px(320.0))
        .positioning(CardPositioning::Right)
        .closeable()
        .coexist()
        .with_body(move |body| {
            for line in &lines {
                body.spawn((
                    Text::new(line.clone()),
                    TextFont {
                        font_size: FontSize::Px(16.0),
                        ..default()
                    },
                    TextColor(theme.text_color),
                ));
            }
            body.spawn((
                Button,
                Node {
                    width: Val::Percent(100.0),
                    min_height: Val::Px(36.0),
                    margin: UiRect::top(Val::Px(12.0)),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                ornate_bar_image(confirm_bar.clone()),
                CraftSubmitButton {
                    npc,
                    item_id: item_id.clone(),
                },
            ))
            .with_children(|button| {
                button.spawn((
                    Text::new("CRAFT"),
                    TextFont {
                        font_size: FontSize::Px(theme.button_font_size),
                        ..default()
                    },
                    TextColor(theme.text_color),
                ));
            });
        })
        .spawn(commands, theme);

    commands.entity(card_entity).insert(CraftDialogCard {
        npc,
        item_id: item.id(),
    });
}

pub fn submit_craft(
    interactions: Query<(&Interaction, &CraftSubmitButton), Changed<Interaction>>,
    npc_entities: Query<&NetworkEntityId>,
    connection: Option<Res<StdbConnection>>,
) {
    let Some(connection) = connection else {
        return;
    };
    for (interaction, button) in interactions.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Ok(network_id) = npc_entities.get(button.npc) else {
            continue;
        };
        if let Err(error) = commands::start_craft(
            &connection,
            network_id.0,
            button.item_id.as_str().to_string(),
            1,
        ) {
            error!("could not start craft: {error}");
        }
    }
}
