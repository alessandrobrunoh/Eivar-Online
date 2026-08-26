//! Caller-filtered views over the tables that hold one owner's data.
//!
//! # Why these exist
//!
//! SpacetimeDB 2.8.1 does not enforce row-level security — upstream marks
//! `client_visibility_filter` unimplemented — so table visibility is all or
//! nothing. A `public` table is readable in full by any connected client,
//! whatever query the *official* client happens to send.
//!
//! Everything here used to be `public`. The desktop client already subscribed
//! with `WHERE character_id = ...`, which kept the wire small but was never an
//! access control: a modified client could subscribe without the clause and
//! read every player's inventory, equipment, gold and stats. The tables are now
//! private and reached through these views, which are computed per caller and
//! cannot be widened from the client side.
//!
//! `reducers::api_keys::my_api_keys` was already doing this for API keys; this
//! module is the same pattern applied to the rest.
//!
//! # Shape
//!
//! Views are **account**-scoped rather than character-scoped, so one
//! subscription covers every character on the account. The desktop client wants
//! its active character and filters locally; the gateway serves
//! `/v1/characters/:id/*` for any character the account owns, and its ownership
//! check (`owned_character`) stays where it is. Neither can see another
//! account's rows at all.
//!
//! Views may only do indexed lookups, never scans, which is why every path here
//! goes `identity -> session -> account_id -> player.account_id -> character_id`
//! and then a primary-key `find` per character. An account holds at most
//! [`crate::MAX_CHARACTERS_PER_ACCOUNT`] characters, so that is a handful of
//! point lookups.

use spacetimedb::{view, Uuid, ViewContext};

use crate::tables::{
    account_economy__view, character_wallet__view, equipment__view, hotbar__view, inventory__view,
    known_ancient_language__view, party_request__view, player__view, player_stats__view,
    resonance__view, session__view, AccountEconomy, CharacterWallet, EquipmentTable, Hotbar,
    InventoryTable, KnownAncientLanguageTable, PartyRequestRow, PlayerStats, Resonance, Session,
};

/// The account behind this connection, or `None` when it has not authenticated.
fn caller_account(ctx: &ViewContext) -> Option<u64> {
    ctx.db
        .session()
        .identity()
        .find(ctx.sender())
        .map(|row| row.account_id)
}

/// Every character on the caller's account. Empty before login.
fn caller_characters(ctx: &ViewContext) -> Vec<Uuid> {
    let Some(account_id) = caller_account(ctx) else {
        return Vec::new();
    };
    ctx.db
        .player()
        .account_id()
        .filter(&account_id)
        .map(|row| row.character_id)
        .collect()
}

/// This connection's own `Session` row.
///
/// The table was public so a client could learn its own `account_id` — the one
/// thing it cannot derive from `player` before it has a character. That reason
/// was sound; making the whole table readable was not. Alongside the account
/// id, it also handed out the SpacetimeDB `Identity` of every connected player,
/// which is a stable handle that links all of an account's characters together
/// and which `player` deliberately does not expose.
#[view(accessor = my_session, public, primary_key = identity)]
fn my_session(ctx: &ViewContext) -> Vec<Session> {
    ctx.db
        .session()
        .identity()
        .find(ctx.sender())
        .into_iter()
        .collect()
}

#[view(accessor = my_inventory, public, primary_key = character_id)]
fn my_inventory(ctx: &ViewContext) -> Vec<InventoryTable> {
    caller_characters(ctx)
        .into_iter()
        .filter_map(|character_id| ctx.db.inventory().character_id().find(character_id))
        .collect()
}

#[view(accessor = my_equipment, public, primary_key = character_id)]
fn my_equipment(ctx: &ViewContext) -> Vec<EquipmentTable> {
    caller_characters(ctx)
        .into_iter()
        .filter_map(|character_id| ctx.db.equipment().character_id().find(character_id))
        .collect()
}

#[view(accessor = my_hotbar, public, primary_key = character_id)]
fn my_hotbar(ctx: &ViewContext) -> Vec<Hotbar> {
    caller_characters(ctx)
        .into_iter()
        .filter_map(|character_id| ctx.db.hotbar().character_id().find(character_id))
        .collect()
}

#[view(accessor = my_ancient_language, public, primary_key = character_id)]
fn my_ancient_language(ctx: &ViewContext) -> Vec<KnownAncientLanguageTable> {
    caller_characters(ctx)
        .into_iter()
        .filter_map(|character_id| {
            ctx.db
                .known_ancient_language()
                .character_id()
                .find(character_id)
        })
        .collect()
}

#[view(accessor = my_wallet, public, primary_key = character_id)]
fn my_wallet(ctx: &ViewContext) -> Vec<CharacterWallet> {
    caller_characters(ctx)
        .into_iter()
        .filter_map(|character_id| ctx.db.character_wallet().character_id().find(character_id))
        .collect()
}

/// Persisted base stats. Distinct from `entity_stats`, which is live combat
/// state for every entity in the world and stays public — a nameplate needs
/// other players' health.
#[view(accessor = my_player_stats, public, primary_key = character_id)]
fn my_player_stats(ctx: &ViewContext) -> Vec<PlayerStats> {
    caller_characters(ctx)
        .into_iter()
        .filter_map(|character_id| ctx.db.player_stats().character_id().find(character_id))
        .collect()
}

#[view(accessor = my_account_economy, public, primary_key = account_id)]
fn my_account_economy(ctx: &ViewContext) -> Vec<AccountEconomy> {
    caller_account(ctx)
        .and_then(|account_id| ctx.db.account_economy().account_id().find(account_id))
        .into_iter()
        .collect()
}

#[view(accessor = my_resonance, public, primary_key = id)]
fn my_resonance(ctx: &ViewContext) -> Vec<Resonance> {
    caller_characters(ctx)
        .into_iter()
        .flat_map(|character_id| {
            ctx.db
                .resonance()
                .by_character()
                .filter(&character_id)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Party invitations the caller is party to, in either direction.
///
/// Both sides need to see a pending request — the recipient to answer it, the
/// initiator to know it is outstanding — so this unions the two indexes rather
/// than filtering on `recipient` alone.
#[view(accessor = my_party_requests, public, primary_key = request_id)]
fn my_party_requests(ctx: &ViewContext) -> Vec<PartyRequestRow> {
    let mut rows: Vec<PartyRequestRow> = Vec::new();
    for character_id in caller_characters(ctx) {
        rows.extend(ctx.db.party_request().by_recipient().filter(&character_id));
        rows.extend(ctx.db.party_request().by_initiator().filter(&character_id));
    }
    // A request whose initiator and recipient are two characters on the same
    // account matches both indexes. The view's primary key must be unique.
    rows.sort_by_key(|row| row.request_id);
    rows.dedup_by_key(|row| row.request_id);
    rows
}
