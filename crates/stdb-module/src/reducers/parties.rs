//! `/party invite|join|accept|decline|leave`.
//!
//! `/party list` and bare `/party` are deliberately absent: `party_member` is
//! a `public` subscribed table, so the client already has the caller's
//! roster locally and renders it without a round trip. See
//! `plans/party-system.md` Decision 1 for why these are five dedicated
//! reducers rather than entries in a future generic `/`-command dispatcher.
//!
//! # Identity vs character
//!
//! Every reducer here resolves "who is calling" with [`caller_character`],
//! never `ctx.sender()` directly: a party is a relationship between
//! *characters*, and a character survives a reconnect under a different
//! `Identity`. See `tables::PartyMemberRow` for the same reasoning applied
//! to the schema. Notifications are the one place `Identity` still matters —
//! [`notify_character`] resolves a character's *current* connection, if any,
//! through `Session`, and says nothing at all if the character is offline.
//!
//! # Fails closed, mutates last
//!
//! Every reducer below validates everything it is going to do *before*
//! writing a single row. A rejected call must leave `party`/`party_member`/
//! `party_request` exactly as it found them — the security requirement this
//! plan calls out explicitly, and also just good manners: a caller who typos
//! a name should not accidentally end up leading a party of one.

use spacetimedb::{reducer, ReducerContext, Table, Uuid};

use crate::reducers::lifecycle::caller_character;
use crate::tables::{
    party, party_member, party_request, player, player_message, session, PartyMemberRow,
    PartyRequestKind, PartyRequestRow, PartyRow, Player, PlayerMessageEvent,
};

// ---------------------------------------------------------------------------
// Lookups
// ---------------------------------------------------------------------------

/// Resolves a display name to its `Player`, the same way [`crate::reducers::lifecycle::join`]
/// resolves one for a login. Client-sent names are never trusted for
/// authorization — only to find who the reducer's own checks then run
/// against.
fn find_player_by_name(ctx: &ReducerContext, name: &str) -> Result<Player, String> {
    let normalized = crate::normalize_name(name);
    ctx.db
        .player()
        .normalized_name()
        .find(&normalized)
        .ok_or_else(|| format!("no player named {name:?}"))
}

/// The caller's current party membership, if any.
fn member_of(ctx: &ReducerContext, character_id: Uuid) -> Option<PartyMemberRow> {
    ctx.db.party_member().character_id().find(character_id)
}

/// How many characters are currently in `party_id`.
fn party_size(ctx: &ReducerContext, party_id: u64) -> usize {
    ctx.db.party_member().by_party().filter(&party_id).count()
}

/// The pending request (if any) that would let `actor_id` `/party accept` or
/// `/party decline` something from `target_id` — the one where `actor_id` is
/// the `recipient` and `target_id` is the `initiator`. Resolves both
/// [`PartyRequestKind`]s identically, matching Decision 5: accept/decline
/// "resolve against whichever pending request (either direction) exists
/// between the sender and `<name>`".
fn find_actionable_request(
    ctx: &ReducerContext,
    actor_id: Uuid,
    target_id: Uuid,
) -> Option<PartyRequestRow> {
    ctx.db
        .party_request()
        .by_recipient()
        .filter(&actor_id)
        .find(|row| matches_actionable_request(row.recipient, row.initiator, actor_id, target_id))
}

/// The pure half of [`find_actionable_request`]'s match — a row is
/// actionable by `actor_id` against `target_id` exactly when `actor_id` is
/// its recipient and `target_id` is its initiator. Split out so the
/// direction logic itself, not just its plumbing, is unit-testable.
fn matches_actionable_request(
    request_recipient: Uuid,
    request_initiator: Uuid,
    actor_id: Uuid,
    target_id: Uuid,
) -> bool {
    request_recipient == actor_id && request_initiator == target_id
}

