//! Riepilogo testuale di un'arma per la scheda item.
//!
//! Solo funzioni pure: prendono catalogo + esemplare + registri e ritornano
//! stringhe già formattate, così la parte di `detail.rs` che costruisce i
//! `Node` resta banale e questo file è testabile senza un `App`.
//!
//! I numeri mostrati passano da [`resolve_root_inscribed_slot`], cioè esattamente
//! la stessa risoluzione che il cast usa un istante dopo.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use bevymmo_gameplay::abilities::{
    resolve_active_ability, resolve_root_inscribed_slot, AbilitySlot, AbilityTag,
    AncientWordRegistry, BaseAbilityRegistry, CastBlockedReason, KnownAncientLanguage,
    RootWordRegistry, SlotPreview,
};
use bevymmo_gameplay::items::{
    components::EquipSlot, instance::ItemInstance, ItemCategory, ItemRarity, ItemRegistry,
};

/// I registri necessari a descrivere un'arma incisa, raggruppati per non far
/// crescere la firma della scheda item di un argomento per tipo.
#[derive(SystemParam)]
pub struct GlyphRegistries<'w> {
    pub abilities: Res<'w, BaseAbilityRegistry>,
    pub root_words: Res<'w, RootWordRegistry>,
    pub ancient_words: Res<'w, AncientWordRegistry>,
}

impl GlyphRegistries<'_> {
    pub fn catalog(&self) -> GlyphCatalog<'_> {
        GlyphCatalog {
            abilities: &self.abilities,
            root_words: &self.root_words,
            ancient_words: &self.ancient_words,
        }
    }
}

/// I registri per riferimento semplice (testabile senza App).
#[derive(Clone, Copy)]
pub struct GlyphCatalog<'a> {
    pub abilities: &'a BaseAbilityRegistry,
    pub root_words: &'a RootWordRegistry,
    pub ancient_words: &'a AncientWordRegistry,
}

/// Riga "Rune" della scheda.
#[derive(Debug, Clone, PartialEq)]
pub struct RuneSummary {
    pub used: u32,
    pub capacity: u32,
    pub stability: f32,
    pub root_word: Option<String>,
}

impl RuneSummary {
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("{}/{} capacity", self.used, self.capacity),
            format!("{:.0}% stability", self.stability * 100.0),
        ];
        if let Some(root_word) = &self.root_word {
            lines.push(format!("Root Word: {root_word}"));
        }
        lines
    }

    pub fn line(&self) -> String {
        self.lines().join("   |   ")
    }
}

/// Un blocco "slot" della scheda: il gesto attivo e tutto ciò che lo descrive.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotSummary {
    /// `Primary` / `Secondary` / `Ultimate` — non `Q`/`W`/`E`: il tasto è
    /// rebindabile e non è un dato dell'arma (vedi `AbilitySlot`).
    pub slot: &'static str,
    pub title: String,
    /// `Some` quando lo slot è inutilizzabile: un Glifo inciso non è nel
    /// Vocabolario del personaggio, quindi il cast verrebbe rifiutato in
    /// blocco (§ "blocco totale" di `cast_inscribed_slot`).
    pub blocked: Option<String>,
    pub shape: String,
    pub stats: String,
    pub tags: String,
    pub glyphs: Option<String>,
    /// Le altre opzioni offerte dall'arma per questo slot, se più di una.
    pub alternatives: Option<String>,
}

/// Tutto ciò che la scheda mostra in più per un'weapon.
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponSummary {
    pub runes: Option<RuneSummary>,
    pub slots: Vec<SlotSummary>,
}

/// Riga di intestazione comune a QUALUNQUE item (anche non arma).
pub fn meta_line(
    category: ItemCategory,
    rarity: ItemRarity,
    equip_slot: Option<EquipSlot>,
    weight: f32,
) -> String {
    let category_name = category_label(category);
    let mut parts = vec![category_name.to_string(), rarity_label(rarity).to_string()];
    if let Some(slot) = equip_slot {
        let slot_name = format!("{slot:?}");
        if slot_name != category_name {
            parts.push(slot_name);
        }
    }
    if weight > 0.0 {
        parts.push(format!("{} wt", number(weight)));
    }
    parts.join("   |   ")
}

