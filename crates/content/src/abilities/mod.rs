//! Base-ability content and its registry.

pub mod aegis;
pub mod blade_storm;
pub mod cataclysm;
pub mod cinder_storm;
pub mod cleave;
pub mod dragon_claw;
pub mod lunge;
pub mod molten_eruption;
pub mod searing_breath;
pub mod tail_sweep;
pub mod wing_buffet;

use crate::abilities::BaseAbilityRegistry;

/// Catalog ids for the dragon kit (`#[base_ability]` in this module).
pub const DRAGON_ABILITY_IDS: &[&str] = &[
    "dragon_claw",
    "searing_breath",
    "cinder_storm",
    "wing_buffet",
    "tail_sweep",
    "molten_eruption",
    "cataclysm",
];

/// Builds the registry containing every base ability shipped by this game build.
pub fn default_base_abilities() -> BaseAbilityRegistry {
    let mut registry = BaseAbilityRegistry::default();
    aegis::register(&mut registry);
    cleave::register(&mut registry);
    lunge::register(&mut registry);
    blade_storm::register(&mut registry);
    dragon_claw::register(&mut registry);
    searing_breath::register(&mut registry);
    cinder_storm::register(&mut registry);
    wing_buffet::register(&mut registry);
    tail_sweep::register(&mut registry);
    molten_eruption::register(&mut registry);
    cataclysm::register(&mut registry);
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_base_abilities_contains_sword_gestures() {
        let registry = default_base_abilities();
        assert_eq!(registry.len(), 4 + DRAGON_ABILITY_IDS.len());
        assert!(registry.contains(&crate::abilities::AbilityId::new("aegis")));
        assert!(registry.contains(&crate::abilities::AbilityId::new("cleave")));
        assert!(registry.contains(&crate::abilities::AbilityId::new("lunge")));
        assert!(registry.contains(&crate::abilities::AbilityId::new("blade_storm")));
    }

    #[test]
    fn default_base_abilities_contains_every_dragon_id() {
        let registry = default_base_abilities();
        for id in DRAGON_ABILITY_IDS {
            assert!(
                registry.contains(&crate::abilities::AbilityId::new(*id)),
                "dragon ability {id} must be registered"
            );
        }
    }

    #[test]
    fn pick_ability_at_hp_0_2_prefers_cataclysm_if_in_kit() {
        use crate::placeable_definitions::creatures::boss_dragon::BossDragon;
        use crate::placeables::{pick_ability, BossPlaceable};

        let kit = BossDragon.boss_config().abilities;
        let picked = pick_ability(&kit, 3.0, 0.2, |_| true).expect("berserk pick");
        assert_eq!(picked.ability_id.as_str(), "cataclysm");
    }

    #[test]
    fn ability_icon_filenames_match_ability_ids() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let icons = assets.join("abilities/icons");
        let registry = default_base_abilities();
        for id in ["cleave", "lunge", "blade_storm"] {
            let ability = registry
                .get(&crate::abilities::AbilityId::new(id))
                .expect("ability is registered");
            let path = assets.join(ability.icon());
            assert!(
                path.is_file(),
                "ability {id} selects missing icon asset {}",
                ability.icon()
            );
        }
        let Ok(entries) = std::fs::read_dir(&icons) else {
            return;
        };
        for entry in entries {
            let path = entry.expect("icon dir entry").path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("png") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .expect("utf-8 icon filename");
            assert!(
                registry.contains(&crate::abilities::AbilityId::new(stem.to_string())),
                "icon {stem}.png has no matching ability id"
            );
        }
    }

    #[test]
    fn catalog_cleave_matches_the_player_gesture() {
        use crate::abilities::{resolve_ability, AbilityId};
        use crate::ancient_word_definitions::default_ancient_words;
        use crate::root_word_definitions::default_root_words;

        let abilities = default_base_abilities();
        let preview = resolve_ability(
            &AbilityId::new("cleave"),
            None,
            &abilities,
            &default_root_words(),
            &default_ancient_words(),
        )
        .expect("cleave is registered");
        assert_eq!(preview.ability.id().as_str(), "cleave");
        assert_eq!(preview.params.potency, 115.0);
        assert_eq!(preview.params.range, 5.0);
        assert_eq!(preview.params.cooldown, 3.0);
    }

    #[test]
    fn catalog_cleave_with_flame_matches_the_inscribed_sword() {
        use crate::abilities::{
            resolve_ability, resolve_root_inscribed_slot, AbilityId, AbilitySelection, AbilitySlot,
            KitInscription, KnownAncientLanguage, ManifestationPayload, RootWordId,
            WeaponInscription,
        };
        use crate::ancient_word_definitions::default_ancient_words;
        use crate::item_definitions::weapons::sword::sword::Sword;
        use crate::items::Item;
        use crate::root_word_definitions::default_root_words;

        let abilities = default_base_abilities();
        let roots = default_root_words();
        let words = default_ancient_words();
        let kit = KitInscription {
            root_word: Some(RootWordId::from("flame")),
            secondary_words: Vec::new(),
        };
        let catalog = resolve_ability(
            &AbilityId::new("cleave"),
            Some(&kit),
            &abilities,
            &roots,
            &words,
        )
        .expect("flame cleave resolves from the catalog");

        let sword = Sword;
        let mut known = KnownAncientLanguage::default();
        known.root_words.insert(RootWordId::from("flame"));
        let player = resolve_root_inscribed_slot(
            AbilitySlot::Primary,
            sword.ability_loadout().expect("sword offers a loadout"),
            &AbilitySelection::default(),
            &WeaponInscription {
                root_word: Some(RootWordId::from("flame")),
                ..Default::default()
            },
            &known,
            &abilities,
            &roots,
            &words,
            Some(&sword),
        )
        .expect("inscribed sword cleave resolves");

        assert_eq!(
            catalog.blueprint.payload,
            ManifestationPayload::damage(["burn"])
        );
        assert_eq!(catalog.blueprint.payload, player.blueprint.payload);
        assert_eq!(catalog.params.potency, player.params.potency);
        assert!((catalog.params.potency - 115.0 * 1.15).abs() < 0.001);
    }
}