/// Whether `recipient_id` already has an unresolved reason to reject a new
/// request from `actor_id`: either `recipient_id`'s own inbox is not empty
/// (Decision 6 — a recipient may have at most one pending request at a
/// time), or a request already connects this exact pair in the *other*
/// direction (an invite and a join request crossing in flight would
/// otherwise leave two live rows between the same two characters, and
/// Decision 5's "whichever pending request" resolution stops being
/// well-defined).
fn assert_no_pending_conflict(
    ctx: &ReducerContext,
    actor_id: Uuid,
    recipient_id: Uuid,
) -> Result<(), String> {
    if ctx
        .db
        .party_request()
        .by_recipient()
        .filter(&recipient_id)
        .next()
        .is_some()
    {
        return Err("that player already has a pending party request".to_string());
    }
    let crossed = ctx.db.party_request().iter().find(|row| {
        (row.initiator == actor_id && row.recipient == recipient_id)
            || (row.initiator == recipient_id && row.recipient == actor_id)
    });
    if crossed.is_some() {
        return Err("there is already a pending request between you and that player".to_string());
    }
    Ok(())
}

/// Finds the character with an active connection, and sends it one line of
/// text through the existing chat/notice path — the same
/// [`PlayerMessageEvent`] `respawn` uses. Silently does nothing for an
/// offline character: there is no connection to address, and that is not an
/// error, the same way an offline recipient elsewhere in the module is not.
///
/// `pub(crate)` rather than private: `sim::combat`'s friendly-fire guard
/// reuses this to tell an attacker why a hit on a party member did nothing,
/// rather than re-deriving "which identity does this character own right
/// now" a second time.
pub(crate) fn notify_character(ctx: &ReducerContext, character_id: Uuid, text: String) {
    let Some(session_row) = ctx
        .db
        .session()
        .iter()
        .find(|row| row.character_id == Some(character_id))
    else {
        return;
    };
    ctx.db.player_message().insert(PlayerMessageEvent {
        target: Some(session_row.identity),
        text,
    });
}

// ---------------------------------------------------------------------------
// Pure boundary logic
// ---------------------------------------------------------------------------

fn is_self_target(actor_id: Uuid, target_id: Uuid) -> bool {
    actor_id == target_id
}

/// Whether a party of this size cannot take another member. `>=`, not `==`:
/// a party can never legitimately exceed the cap, but every mutation path
/// re-checks this immediately before inserting rather than trusting an
/// earlier check, so `>=` is the fail-closed spelling even if `size` were
/// somehow already past it.
fn party_is_full(size: usize) -> bool {
    size >= crate::MAX_PARTY_SIZE
}

/// Which character joins the party when `request` is accepted.
///
/// The two [`PartyRequestKind`]s put the joiner on opposite sides of the
/// request: an `Invite`'s `recipient` (the invitee, `actor_id` here) is who
/// joins; a `JoinRequest`'s `recipient` is the *leader* granting entry, so it
/// is the `initiator` (the original joiner) who joins instead.
fn new_member_on_accept(kind: PartyRequestKind, actor_id: Uuid, initiator_id: Uuid) -> Uuid {
    match kind {
        PartyRequestKind::Invite => actor_id,
        PartyRequestKind::JoinRequest => initiator_id,
    }
}

/// The longest-tenured remaining member — the lowest `joined_at`, expressed
/// as microseconds since the Unix epoch so this stays a pure, DB-free
/// function. Ties (which cannot happen from real gameplay timestamps, but
/// must still resolve to *something* deterministic) go to the lower
/// `character_id`.
fn promote_leader(remaining: &[(Uuid, i64)]) -> Option<Uuid> {
    remaining
        .iter()
        .min_by_key(|(character_id, joined_at)| (*joined_at, *character_id))
        .map(|(character_id, _)| *character_id)
}

// ---------------------------------------------------------------------------
// Reducers
// ---------------------------------------------------------------------------