const fn category_label(category: ItemCategory) -> &'static str {
    match category {
        ItemCategory::Weapon => "Weapon",
        ItemCategory::Armor => "Armor",
        ItemCategory::Consumable => "Consumable",
        ItemCategory::Material => "Material",
        ItemCategory::Quest => "Quest",
        ItemCategory::Accessory => "Accessory",
        ItemCategory::Tool => "Tool",
    }
}

const fn rarity_label(rarity: ItemRarity) -> &'static str {
    match rarity {
        ItemRarity::Common => "Common",
        ItemRarity::Uncommon => "Uncommon",
        ItemRarity::Rare => "Rare",
        ItemRarity::Epic => "Epic",
        ItemRarity::Legendary => "Legendary",
    }
}

/// Costruisce il riepilogo weapon di `instance`, o `None` se l'item non è
/// un'arma con gesti propri (armor, pozioni, armi senza loadout).
pub fn summarize_weapon(
    instance: &ItemInstance,
    items: &ItemRegistry,
    glyphs: GlyphCatalog,
    known: &KnownAncientLanguage,
) -> Option<WeaponSummary> {
    let item = items.get(&instance.item_id)?;
    let abilities = item.ability_loadout()?;
    let inscription = instance.root_inscription.as_ref()?;
    let profile = item.rune_profile();

    let runes = profile.map(|profile| RuneSummary {
        used: 0, // TODO: rune cost for new model when defined
        capacity: profile.capacity,
        stability: profile.stability,
        root_word: inscription.root_word.as_ref().and_then(|id| {
            glyphs
                .root_words
                .get(id)
                .map(|rw| rw.metadata().display_name.to_string())
        }),
    });

    let slots = AbilitySlot::ALL
        .iter()
        .filter_map(|&slot| {
            summarize_slot(
                slot,
                abilities,
                &instance.ability_selection,
                inscription,
                glyphs,
                known,
                item.as_ref(),
            )
        })
        .collect();

    Some(WeaponSummary { runes, slots })
}

/// Placeholder: rune cost will be defined by the new Root Word model.
/// Returns 0 until the gameplay layer exposes cost data.
fn _total_rune_cost(
    _inscription: &bevymmo_gameplay::abilities::inscription::WeaponInscription,
) -> u32 {
    0
}

fn summarize_slot(
    slot: AbilitySlot,
    abilities: &bevymmo_gameplay::abilities::WeaponAbilities,
    selection: &bevymmo_gameplay::abilities::AbilitySelection,
    inscription: &bevymmo_gameplay::abilities::inscription::WeaponInscription,
    glyphs: GlyphCatalog,
    known: &KnownAncientLanguage,
    item: &dyn bevymmo_gameplay::items::Item,
) -> Option<SlotSummary> {
    let active_id = resolve_active_ability(slot, abilities, selection)?;
    let ability = glyphs.abilities.get(active_id)?;

    // `resolve_root_inscribed_slot` è la stessa risoluzione del cast.
    let preview = resolve_root_inscribed_slot(
        slot,
        abilities,
        selection,
        inscription,
        known,
        glyphs.abilities,
        glyphs.root_words,
        glyphs.ancient_words,
        Some(item),
    );
    let blocked = match &preview {
        Err(CastBlockedReason::UnknownRootWord) => Some("LOCKED - unknown Root Word".to_string()),
        Err(CastBlockedReason::UnknownAncientWord) => {
            Some("LOCKED - unknown Ancient Word".to_string())
        }
        Err(reason) => Some(format!("UNAVAILABLE - {reason:?}")),
        Ok(_) => None,
    };
    let params = match preview {
        Ok(SlotPreview { params, .. }) => params,
        Err(_) => ability.base_params(),
    };

    let root_name = inscription
        .root_word
        .as_ref()
        .and_then(|id| glyphs.root_words.get(id))
        .map(|rw| rw.metadata().display_name.to_string());
    let title = match &root_name {
        Some(name) => format!("{} - {name}", ability.display_name()),
        None => format!("{} - raw", ability.display_name()),
    };

    let alternatives = {
        let others: Vec<String> = abilities
            .options_for(slot)
            .iter()
            .filter(|id| *id != active_id)
            .filter_map(|id| glyphs.abilities.get(id))
            .map(|other| other.display_name().to_string())
            .collect();
        (!others.is_empty()).then(|| format!("Also offers: {}", others.join(", ")))
    };

    Some(SlotSummary {
        slot: slot_name(slot),
        title,
        blocked,
        shape: describe_geometry(ability.geometry()),
        stats: describe_params(&params),
        tags: describe_tags(ability.tags()),
        glyphs: describe_inscription(inscription, slot, glyphs),
        alternatives,
    })
}

