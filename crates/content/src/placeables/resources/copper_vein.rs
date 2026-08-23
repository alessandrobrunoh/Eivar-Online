//! Harvestable copper vein. The visual is `models/resources/copper_vein.glb`.

use crate::placeables::resource;

#[resource(
    id = "resource_copper_vein",
    name = "Copper Vein",
    icon = "🪨",
    asset = "models/resources/copper_vein.glb",
    blocks_movement = true,
    collision = cylinder(radius = 0.6, height = 1.2),
    max_pieces = 8,
    channel_seconds = 2.0,
    min_channel_seconds = 0.25,
    yield_item = "copper",
    yield_amount = 1,
    regen_interval_seconds = 60.0,
    regen_amount = 2,
    interact_range = 2.5,
    bonus_tools = [Hammer],
)]
pub struct CopperVeinResource;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placeables::{PlaceableDefinition, PlaceableRegistry, ResourceNodePlaceable};

    #[test]
    fn copper_vein_is_a_resource_that_yields_copper() {
        let def = CopperVeinResource;
        assert_eq!(def.id().as_str(), "resource_copper_vein");
        let config = def.resource_config();
        assert_eq!(config.max_pieces, 8);
        assert_eq!(config.yield_item.as_str(), "copper");
        assert_eq!(config.yield_amount, 1);
        assert_eq!(
            config.bonus_tools,
            vec![crate::items::GatheringToolKind::Hammer]
        );
        let mut registry = PlaceableRegistry::default();
        register(&mut registry);
        assert!(registry.resources.contains_key(&def.id()));
    }
}
