//! ECS components for inventory and equipment state.
//!
//! These components are replicated by lightyear (see `network::protocol`) so
//! the client renders server-authoritative state. The client never mutates
//! them directly; it sends [`super::events`] commands to request changes.

use serde::{Deserialize, Serialize};

use super::definition::ItemCategory;
use super::instance::{ItemInstance, ItemInstanceId};
use super::registry::ItemId;

/// Dedicated equipment slot.
///
/// One variant per body/utility slot shown by the inventory UI. Adding a new
/// variant requires: a new [`Equipment`] field, a migration adding the matching
/// DB column, and appending the variant here (existing serialized data stays
/// valid as long as variants are never removed or reordered destructively).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum EquipSlot {
    Bag,
    Helmet,
    Cape,
    #[default]
    Weapon,
    Armor,
    Offhand,
    Potion,
    Shoes,
    Food,
    Mount,
}

impl EquipSlot {
    /// Every equip slot, in the display order used by the inventory UI
    /// (matches the 3x3 grid + Mount layout of the reference design).
    pub const ALL: [EquipSlot; 10] = [
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

    /// Short uppercase label shown above the slot box in the UI.
    pub fn label(&self) -> &'static str {
        match self {
            EquipSlot::Bag => "BAG",
            EquipSlot::Helmet => "HELMET",
            EquipSlot::Cape => "CAPE",
            EquipSlot::Weapon => "WEAPON",
            EquipSlot::Armor => "ARMOR",
            EquipSlot::Offhand => "OFFHAND",
            EquipSlot::Potion => "POTION",
            EquipSlot::Shoes => "SHOES",
            EquipSlot::Food => "FOOD",
            EquipSlot::Mount => "MOUNT",
        }
    }
}

/// Number of generic rectangular slots in [`Inventory`].
///
/// Adding slots changes the serialized schema because the inventory uses a
/// fixed-size array on the wire and on disk.
pub const INVENTORY_CAPACITY: usize = 30;

/// Why a stack operation was refused. The bag is left unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackOpError {
    SlotOutOfRange,
    SlotEmpty,
    NotStackable,
    AmountZero,
    AmountNotLessThanQuantity,
    AmountExceedsQuantity,
    InventoryFull,
}

impl StackOpError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SlotOutOfRange => "inventory slot out of range",
            Self::SlotEmpty => "inventory slot is empty",
            Self::NotStackable => "item does not stack",
            Self::AmountZero => "amount must be at least 1",
            Self::AmountNotLessThanQuantity => "split amount must leave at least one piece",
            Self::AmountExceedsQuantity => "not enough of that item",
            Self::InventoryFull => "inventory is full",
        }
    }
}

impl std::fmt::Display for StackOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::error::Error for StackOpError {}

/// Generic inventory of a player.
///
/// Unique items occupy one slot each. Materials stack in a slot up to
/// [`Inventory::MAX_STACK`]; the player can split a pile into two slots or
/// merge two piles of the same id. The layout is a fixed-size array so the
/// UI is stable (slot 7 is always slot 7) and serialization is compact.
///
/// The component is replicated: the client sees server changes as soon as
/// they arrive, and never writes here directly.
#[cfg_attr(feature = "bevy", derive(bevy_ecs::component::Component))]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Inventory {
    pub slots: [Option<ItemInstance>; INVENTORY_CAPACITY],
}

impl Inventory {
    /// Maximum quantity in one occupied slot. The bag owns this cap, not the item.
    pub const MAX_STACK: u32 = 200;

    /// Materials stack. Everything else stays unique (one instance per slot).
    pub fn stacks_category(category: ItemCategory) -> bool {
        matches!(category, ItemCategory::Material)
    }

    /// Total quantity of `item_id` across every occupied slot.
    pub fn count_item(&self, item_id: &ItemId) -> u32 {
        self.slots
            .iter()
            .flatten()
            .filter(|item| item.item_id == *item_id)
            .map(|item| item.quantity)
            .sum()
    }

    /// Removes `amount` pieces of `item_id` from the first matching slots.
    ///
    /// Whole stacks whose remaining quantity hits zero are cleared. Unique
    /// items (quantity 1) are consumed one instance at a time.
    pub fn remove_item_amount(
        &mut self,
        item_id: &ItemId,
        amount: u32,
    ) -> Result<(), StackOpError> {
        if amount == 0 {
            return Err(StackOpError::AmountZero);
        }
        if self.count_item(item_id) < amount {
            return Err(StackOpError::AmountExceedsQuantity);
        }
        let mut remaining = amount;
        for slot in &mut self.slots {
            if remaining == 0 {
                break;
            }
            let Some(item) = slot.as_mut() else {
                continue;
            };
            if item.item_id != *item_id {
                continue;
            }
            if item.quantity <= remaining {
                remaining -= item.quantity;
                *slot = None;
            } else {
                item.quantity -= remaining;
                remaining = 0;
            }
        }
        debug_assert_eq!(remaining, 0);
        Ok(())
    }

