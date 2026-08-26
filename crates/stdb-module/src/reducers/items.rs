//! Inventory, equipment, hotbar selection and weapon inscriptions.
//!
//! Ported from `crates/server/src/items` (`systems.rs`, `bonuses.rs`,
//! `available_spells.rs`) and the three request handlers that lived in
//! `crates/server/src/network/server.rs`.
//!
//! # What changed against the Bevy server
//!
//! - **No anti-spoofing checks.** Every Bevy handler started by scanning the
//!   player query for the entity whose `PlayerId` matched the sending peer,
//!   precisely so a client could not name someone else's entity in the command.
//!   Here the working key is the caller's active character, resolved from
//!   `ctx.sender()` through `Session` and `Player` (see
//!   `reducers::lifecycle::caller_character`), so there is nothing to spoof and
//!   nothing to check — the lookup and the authorisation are the same
//!   operation.
//! - **Rejections are returned, not logged.** The Bevy handlers `warn!`ed and
//!   dropped an invalid command because a message receiver has no reply
//!   channel. A reducer's `Err` reaches the caller, so an invalid request now
//!   tells the player *why* instead of silently doing nothing.
//! - **No explicit persistence step.** `persist_inventory_and_equipment` (which
//!   spawned Tokio tasks against Postgres and could lose the write on a crash)
//!   has no equivalent: writing the row *is* persisting it, inside the same
//!   transaction as the mutation.
//! - **Derived state is recomputed by an explicit call**, not by a
//!   `Changed<Equipment>` query. `recompute_equipment_bonuses` and
//!   `recompute_available_spells` both ran reactively; there is no change
//!   detection here, so every reducer that touches `equipment` calls
//!   [`recompute_effective_stats`] itself.
//!
//! # Base versus effective stats
//!
//! `player_stats` holds the character's stats **without** equipment bonuses —
//! the same distinction the Bevy server kept with
//! `bonuses::base_stats_without_equipment`, and for the same reason: if the
//! stored value already contained the bonus, re-applying it on the next login
//! (or the next equip) would compound it. `entity_stats` holds the derived,
//! effective value that combat and the client read. Nothing writes bonuses back
//! into `player_stats`.

use std::collections::HashSet;
use std::sync::OnceLock;

use bevymmo_domain::abilities::inscription::{
    ArmorInscription, SecondaryWord, SlotInscription, WeaponInscription,
};
use bevymmo_domain::abilities::{
    resolve_active_ability, AbilityId, AbilitySlot, AncientWordId, AncientWordRegistry,
    BaseAbilityRegistry, RootWordId, RootWordRegistry,
};
use bevymmo_domain::items::components::{EquipSlot, Equipment, Inventory, INVENTORY_CAPACITY};
use bevymmo_domain::items::definition::{EquipRequirement, ItemCategory};
use bevymmo_domain::items::instance::{ItemInstance, ItemInstanceId, STARTER_WEAPON_ITEM_ID};
use bevymmo_domain::items::registry::{ItemId, ItemRegistry};
use spacetimedb::{reducer, ReducerContext, Table, Uuid};

use crate::reducers::lifecycle::caller_character;
use crate::rows::{
    equipment_from_rows, equipment_to_rows, inventory_from_rows, inventory_to_rows,
    known_ancient_language_from_rows,
};
use crate::tables::{
    equipment, game_entity, inventory, known_ancient_language, loot_bag_slot, market_sell_order,
    player, EntityKindRow, EquipmentTable, InventoryTable,
};

// ---------------------------------------------------------------------------
// Content registries
// ---------------------------------------------------------------------------
//
// The Bevy server built these once at `Startup` and handed them around as
// `Res<...>`. A module has no startup schedule and no resources, so they are
// process-wide statics built on first use instead. They must not be rebuilt per
// call: `default_items()` allocates a dozen `Arc`s and a `HashMap`, and every
// equip would pay for it.
//
// `OnceLock` rather than a plain `static`: the registries own trait objects, so
// they cannot be `const`-constructed. It is sound here for the same reason it is
// anywhere — the contents are immutable after initialisation.

/// Every item this build ships.
fn item_registry() -> &'static ItemRegistry {
    static REGISTRY: OnceLock<ItemRegistry> = OnceLock::new();
    REGISTRY.get_or_init(bevymmo_domain::content::items::default_items)
}

/// The weapon abilities (`BaseAbility`) items can offer.
fn ability_registry() -> &'static BaseAbilityRegistry {
    static REGISTRY: OnceLock<BaseAbilityRegistry> = OnceLock::new();
    REGISTRY.get_or_init(bevymmo_domain::content::abilities::default_base_abilities)
}

fn ancient_word_registry() -> &'static AncientWordRegistry {
    static REGISTRY: OnceLock<AncientWordRegistry> = OnceLock::new();
    REGISTRY.get_or_init(bevymmo_domain::content::ancient_words::default_ancient_words)
}

