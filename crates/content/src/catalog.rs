//! Public catalog snapshot of compiled game content.
//!
//! Built from the same registries the client and the authoritative module
//! use. The HTTP gateway serves this snapshot on `/v1/public/catalog/*`;
//! it is not SpacetimeDB state.

use serde::{Deserialize, Serialize};

use crate::item_definitions;
use crate::items::definition::Item;
use crate::items::effects::ItemEffect;
use crate::items::registry::ItemRegistry;

/// Full compiled catalog. Slice 1 ships items; later slices append collections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Catalog {
    pub items: Vec<CatalogItem>,
}

impl Catalog {
    /// Lookup by stable item id (`sword`, `wood`, …).
    pub fn item(&self, id: &str) -> Option<&CatalogItem> {
        self.items.iter().find(|item| item.id == id)
    }
}

/// One item as the public catalog exposes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CatalogItem {
    pub id: String,
    pub name: String,
    pub description: String,
    /// `ItemCategory` variant name (`Weapon`, `Armor`, `Material`, …).
    pub category: String,
    /// `ItemRarity` variant name (`Common`, `Rare`, `Legendary`, …).
    pub rarity: String,
    /// `EquipSlot` variant name, omitted for inventory-only items.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    pub tradable: bool,
    pub effects: Vec<CatalogEffect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rune_profile: Option<CatalogRuneProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abilities: Option<CatalogAbilityLoadout>,
    /// Bevy asset path under `assets/` for the inventory icon.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// `GatheringToolKind` variant name (`Axe`, `Hammer`), omitted otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gathering_tool: Option<String>,
    /// Present only on craftable items.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crafting: Option<CatalogCraftRecipe>,
}

/// One recipe ingredient as the public catalog exposes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CatalogCraftIngredient {
    pub id: String,
    pub amount: u32,
}

/// How to craft one copy of an item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CatalogCraftRecipe {
    pub channel_seconds: f32,
    pub ingredients: Vec<CatalogCraftIngredient>,
}

