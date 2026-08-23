//! Crafter NPC list and confirm dialog.

pub mod components;
pub mod systems;

use bevy::prelude::*;

use crate::game_state::Screen;
use crate::ui::crafting::systems::{crafter_npc_on_click, open_craft_dialog, submit_craft};

use bevymmo_gameplay::items::definition::ItemCategory;
use bevymmo_gameplay::placeables::{InteractionKind, KindId, PlaceableRegistry};

pub struct CraftingUiPlugin;

impl Plugin for CraftingUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (crafter_npc_on_click, open_craft_dialog, submit_craft)
                .chain()
                .run_if(in_state(Screen::InGame)),
        );
    }
}

/// `Some` when this NPC kind is a crafter for those item categories.
pub fn crafter_categories(
    kind_id: &str,
    placeables: &PlaceableRegistry,
) -> Option<Vec<ItemCategory>> {
    let definition = placeables.npcs.get(&KindId::new(kind_id.to_string()))?;
    match definition.interaction() {
        InteractionKind::Craft { categories } => Some(categories),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevymmo_content::placeable_definitions::register_all;

    #[test]
    fn weapon_crafter_kind_maps_to_weapon_recipes() {
        let mut registry = PlaceableRegistry::default();
        register_all(&mut registry);
        assert_eq!(
            crafter_categories("npc_weapon_crafter", &registry),
            Some(vec![ItemCategory::Weapon, ItemCategory::Tool])
        );
        assert_eq!(crafter_categories("npc_greeter", &registry), None);
        assert_eq!(crafter_categories("npc_market_1", &registry), None);
    }
}