fn root_word_registry() -> &'static RootWordRegistry {
    static REGISTRY: OnceLock<RootWordRegistry> = OnceLock::new();
    REGISTRY.get_or_init(bevymmo_domain::content::root_words::default_root_words)
}

// ---------------------------------------------------------------------------
// Reducers
// ---------------------------------------------------------------------------

/// Equips the item held in inventory slot `slot_index` into the slot its
/// catalogue entry declares.
///
/// Validation order, unchanged from `items::systems::equip_item`: the index must
/// be in range, the slot must hold something, the item must exist in the
/// registry, and it must declare an `equippable_into`. What the Bevy version did
/// *not* do is check `Item::equip_requirements` — the hook existed and was never
/// read — so that check is added here (see [`check_equip_requirements`]).
///
/// If the target equipment slot is already occupied, the item that was there
/// swaps back into the inventory slot this one just left; the inventory can
/// therefore never overflow, which is why equipping has no "inventory full"
/// failure the way unequipping does.
#[reducer]
pub fn equip_item(ctx: &ReducerContext, slot_index: u8) -> Result<(), String> {
    // The caller *is* the character. No `PlayerId`-to-entity scan, and no
    // "is this really your entity" check: see the module docs.
    let character_id = caller_character(ctx)?.character_id;
    let mut inventory = load_inventory(ctx, character_id)?;
    let mut equipment = load_equipment(ctx, character_id)?;

    let index = usize::from(slot_index);
    if index >= INVENTORY_CAPACITY {
        return Err(format!(
            "inventory slot {slot_index} out of range (0..{INVENTORY_CAPACITY})"
        ));
    }

    let Some(mut instance) = inventory.slots[index].clone() else {
        return Err(format!("inventory slot {slot_index} is empty"));
    };

    let item = item_registry()
        .get(&instance.item_id)
        .ok_or_else(|| format!("unknown item {:?}", instance.item_id.as_str()))?;

    let target = item
        .config()
        .equippable_into
        .ok_or_else(|| format!("{:?} is not equippable", instance.item_id.as_str()))?;

    check_equip_requirements(item.equip_requirements())?;

    // An esemplare that has never been stored carries id 0. Give it one now:
    // from here on it can hold an Incisione, and an inscription that cannot be
    // told apart from another copy's is worse than useless.
    if !instance.instance_id.is_assigned() {
        instance.instance_id = ItemInstanceId(next_instance_id(ctx));
    }

    let previous = equipment.get_mut(target).take();
    *equipment.get_mut(target) = Some(instance);
    inventory.slots[index] = previous;

    store_inventory(ctx, character_id, &inventory);
    store_equipment(ctx, character_id, &equipment);

    // Equipment changed, so both things derived from it are now stale.
    recompute_effective_stats(ctx, character_id)?;
    Ok(())
}

/// Unequips `slot` and returns the item to the first free inventory slot.
///
/// `slot` is the equipment slot's name, case-insensitive (`"weapon"`,
/// `"helmet"`, ... — see [`parse_equip_slot`]). A string rather than an enum
/// because the reducer signature is the client-facing API and a name is
/// readable in `spacetime call` and in logs; the parse rejects anything else.
///
/// Fails, and changes nothing, when the inventory is full — same as the Bevy
/// version, which restored the item before returning the error.
#[reducer]
pub fn unequip_item(ctx: &ReducerContext, slot: String) -> Result<(), String> {
    let character_id = caller_character(ctx)?.character_id;
    let target = parse_equip_slot(&slot)?;
    let mut inventory = load_inventory(ctx, character_id)?;
    let mut equipment = load_equipment(ctx, character_id)?;

    let Some(instance) = equipment.get_mut(target).take() else {
        return Err(format!("equipment slot {slot:?} is empty"));
    };

    let Some(free) = inventory.slots.iter().position(Option::is_none) else {
        // Nothing has been written yet, so "restoring" is only a matter of not
        // storing the local copy — but put it back anyway so the local state
        // stays truthful if this function ever grows a later step.
        *equipment.get_mut(target) = Some(instance);
        return Err("inventory is full".to_string());
    };

    inventory.slots[free] = Some(instance);

    store_inventory(ctx, character_id, &inventory);
    store_equipment(ctx, character_id, &equipment);

    recompute_effective_stats(ctx, character_id)?;
    Ok(())
}