const fn slot_name(slot: AbilitySlot) -> &'static str {
    match slot {
        AbilitySlot::Primary => "Primary",
        AbilitySlot::Secondary => "Secondary",
        AbilitySlot::Ultimate => "Ultimate",
    }
}

fn describe_geometry(geometry: bevymmo_gameplay::abilities::AbilityGeometry) -> String {
    use bevymmo_gameplay::abilities::AbilityGeometry::*;
    match geometry {
        Cone { radius, angle_deg } => {
            format!("Cone {} m / {} deg", number(radius), number(angle_deg))
        }
        Circle { radius } => format!("Circle {} m", number(radius)),
        Projectile { speed } => {
            format!("Projectile @ {} m/s", number(speed))
        }
        SelfBuff { duration_seconds } => format!("Self buff {} s", number(duration_seconds)),
    }
}

/// Solo i campi che portano informazione: un `0` ovunque è il default di
/// `AbilityParams`, e stamparlo riempirebbe la riga di rumore.
fn describe_params(params: &bevymmo_gameplay::abilities::AbilityParams) -> String {
    let mut parts = Vec::new();
    if params.potency != 0.0 {
        parts.push(format!("{} potency", number(params.potency)));
    }
    if params.area != 0.0 {
        parts.push(format!("{} m area", number(params.area)));
    }
    if params.range != 0.0 {
        parts.push(format!("{} m range", number(params.range)));
    }
    if params.cast_time != 0.0 {
        parts.push(format!("{} s cast", number(params.cast_time)));
    }
    if params.cooldown != 0.0 {
        parts.push(format!("{} s cooldown", number(params.cooldown)));
    }
    if params.mana_cost != 0.0 {
        parts.push(format!("{} mana", number(params.mana_cost)));
    }
    if parts.is_empty() {
        return "-".to_string();
    }
    parts.join("   |   ")
}