/// Invites `target_name` to the caller's party, implicitly creating one with
/// the caller as leader if they are not already in one (Decision 3).
#[reducer]
pub fn party_invite(ctx: &ReducerContext, target_name: String) -> Result<(), String> {
    let actor = caller_character(ctx)?;
    let target = find_player_by_name(ctx, &target_name)?;

    if is_self_target(actor.character_id, target.character_id) {
        return Err("you cannot invite yourself".to_string());
    }
    if member_of(ctx, target.character_id).is_some() {
        return Err(format!("{} is already in a party", target.display_name));
    }
    assert_no_pending_conflict(ctx, actor.character_id, target.character_id)?;

    // Validate the caller's own party state without touching anything yet:
    // a rejected invite must not leave a fresh, empty party behind.
    let existing_party = match member_of(ctx, actor.character_id) {
        Some(membership) => {
            let party_row = ctx
                .db
                .party()
                .party_id()
                .find(membership.party_id)
                .ok_or_else(|| "your party no longer exists".to_string())?;
            if party_row.leader != actor.character_id {
                return Err("only the party leader can invite".to_string());
            }
            if party_is_full(party_size(ctx, party_row.party_id)) {
                return Err("your party is full".to_string());
            }
            Some(party_row)
        }
        None => None,
    };

    // Every check passed: mutate now, and only now.
    let party_id = match existing_party {
        Some(party_row) => party_row.party_id,
        None => {
            let party_row = ctx.db.party().insert(PartyRow {
                party_id: 0,
                leader: actor.character_id,
                created_at: ctx.timestamp,
            });
            ctx.db.party_member().insert(PartyMemberRow {
                character_id: actor.character_id,
                party_id: party_row.party_id,
                joined_at: ctx.timestamp,
            });
            party_row.party_id
        }
    };

    ctx.db.party_request().insert(PartyRequestRow {
        request_id: 0,
        party_id,
        kind: PartyRequestKind::Invite,
        initiator: actor.character_id,
        recipient: target.character_id,
        created_at: ctx.timestamp,
    });

    notify_character(
        ctx,
        target.character_id,
        format!(
            "{} invited you to their party. Use /party accept {} to join, or /party decline {} to refuse.",
            actor.display_name, actor.display_name, actor.display_name
        ),
    );
    Ok(())
}

/// Asks to join `leader_name`'s party. `leader_name` must currently lead a
/// party of their own (Decision 5) — `/party join` can only attach to a
/// party that already exists, never create one.
#[reducer]
pub fn party_join(ctx: &ReducerContext, leader_name: String) -> Result<(), String> {
    let actor = caller_character(ctx)?;
    let leader_player = find_player_by_name(ctx, &leader_name)?;

    if is_self_target(actor.character_id, leader_player.character_id) {
        return Err("you cannot join your own party this way".to_string());
    }
    if member_of(ctx, actor.character_id).is_some() {
        return Err("you are already in a party".to_string());
    }
    let party_row = ctx
        .db
        .party()
        .leader()
        .find(leader_player.character_id)
        .ok_or_else(|| format!("{} does not lead a party", leader_player.display_name))?;
    if party_is_full(party_size(ctx, party_row.party_id)) {
        return Err("that party is full".to_string());
    }
    assert_no_pending_conflict(ctx, actor.character_id, leader_player.character_id)?;

    ctx.db.party_request().insert(PartyRequestRow {
        request_id: 0,
        party_id: party_row.party_id,
        kind: PartyRequestKind::JoinRequest,
        initiator: actor.character_id,
        recipient: leader_player.character_id,
        created_at: ctx.timestamp,
    });

    notify_character(
        ctx,
        leader_player.character_id,
        format!(
            "{} asked to join your party. Use /party accept {} to let them in, or /party decline {} to refuse.",
            actor.display_name, actor.display_name, actor.display_name
        ),
    );
    Ok(())
}