/// Moves `from` onto `to`: same-item Material piles merge up to the bag cap,
/// everything else swaps (including two full Wood stacks, so they stay
/// rearrangable).
///
/// Purely positional besides the merge: nothing derived depends on *where*
/// in the inventory an item sits, so unlike equip/unequip this touches no
/// stats and no hotbar.
#[reducer]
pub fn move_item(ctx: &ReducerContext, from: u8, to: u8) -> Result<(), String> {
    let character_id = caller_character(ctx)?.character_id;
    let (from_index, to_index) = (usize::from(from), usize::from(to));
    if from_index >= INVENTORY_CAPACITY || to_index >= INVENTORY_CAPACITY {
        return Err(format!(
            "inventory slots {from}/{to} out of range (0..{INVENTORY_CAPACITY})"
        ));
    }

    let mut inventory = load_inventory(ctx, character_id)?;
    let stacks = slots_stack_together(&inventory, from_index, to_index);
    inventory
        .move_or_merge(from_index, to_index, stacks)
        .map_err(|error| error.to_string())?;
    store_inventory(ctx, character_id, &inventory);
    Ok(())
}

/// Peels `amount` off inventory slot `slot_index` into the first empty slot.
///
/// `amount` is the size of the **new** pile (7 off 50 leaves 43). Materials
/// only; unique items are refused even if they somehow share a quantity.
#[reducer]
pub fn split_item(ctx: &ReducerContext, slot_index: u8, amount: u32) -> Result<(), String> {
    let character_id = caller_character(ctx)?.character_id;
    let index = usize::from(slot_index);
    if index >= INVENTORY_CAPACITY {
        return Err(format!(
            "inventory slot {slot_index} out of range (0..{INVENTORY_CAPACITY})"
        ));
    }

    let mut inventory = load_inventory(ctx, character_id)?;
    let stacks = match inventory.slots[index].as_ref() {
        Some(instance) => item_stacks(&instance.item_id),
        None => return Err(format!("inventory slot {slot_index} is empty")),
    };
    let next_id = next_instance_id(ctx);
    inventory
        .split_stack(index, amount, stacks, || ItemInstanceId(next_id))
        .map_err(|error| error.to_string())?;
    store_inventory(ctx, character_id, &inventory);
    Ok(())
}

/// Pulls other piles of the same Material into `slot_index` up to the bag cap.
#[reducer]
pub fn combine_item(ctx: &ReducerContext, slot_index: u8) -> Result<(), String> {
    let character_id = caller_character(ctx)?.character_id;
    let index = usize::from(slot_index);
    if index >= INVENTORY_CAPACITY {
        return Err(format!(
            "inventory slot {slot_index} out of range (0..{INVENTORY_CAPACITY})"
        ));
    }

    let mut inventory = load_inventory(ctx, character_id)?;
    let stacks = match inventory.slots[index].as_ref() {
        Some(instance) => item_stacks(&instance.item_id),
        None => return Err(format!("inventory slot {slot_index} is empty")),
    };
    inventory
        .combine_into(index, stacks)
        .map_err(|error| error.to_string())?;
    store_inventory(ctx, character_id, &inventory);
    Ok(())
}

/// Grants a catalogue item through a nearby NPC vendor.
///
/// This is intentionally server-authoritative: the caller supplies only an
/// item id and NPC entity id, while the module verifies the NPC, proximity,
/// catalogue entry and inventory capacity before creating a new instance.
#[reducer]
pub fn claim_npc_item(
    ctx: &ReducerContext,
    npc_entity_id: u64,
    item_id: String,
) -> Result<(), String> {
    let character = caller_character(ctx)?;
    // Derived from the already-resolved `character` instead of calling
    // `caller_entity` (which would resolve `caller_character` a second
    // time via `ctx.sender() -> Session -> Player`).
    let player = ctx
        .db
        .game_entity()
        .entity_id()
        .find(character.entity_id)
        .ok_or_else(|| "character has no entity".to_string())?;
    let npc = ctx
        .db
        .game_entity()
        .entity_id()
        .find(npc_entity_id)
        .ok_or_else(|| "NPC not found".to_string())?;
    if npc.kind != EntityKindRow::Npc {
        return Err("that entity is not an NPC vendor".to_string());
    }

    let dx = player.position.x - npc.position.x;
    let dy = player.position.y - npc.position.y;
    let dz = player.position.z - npc.position.z;
    if dx * dx + dy * dy + dz * dz > 36.0 {
        return Err("you are too far from that NPC".to_string());
    }

    if !is_greeter_stock(&item_id) {
        return Err(format!("unknown item {item_id:?}"));
    }

    grant_item(ctx, character.character_id, &item_id)?;
    Ok(())
}

