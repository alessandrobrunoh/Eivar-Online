//! Typed wrappers around the module's reducers.
//!
//! The UI used to send lightyear messages: `sender.send::<Channel2>(EquipItemCommand
//! { slot_index })`. It now calls a reducer, and this module is the seam — so
//! that `bevymmo_presentation` never imports the generated bindings, and so the
//! reducers' stringly-typed parameters are built in exactly one place.
//!
//! The module parses slot names case-insensitively from the domain's own
//! labels; passing the enum here and converting once means a rename in
//! `EquipSlot` cannot silently start sending an unknown slot name.
//!
//! # Two different failures
//!
//! Every call here goes through the generated `*_then` form, which takes a
//! callback and runs it when the reducer's own result comes back. That splits
//! the two things that can go wrong:
//!
//! - the returned `Result` is *transport*: the request could not be handed to
//!   the SDK at all. The caller sees it immediately and logs it.
//! - the callback carries the module's `Result<(), String>` — "inventory is
//!   full", "target is out of range", "that name is taken". It arrives later, on
//!   the SDK's thread, so it is pushed onto the same channel row changes use and
//!   surfaces as a [`crate::server_feed::ServerNotice`].
//!
//! The plain fire-and-forget forms reported only the first kind, which is why
//! every carefully worded refusal in the module used to vanish.

use bevy::prelude::Vec3;
use bevymmo_domain::abilities::AbilitySlot;
use bevymmo_domain::items::EquipSlot;

use super::module_bindings::armor_cast_reducer::armor_cast as armor_cast_reducer;
use super::module_bindings::cancel_buy_order_reducer::cancel_buy_order as cancel_buy_order_reducer;
use super::module_bindings::cancel_sell_order_reducer::cancel_sell_order as cancel_sell_order_reducer;
use super::module_bindings::cast_weapon_reducer::cast_weapon as cast_weapon_reducer;
use super::module_bindings::claim_npc_item_reducer::claim_npc_item as claim_npc_item_reducer;
use super::module_bindings::combine_item_reducer::combine_item as combine_item_reducer;
use super::module_bindings::destroy_item_reducer::destroy_item as destroy_item_reducer;
use super::module_bindings::equip_item_reducer::equip_item as equip_item_reducer;
use super::module_bindings::market_buy_reducer::market_buy as market_buy_reducer;
use super::module_bindings::market_sell_reducer::market_sell as market_sell_reducer;
use super::module_bindings::move_item_reducer::move_item as move_item_reducer;
use super::module_bindings::party_accept_reducer::party_accept as party_accept_reducer;
use super::module_bindings::party_decline_reducer::party_decline as party_decline_reducer;
use super::module_bindings::party_invite_reducer::party_invite as party_invite_reducer;
use super::module_bindings::party_join_reducer::party_join as party_join_reducer;
use super::module_bindings::party_leave_reducer::party_leave as party_leave_reducer;
use super::module_bindings::place_buy_order_reducer::place_buy_order as place_buy_order_reducer;
use super::module_bindings::place_sell_order_reducer::place_sell_order as place_sell_order_reducer;
use super::module_bindings::release_cast_reducer::release_cast as release_cast_reducer;
use super::module_bindings::respawn_reducer::respawn as respawn_reducer;
use super::module_bindings::send_chat_message_reducer::send_chat_message as send_chat_message_reducer;
use super::module_bindings::set_ability_selection_reducer::set_ability_selection as set_ability_selection_reducer;
use super::module_bindings::set_armor_inscription_reducer::set_armor_inscription as set_armor_inscription_reducer;