    /// How many more pieces of `item_id` this bag can accept.
    pub fn space_for(&self, item_id: &ItemId, stacks: bool) -> u32 {
        if !stacks {
            return self.slots.iter().filter(|slot| slot.is_none()).count() as u32;
        }
        let mut space = 0u32;
        for slot in &self.slots {
            match slot {
                Some(item) if item.item_id == *item_id && item.is_stack_mergeable() => {
                    space = space.saturating_add(Self::MAX_STACK.saturating_sub(item.quantity));
                }
                None => space = space.saturating_add(Self::MAX_STACK),
                _ => {}
            }
        }
        space
    }

    /// Adds up to `amount` of a stackable item. Returns how many were placed.
    pub fn add_stackable(
        &mut self,
        item_id: ItemId,
        amount: u32,
        mut mint: impl FnMut() -> ItemInstanceId,
    ) -> u32 {
        if amount == 0 {
            return 0;
        }
        let mut remaining = amount;
        for slot in &mut self.slots {
            if remaining == 0 {
                break;
            }
            let Some(item) = slot.as_mut() else {
                continue;
            };
            if item.item_id != item_id || !item.is_stack_mergeable() {
                continue;
            }
            let room = Self::MAX_STACK.saturating_sub(item.quantity);
            let add = remaining.min(room);
            item.quantity += add;
            remaining -= add;
        }
        while remaining > 0 {
            let Some(index) = self.slots.iter().position(Option::is_none) else {
                break;
            };
            let add = remaining.min(Self::MAX_STACK);
            let mut instance = ItemInstance::new(item_id.clone());
            instance.instance_id = mint();
            instance.quantity = add;
            self.slots[index] = Some(instance);
            remaining -= add;
        }
        amount - remaining
    }

    /// Places `instance` into this bag.
    ///
    /// Mergeable materials fill existing stacks first, then occupy at most one
    /// empty slot. Anything that still does not fit is returned so the caller
    /// can leave it where it came from (a loot bag). Unique items are
    /// all-or-nothing: [`StackOpError::InventoryFull`] leaves the bag unchanged.
    pub fn insert_instance(
        &mut self,
        mut instance: ItemInstance,
        stacks: bool,
    ) -> Result<Option<ItemInstance>, StackOpError> {
        if instance.quantity == 0 {
            return Err(StackOpError::AmountZero);
        }
        let merge = stacks && instance.is_stack_mergeable();
        if !merge {
            let Some(index) = self.slots.iter().position(Option::is_none) else {
                return Err(StackOpError::InventoryFull);
            };
            self.slots[index] = Some(instance);
            return Ok(None);
        }

        let mut placed = 0u32;
        for slot in &mut self.slots {
            if instance.quantity == 0 {
                break;
            }
            let Some(existing) = slot.as_mut() else {
                continue;
            };
            if existing.item_id != instance.item_id || !existing.is_stack_mergeable() {
                continue;
            }
            let room = Self::MAX_STACK.saturating_sub(existing.quantity);
            let add = instance.quantity.min(room);
            existing.quantity += add;
            instance.quantity -= add;
            placed += add;
        }

        if instance.quantity > 0 {
            if let Some(index) = self.slots.iter().position(Option::is_none) {
                let qty = instance.quantity.min(Self::MAX_STACK);
                let leftover_qty = instance.quantity - qty;
                let mut occupying = instance.clone();
                occupying.quantity = qty;
                if leftover_qty > 0 {
                    occupying.instance_id = ItemInstanceId::unassigned();
                }
                self.slots[index] = Some(occupying);
                instance.quantity = leftover_qty;
                placed += qty;
            }
        }

        if placed == 0 {
            Err(StackOpError::InventoryFull)
        } else if instance.quantity == 0 {
            Ok(None)
        } else {
            Ok(Some(instance))
        }
    }

    /// Amount the item-info split control should start at for a pile of `quantity`.
    ///
    /// Half the pile, leaving at least one piece in the source. `0` when the
    /// pile cannot be split.
    pub fn default_split_amount(quantity: u32) -> u32 {
        match Self::split_amount_bounds(quantity) {
            Some((lo, hi)) => (quantity / 2).clamp(lo, hi),
            None => 0,
        }
    }

    /// Inclusive `1 ..= quantity - 1` when `quantity >= 2`.
    pub fn split_amount_bounds(quantity: u32) -> Option<(u32, u32)> {
        if quantity < 2 {
            None
        } else {
            Some((1, quantity - 1))
        }
    }