/// Permanently destroys an item instance from the caller's inventory.
///
/// Equipment cannot be destroyed through this reducer: the UI's drag-out
/// gesture originates from the inventory, and forcing an explicit unequip
/// first prevents accidental loss of currently equipped stats/abilities.
#[reducer]
pub fn destroy_item(ctx: &ReducerContext, instance_id: u64) -> Result<(), String> {
    if instance_id == 0 {
        return Err("item instance is not assigned".to_string());
    }

    let character_id = caller_character(ctx)?.character_id;
    let mut inventory = load_inventory(ctx, character_id)?;
    let instance_id = ItemInstanceId(instance_id);
    let Some(slot) = inventory.slots.iter().position(|item| {
        item.as_ref()
            .is_some_and(|item| item.instance_id == instance_id)
    }) else {
        return Err("item instance is not in your inventory".to_string());
    };

    inventory.slots[slot] = None;
    store_inventory(ctx, character_id, &inventory);
    Ok(())
}

/// Writes the new RootWord-based inscription for the equipped weapon.
///
/// This reducer is additive to [`set_inscription`]. It persists only the new
/// `root_inscription` field, so legacy characters can be migrated without
/// rewriting their old data. All derived spell values remain server-owned.
#[reducer]
pub fn set_root_inscription(
    ctx: &ReducerContext,
    root_word: Option<String>,
    primary_words: Vec<String>,
    secondary_words: Vec<String>,
    ultimate_words: Vec<String>,
) -> Result<(), String> {
    let character_id = caller_character(ctx)?.character_id;
    let mut equipment = load_equipment(ctx, character_id)?;
    let weapon = equipment
        .get(EquipSlot::Weapon)
        .clone()
        .ok_or_else(|| "no weapon equipped".to_string())?;
    let item = item_registry()
        .get(&weapon.item_id)
        .ok_or_else(|| format!("unknown item {:?}", weapon.item_id.as_str()))?;
    let abilities = crate::sim::spells::ability_loadout_for_item(item.as_ref())
        .ok_or_else(|| format!("{:?} has no ability loadout", weapon.item_id.as_str()))?;
    let profile = item
        .rune_profile()
        .ok_or_else(|| format!("{:?} has no rune profile", weapon.item_id.as_str()))?;

    let known = ctx
        .db
        .known_ancient_language()
        .character_id()
        .find(character_id)
        .map(|row| {
            known_ancient_language_from_rows(
                &row.root_words,
                &row.ancient_words,
                &row.base_abilities,
            )
        })
        .ok_or_else(|| {
            "ancient language has not been initialized for this character".to_string()
        })?;

    let root_id = root_word.map(RootWordId::new);
    let root_cost = match &root_id {
        Some(id) => {
            if !known.knows_root_word(id) {
                return Err(format!("Root Word {:?} is not known", id.as_str()));
            }
            root_word_registry()
                .get(id)
                .ok_or_else(|| format!("unknown Root Word {:?}", id.as_str()))?
                .metadata()
                .rune_cost
        }
        None => 0,
    };

    let slot_inputs = [
        (AbilitySlot::Primary, primary_words, 2usize),
        (AbilitySlot::Secondary, secondary_words, 2usize),
        (AbilitySlot::Ultimate, ultimate_words, 1usize),
    ];
    let mut total_cost = root_cost;
    let mut slots = [
        SlotInscription::default(),
        SlotInscription::default(),
        SlotInscription::default(),
    ];

    for (index, (slot, word_ids, max_words)) in slot_inputs.into_iter().enumerate() {
        if word_ids.len() > max_words {
            return Err(format!(
                "{slot:?} accepts at most {max_words} Ancient Words"
            ));
        }
        let ability_id = resolve_active_ability(slot, abilities, &weapon.ability_selection)
            .ok_or_else(|| format!("no ability offered for {slot:?}"))?;
        let ability = ability_registry()
            .get(ability_id)
            .ok_or_else(|| format!("unknown ability {:?}", ability_id.as_str()))?;
        let mut ids = HashSet::new();
        let mut groups = HashSet::new();
        let mut words = Vec::with_capacity(word_ids.len());

        for word_id in word_ids {
            let id = AncientWordId::new(word_id);
            if !ids.insert(id.clone()) {
                return Err(format!("duplicate Ancient Word {:?}", id.as_str()));
            }
            if !known.knows_ancient_word(&id) {
                return Err(format!("Ancient Word {:?} is not known", id.as_str()));
            }
            let word = ancient_word_registry()
                .get(&id)
                .ok_or_else(|| format!("unknown Ancient Word {:?}", id.as_str()))?;
            let metadata = word.metadata();
            if !metadata.is_compatible_with(ability.tags()) {
                return Err(format!(
                    "Ancient Word {:?} is incompatible with {slot:?}",
                    id.as_str()
                ));
            }
            if let Some(group) = metadata.exclusive_group {
                if !groups.insert(group) {
                    return Err(format!("Ancient Word conflict in group {group:?}"));
                }
            }
            total_cost += metadata.rune_cost;
            words.push(SecondaryWord::new(id));
        }
        slots[index] = SlotInscription {
            secondary_words: words,
        };
    }

    if total_cost > profile.capacity {
        return Err(format!(
            "rune capacity exceeded: {total_cost} / {}",
            profile.capacity
        ));
    }

    let mut updated = weapon;
    updated.root_inscription = Some(WeaponInscription {
        root_word: root_id,
        primary: slots[0].clone(),
        secondary: slots[1].clone(),
        ultimate: slots[2].clone(),
    });
    *equipment.get_mut(EquipSlot::Weapon) = Some(updated);
    store_equipment(ctx, character_id, &equipment);
    Ok(())
}