/// Accepts the pending request between the caller and `name`, in whichever
/// direction it runs (Decision 5).
#[reducer]
pub fn party_accept(ctx: &ReducerContext, name: String) -> Result<(), String> {
    let actor = caller_character(ctx)?;
    let target = find_player_by_name(ctx, &name)?;
    let request = find_actionable_request(ctx, actor.character_id, target.character_id)
        .ok_or_else(|| format!("no pending party request from {}", target.display_name))?;

    // Re-validated here, not trusted from when the request was created:
    // the party's roster and existence can both have changed since.
    let Some(party_row) = ctx.db.party().party_id().find(request.party_id) else {
        ctx.db
            .party_request()
            .request_id()
            .delete(request.request_id);
        return Err("that party no longer exists".to_string());
    };

    let new_member = new_member_on_accept(request.kind, actor.character_id, request.initiator);
    if member_of(ctx, new_member).is_some() {
        ctx.db
            .party_request()
            .request_id()
            .delete(request.request_id);
        return Err("that player is already in a party".to_string());
    }
    if party_is_full(party_size(ctx, party_row.party_id)) {
        return Err("that party is full".to_string());
    }

    ctx.db.party_member().insert(PartyMemberRow {
        character_id: new_member,
        party_id: party_row.party_id,
        joined_at: ctx.timestamp,
    });
    ctx.db
        .party_request()
        .request_id()
        .delete(request.request_id);

    notify_character(
        ctx,
        request.initiator,
        format!("{} accepted your party request.", actor.display_name),
    );
    notify_character(ctx, new_member, "You joined the party.".to_string());
    Ok(())
}

/// Declines the pending request between the caller and `name`, in whichever
/// direction it runs (Decision 5). Only ever deletes the request row.
#[reducer]
pub fn party_decline(ctx: &ReducerContext, name: String) -> Result<(), String> {
    let actor = caller_character(ctx)?;
    let target = find_player_by_name(ctx, &name)?;
    let request = find_actionable_request(ctx, actor.character_id, target.character_id)
        .ok_or_else(|| format!("no pending party request from {}", target.display_name))?;

    ctx.db
        .party_request()
        .request_id()
        .delete(request.request_id);

    notify_character(
        ctx,
        request.initiator,
        format!("{} declined your party request.", actor.display_name),
    );
    Ok(())
}

/// Removes the caller from their party, promoting the longest-tenured
/// remaining member to leader, or disbanding the party (and dropping its
/// pending requests) if that empties it (Decision 8).
#[reducer]
pub fn party_leave(ctx: &ReducerContext) -> Result<(), String> {
    let actor = caller_character(ctx)?;
    let membership =
        member_of(ctx, actor.character_id).ok_or_else(|| "you are not in a party".to_string())?;
    let party_id = membership.party_id;

    ctx.db
        .party_member()
        .character_id()
        .delete(actor.character_id);

    let Some(party_row) = ctx.db.party().party_id().find(party_id) else {
        // Defensive: a member row should never outlive its party, but if it
        // somehow did, there is nothing left to reconcile.
        return Ok(());
    };

    let remaining: Vec<PartyMemberRow> =
        ctx.db.party_member().by_party().filter(&party_id).collect();

    if remaining.is_empty() {
        ctx.db.party().party_id().delete(party_id);
        let stale_request_ids: Vec<u64> = ctx
            .db
            .party_request()
            .iter()
            .filter(|row| row.party_id == party_id)
            .map(|row| row.request_id)
            .collect();
        for request_id in stale_request_ids {
            ctx.db.party_request().request_id().delete(request_id);
        }
        return Ok(());
    }

    if party_row.leader == actor.character_id {
        let candidates: Vec<(Uuid, i64)> = remaining
            .iter()
            .map(|row| (row.character_id, row.joined_at.to_micros_since_unix_epoch()))
            .collect();
        if let Some(new_leader) = promote_leader(&candidates) {
            ctx.db.party().party_id().update(PartyRow {
                leader: new_leader,
                ..party_row
            });
            notify_character(ctx, new_leader, "You are now the party leader.".to_string());
        }
    }

    for member in remaining {
        notify_character(
            ctx,
            member.character_id,
            format!("{} left the party.", actor.display_name),
        );
    }

    Ok(())
}