    /// Clamps a typed/stepped amount into the legal split window.
    pub fn clamp_split_amount(amount: u32, quantity: u32) -> u32 {
        let Some((lo, hi)) = Self::split_amount_bounds(quantity) else {
            return 0;
        };
        amount.clamp(lo, hi)
    }

    /// Inclusive `1 ..= available` when the pile is not empty.
    pub fn trade_amount_bounds(available: u32) -> Option<(u32, u32)> {
        if available == 0 {
            None
        } else {
            Some((1, available))
        }
    }

    /// Sell-all by default. Unique items stay at 1.
    pub fn default_trade_amount(available: u32) -> u32 {
        available
    }

    /// Clamps a listing/instant-sell amount into `1 ..= available`.
    pub fn clamp_trade_amount(amount: u32, available: u32) -> u32 {
        let Some((lo, hi)) = Self::trade_amount_bounds(available) else {
            return 0;
        };
        amount.clamp(lo, hi)
    }

    /// Peels `amount` off `from` into the first empty slot.
    ///
    /// `stacks` is the bag rule for this item type (Materials only). Unique
    /// items — even two uninscribed swords of the same id — must pass `false`.
    pub fn split_stack(
        &mut self,
        from: usize,
        amount: u32,
        stacks: bool,
        mint: impl FnOnce() -> ItemInstanceId,
    ) -> Result<usize, StackOpError> {
        if from >= INVENTORY_CAPACITY {
            return Err(StackOpError::SlotOutOfRange);
        }
        if !stacks {
            return Err(StackOpError::NotStackable);
        }
        if amount == 0 {
            return Err(StackOpError::AmountZero);
        }
        let Some(source) = self.slots[from].as_ref() else {
            return Err(StackOpError::SlotEmpty);
        };
        if !source.is_stack_mergeable() {
            return Err(StackOpError::NotStackable);
        }
        if amount >= source.quantity {
            return Err(StackOpError::AmountNotLessThanQuantity);
        }
        let dest = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or(StackOpError::InventoryFull)?;

        let mut peeled = source.clone();
        peeled.instance_id = mint();
        peeled.quantity = amount;
        if let Some(source) = self.slots[from].as_mut() {
            source.quantity -= amount;
        }
        self.slots[dest] = Some(peeled);
        Ok(dest)
    }

    /// Removes `amount` from `slot` and returns that pile.
    ///
    /// Taking the whole pile empties the slot and keeps the original
    /// `instance_id`. Taking less peels a new instance (minted here) and
    /// leaves the remainder in place — the bag does not need a free slot,
    /// because the peeled copy is leaving the inventory (market listing).
    pub fn take_amount(
        &mut self,
        slot: usize,
        amount: u32,
        stacks: bool,
        mint: impl FnOnce() -> ItemInstanceId,
    ) -> Result<ItemInstance, StackOpError> {
        if slot >= INVENTORY_CAPACITY {
            return Err(StackOpError::SlotOutOfRange);
        }
        if amount == 0 {
            return Err(StackOpError::AmountZero);
        }
        let Some(source) = self.slots[slot].as_ref() else {
            return Err(StackOpError::SlotEmpty);
        };
        if amount > source.quantity {
            return Err(StackOpError::AmountExceedsQuantity);
        }
        if amount == source.quantity {
            return Ok(self.slots[slot].take().expect("slot was occupied"));
        }
        if !stacks || !source.is_stack_mergeable() {
            return Err(StackOpError::NotStackable);
        }

        let mut peeled = source.clone();
        peeled.instance_id = mint();
        peeled.quantity = amount;
        if let Some(source) = self.slots[slot].as_mut() {
            source.quantity -= amount;
        }
        Ok(peeled)
    }

    /// Whether `from` can pour into `to` without swapping.
    ///
    /// Dest must have room (`quantity < MAX_STACK`). A full dest is *not*
    /// mergeable so [`Self::move_or_merge`] can fall back to a swap.
    pub fn can_merge_slots(&self, from: usize, to: usize, stacks: bool) -> bool {
        if !stacks || from == to || from >= INVENTORY_CAPACITY || to >= INVENTORY_CAPACITY {
            return false;
        }
        let (Some(src), Some(dst)) = (&self.slots[from], &self.slots[to]) else {
            return false;
        };
        src.item_id == dst.item_id
            && src.is_stack_mergeable()
            && dst.is_stack_mergeable()
            && dst.quantity < Self::MAX_STACK
    }

    /// True when another occupied slot can pour into `dest`.
    pub fn has_other_mergeable_stack(&self, dest: usize, stacks: bool) -> bool {
        (0..INVENTORY_CAPACITY).any(|index| self.can_merge_slots(index, dest, stacks))
    }

