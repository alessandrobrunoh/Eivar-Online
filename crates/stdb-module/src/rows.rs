//! Row spellings of the domain types, and the conversions to and from them.
//!
//! `bevymmo_domain` carries no SATS derives — its newtypes would panic the
//! derive, and the derive assumes it lives in a module crate (see that crate's
//! `lib.rs`). So every domain type that has to be stored gets a mirror here with
//! named fields, plus `From` in both directions.
//!
//! The mirrors are deliberately flat: ids become `String`, `Cow` disappears,
//! fixed-size arrays become `Vec`. SATS has no impl for `[T; N]`, `Cow` or
//! `HashMap`, so those would not survive the trip regardless.
//!
//! Note what is *not* here: JSON. The Postgres schema stored inventories,
//! equipment and glyphs as JSON in `TEXT` columns, which meant the database
//! could not see inside them. These are real columns, so `spacetime sql` can.

use bevymmo_domain::abilities::inscription::{
    AbilityInscription, ArmorInscription, SecondaryWord, SlotInscription, WeaponInscription,
};
use bevymmo_domain::abilities::known_glyphs::KnownAncientLanguage;
use bevymmo_domain::abilities::root_word::RootWordId;
use bevymmo_domain::abilities::weapon_abilities::AbilitySelection;
use bevymmo_domain::abilities::{AbilityId, AncientWordId};
use bevymmo_domain::effects::{ApplyStatusEffect, EffectSpec};
use bevymmo_domain::items::components::{Equipment, Inventory};
use bevymmo_domain::items::instance::{ItemInstance, ItemInstanceId};
use bevymmo_domain::items::registry::ItemId;
use bevymmo_domain::items::EquipSlot;
use bevymmo_domain::stats::components::{
    CombatStats, GatheringStats, MovementStats, StatsBundleData, VitalStats,
};
use glam::Vec3;
use spacetimedb::SpacetimeType;

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectPayloadKindRow {
    Damage,
    Heal,
    ApplyStatus,
    Cleanse,
    Purge,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectPayloadFilterRow {
    Buffs,
    Debuffs,
    All,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectPayloadSelectionRow {
    Oldest,
    Newest,
    ShortestRemaining,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq)]
pub struct EffectPayloadRow {
    pub kind: EffectPayloadKindRow,
    pub amount: f32,
    pub status_id: Option<String>,
    pub duration_override_seconds: Option<f32>,
    pub potency: f32,
    pub status_filter: Option<EffectPayloadFilterRow>,
    pub max_statuses: Option<u16>,
    pub selection: Option<EffectPayloadSelectionRow>,
}

impl From<&EffectSpec> for EffectPayloadRow {
    fn from(effect: &EffectSpec) -> Self {
        let empty = || Self {
            kind: EffectPayloadKindRow::Damage,
            amount: 0.0,
            status_id: None,
            duration_override_seconds: None,
            potency: 0.0,
            status_filter: None,
            max_statuses: None,
            selection: None,
        };
        match effect {
            EffectSpec::Damage(damage) => Self {
                kind: EffectPayloadKindRow::Damage,
                amount: damage.amount,
                ..empty()
            },
            EffectSpec::Heal(heal) => Self {
                kind: EffectPayloadKindRow::Heal,
                amount: heal.amount,
                ..empty()
            },
            EffectSpec::ApplyStatus(ApplyStatusEffect {
                status_id,
                duration_override_seconds,
                potency,
            }) => Self {
                kind: EffectPayloadKindRow::ApplyStatus,
                status_id: Some(status_id.as_str().to_string()),
                duration_override_seconds: *duration_override_seconds,
                potency: *potency,
                ..empty()
            },
            EffectSpec::Cleanse(cleanse) => Self {
                kind: EffectPayloadKindRow::Cleanse,
                status_filter: Some(cleanse.filter.into()),
                max_statuses: cleanse.max_statuses,
                selection: Some(cleanse.selection.into()),
                ..empty()
            },
            EffectSpec::Purge(purge) => Self {
                kind: EffectPayloadKindRow::Purge,
                status_filter: Some(purge.filter.into()),
                max_statuses: purge.max_statuses,
                selection: Some(purge.selection.into()),
                ..empty()
            },
        }
    }
}

impl From<bevymmo_domain::effects::StatusFilter> for EffectPayloadFilterRow {
    fn from(filter: bevymmo_domain::effects::StatusFilter) -> Self {
        match filter {
            bevymmo_domain::effects::StatusFilter::Buffs => Self::Buffs,
            bevymmo_domain::effects::StatusFilter::Debuffs => Self::Debuffs,
            bevymmo_domain::effects::StatusFilter::All => Self::All,
        }
    }
}

impl From<bevymmo_domain::effects::StatusSelection> for EffectPayloadSelectionRow {
    fn from(selection: bevymmo_domain::effects::StatusSelection) -> Self {
        match selection {
            bevymmo_domain::effects::StatusSelection::Oldest => Self::Oldest,
            bevymmo_domain::effects::StatusSelection::Newest => Self::Newest,
            bevymmo_domain::effects::StatusSelection::ShortestRemaining => Self::ShortestRemaining,
        }
    }
}

/// A vector as a database column.
#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Default)]
pub struct Vec3Row {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl From<Vec3> for Vec3Row {
    fn from(v: Vec3) -> Self {
        Self {
            x: v.x,
            y: v.y,
            z: v.z,
        }
    }
}

impl From<Vec3Row> for Vec3 {
    fn from(v: Vec3Row) -> Self {
        Vec3::new(v.x, v.y, v.z)
    }
}

/// The numbers that make up a character's stats.
///
/// Stored as *base* values, without equipment bonuses. The Bevy server was
/// careful about this too — it persisted `base_stats_without_equipment` so that
/// re-equipping on login would not compound the bonuses — and the distinction
/// has to survive: effective stats are derived, never stored.
#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct StatsRow {
    pub current_health: f32,
    pub max_health: f32,
    /// Current pure shield pool, consumed before armor and health.
    pub current_shield: f32,
    /// Maximum pure shield capacity.
    pub max_shield: f32,
    pub max_mana: f32,
    pub mana_regeneration: f32,
    pub armor: f32,
    pub movement_speed: f32,
    pub attack_power: f32,
    /// Multiplier on threat this entity generates when it deals damage.
    ///
    /// Adding this column is not auto-filled to `1.0` for existing character
    /// rows; `./scripts/stdb.sh reset` is required after publish.
    pub threat_generation: f32,
    pub gathering_speed: f32,
    pub gathering_bonus: f32,
}

impl From<&StatsBundleData> for StatsRow {
    fn from(s: &StatsBundleData) -> Self {
        Self {
            current_health: s.vital.current_health,
            max_health: s.vital.max_health,
            current_shield: 0.0,
            max_shield: 0.0,
            max_mana: s.vital.max_mana,
            mana_regeneration: s.vital.mana_regeneration,
            armor: s.combat.armor,
            movement_speed: s.movement.speed,
            attack_power: s.combat.attack_power,
            threat_generation: s.combat.threat_generation,
            gathering_speed: s.gathering.speed,
            gathering_bonus: s.gathering.bonus,
        }
    }
}

impl From<StatsRow> for StatsBundleData {
    fn from(s: StatsRow) -> Self {
        StatsBundleData {
            vital: VitalStats {
                current_health: s.current_health,
                max_health: s.max_health,
                current_mana: s.max_mana,
                max_mana: s.max_mana,
                mana_regeneration: s.mana_regeneration,
            },
            combat: CombatStats {
                armor: s.armor,
                attack_power: s.attack_power,
                threat_generation: s.threat_generation,
            },
            movement: MovementStats {
                speed: s.movement_speed,
            },
            gathering: GatheringStats {
                speed: s.gathering_speed,
                bonus: s.gathering_bonus,
            },
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// NEW ROOTWORD-BASED INSCRIPTION ROWS (additive to legacy)
// ══════════════════════════════════════════════════════════════════

#[derive(SpacetimeType, Clone, Debug, PartialEq, Default)]
pub struct SecondaryWordRow {
    pub word_id: String,
    /// Power scaling factor (0.0-1.0).
    pub intensity: f32,
}

impl From<&SecondaryWord> for SecondaryWordRow {
    fn from(s: &SecondaryWord) -> Self {
        Self {
            word_id: s.word_id.as_str().to_string(),
            intensity: s.intensity,
        }
    }
}

impl From<&SecondaryWordRow> for SecondaryWord {
    fn from(s: &SecondaryWordRow) -> Self {
        SecondaryWord {
            word_id: AncientWordId::new(s.word_id.clone()),
            intensity: s.intensity,
        }
    }
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Default)]
pub struct SlotInscriptionRow {
    /// Secondary words applied to the item's shared Root Word.
    pub secondary_words: Vec<SecondaryWordRow>,
}

impl From<&SlotInscription> for SlotInscriptionRow {
    fn from(s: &SlotInscription) -> Self {
        Self {
            secondary_words: s.secondary_words.iter().map(Into::into).collect(),
        }
    }
}

impl From<&SlotInscriptionRow> for SlotInscription {
    fn from(s: &SlotInscriptionRow) -> Self {
        SlotInscription {
            secondary_words: s.secondary_words.iter().map(Into::into).collect(),
        }
    }
}

/// Complete weapon inscription using the new RootWord-based model.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Default)]
pub struct WeaponInscriptionRow {
    pub root_word: Option<String>,
    pub primary: SlotInscriptionRow,
    pub secondary: SlotInscriptionRow,
    pub ultimate: SlotInscriptionRow,
}

impl From<&WeaponInscription> for WeaponInscriptionRow {
    fn from(w: &WeaponInscription) -> Self {
        Self {
            root_word: w.root_word.as_ref().map(|word| word.as_str().to_string()),
            primary: (&w.primary).into(),
            secondary: (&w.secondary).into(),
            ultimate: (&w.ultimate).into(),
        }
    }
}

impl From<&WeaponInscriptionRow> for WeaponInscription {
    fn from(w: &WeaponInscriptionRow) -> Self {
        WeaponInscription {
            root_word: w.root_word.clone().map(RootWordId::new),
            primary: (&w.primary).into(),
            secondary: (&w.secondary).into(),
            ultimate: (&w.ultimate).into(),
        }
    }
}

/// Ability-level inscription for fine-grained ability customization.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Default)]
pub struct AbilityInscriptionRow {
    /// Secondary words that modify the item's shared Root Word.
    pub secondary_words: Vec<SecondaryWordRow>,
}

impl From<&AbilityInscription> for AbilityInscriptionRow {
    fn from(a: &AbilityInscription) -> Self {
        Self {
            secondary_words: a.secondary_words.iter().map(Into::into).collect(),
        }
    }
}

impl From<&AbilityInscriptionRow> for AbilityInscription {
    fn from(a: &AbilityInscriptionRow) -> Self {
        AbilityInscription {
            secondary_words: a.secondary_words.iter().map(Into::into).collect(),
        }
    }
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Default)]
pub struct AbilitySelectionRow {
    pub primary: Option<String>,
    pub secondary: Option<String>,
    pub ultimate: Option<String>,
}

impl From<&AbilitySelection> for AbilitySelectionRow {
    fn from(a: &AbilitySelection) -> Self {
        Self {
            primary: a.primary.as_ref().map(|id| id.as_str().to_string()),
            secondary: a.secondary.as_ref().map(|id| id.as_str().to_string()),
            ultimate: a.ultimate.as_ref().map(|id| id.as_str().to_string()),
        }
    }
}

impl From<&AbilitySelectionRow> for AbilitySelection {
    fn from(a: &AbilitySelectionRow) -> Self {
        AbilitySelection {
            primary: a.primary.clone().map(AbilityId::new),
            secondary: a.secondary.clone().map(AbilityId::new),
            ultimate: a.ultimate.clone().map(AbilityId::new),
        }
    }
}

/// Independent inscription carried by an armor item.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Default)]
pub struct ArmorInscriptionRow {
    pub root_word: Option<String>,
    pub secondary_words: Vec<SecondaryWordRow>,
}

impl From<&ArmorInscription> for ArmorInscriptionRow {
    fn from(a: &ArmorInscription) -> Self {
        Self {
            root_word: a.root_word.as_ref().map(|word| word.as_str().to_string()),
            secondary_words: a.secondary_words.iter().map(Into::into).collect(),
        }
    }
}

impl From<&ArmorInscriptionRow> for ArmorInscription {
    fn from(a: &ArmorInscriptionRow) -> Self {
        Self {
            root_word: a.root_word.clone().map(RootWordId::new),
            secondary_words: a.secondary_words.iter().map(Into::into).collect(),
        }
    }
}

#[derive(SpacetimeType, Clone, Debug, PartialEq)]
pub struct ItemInstanceRow {
    /// Zero means "not stored yet"; see [`ItemInstanceId`].
    pub instance_id: u64,
    pub item_id: String,
    pub quantity: u32,
    pub ability_selection: AbilitySelectionRow,
    /// New RootWord-based weapon inscription model.
    pub root_inscription: Option<WeaponInscriptionRow>,
    /// New independent armor inscription model.
    pub armor_inscription: Option<ArmorInscriptionRow>,
}

impl From<&ItemInstance> for ItemInstanceRow {
    fn from(i: &ItemInstance) -> Self {
        Self {
            instance_id: i.instance_id.0,
            item_id: i.item_id.as_str().to_string(),
            quantity: i.quantity.max(1),
            ability_selection: (&i.ability_selection).into(),
            root_inscription: i.root_inscription.as_ref().map(Into::into),
            armor_inscription: i.armor_inscription.as_ref().map(Into::into),
        }
    }
}

impl From<&ItemInstanceRow> for ItemInstance {
    fn from(i: &ItemInstanceRow) -> Self {
        ItemInstance {
            instance_id: ItemInstanceId(i.instance_id),
            item_id: ItemId::new(i.item_id.clone()),
            quantity: i.quantity.max(1),
            ability_selection: (&i.ability_selection).into(),
            root_inscription: i.root_inscription.as_ref().map(Into::into),
            armor_inscription: i.armor_inscription.as_ref().map(Into::into),
        }
    }
}

/// The ten equipment slots, in the order [`EquipSlot`] declares them.
pub const EQUIP_SLOTS: [EquipSlot; 10] = [
    EquipSlot::Bag,
    EquipSlot::Helmet,
    EquipSlot::Cape,
    EquipSlot::Weapon,
    EquipSlot::Armor,
    EquipSlot::Offhand,
    EquipSlot::Potion,
    EquipSlot::Shoes,
    EquipSlot::Food,
    EquipSlot::Mount,
];

/// Converts an inventory to its stored slot list.
///
/// `Inventory` is `[Option<ItemInstance>; 10]` and SATS has no impl for
/// fixed-size arrays, so the length is carried by convention. Reading back
/// tolerates a short or long list rather than panicking: a schema change that
/// alters the slot count should degrade, not crash the module.
pub fn inventory_to_rows(inventory: &Inventory) -> Vec<Option<ItemInstanceRow>> {
    inventory
        .slots
        .iter()
        .map(|slot| slot.as_ref().map(Into::into))
        .collect()
}

pub fn inventory_from_rows(rows: &[Option<ItemInstanceRow>]) -> Inventory {
    let mut inventory = Inventory::default();
    for (slot, row) in inventory.slots.iter_mut().zip(rows) {
        *slot = row.as_ref().map(Into::into);
    }
    inventory
}

pub fn equipment_to_rows(equipment: &Equipment) -> Vec<Option<ItemInstanceRow>> {
    EQUIP_SLOTS
        .iter()
        .map(|slot| equipment.get(*slot).as_ref().map(Into::into))
        .collect()
}

pub fn equipment_from_rows(rows: &[Option<ItemInstanceRow>]) -> Equipment {
    let mut equipment = Equipment::default();
    for (slot, row) in EQUIP_SLOTS.iter().zip(rows) {
        *equipment.get_mut(*slot) = row.as_ref().map(Into::into);
    }
    equipment
}

/// The three hotbar slots as stored.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Default)]
pub struct HotbarRow {
    pub primary: Option<String>,
    pub secondary: Option<String>,
    pub ultimate: Option<String>,
}

pub fn known_ancient_language_from_rows(
    root_words: &[String],
    ancient_words: &[String],
    base_abilities: &[String],
) -> KnownAncientLanguage {
    KnownAncientLanguage {
        root_words: root_words.iter().cloned().map(RootWordId::new).collect(),
        ancient_words: ancient_words
            .iter()
            .cloned()
            .map(AncientWordId::new)
            .collect(),
        base_abilities: base_abilities.iter().cloned().map(AbilityId::new).collect(),
    }
}

/// A player's resonance with an Ancient Word.
///
/// Domain-independent mirror: root_word_id is a string here, not the domain's
/// newtype, because SATS derives require named fields and the domain types
/// use tuple structs.
#[derive(SpacetimeType, Clone, Debug, PartialEq)]
pub struct ResonanceRow {
    pub character_id: spacetimedb::Uuid,
    pub root_word_id: String,
    pub xp: u64,
    pub level: u32,
}
