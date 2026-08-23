//! Harvestable oak. The visual is the existing medium oak GLB.

use crate::placeables::resource;

#[resource(
    id = "resource_oak_tree",
    name = "Oak Tree",
    icon = "🌳",
    asset = "models/new/tree_oak_medium.glb",
    blocks_movement = true,
    collision = cylinder(radius = 0.4, height = 5.5),
    max_pieces = 50,
    channel_seconds = 2.0,
    min_channel_seconds = 0.25,
    yield_item = "wood",
    yield_amount = 1,
    regen_interval_seconds = 600.0,
    regen_amount = 10,
    interact_range = 6.0,
    bonus_tools = [Axe],
)]
pub struct OakTreeResource;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placeables::{PlaceableDefinition, PlaceableRegistry, ResourceNodePlaceable};

    #[test]
    fn oak_tree_is_a_resource_that_yields_wood() {
        let def = OakTreeResource;
        assert_eq!(def.id().as_str(), "resource_oak_tree");
        let config = def.resource_config();
        assert_eq!(config.max_pieces, 50);
        assert_eq!(config.yield_item.as_str(), "wood");
        assert_eq!(config.yield_amount, 1);
        assert_eq!(
            config.bonus_tools,
            vec![crate::items::GatheringToolKind::Axe]
        );
        let mut registry = PlaceableRegistry::default();
        register(&mut registry);
        assert!(registry.resources.contains_key(&def.id()));
    }
}