use super::module_bindings::set_root_inscription_reducer::set_root_inscription as set_root_inscription_reducer;
use super::module_bindings::split_item_reducer::split_item as split_item_reducer;
use super::module_bindings::start_craft_reducer::start_craft as start_craft_reducer;
use super::module_bindings::start_gather_reducer::start_gather as start_gather_reducer;
use super::module_bindings::stop_craft_reducer::stop_craft as stop_craft_reducer;
use super::module_bindings::stop_gather_reducer::stop_gather as stop_gather_reducer;
use super::module_bindings::loot_take_all_reducer::loot_take_all as loot_take_all_reducer;
use super::module_bindings::loot_take_gold_reducer::loot_take_gold as loot_take_gold_reducer;
use super::module_bindings::loot_take_reducer::loot_take as loot_take_reducer;
use super::module_bindings::unequip_item_reducer::unequip_item as unequip_item_reducer;
use super::module_bindings::Vec3Row;
use super::plugin::StdbConnection;

/// Whether the *request* reached the SDK. The server's own answer arrives
/// later, through the rejection callback.
type Sent = Result<(), spacetimedb_sdk::Error>;

fn to_row(v: Vec3) -> Vec3Row {
    Vec3Row {
        x: v.x,
        y: v.y,
        z: v.z,
    }
}

/// Claims a catalogue item from a nearby NPC vendor.
pub fn claim_npc_item(conn: &StdbConnection, npc_entity_id: u64, item_id: String) -> Sent {
    conn.reducers().claim_npc_item_then(
        npc_entity_id,
        item_id,
        conn.report_rejection("could not claim that item"),
    )
}

/// Lists `quantity` of an inventory pile on the NPC's isolated market.
///
/// `price` is gold per unit. The module stores the total (`price * quantity`).
pub fn place_sell_order(
    conn: &StdbConnection,
    npc_entity_id: u64,
    instance_id: u64,
    price: u64,
    quantity: u32,
) -> Sent {
    conn.reducers().place_sell_order_then(
        npc_entity_id,
        instance_id,
        price,
        quantity,
        conn.report_rejection("could not list that item"),
    )
}

/// Buys a sell order from the NPC's isolated market.
pub fn market_buy(conn: &StdbConnection, npc_entity_id: u64, sell_order_id: u64) -> Sent {
    conn.reducers().market_buy_then(
        npc_entity_id,
        sell_order_id,
        conn.report_rejection("could not buy that listing"),
    )
}

/// Cancels one of the caller's sell listings.
pub fn cancel_sell_order(conn: &StdbConnection, order_id: u64) -> Sent {
    conn.reducers().cancel_sell_order_then(
        order_id,
        conn.report_rejection("could not cancel that listing"),
    )
}

/// Places a Gold bid for a catalogue item in the NPC's market.
pub fn place_buy_order(
    conn: &StdbConnection,
    npc_entity_id: u64,
    item_id: String,
    price: u64,
) -> Sent {
    conn.reducers().place_buy_order_then(
        npc_entity_id,
        item_id,
        price,
        conn.report_rejection("could not place that bid"),
    )
}

/// Instant-sells `quantity` of an inventory pile into the best matching bid.
///
/// `min_price` is gold per unit.
pub fn market_sell(
    conn: &StdbConnection,
    npc_entity_id: u64,
    instance_id: u64,
    min_price: u64,
    quantity: u32,
) -> Sent {
    conn.reducers().market_sell_then(
        npc_entity_id,
        instance_id,
        min_price,
        quantity,
        conn.report_rejection("could not sell that item"),
    )
}

/// Cancels one of the caller's bids and refunds escrowed Gold.
pub fn cancel_buy_order(conn: &StdbConnection, order_id: u64) -> Sent {
    conn.reducers()
        .cancel_buy_order_then(order_id, conn.report_rejection("could not cancel that bid"))
}

/// Permanently destroys an item instance from the inventory.
pub fn destroy_item(conn: &StdbConnection, instance_id: u64) -> Sent {
    conn.reducers().destroy_item_then(
        instance_id,
        conn.report_rejection("could not destroy that item"),
    )
}

/// Moves an inventory item into the equipment slot its definition allows.
pub fn equip_item(conn: &StdbConnection, slot_index: u8) -> Sent {
    conn.reducers()
        .equip_item_then(slot_index, conn.report_rejection("could not equip"))
}