/// Gameplay effect, tagged for a stable HTTP contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum CatalogEffect {
    StatBonus {
        field: String,
        op: String,
        value: f32,
    },
    InstantHeal {
        amount: f32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CatalogRuneProfile {
    pub capacity: u32,
    pub stability: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CatalogAbilityLoadout {
    pub primary: Vec<String>,
    pub secondary: Vec<String>,
    pub ultimate: Vec<String>,
}

/// Snapshot of every item shipped by this game build, sorted by id.
pub fn snapshot() -> Catalog {
    Catalog {
        items: catalog_items(&item_definitions::default_items()),
    }
}

fn catalog_items(registry: &ItemRegistry) -> Vec<CatalogItem> {
    let mut items: Vec<CatalogItem> = registry
        .sorted_items()
        .into_iter()
        .map(|(_, item)| catalog_item(item.as_ref()))
        .collect();
    items.sort_by(|a, b| a.id.cmp(&b.id));
    items
}

fn catalog_item(item: &dyn Item) -> CatalogItem {
    let config = item.config();
    CatalogItem {
        id: item.id().as_str().to_string(),
        name: config.display_name.to_string(),
        description: config.description.to_string(),
        category: format!("{:?}", config.category),
        rarity: format!("{:?}", config.rarity),
        slot: config.equippable_into.map(|slot| format!("{slot:?}")),
        family: item.weapon_family().map(|id| id.as_str().to_string()),
        tradable: item.tradable(),
        effects: item.effects().iter().map(catalog_effect).collect(),
        rune_profile: item.rune_profile().map(|profile| CatalogRuneProfile {
            capacity: profile.capacity,
            stability: profile.stability,
        }),
        abilities: item.ability_loadout().map(|loadout| CatalogAbilityLoadout {
            primary: loadout
                .primary
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            secondary: loadout
                .secondary
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            ultimate: loadout
                .ultimate
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
        }),
        icon: item.icon().map(str::to_string),
        gathering_tool: item.gathering_tool().map(|kind| format!("{kind:?}")),
        crafting: item.craft_recipe().map(|recipe| CatalogCraftRecipe {
            channel_seconds: recipe.channel_seconds,
            ingredients: recipe
                .ingredients
                .iter()
                .map(|ingredient| CatalogCraftIngredient {
                    id: ingredient.item_id.as_str().to_string(),
                    amount: ingredient.amount,
                })
                .collect(),
        }),
    }
}

fn catalog_effect(effect: &ItemEffect) -> CatalogEffect {
    match effect {
        ItemEffect::StatBonus { field, op, value } => CatalogEffect::StatBonus {
            field: format!("{field:?}"),
            op: format!("{op:?}"),
            value: *value,
        },
        ItemEffect::InstantHeal { amount } => CatalogEffect::InstantHeal { amount: *amount },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::registry::ItemId;

    #[test]
    fn snapshot_contains_every_registered_item() {
        let registry = item_definitions::default_items();
        let catalog = snapshot();
        assert_eq!(catalog.items.len(), registry.len());
        for (id, _) in registry.sorted_items() {
            assert!(
                catalog.item(id.as_str()).is_some(),
                "catalog is missing registered item {}",
                id.as_str()
            );
        }
    }

    #[test]
    fn items_are_sorted_by_id() {
        let catalog = snapshot();
        let ids: Vec<&str> = catalog.items.iter().map(|item| item.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn snapshot_is_deterministic() {
        assert_eq!(snapshot(), snapshot());
    }

    #[test]
    fn sword_carries_loadout_stats_and_rune_profile() {
        let sword = snapshot()
            .item("sword")
            .cloned()
            .expect("sword is registered");
        assert_eq!(sword.name, "Spada");
        assert_eq!(sword.category, "Weapon");
        assert_eq!(sword.rarity, "Rare");
        assert_eq!(sword.slot.as_deref(), Some("Weapon"));
        assert_eq!(sword.family.as_deref(), Some("sword"));
        assert!(sword.tradable);
        assert_eq!(
            sword.effects,
            vec![CatalogEffect::StatBonus {
                field: "AttackPower".into(),
                op: "Add".into(),
                value: 70.0,
            }]
        );
        let profile = sword.rune_profile.expect("sword has a rune profile");
        assert_eq!(profile.capacity, 11);
        assert!((profile.stability - 0.86).abs() < f32::EPSILON);
        let abilities = sword.abilities.expect("sword offers gestures");
        assert_eq!(abilities.primary, vec!["cleave"]);
        assert_eq!(abilities.secondary, vec!["lunge"]);
        assert_eq!(abilities.ultimate, vec!["blade_storm"]);
        let crafting = sword.crafting.expect("sword has a recipe");
        assert!((crafting.channel_seconds - 3.0).abs() < f32::EPSILON);
        assert_eq!(
            crafting.ingredients,
            vec![
                CatalogCraftIngredient {
                    id: "wood".into(),
                    amount: 2,
                },
                CatalogCraftIngredient {
                    id: "copper".into(),
                    amount: 4,
                },
            ]
        );
    }

    #[test]
    fn simple_axe_is_a_craftable_gathering_tool() {
        let axe = snapshot()
            .item("simple_axe")
            .cloned()
            .expect("simple_axe is registered");
        assert_eq!(axe.name, "Ascia");
        assert_eq!(axe.category, "Tool");
        assert_eq!(axe.rarity, "Common");
        assert_eq!(axe.slot.as_deref(), Some("Weapon"));
        assert_eq!(axe.gathering_tool.as_deref(), Some("Axe"));
        assert!(axe.tradable);
        assert!(axe.abilities.is_none());
        assert!(axe.rune_profile.is_none());
        assert_eq!(
            axe.effects,
            vec![
                CatalogEffect::StatBonus {
                    field: "GatheringSpeed".into(),
                    op: "Add".into(),
                    value: 50.0,
                },
                CatalogEffect::StatBonus {
                    field: "GatheringBonus".into(),
                    op: "Add".into(),
                    value: 0.25,
                },
            ]
        );
        let crafting = axe.crafting.expect("axe is craftable");
        assert!((crafting.channel_seconds - 3.0).abs() < f32::EPSILON);
        assert_eq!(
            crafting.ingredients,
            vec![
                CatalogCraftIngredient {
                    id: "wood".into(),
                    amount: 4,
                },
                CatalogCraftIngredient {
                    id: "copper".into(),
                    amount: 3,
                },
            ]
        );
    }

    #[test]
    fn wood_is_an_inventory_only_material() {
        let catalog = snapshot();
        let wood = catalog.item("wood").expect("wood is registered");
        assert_eq!(wood.category, "Material");
        assert!(wood.slot.is_none());
        assert!(wood.abilities.is_none());
        assert!(wood.rune_profile.is_none());
        assert!(wood.icon.is_none());
        assert!(wood.crafting.is_none());
    }

    #[test]
    fn copper_is_an_inventory_only_material() {
        let catalog = snapshot();
        let copper = catalog.item("copper").expect("copper is registered");
        assert_eq!(copper.name, "Copper");
        assert_eq!(copper.category, "Material");
        assert_eq!(copper.rarity, "Common");
        assert!(copper.tradable);
        assert!(copper.slot.is_none());
        assert!(copper.abilities.is_none());
        assert!(copper.rune_profile.is_none());
        assert!(copper.icon.is_none());
    }

    #[test]
    fn unknown_id_is_absent() {
        assert!(snapshot().item("channeling-staff").is_none());
        assert!(!item_definitions::default_items().contains(&ItemId::new("channeling-staff")));
    }

    #[test]
    fn catalog_roundtrips_through_json() {
        let catalog = snapshot();
        let json = serde_json::to_string(&catalog).expect("serialize");
        let back: Catalog = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(catalog, back);
        assert!(json.contains("\"kind\":\"stat_bonus\""));
        assert!(json.contains("\"crafting\""));
        let wood_json =
            serde_json::to_string(catalog.item("wood").expect("wood")).expect("wood json");
        assert!(
            !wood_json.contains("\"crafting\""),
            "non-craftable items omit the crafting field"
        );
    }
}