/// Writes the independent inscription of an equipped armor item.
///
/// Armor deliberately has its own compact shape instead of pretending to have
/// weapon Primary/Secondary/Ultimate slots. The item remains authoritative for
/// the offered abilities; this reducer only persists the Root Word language data.
#[reducer]
pub fn set_armor_inscription(
    ctx: &ReducerContext,
    slot: String,
    root_word: Option<String>,
    secondary_words: Vec<String>,
) -> Result<(), String> {
    let character_id = caller_character(ctx)?.character_id;
    let target = parse_equip_slot(&slot)?;
    if !matches!(
        target,
        EquipSlot::Helmet | EquipSlot::Armor | EquipSlot::Shoes
    ) {
        return Err("armor inscriptions are only valid for helmet, armor or shoes".to_string());
    }

    let mut equipment = load_equipment(ctx, character_id)?;
    let item_instance = equipment
        .get(target)
        .clone()
        .ok_or_else(|| format!("equipment slot {slot:?} is empty"))?;
    let item = item_registry()
        .get(&item_instance.item_id)
        .ok_or_else(|| format!("unknown item {:?}", item_instance.item_id.as_str()))?;
    let abilities =
        crate::sim::spells::ability_loadout_for_item(item.as_ref()).ok_or_else(|| {
            format!(
                "{:?} has no armor abilities",
                item_instance.item_id.as_str()
            )
        })?;
    let profile = item
        .rune_profile()
        .ok_or_else(|| format!("{:?} has no rune profile", item_instance.item_id.as_str()))?;

    let language_row = ctx
        .db
        .known_ancient_language()
        .character_id()
        .find(character_id)
        .ok_or_else(|| "ancient language has not been initialized".to_string())?;
    let language = known_ancient_language_from_rows(
        &language_row.root_words,
        &language_row.ancient_words,
        &language_row.base_abilities,
    );

    let root_id = root_word.map(RootWordId::new);
    let mut total_cost = match &root_id {
        Some(id) => {
            if !language.knows_root_word(id) {
                return Err(format!("Root Word {:?} is not known", id.as_str()));
            }
            root_word_registry()
                .get(id)
                .ok_or_else(|| format!("unknown Root Word {:?}", id.as_str()))?
                .metadata()
                .rune_cost
        }
        None => 0,
    };

    if secondary_words.len() > 2 {
        return Err("armor accepts at most 2 Ancient Words".to_string());
    }
    let ability_id = abilities
        .primary
        .first()
        .ok_or_else(|| "armor has no primary ability".to_string())?;
    let ability = ability_registry()
        .get(ability_id)
        .ok_or_else(|| format!("unknown armor ability {:?}", ability_id.as_str()))?;
    let mut seen = HashSet::new();
    let mut words = Vec::with_capacity(secondary_words.len());
    for word_id in secondary_words {
        let id = AncientWordId::new(word_id);
        if !seen.insert(id.clone()) {
            return Err(format!("duplicate Ancient Word {:?}", id.as_str()));
        }
        if !language.knows_ancient_word(&id) {
            return Err(format!("Ancient Word {:?} is not known", id.as_str()));
        }
        let word = ancient_word_registry()
            .get(&id)
            .ok_or_else(|| format!("unknown Ancient Word {:?}", id.as_str()))?;
        let metadata = word.metadata();
        if !metadata.is_compatible_with(ability.tags()) {
            return Err(format!(
                "Ancient Word {:?} is incompatible with armor",
                id.as_str()
            ));
        }
        total_cost += metadata.rune_cost;
        words.push(SecondaryWord::new(id));
    }

    if total_cost > profile.capacity {
        return Err(format!(
            "rune capacity exceeded: {total_cost} / {}",
            profile.capacity
        ));
    }

    let mut updated = item_instance;
    updated.armor_inscription = Some(ArmorInscription {
        root_word: root_id,
        secondary_words: words,
    });
    *equipment.get_mut(target) = Some(updated);
    store_equipment(ctx, character_id, &equipment);
    Ok(())
}