/// Drops party membership, leadership and pending requests for a character
/// that is being deleted. Shared with `delete_character` so a removed leader
/// cannot leave a `PartyRow.leader` unique key pointing at a ghost.
pub(crate) fn forget_deleted_character(ctx: &ReducerContext, actor: &Player) {
    let character_id = actor.character_id;
    let request_ids: Vec<u64> = ctx
        .db
        .party_request()
        .iter()
        .filter(|row| row.initiator == character_id || row.recipient == character_id)
        .map(|row| row.request_id)
        .collect();
    for request_id in request_ids {
        ctx.db.party_request().request_id().delete(request_id);
    }

    let Some(membership) = member_of(ctx, character_id) else {
        return;
    };
    let party_id = membership.party_id;
    ctx.db.party_member().character_id().delete(character_id);

    let Some(party_row) = ctx.db.party().party_id().find(party_id) else {
        return;
    };
    let remaining: Vec<PartyMemberRow> =
        ctx.db.party_member().by_party().filter(&party_id).collect();
    if remaining.is_empty() {
        ctx.db.party().party_id().delete(party_id);
        return;
    }
    if party_row.leader == character_id {
        let candidates: Vec<(Uuid, i64)> = remaining
            .iter()
            .map(|row| (row.character_id, row.joined_at.to_micros_since_unix_epoch()))
            .collect();
        if let Some(new_leader) = promote_leader(&candidates) {
            ctx.db.party().party_id().update(PartyRow {
                leader: new_leader,
                ..party_row
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic, readable stand-in for a real `ctx.new_uuid_v4()`
    /// character id — tests only care that these compare equal/unequal
    /// consistently with the small integer they were built from.
    fn cid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn self_target_is_detected() {
        assert!(is_self_target(cid(1), cid(1)));
        assert!(!is_self_target(cid(1), cid(2)));
    }

    #[test]
    fn party_full_boundary_is_exactly_five() {
        assert!(!party_is_full(4));
        assert!(party_is_full(5));
        assert!(party_is_full(6));
    }

    #[test]
    fn accept_resolves_invite_to_the_recipient() {
        assert_eq!(
            new_member_on_accept(PartyRequestKind::Invite, cid(10), cid(20)),
            cid(10)
        );
    }

    #[test]
    fn accept_resolves_join_request_to_the_initiator() {
        assert_eq!(
            new_member_on_accept(PartyRequestKind::JoinRequest, cid(10), cid(20)),
            cid(20)
        );
    }

    #[test]
    fn actionable_request_requires_actor_as_recipient_and_target_as_initiator() {
        assert!(matches_actionable_request(cid(1), cid(2), cid(1), cid(2)));
        // Direction swapped: actor is the initiator, not the recipient.
        assert!(!matches_actionable_request(cid(2), cid(1), cid(1), cid(2)));
        // Right recipient, wrong initiator.
        assert!(!matches_actionable_request(cid(1), cid(3), cid(1), cid(2)));
    }

    #[test]
    fn promote_leader_picks_the_earliest_join() {
        let candidates = [(cid(3), 500), (cid(1), 100), (cid(2), 200)];
        assert_eq!(promote_leader(&candidates), Some(cid(1)));
    }

    #[test]
    fn promote_leader_breaks_ties_by_lowest_character_id() {
        let candidates = [(cid(5), 100), (cid(2), 100)];
        assert_eq!(promote_leader(&candidates), Some(cid(2)));
    }

    #[test]
    fn promote_leader_of_no_remaining_members_is_none() {
        assert_eq!(promote_leader(&[]), None);
    }
}