fn describe_tags(tags: &[AbilityTag]) -> String {
    if tags.is_empty() {
        return "-".to_string();
    }
    tags.iter()
        .map(|tag| format!("{tag:?}"))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Descrive l'incisione di uno slot: Root Word + Ancient Words secondari.
fn describe_inscription(
    inscription: &bevymmo_gameplay::abilities::inscription::WeaponInscription,
    slot: AbilitySlot,
    glyphs: GlyphCatalog,
) -> Option<String> {
    let slot_ins = inscription.get(slot);
    if inscription.root_word.is_none() && slot_ins.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    if let Some(root_id) = &inscription.root_word {
        if let Some(root) = glyphs.root_words.get(root_id) {
            parts.push(root.metadata().display_name.to_string());
        }
    }
    for word in &slot_ins.secondary_words {
        if let Some(aw) = glyphs.ancient_words.get(&word.word_id) {
            parts.push(aw.display_name().to_string());
        }
    }

    (!parts.is_empty()).then(|| parts.join(" + "))
}

/// Formato compatto: `2.5` resta `2.5`, `22.0` diventa `22`. Una scheda piena
/// di `.0` inutili è più difficile da leggere a colpo d'occhio.
fn number(value: f32) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{:.0}", value.round())
    } else {
        format!("{value:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevymmo_gameplay::abilities::{
        inscription::WeaponInscription, root_word::RootWordId, AbilityGeometry, AbilityParams,
    };

    fn params() -> AbilityParams {
        AbilityParams {
            potency: 220.0,
            area: 0.0,
            range: 22.0,
            cast_time: 0.25,
            cooldown: 2.5,
            mana_cost: 10.0,
        }
    }

    /// Builds the four registries with the game's real content, so the
    /// summary is exercised against the same data the player sees.
    fn catalog_app() -> App {
        let mut app = App::new();
        // Registries are plain values now, so the fixture inserts them
        // directly instead of running a `Startup` schedule to fill them.
        app.insert_resource(bevymmo_content::item_definitions::default_items());
        app.insert_resource(bevymmo_content::ability_definitions::default_base_abilities());
        app.insert_resource(bevymmo_content::root_word_definitions::default_root_words());
        app.insert_resource(bevymmo_content::ancient_word_definitions::default_ancient_words());
        app
    }

    fn summarize(
        app: &App,
        instance: &ItemInstance,
        known: &KnownAncientLanguage,
    ) -> WeaponSummary {
        let items = app.world().resource::<ItemRegistry>();
        let catalog = GlyphCatalog {
            abilities: app.world().resource::<BaseAbilityRegistry>(),
            root_words: app.world().resource::<RootWordRegistry>(),
            ancient_words: app.world().resource::<AncientWordRegistry>(),
        };
        summarize_weapon(instance, items, catalog, known).expect("sword is a weapon weapon")
    }

    fn sword_with_flame() -> ItemInstance {
        let mut instance = ItemInstance::new(bevymmo_gameplay::items::ItemId::new("sword"));
        instance.root_inscription = Some(WeaponInscription {
            root_word: Some(RootWordId::from("flame")),
            ..Default::default()
        });
        instance
    }

    /// A weapon with a known root word inscription describes all three slots.
    #[test]
    fn a_virgin_weapon_summarizes_every_slot() {
        let app = catalog_app();
        let mut known = KnownAncientLanguage::default();
        known.root_words.insert(RootWordId::from("flame"));
        let instance = sword_with_flame();
        let summary = summarize(&app, &instance, &known);

        assert_eq!(summary.slots.len(), 3);
        assert_eq!(summary.slots[0].slot, "Primary");
        assert_eq!(summary.slots[2].slot, "Ultimate");
        assert!(summary.slots.iter().all(|slot| slot.blocked.is_none()));

        let runes = summary.runes.expect("sword has a rune profile");
        assert_eq!(runes.capacity, 11);
        assert_eq!(runes.root_word.as_deref(), Some("Flame"));
    }

    #[test]
    fn sword_has_one_ultimate_ability() {
        let app = catalog_app();
        let instance = sword_with_flame();
        let summary = summarize(&app, &instance, &KnownAncientLanguage::default());

        assert!(summary.slots[0].alternatives.is_none());
        assert!(summary.slots[1].alternatives.is_none());
        assert!(summary.slots[2].alternatives.is_none());
    }

    /// A weapon with a root inscription shows the root word name.
    #[test]
    fn inscribed_root_word_is_listed() {
        let app = catalog_app();
        let instance = sword_with_flame();
        let mut known = KnownAncientLanguage::default();
        known.root_words.insert(RootWordId::from("flame"));

        let summary = summarize(&app, &instance, &known);
        let primary = &summary.slots[0];

        assert!(primary.blocked.is_none(), "root word is known");
        assert!(primary.title.contains("Flame"), "got: {}", primary.title);
        let glyphs = primary.glyphs.as_ref().expect("primary is inscribed");
        assert!(!glyphs.is_empty(), "glyphs should show the root word");
    }

    /// A weapon with an unknown root word is marked as locked.
    #[test]
    fn a_slot_with_an_unknown_root_word_is_marked_locked() {
        let app = catalog_app();
        let instance = sword_with_flame();

        // Empty knowledge: the player knows nothing.
        let summary = summarize(&app, &instance, &KnownAncientLanguage::default());

        assert!(
            summary.slots[0].blocked.is_some(),
            "unknown root word blocks the slot"
        );
        // Still described: a locked slot must not become a blank block.
        assert!(!summary.slots[0].stats.is_empty());
    }

    #[test]
    fn number_drops_a_meaningless_decimal_but_keeps_a_real_one() {
        assert_eq!(number(22.0), "22");
        assert_eq!(number(2.5), "2.5");
        assert_eq!(number(0.25), "0.2");
    }

    #[test]
    fn params_line_lists_only_the_fields_that_carry_information() {
        let line = describe_params(&params());
        assert!(line.contains("220 potency"));
        assert!(line.contains("22 m range"));
        assert!(line.contains("2.5 s cooldown"));
        // `area` is 0 for a pure projectile: printing it would be noise.
        assert!(!line.contains("area"), "got: {line}");
    }

    #[test]
    fn params_line_never_comes_back_empty() {
        let empty = AbilityParams {
            potency: 0.0,
            area: 0.0,
            range: 0.0,
            cast_time: 0.0,
            cooldown: 0.0,
            mana_cost: 0.0,
        };
        assert_eq!(describe_params(&empty), "-");
    }

    #[test]
    fn geometry_is_described_per_shape() {
        assert_eq!(
            describe_geometry(AbilityGeometry::Projectile { speed: 24.0 }),
            "Projectile @ 24 m/s"
        );
        assert_eq!(
            describe_geometry(AbilityGeometry::Circle { radius: 4.5 }),
            "Circle 4.5 m"
        );
        assert_eq!(
            describe_geometry(AbilityGeometry::Cone {
                radius: 8.0,
                angle_deg: 60.0
            }),
            "Cone 8 m / 60 deg"
        );
    }

    #[test]
    fn tags_fall_back_to_a_dash_when_empty() {
        assert_eq!(describe_tags(&[]), "-");
        assert_eq!(
            describe_tags(&[AbilityTag::Ranged, AbilityTag::Projectile]),
            "Ranged / Projectile"
        );
    }

    #[test]
    fn rune_line_reports_usage_against_capacity() {
        let runes = RuneSummary {
            used: 6,
            capacity: 8,
            stability: 0.96,
            root_word: Some("Danno".to_string()),
        };
        let line = runes.line();
        assert!(line.contains("6/8 capacity"), "got: {line}");
        assert!(line.contains("96% stability"), "got: {line}");
        assert!(line.contains("Root Word: Danno"), "got: {line}");
    }

    #[test]
    fn rune_lines_are_short_and_unpiped() {
        let runes = RuneSummary {
            used: 6,
            capacity: 8,
            stability: 0.96,
            root_word: Some("Danno".to_string()),
        };
        assert_eq!(
            runes.lines(),
            vec![
                "6/8 capacity".to_string(),
                "96% stability".to_string(),
                "Root Word: Danno".to_string(),
            ]
        );
    }

    #[test]
    fn rune_line_omits_root_word_when_the_weapon_has_none() {
        let runes = RuneSummary {
            used: 0,
            capacity: 4,
            stability: 1.0,
            root_word: None,
        };
        assert!(!runes.line().contains("Root Word"));
        assert_eq!(runes.lines().len(), 2);
    }

    #[test]
    fn meta_line_skips_a_weightless_inventory_only_item() {
        let line = meta_line(ItemCategory::Material, ItemRarity::Common, None, 0.0);
        assert_eq!(line, "Material   |   Common");
    }

    #[test]
    fn meta_line_omits_the_equip_slot_when_it_matches_the_category() {
        let line = meta_line(
            ItemCategory::Weapon,
            ItemRarity::Rare,
            Some(EquipSlot::Weapon),
            1.5,
        );
        assert_eq!(line, "Weapon   |   Rare   |   1.5 wt");
        assert!(line.contains("Weapon"));
        assert!(line.contains("Rare"));
    }

    #[test]
    fn meta_line_includes_the_equip_slot_when_it_differs_from_the_category() {
        let line = meta_line(
            ItemCategory::Accessory,
            ItemRarity::Epic,
            Some(EquipSlot::Helmet),
            0.0,
        );
        assert_eq!(line, "Accessory   |   Epic   |   Helmet");
    }

    #[test]
    fn gathering_tool_meta_line_shows_tool_and_weapon_slot() {
        let line = meta_line(
            ItemCategory::Tool,
            ItemRarity::Common,
            Some(EquipSlot::Weapon),
            0.0,
        );
        assert_eq!(line, "Tool   |   Common   |   Weapon");
    }
}