/// Picks which of the equipped weapon's offered gestures is active on
/// `"primary"` or `"secondary"`.
///
/// The salvage rule is kept: when the new gesture makes the slot's existing
/// Incisione invalid — a Modificatore that needed a tag the old gesture had —
/// the slot's glyphs are cleared rather than the request refused, otherwise a
/// player could get stuck unable to switch gesture at all.
#[reducer]
pub fn set_ability_selection(
    ctx: &ReducerContext,
    slot: String,
    ability_id: String,
) -> Result<(), String> {
    let character_id = caller_character(ctx)?.character_id;
    let target = parse_ability_slot(&slot)?;
    let mut equipment = load_equipment(ctx, character_id)?;

    let weapon = equipment
        .get(EquipSlot::Weapon)
        .clone()
        .ok_or_else(|| "no weapon equipped".to_string())?;
    let item = item_registry()
        .get(&weapon.item_id)
        .ok_or_else(|| format!("unknown item {:?}", weapon.item_id.as_str()))?;
    let Some(abilities) = crate::sim::spells::ability_loadout_for_item(item.as_ref()) else {
        return Err(format!(
            "{:?} offers no gestures to choose from",
            weapon.item_id.as_str()
        ));
    };

    let requested = AbilityId::new(ability_id.clone());
    if !abilities.options_for(target).contains(&requested) {
        return Err(format!(
            "{ability_id:?} is not offered on {slot:?} by {:?}",
            weapon.item_id.as_str()
        ));
    }

    let mut selection = weapon.ability_selection.clone();
    selection.assign(target, Some(requested));

    let mut updated = weapon;
    updated.ability_selection = selection;
    *equipment.get_mut(EquipSlot::Weapon) = Some(updated);
    store_equipment(ctx, character_id, &equipment);
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared API: derived state
// ---------------------------------------------------------------------------

/// Recomputes `entity_stats` for `character_id` from `player_stats` plus the
/// bonuses of everything currently equipped.
///
/// **Shared API — call this from any reducer that changes a character's base
/// stats or their equipment.** It replaces `bonuses::recompute_equipment_bonuses`,
/// which ran reactively on `Changed<Equipment>`; there is no change detection in
/// a module, so the caller is responsible for invoking it. It is idempotent:
/// running it twice in a row produces the same row, because it always rebuilds
/// from the base values rather than adjusting the previous result. That is what
/// makes the `AppliedEquipmentBonus` snapshot the Bevy server carried around
/// unnecessary — bonuses are never "reverted" here, they are simply not part of
/// what is stored.
///
/// Live pools are preserved: equipment changes `max_health`/`max_mana`, so
/// `current_health` and `current_mana` are taken from the existing
/// `entity_stats` row (falling back to the base row on first computation) and
/// re-clamped to the new maxima. Taking them from `player_stats` instead would
/// silently full-heal a character every time they swapped a helmet.
pub fn recompute_effective_stats(ctx: &ReducerContext, character_id: Uuid) -> Result<(), String> {
    let player = ctx
        .db
        .player()
        .character_id()
        .find(character_id)
        .ok_or_else(|| "no character with this id".to_string())?;

    // Delegates rather than deriving the stats here. `sim::combat` owns
    // `entity_stats`: it folds equipment *and* timed modifiers into the base,
    // and it is the only writer of `game_entity.speed`. Computing equipment
    // bonuses separately here would produce a row missing every active buff,
    // which the next combat tick would silently overwrite.
    crate::sim::combat::recalculate_effective_stats(ctx, player.entity_id);
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared API: minting items
// ---------------------------------------------------------------------------

fn is_greeter_stock(item_id: &str) -> bool {
    bevymmo_domain::content::items::greeter_stock().contains(&item_id)
}

/// Puts a freshly minted esemplare of `item_id` into the first free inventory
/// slot, returning the slot it landed in.
///
/// **Shared API — this is the only sanctioned way to create an item.** It is a
/// plain function and not a reducer on purpose: a client-callable "give me an
/// item" is an item duplication exploit with extra steps. Loot, quest rewards
/// and starter kits call it from inside their own reducer.
///
/// The `item_id` is checked against the registry: an inventory holding an id
/// nothing can look up is a slot the player can never equip or drop.
pub fn grant_item(ctx: &ReducerContext, character_id: Uuid, item_id: &str) -> Result<u8, String> {
    grant_items(ctx, character_id, item_id, 1)?;
    Ok(0)
}

pub(crate) fn item_category(item_id: &str) -> Option<ItemCategory> {
    item_registry()
        .get(&ItemId::new(item_id.to_string()))
        .map(|item| item.config().category)
}

pub(crate) fn item_stacks(item_id: &ItemId) -> bool {
    item_registry()
        .get(item_id)
        .is_some_and(|item| Inventory::stacks_category(item.config().category))
}

fn slots_stack_together(inventory: &Inventory, from: usize, to: usize) -> bool {
    match (&inventory.slots[from], &inventory.slots[to]) {
        (Some(src), Some(dst)) if src.item_id == dst.item_id => item_stacks(&src.item_id),
        _ => false,
    }
}

/// Puts `amount` of `item_id` into the inventory, stacking Materials in the bag.
///
/// Returns how many pieces were actually placed. A short grant still persists:
/// callers that must not debit a node for undelivered pieces should check
/// `space_for` first (gathering does).
pub fn grant_items(
    ctx: &ReducerContext,
    character_id: Uuid,
    item_id: &str,
    amount: u32,
) -> Result<u32, String> {
    if amount == 0 {
        return Ok(0);
    }
    let id = ItemId::new(item_id.to_string());
    let item = item_registry()
        .get(&id)
        .ok_or_else(|| format!("unknown item {item_id:?}"))?;

    let mut inventory = load_inventory(ctx, character_id)?;
    let mut next_id = next_instance_id(ctx);
    let added = if Inventory::stacks_category(item.config().category) {
        inventory.add_stackable(id, amount, || {
            let minted = ItemInstanceId(next_id);
            next_id += 1;
            minted
        })
    } else {
        let mut added = 0u32;
        for _ in 0..amount {
            let Some(free) = inventory.slots.iter().position(Option::is_none) else {
                break;
            };
            let mut instance = ItemInstance::new(id.clone());
            instance.instance_id = ItemInstanceId(next_id);
            next_id += 1;
            inventory.slots[free] = Some(instance);
            added += 1;
        }
        added
    };

    if added == 0 {
        return Err("inventory is full".to_string());
    }
    store_inventory(ctx, character_id, &inventory);
    Ok(added)
}

/// Places an existing esemplare into the bag, keeping its `instance_id` and
/// inscriptions. Used by loot so a corpse dump is not a re-mint.
pub(crate) fn grant_instance(
    ctx: &ReducerContext,
    character_id: Uuid,
    instance: ItemInstance,
    stacks: bool,
) -> Result<Option<ItemInstance>, String> {
    let mut inventory = load_inventory(ctx, character_id)?;
    let leftover = inventory
        .insert_instance(instance, stacks)
        .map_err(|error| error.to_string())?;
    let mut next_id = next_instance_id(ctx);
    for slot in &mut inventory.slots {
        let Some(item) = slot else {
            continue;
        };
        if item.instance_id.is_assigned() {
            continue;
        }
        item.instance_id = ItemInstanceId(next_id);
        next_id += 1;
    }
    store_inventory(ctx, character_id, &inventory);
    Ok(leftover)
}

/// Moves the granted starter staff from inventory onto the weapon slot and
/// inscribes the default Root Word. Called from `join` after both tables exist.
pub(crate) fn equip_granted_starter_staff(
    ctx: &ReducerContext,
    character_id: Uuid,
    entity_id: u64,
) -> Result<(), String> {
    let mut inventory = load_inventory(ctx, character_id)?;
    let slot = inventory
        .slots
        .iter()
        .position(|entry| {
            entry
                .as_ref()
                .is_some_and(|item| item.item_id.as_str() == STARTER_WEAPON_ITEM_ID)
        })
        .ok_or_else(|| "starter weapon was not granted".to_string())?;
    let mut instance = inventory.slots[slot]
        .take()
        .expect("slot was just found occupied");
    instance.inscribe_starter_root_word();

    let mut equipment = load_equipment(ctx, character_id)?;
    *equipment.get_mut(EquipSlot::Weapon) = Some(instance);
    store_inventory(ctx, character_id, &inventory);
    store_equipment(ctx, character_id, &equipment);
    crate::sim::combat::recalculate_effective_stats(ctx, entity_id);
    Ok(())
}

/// The next free `ItemInstanceId`: one past the highest one stored anywhere.
///
/// `ItemInstance::new` no longer mints an id (it used to be a random `Uuid`, and
/// `getrandom` has no backend in the sandbox), so someone has to. It cannot be
/// an `#[auto_inc]` column either, because instances are not rows — they live
/// *inside* the `Vec<Option<ItemInstanceRow>>` of `inventory` and `equipment`.
///
/// So: scan. The cost is one pass over both tables per minted item, which is
/// fine at the rate items are created and wrong at the rate they would be if
/// this were ever called in a loop. If it becomes hot, the fix is a one-row
/// counter table, which the schema does not currently have.
pub(crate) fn next_instance_id(ctx: &ReducerContext) -> u64 {
    let from_inventories = ctx
        .db
        .inventory()
        .iter()
        .flat_map(|row| row.slots.into_iter().flatten())
        .map(|item| item.instance_id);
    let from_equipment = ctx
        .db
        .equipment()
        .iter()
        .flat_map(|row| row.slots.into_iter().flatten())
        .map(|item| item.instance_id);
    // Listed piles leave the bag but keep their instance id. Skipping them
    // here would reuse an escrowed id the next time a stack is peeled.
    let from_orders = ctx
        .db
        .market_sell_order()
        .iter()
        .map(|row| row.item.instance_id);
    let from_loot = ctx
        .db
        .loot_bag_slot()
        .iter()
        .map(|row| row.item.instance_id);

    from_inventories
        .chain(from_equipment)
        .chain(from_orders)
        .chain(from_loot)
        .max()
        .unwrap_or(0)
        + 1
}

// Equipment bonuses are folded into the effective stats by `sim::combat`, which
// also folds in the timed modifiers. Computing them a second time here would
// produce a row missing every active buff.

/// Rejects an item whose equip requirements the character cannot meet.
///
/// `EquipRequirement::MinLevel` is currently unmeetable by construction: the
/// module has no character level anywhere in the schema. Failing closed is the
/// safer half of the trade — no shipped item declares a requirement, so nothing
/// is blocked today, and the day one does it will be blocked loudly instead of
/// silently equipped by a server that forgot to check.
fn check_equip_requirements(requirements: &[EquipRequirement]) -> Result<(), String> {
    for requirement in requirements {
        match requirement {
            EquipRequirement::MinLevel { value: 0 } => {}
            EquipRequirement::MinLevel { value } => {
                return Err(format!(
                    "requires level {value}, and characters have no level yet"
                ))
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Row access
// ---------------------------------------------------------------------------

pub(crate) fn load_inventory(
    ctx: &ReducerContext,
    character_id: Uuid,
) -> Result<Inventory, String> {
    ctx.db
        .inventory()
        .character_id()
        .find(character_id)
        .map(|row| inventory_from_rows(&row.slots))
        .ok_or_else(|| "no character for this identity; call `join` first".to_string())
}

pub(crate) fn store_inventory(ctx: &ReducerContext, character_id: Uuid, inventory: &Inventory) {
    ctx.db.inventory().character_id().update(InventoryTable {
        character_id,
        slots: inventory_to_rows(inventory),
    });
}

pub(crate) fn load_equipment(
    ctx: &ReducerContext,
    character_id: Uuid,
) -> Result<Equipment, String> {
    ctx.db
        .equipment()
        .character_id()
        .find(character_id)
        .map(|row| equipment_from_rows(&row.slots))
        .ok_or_else(|| "no character for this identity; call `join` first".to_string())
}

pub(crate) fn store_equipment(ctx: &ReducerContext, character_id: Uuid, equipment: &Equipment) {
    ctx.db.equipment().character_id().update(EquipmentTable {
        character_id,
        slots: equipment_to_rows(equipment),
    });
}

// ---------------------------------------------------------------------------
// Parsing the string parameters
// ---------------------------------------------------------------------------
//
// Reducer parameters could be SATS enums instead of strings, and for an enum
// with ten variants that would be tempting. They are strings because the module
// is also driven by hand — `spacetime call bevymmo unequip_item '["weapon"]'` —
// and because the client bindings turn either into the same thing. The parse is
// strict and total, so nothing is lost on the validation side.

/// Parses an equipment slot name, case-insensitively.
fn parse_equip_slot(name: &str) -> Result<EquipSlot, String> {
    EquipSlot::ALL
        .into_iter()
        .find(|slot| slot.label().eq_ignore_ascii_case(name.trim()))
        .ok_or_else(|| {
            format!(
                "unknown equipment slot {name:?}; expected one of bag, helmet, cape, weapon, \
                 armor, offhand, potion, shoes, food, mount"
            )
        })
}

/// Parses an ability slot name, case-insensitively.
fn parse_ability_slot(name: &str) -> Result<AbilitySlot, String> {
    match name.trim().to_ascii_lowercase().as_str() {
        "primary" => Ok(AbilitySlot::Primary),
        "secondary" => Ok(AbilitySlot::Secondary),
        "ultimate" => Ok(AbilitySlot::Ultimate),
        other => Err(format!(
            "unknown ability slot {other:?}; expected primary, secondary or ultimate"
        )),
    }
}

#[cfg(test)]
mod greeter_stock_tests {
    use super::*;

    #[test]
    fn greeter_accepts_the_sword_and_simple_armor() {
        assert!(is_greeter_stock("sword"));
        assert!(is_greeter_stock("simple_helm"));
        assert!(is_greeter_stock("simple_cuirass"));
        assert!(is_greeter_stock("simple_buckler"));
        assert!(!is_greeter_stock("bow"));
        assert!(!is_greeter_stock("hammer"));
        assert!(!is_greeter_stock("mage_staff"));
    }

    #[test]
    fn greeter_rejects_retired_and_unknown_ids() {
        assert!(!is_greeter_stock("longbow"));
        assert!(!is_greeter_stock("conduit_staff_t4"));
        assert!(!is_greeter_stock("arcane_focus"));
        assert!(!is_greeter_stock("not_an_item"));
    }
}