    /// Moves `from` onto `to`, merging same-item Material piles when there is
    /// room and swapping otherwise (including a dest already at `MAX_STACK`).
    pub fn move_or_merge(
        &mut self,
        from: usize,
        to: usize,
        stacks: bool,
    ) -> Result<(), StackOpError> {
        if from >= INVENTORY_CAPACITY || to >= INVENTORY_CAPACITY {
            return Err(StackOpError::SlotOutOfRange);
        }
        if from == to {
            return Ok(());
        }
        if self.can_merge_slots(from, to, stacks) {
            self.transfer_stack(from, to);
            return Ok(());
        }
        self.slots.swap(from, to);
        Ok(())
    }

    /// Pulls other piles of the same id into `dest` up to [`Self::MAX_STACK`].
    ///
    /// Returns how many pieces were added. `Ok(0)` when nothing could move.
    pub fn combine_into(&mut self, dest: usize, stacks: bool) -> Result<u32, StackOpError> {
        if dest >= INVENTORY_CAPACITY {
            return Err(StackOpError::SlotOutOfRange);
        }
        if !stacks {
            return Err(StackOpError::NotStackable);
        }
        let Some(target) = self.slots[dest].as_ref() else {
            return Err(StackOpError::SlotEmpty);
        };
        if !target.is_stack_mergeable() {
            return Err(StackOpError::NotStackable);
        }

        let mut added = 0u32;
        for index in 0..INVENTORY_CAPACITY {
            if !self.can_merge_slots(index, dest, true) {
                continue;
            }
            let before = self.slots[dest]
                .as_ref()
                .map(|item| item.quantity)
                .unwrap_or(0);
            self.transfer_stack(index, dest);
            let after = self.slots[dest]
                .as_ref()
                .map(|item| item.quantity)
                .unwrap_or(0);
            added += after.saturating_sub(before);
        }
        Ok(added)
    }

    fn transfer_stack(&mut self, from: usize, to: usize) {
        let src_qty = self.slots[from]
            .as_ref()
            .map(|item| item.quantity)
            .unwrap_or(0);
        let dest_qty = self.slots[to]
            .as_ref()
            .map(|item| item.quantity)
            .unwrap_or(0);
        let moved = src_qty.min(Self::MAX_STACK.saturating_sub(dest_qty));
        if moved == 0 {
            return;
        }
        if let Some(dest) = self.slots[to].as_mut() {
            dest.quantity = dest.quantity.saturating_add(moved);
        }
        match self.slots[from].as_mut() {
            Some(src) if src.quantity > moved => src.quantity -= moved,
            Some(_) => self.slots[from] = None,
            None => {}
        }
    }
}

/// Current equipment of a player: one optional item instance per [`EquipSlot`].
///
/// The component is replicated and predicted, same as [`Inventory`]. The
/// client never writes here directly; it sends [`super::events::EquipItemCommand`]
/// / [`super::events::UnequipItemCommand`] and reads the replicated result.
#[cfg_attr(feature = "bevy", derive(bevy_ecs::component::Component))]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Equipment {
    pub bag: Option<ItemInstance>,
    pub helmet: Option<ItemInstance>,
    pub cape: Option<ItemInstance>,
    pub weapon: Option<ItemInstance>,
    pub armor: Option<ItemInstance>,
    pub offhand: Option<ItemInstance>,
    pub potion: Option<ItemInstance>,
    pub shoes: Option<ItemInstance>,
    pub food: Option<ItemInstance>,
    pub mount: Option<ItemInstance>,
}

impl Equipment {
    /// Reads the item instance currently occupying `slot`, if any.
    pub fn get(&self, slot: EquipSlot) -> &Option<ItemInstance> {
        match slot {
            EquipSlot::Bag => &self.bag,
            EquipSlot::Helmet => &self.helmet,
            EquipSlot::Cape => &self.cape,
            EquipSlot::Weapon => &self.weapon,
            EquipSlot::Armor => &self.armor,
            EquipSlot::Offhand => &self.offhand,
            EquipSlot::Potion => &self.potion,
            EquipSlot::Shoes => &self.shoes,
            EquipSlot::Food => &self.food,
            EquipSlot::Mount => &self.mount,
        }
    }

    /// Mutable access to the item instance occupying `slot`.
    pub fn get_mut(&mut self, slot: EquipSlot) -> &mut Option<ItemInstance> {
        match slot {
            EquipSlot::Bag => &mut self.bag,
            EquipSlot::Helmet => &mut self.helmet,
            EquipSlot::Cape => &mut self.cape,
            EquipSlot::Weapon => &mut self.weapon,
            EquipSlot::Armor => &mut self.armor,
            EquipSlot::Offhand => &mut self.offhand,
            EquipSlot::Potion => &mut self.potion,
            EquipSlot::Shoes => &mut self.shoes,
            EquipSlot::Food => &mut self.food,
            EquipSlot::Mount => &mut self.mount,
        }
    }