/// Takes an equipped item off and puts it in the first free inventory slot.
pub fn unequip_item(conn: &StdbConnection, slot: EquipSlot) -> Sent {
    conn.reducers().unequip_item_then(
        slot.label().to_string(),
        conn.report_rejection("could not unequip"),
    )
}

/// Swaps two inventory slots, or merges same-item Material piles.
pub fn move_item(conn: &StdbConnection, from: u8, to: u8) -> Sent {
    conn.reducers()
        .move_item_then(from, to, conn.report_rejection("could not move that item"))
}

/// Peels `amount` off inventory slot `slot_index` into the first empty slot.
pub fn split_item(conn: &StdbConnection, slot_index: u8, amount: u32) -> Sent {
    conn.reducers().split_item_then(
        slot_index,
        amount,
        conn.report_rejection("could not split that stack"),
    )
}

/// Pulls other piles of the same Material into `slot_index` up to the bag cap.
pub fn combine_item(conn: &StdbConnection, slot_index: u8) -> Sent {
    conn.reducers().combine_item_then(
        slot_index,
        conn.report_rejection("could not combine those stacks"),
    )
}

/// Writes the equipped weapon's shared Root Word and per-slot Ancient Words.
pub fn set_root_inscription(
    conn: &StdbConnection,
    root_word: Option<String>,
    primary_words: Vec<String>,
    secondary_words: Vec<String>,
    ultimate_words: Vec<String>,
) -> Sent {
    conn.reducers().set_root_inscription_then(
        root_word,
        primary_words,
        secondary_words,
        ultimate_words,
        conn.report_rejection("could not write that Root Word inscription"),
    )
}

/// Writes an armor item's independent Root Word inscription.
pub fn set_armor_inscription(
    conn: &StdbConnection,
    slot: EquipSlot,
    root_word: Option<String>,
    secondary_words: Vec<String>,
) -> Sent {
    conn.reducers().set_armor_inscription_then(
        slot.label().to_string(),
        root_word,
        secondary_words,
        conn.report_rejection("could not write that armor inscription"),
    )
}

/// Chooses which of the weapon's offered abilities is active in a slot.
pub fn set_ability_selection(conn: &StdbConnection, slot: AbilitySlot, ability_id: String) -> Sent {
    conn.reducers().set_ability_selection_then(
        ability_label(slot).to_string(),
        ability_id,
        conn.report_rejection("could not choose that ability"),
    )
}

/// Takes one slot from a loot bag.
pub fn loot_take(conn: &StdbConnection, bag_id: u64, slot_index: u8) -> Sent {
    conn.reducers().loot_take_then(
        bag_id,
        slot_index,
        conn.report_rejection("could not take that item"),
    )
}

/// Takes the gold sitting in a loot bag.
pub fn loot_take_gold(conn: &StdbConnection, bag_id: u64) -> Sent {
    conn.reducers()
        .loot_take_gold_then(bag_id, conn.report_rejection("could not take that gold"))
}

/// Takes gold, then as many items as fit.
pub fn loot_take_all(conn: &StdbConnection, bag_id: u64) -> Sent {
    conn.reducers()
        .loot_take_all_then(bag_id, conn.report_rejection("could not loot that bag"))
}

/// Starts gathering the targeted resource node.
pub fn start_gather(conn: &StdbConnection, node_entity_id: u64) -> Sent {
    conn.reducers()
        .start_gather_then(node_entity_id, conn.report_rejection("could not gather"))
}

/// Stops the local gather channel, if any.
pub fn stop_gather(conn: &StdbConnection) -> Sent {
    conn.reducers()
        .stop_gather_then(conn.report_rejection("could not stop gathering"))
}

/// Starts crafting `quantity` of `item_id` at a nearby crafter NPC.
pub fn start_craft(
    conn: &StdbConnection,
    npc_entity_id: u64,
    item_id: String,
    quantity: u32,
) -> Sent {
    conn.reducers().start_craft_then(
        npc_entity_id,
        item_id,
        quantity,
        conn.report_rejection("could not craft"),
    )
}

