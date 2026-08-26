//! Shared targeting predicates used by AI and spell queries.

use std::collections::HashSet;

use spacetimedb::{ReducerContext, Uuid};

use crate::tables::{player, EntityKindRow, EntityStateRow};

/// Character ids whose `player.online` flag is currently true.
pub fn online_character_ids(ctx: &ReducerContext) -> HashSet<Uuid> {
    // Indexed lookup, not `.iter().filter(..)`: this runs three times per tick
    // (once from `ai::step`, twice from `sim::spells`), and the scan it
    // replaces was over every character ever created rather than over the
    // players actually connected.
    ctx.db
        .player()
        .online()
        .filter(&true)
        .map(|player| player.character_id)
        .collect()
}

/// Whether this entity is a living player whose character is currently online.
pub fn is_online_living_player(
    kind: EntityKindRow,
    state: EntityStateRow,
    online: Option<bool>,
) -> bool {
    kind == EntityKindRow::Player && state != EntityStateRow::Dead && online == Some(true)
}

/// Whether a spell or projectile may hit this entity.
///
/// Offline players stay in `game_entity` but are not valid targets. Enemies,
/// bosses, dummies and NPCs do not use `player.online`.
pub fn is_valid_spell_target(
    kind: EntityKindRow,
    state: EntityStateRow,
    online: Option<bool>,
) -> bool {
    if state == EntityStateRow::Dead {
        return false;
    }
    if kind == EntityKindRow::Player {
        return online == Some(true);
    }
    if kind == EntityKindRow::ResourceNode {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_players_are_not_combat_targets() {
        assert!(is_online_living_player(
            EntityKindRow::Player,
            EntityStateRow::Idle,
            Some(true),
        ));
        assert!(!is_online_living_player(
            EntityKindRow::Player,
            EntityStateRow::Idle,
            Some(false),
        ));
        assert!(!is_online_living_player(
            EntityKindRow::Player,
            EntityStateRow::Dead,
            Some(true),
        ));
        assert!(!is_online_living_player(
            EntityKindRow::Enemy,
            EntityStateRow::Idle,
            None,
        ));
    }

    #[test]
    fn spells_can_still_hit_online_players_and_mobs() {
        assert!(is_valid_spell_target(
            EntityKindRow::Enemy,
            EntityStateRow::Idle,
            None,
        ));
        assert!(is_valid_spell_target(
            EntityKindRow::Player,
            EntityStateRow::Idle,
            Some(true),
        ));
        assert!(!is_valid_spell_target(
            EntityKindRow::Player,
            EntityStateRow::Idle,
            Some(false),
        ));
        assert!(!is_valid_spell_target(
            EntityKindRow::Enemy,
            EntityStateRow::Dead,
            None,
        ));
        assert!(is_valid_spell_target(
            EntityKindRow::Dummy,
            EntityStateRow::Idle,
            None,
        ));
        assert!(!is_valid_spell_target(
            EntityKindRow::Dummy,
            EntityStateRow::Dead,
            None,
        ));
        assert!(!is_valid_spell_target(
            EntityKindRow::ResourceNode,
            EntityStateRow::Idle,
            None,
        ));
    }

    #[test]
    fn missing_online_flag_is_not_a_player_target() {
        assert!(!is_online_living_player(
            EntityKindRow::Player,
            EntityStateRow::Idle,
            None,
        ));
        assert!(!is_valid_spell_target(
            EntityKindRow::Player,
            EntityStateRow::Idle,
            None,
        ));
    }
}