    /// Finds which slot (if any) currently holds the physical esemplare
    /// `instance_id` — per-instance, not per-type: se il giocatore ha due
    /// Flame Staff, questo trova quello specifico, non "un" Flame Staff.
    pub fn slot_holding(&self, instance_id: ItemInstanceId) -> Option<EquipSlot> {
        EquipSlot::ALL.into_iter().find(|slot| {
            self.get(*slot)
                .as_ref()
                .is_some_and(|item| item.instance_id == instance_id)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::definition::ItemCategory;
    use super::super::registry::ItemId;
    use super::*;

    #[test]
    fn default_inventory_has_empty_slots() {
        let inv = Inventory::default();
        assert_eq!(inv.slots.len(), INVENTORY_CAPACITY);
        for slot in &inv.slots {
            assert!(slot.is_none());
        }
    }

    #[test]
    fn default_equipment_has_no_weapon() {
        let eq = Equipment::default();
        assert!(eq.weapon.is_none());
    }

    #[test]
    fn inventory_roundtrips_through_serde() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(ItemInstance::new(ItemId::new("iron_sword")));
        inv.slots[3] = Some(ItemInstance::new(ItemId::new("potion")));

        let json = serde_json::to_string(&inv).expect("serialize");
        let back: Inventory = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(inv, back);
    }

    #[test]
    fn equip_slot_default_is_weapon() {
        assert_eq!(EquipSlot::default(), EquipSlot::Weapon);
    }

    #[test]
    fn get_and_get_mut_round_trip_for_every_slot() {
        for slot in EquipSlot::ALL {
            let mut eq = Equipment::default();
            assert!(eq.get(slot).is_none());
            let instance = ItemInstance::new(ItemId::new("test_item"));
            *eq.get_mut(slot) = Some(instance.clone());
            assert_eq!(eq.get(slot), &Some(instance));
        }
    }

    #[test]
    fn slot_holding_finds_the_right_physical_instance() {
        // The ids are set explicitly: `ItemInstance::new` leaves them
        // unassigned now that the database issues them, so two freshly minted
        // copies would be indistinguishable here.
        let mut eq = Equipment::default();
        let mut helmet = ItemInstance::new(ItemId::new("leather_helmet"));
        helmet.instance_id = ItemInstanceId(1);
        let mut other_helmet = ItemInstance::new(ItemId::new("leather_helmet"));
        other_helmet.instance_id = ItemInstanceId(2);

        eq.helmet = Some(helmet.clone());
        assert_eq!(eq.slot_holding(helmet.instance_id), Some(EquipSlot::Helmet));
        // Stesso tipo, esemplare diverso: non deve essere trovato.
        assert_eq!(eq.slot_holding(other_helmet.instance_id), None);
    }

    #[test]
    fn equipment_roundtrips_through_serde() {
        let eq = Equipment {
            weapon: Some(ItemInstance::new(ItemId::new("iron_sword"))),
            mount: Some(ItemInstance::new(ItemId::new("swift_steed"))),
            ..Default::default()
        };

        let json = serde_json::to_string(&eq).expect("serialize");
        let back: Equipment = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(eq, back);
    }

    #[test]
    fn materials_stack_in_the_bag_not_on_the_item() {
        assert!(Inventory::stacks_category(ItemCategory::Material));
        assert!(!Inventory::stacks_category(ItemCategory::Weapon));
        assert_eq!(Inventory::MAX_STACK, 200);
    }

    #[test]
    fn add_stackable_fills_existing_then_opens_a_new_slot() {
        let mut inv = Inventory::default();
        let mut next_id = 1u64;
        let mut mint = || {
            let id = ItemInstanceId(next_id);
            next_id += 1;
            id
        };
        let wood = ItemId::new("wood");
        assert_eq!(inv.add_stackable(wood.clone(), 3, &mut mint), 3);
        assert_eq!(inv.slots[0].as_ref().map(|i| i.quantity), Some(3));
        assert_eq!(inv.add_stackable(wood.clone(), 2, &mut mint), 2);
        assert_eq!(inv.slots[0].as_ref().map(|i| i.quantity), Some(5));
        assert!(inv.slots[1].is_none());
    }

    #[test]
    fn add_stackable_stops_when_the_bag_is_full() {
        let mut inv = Inventory::default();
        let wood = ItemId::new("wood");
        let mut next_id = 1u64;
        let mut mint = || {
            let id = ItemInstanceId(next_id);
            next_id += 1;
            id
        };
        for slot in &mut inv.slots {
            let mut instance = ItemInstance::new(ItemId::new("sword"));
            instance.instance_id = mint();
            *slot = Some(instance);
        }
        assert_eq!(inv.space_for(&wood, true), 0);
        assert_eq!(inv.add_stackable(wood, 1, &mut mint), 0);
    }

    fn wood_stack(id: u64, quantity: u32) -> ItemInstance {
        let mut instance = ItemInstance::new(ItemId::new("wood"));
        instance.instance_id = ItemInstanceId(id);
        instance.quantity = quantity;
        instance
    }

    fn sword(id: u64) -> ItemInstance {
        let mut instance = ItemInstance::new(ItemId::new("sword"));
        instance.instance_id = ItemInstanceId(id);
        instance
    }

    #[test]
    fn insert_instance_moves_a_unique_item_with_its_id() {
        let mut inv = Inventory::default();
        assert_eq!(inv.insert_instance(sword(42), false).unwrap(), None);
        assert_eq!(
            inv.slots[0].as_ref().map(|item| item.instance_id),
            Some(ItemInstanceId(42))
        );
    }

    #[test]
    fn insert_instance_refuses_a_unique_item_when_full() {
        let mut inv = Inventory::default();
        for (slot, id) in inv.slots.iter_mut().zip(1u64..) {
            *slot = Some(sword(id));
        }
        assert_eq!(
            inv.insert_instance(sword(99), false),
            Err(StackOpError::InventoryFull)
        );
    }

    #[test]
    fn insert_instance_merges_wood_into_an_existing_pile() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(wood_stack(1, 10));
        assert_eq!(inv.insert_instance(wood_stack(7, 4), true).unwrap(), None);
        assert_eq!(inv.slots[0].as_ref().map(|item| item.quantity), Some(14));
        assert!(inv.slots[1].is_none());
    }

    #[test]
    fn insert_instance_returns_leftover_when_only_part_fits() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(wood_stack(1, 190));
        for slot in inv.slots.iter_mut().skip(1) {
            *slot = Some(sword(2));
        }
        let leftover = inv.insert_instance(wood_stack(7, 20), true).unwrap();
        assert_eq!(leftover.as_ref().map(|item| item.quantity), Some(10));
        assert_eq!(inv.slots[0].as_ref().map(|item| item.quantity), Some(200));
    }