/// Stops the local craft channel, if any.
pub fn stop_craft(conn: &StdbConnection) -> Sent {
    conn.reducers()
        .stop_craft_then(conn.report_rejection("could not stop crafting"))
}

/// Ends a channelled or charged cast. Naming the spell stops a stale release
/// from cancelling a cast that started after it.
///
/// Charge uses `target_position` / `target_entity` as the aim at release;
/// Channeling ignores them.
pub fn release_cast(
    conn: &StdbConnection,
    spell_id: String,
    target_entity: Option<u64>,
    target_position: Option<Vec3>,
) -> Sent {
    conn.reducers().release_cast_then(
        spell_id,
        target_entity,
        target_position.map(to_row),
        conn.report_rejection("could not end that cast"),
    )
}

/// Casts the weapon ability bound to an ability slot.
pub fn cast_weapon(
    conn: &StdbConnection,
    slot: AbilitySlot,
    target_entity: Option<u64>,
    target_position: Option<Vec3>,
) -> Sent {
    conn.reducers().cast_weapon_then(
        ability_label(slot).to_string(),
        target_entity,
        target_position.map(to_row),
        conn.report_rejection("could not cast that weapon ability"),
    )
}

/// Casts the first active ability supplied by an equipped armor item.
///
/// The server resolves the Armor inscription and cast mode; the client only
/// identifies the equipment slot and target.
pub fn armor_cast(
    conn: &StdbConnection,
    slot: EquipSlot,
    ability_slot: AbilitySlot,
    target_entity: Option<u64>,
    target_position: Option<Vec3>,
) -> Sent {
    conn.reducers().armor_cast_then(
        slot.label().to_ascii_lowercase(),
        ability_label(ability_slot).to_string(),
        target_entity,
        target_position.map(to_row),
        conn.report_rejection("could not cast that armor ability"),
    )
}

/// Sends a message to the global server chat.
pub fn send_chat_message(conn: &StdbConnection, text: String) -> Sent {
    conn.reducers()
        .send_chat_message_then(text, conn.report_rejection("could not send chat message"))
}

/// `/party invite <name>` — invites `target_name`, implicitly creating a
/// party with the sender as leader if they are not already in one.
pub fn party_invite(conn: &StdbConnection, target_name: String) -> Sent {
    conn.reducers()
        .party_invite_then(target_name, conn.report_rejection("could not invite"))
}

/// `/party join <name>` — asks to join `leader_name`'s party.
pub fn party_join(conn: &StdbConnection, leader_name: String) -> Sent {
    conn.reducers()
        .party_join_then(leader_name, conn.report_rejection("could not ask to join"))
}

/// `/party accept <name>` — accepts the pending request between the caller
/// and `name`, whichever direction it runs.
pub fn party_accept(conn: &StdbConnection, name: String) -> Sent {
    conn.reducers()
        .party_accept_then(name, conn.report_rejection("could not accept"))
}

/// `/party decline <name>` — declines the pending request between the caller
/// and `name`, whichever direction it runs.
pub fn party_decline(conn: &StdbConnection, name: String) -> Sent {
    conn.reducers()
        .party_decline_then(name, conn.report_rejection("could not decline"))
}

/// `/party leave` — leaves the caller's current party.
pub fn party_leave(conn: &StdbConnection) -> Sent {
    conn.reducers()
        .party_leave_then(conn.report_rejection("could not leave the party"))
}

/// Brings a dead character back at its spawn point.
pub fn respawn(conn: &StdbConnection) -> Sent {
    conn.reducers()
        .respawn_then(conn.report_rejection("could not respawn"))
}

fn ability_label(slot: AbilitySlot) -> &'static str {
    match slot {
        AbilitySlot::Primary => "primary",
        AbilitySlot::Secondary => "secondary",
        AbilitySlot::Ultimate => "ultimate",
    }
}