    #[test]
    fn default_split_amount_is_half_leaving_at_least_one() {
        assert_eq!(Inventory::default_split_amount(50), 25);
        assert_eq!(Inventory::default_split_amount(7), 3);
        assert_eq!(Inventory::default_split_amount(2), 1);
        assert_eq!(Inventory::default_split_amount(1), 0);
        assert_eq!(Inventory::clamp_split_amount(7, 50), 7);
        assert_eq!(Inventory::clamp_split_amount(0, 50), 1);
        assert_eq!(Inventory::clamp_split_amount(50, 50), 49);
    }

    #[test]
    fn split_stack_peels_seven_off_fifty_wood() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(wood_stack(1, 50));
        let dest = inv
            .split_stack(0, 7, true, || ItemInstanceId(2))
            .expect("split");
        assert_eq!(dest, 1);
        assert_eq!(inv.slots[0].as_ref().map(|item| item.quantity), Some(43));
        assert_eq!(
            inv.slots[0].as_ref().map(|item| item.instance_id),
            Some(ItemInstanceId(1))
        );
        assert_eq!(inv.slots[1].as_ref().map(|item| item.quantity), Some(7));
        assert_eq!(
            inv.slots[1].as_ref().map(|item| item.instance_id),
            Some(ItemInstanceId(2))
        );
        assert_eq!(
            inv.slots[1].as_ref().map(|item| item.item_id.as_str()),
            Some("wood")
        );
    }

    #[test]
    fn split_stack_rejects_invalid_amounts_and_leaves_the_bag() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(wood_stack(1, 50));
        let snapshot = inv.clone();
        assert_eq!(
            inv.split_stack(0, 0, true, || ItemInstanceId(2)),
            Err(StackOpError::AmountZero)
        );
        assert_eq!(
            inv.split_stack(0, 50, true, || ItemInstanceId(2)),
            Err(StackOpError::AmountNotLessThanQuantity)
        );
        assert_eq!(
            inv.split_stack(0, 7, false, || ItemInstanceId(2)),
            Err(StackOpError::NotStackable)
        );
        assert_eq!(
            inv.split_stack(99, 7, true, || ItemInstanceId(2)),
            Err(StackOpError::SlotOutOfRange)
        );
        assert_eq!(
            inv.split_stack(1, 7, true, || ItemInstanceId(2)),
            Err(StackOpError::SlotEmpty)
        );
        assert_eq!(inv, snapshot);
    }

    #[test]
    fn split_stack_fails_when_the_bag_is_full() {
        let mut inv = Inventory::default();
        for (index, slot) in inv.slots.iter_mut().enumerate() {
            *slot = Some(wood_stack(index as u64 + 1, 2));
        }
        assert_eq!(
            inv.split_stack(0, 1, true, || ItemInstanceId(99)),
            Err(StackOpError::InventoryFull)
        );
        assert_eq!(inv.slots[0].as_ref().map(|item| item.quantity), Some(2));
    }

    #[test]
    fn split_stack_rejects_inscribed_items() {
        let mut inv = Inventory::default();
        let mut item = wood_stack(1, 10);
        item.root_inscription = Some(crate::abilities::inscription::WeaponInscription::default());
        inv.slots[0] = Some(item);
        assert_eq!(
            inv.split_stack(0, 3, true, || ItemInstanceId(2)),
            Err(StackOpError::NotStackable)
        );
    }

    #[test]
    fn move_or_merge_combines_same_material_piles() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(wood_stack(1, 20));
        inv.slots[1] = Some(wood_stack(2, 30));
        inv.move_or_merge(0, 1, true).expect("merge");
        assert!(inv.slots[0].is_none());
        assert_eq!(inv.slots[1].as_ref().map(|item| item.quantity), Some(50));
        assert_eq!(
            inv.slots[1].as_ref().map(|item| item.instance_id),
            Some(ItemInstanceId(2))
        );
    }

    #[test]
    fn move_or_merge_leaves_remainder_when_dest_hits_the_cap() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(wood_stack(1, 20));
        inv.slots[1] = Some(wood_stack(2, 190));
        inv.move_or_merge(0, 1, true).expect("partial merge");
        assert_eq!(inv.slots[0].as_ref().map(|item| item.quantity), Some(10));
        assert_eq!(inv.slots[1].as_ref().map(|item| item.quantity), Some(200));
    }

    #[test]
    fn move_or_merge_swaps_when_dest_is_already_full() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(wood_stack(1, 50));
        inv.slots[1] = Some(wood_stack(2, Inventory::MAX_STACK));
        inv.move_or_merge(0, 1, true).expect("swap full piles");
        assert_eq!(inv.slots[0].as_ref().map(|item| item.quantity), Some(200));
        assert_eq!(inv.slots[1].as_ref().map(|item| item.quantity), Some(50));
    }

    #[test]
    fn move_or_merge_swaps_unique_items_and_different_ids() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(sword(1));
        inv.slots[1] = Some(sword(2));
        inv.move_or_merge(0, 1, false).expect("swap swords");
        assert_eq!(
            inv.slots[0].as_ref().map(|item| item.instance_id),
            Some(ItemInstanceId(2))
        );
        assert_eq!(
            inv.slots[1].as_ref().map(|item| item.instance_id),
            Some(ItemInstanceId(1))
        );

        let mut piles = Inventory::default();
        piles.slots[0] = Some(wood_stack(1, 5));
        let mut copper = wood_stack(2, 8);
        copper.item_id = ItemId::new("copper");
        piles.slots[1] = Some(copper);
        piles
            .move_or_merge(0, 1, true)
            .expect("different materials swap");
        assert_eq!(
            piles.slots[0].as_ref().map(|item| item.item_id.as_str()),
            Some("copper")
        );
        assert_eq!(
            piles.slots[1].as_ref().map(|item| item.item_id.as_str()),
            Some("wood")
        );
    }

    #[test]
    fn move_or_merge_onto_an_empty_slot_is_a_move() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(wood_stack(1, 12));
        inv.move_or_merge(0, 3, true).expect("move");
        assert!(inv.slots[0].is_none());
        assert_eq!(inv.slots[3].as_ref().map(|item| item.quantity), Some(12));
    }

    #[test]
    fn combine_into_vacuums_other_piles_up_to_the_cap() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(wood_stack(1, 10));
        inv.slots[2] = Some(wood_stack(2, 8));
        inv.slots[4] = Some(wood_stack(3, 5));
        assert_eq!(inv.combine_into(0, true).expect("combine"), 13);
        assert_eq!(inv.slots[0].as_ref().map(|item| item.quantity), Some(23));
        assert!(inv.slots[2].is_none());
        assert!(inv.slots[4].is_none());

        let mut capped = Inventory::default();
        capped.slots[0] = Some(wood_stack(1, 190));
        capped.slots[1] = Some(wood_stack(2, 20));
        assert_eq!(capped.combine_into(0, true).expect("partial"), 10);
        assert_eq!(
            capped.slots[0].as_ref().map(|item| item.quantity),
            Some(200)
        );
        assert_eq!(capped.slots[1].as_ref().map(|item| item.quantity), Some(10));
    }

    #[test]
    fn combine_into_does_nothing_without_other_stacks() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(wood_stack(1, 10));
        inv.slots[1] = Some(sword(2));
        assert!(!inv.has_other_mergeable_stack(0, true));
        assert_eq!(inv.combine_into(0, true).expect("no-op"), 0);
        assert_eq!(inv.slots[0].as_ref().map(|item| item.quantity), Some(10));
        assert!(inv.slots[1].is_some());
        assert_eq!(inv.combine_into(0, false), Err(StackOpError::NotStackable));
    }

    #[test]
    fn take_amount_peels_part_of_a_wood_pile() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(wood_stack(1, 50));
        let listed = inv
            .take_amount(0, 10, true, || ItemInstanceId(9))
            .expect("peel");
        assert_eq!(listed.instance_id, ItemInstanceId(9));
        assert_eq!(listed.quantity, 10);
        assert_eq!(
            inv.slots[0].as_ref().map(|item| item.instance_id),
            Some(ItemInstanceId(1))
        );
        assert_eq!(inv.slots[0].as_ref().map(|item| item.quantity), Some(40));
        assert!(inv.slots.iter().filter(|slot| slot.is_some()).count() == 1);
    }

    #[test]
    fn take_amount_of_the_whole_pile_keeps_the_original_id() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(wood_stack(1, 50));
        let listed = inv
            .take_amount(0, 50, true, || panic!("whole take must not mint"))
            .expect("take all");
        assert_eq!(listed.instance_id, ItemInstanceId(1));
        assert_eq!(listed.quantity, 50);
        assert!(inv.slots[0].is_none());
    }

    #[test]
    fn take_amount_refuses_zero_overdraw_and_unique_partial() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(wood_stack(1, 7));
        inv.slots[1] = Some(sword(2));
        assert_eq!(
            inv.take_amount(0, 0, true, || ItemInstanceId(3))
                .unwrap_err(),
            StackOpError::AmountZero
        );
        assert_eq!(
            inv.take_amount(0, 8, true, || ItemInstanceId(3))
                .unwrap_err(),
            StackOpError::AmountExceedsQuantity
        );
        assert_eq!(
            inv.take_amount(1, 1, false, || panic!("unique take-all"))
                .expect("sword")
                .item_id
                .as_str(),
            "sword"
        );
        inv.slots[1] = Some(sword(4));
        assert_eq!(
            inv.take_amount(1, 1, false, || panic!("unique"))
                .unwrap()
                .quantity,
            1
        );
        inv.slots[2] = Some(sword(5));
        assert_eq!(
            inv.take_amount(2, 2, false, || ItemInstanceId(6))
                .unwrap_err(),
            StackOpError::AmountExceedsQuantity
        );
    }

    #[test]
    fn clamp_trade_amount_covers_the_whole_pile() {
        assert_eq!(Inventory::default_trade_amount(50), 50);
        assert_eq!(Inventory::clamp_trade_amount(0, 50), 1);
        assert_eq!(Inventory::clamp_trade_amount(7, 50), 7);
        assert_eq!(Inventory::clamp_trade_amount(50, 50), 50);
        assert_eq!(Inventory::clamp_trade_amount(99, 50), 50);
        assert_eq!(Inventory::clamp_trade_amount(1, 1), 1);
        assert_eq!(Inventory::clamp_trade_amount(3, 0), 0);
    }

    #[test]
    fn count_item_sums_matching_piles() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(wood_stack(1, 5));
        inv.slots[2] = Some(wood_stack(2, 7));
        inv.slots[3] = Some(sword(3));
        assert_eq!(inv.count_item(&ItemId::new("wood")), 12);
        assert_eq!(inv.count_item(&ItemId::new("sword")), 1);
        assert_eq!(inv.count_item(&ItemId::new("copper")), 0);
    }

    #[test]
    fn remove_item_amount_drains_first_piles_then_clears() {
        let mut inv = Inventory::default();
        inv.slots[0] = Some(wood_stack(1, 5));
        inv.slots[1] = Some(wood_stack(2, 8));
        inv.remove_item_amount(&ItemId::new("wood"), 7)
            .expect("drain");
        assert_eq!(inv.slots[0], None);
        assert_eq!(inv.slots[1].as_ref().map(|item| item.quantity), Some(6));
        inv.remove_item_amount(&ItemId::new("wood"), 6)
            .expect("clear last pile");
        assert!(inv.slots[1].is_none());
        assert_eq!(
            inv.remove_item_amount(&ItemId::new("wood"), 1).unwrap_err(),
            StackOpError::AmountExceedsQuantity
        );
        assert_eq!(
            inv.remove_item_amount(&ItemId::new("wood"), 0).unwrap_err(),
            StackOpError::AmountZero
        );
    }
}
