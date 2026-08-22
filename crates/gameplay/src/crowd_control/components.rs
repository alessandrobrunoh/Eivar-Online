//! Runtime components for the Crowd Control framework.

// `#[reflect(Component)]` expands to a reference to this type.
#[cfg(feature = "bevy")]
use bevy_ecs::reflect::ReflectComponent;

use serde::{Deserialize, Serialize};

/// Kinds of hard crowd control an entity can suffer.
///
/// Slow is a movement-speed modifier, not a kind: it does not belong here.
#[cfg_attr(
    feature = "bevy",
    derive(bevy_ecs::component::Component, bevy_reflect::Reflect)
)]
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "bevy", reflect(Component))]
pub enum CrowdControlKind {
    /// Fully blocks movement and casting until expiry.
    #[default]
    Stun,
    /// Blocks movement but not casting.
    Root,
    /// Blocks casting but not movement.
    Silence,
}

impl CrowdControlKind {
    /// Returns `true` when this kind suppresses *all* actions (movement and
    /// casting).
    pub fn is_blocking(self) -> bool {
        matches!(self, CrowdControlKind::Stun)
    }

    /// Movement is frozen (stun or root).
    pub fn blocks_movement(self) -> bool {
        matches!(self, CrowdControlKind::Stun | CrowdControlKind::Root)
    }

    /// Casting is gagged (stun or silence).
    pub fn blocks_casting(self) -> bool {
        matches!(self, CrowdControlKind::Stun | CrowdControlKind::Silence)
    }

    /// Tie-break when two effects have the same remaining time on the world bar.
    /// Stun outranks Silence outranks Root.
    pub fn world_bar_priority(self) -> u8 {
        match self {
            CrowdControlKind::Stun => 3,
            CrowdControlKind::Silence => 2,
            CrowdControlKind::Root => 1,
        }
    }
}

/// One active CC effect on an entity.
///
/// `total_seconds` is retained alongside `remaining_seconds` so the UI can
/// render the bar fill as a stable ratio even under network jitter.
#[cfg_attr(feature = "bevy", derive(bevy_reflect::Reflect))]
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ActiveCrowdControl {
    pub kind: CrowdControlKind,
    /// Remaining time before this effect expires (server-authoritative;
    /// clients read this as a snapshot).
    pub remaining_seconds: f32,
    /// Original duration. Used by the UI to compute the fill percentage.
    pub total_seconds: f32,
}

/// Server-authoritative CC state, replicated (and predicted) to clients.
///
/// Holds every active CC effect on the entity. Applying a new effect of an
/// already-present kind **refreshes** it (prevents stacking); different kinds
/// coexist so a Stun and a future Silence could overlap.
///
/// The component stays attached (empty) after all effects expire, to avoid
/// insert/remove churn on the entity. UI and gating systems treat an empty
/// state as "no CC".
///
/// # Example
/// ```rust,ignore
/// let mut state = CrowdControlState::default();
/// state.apply(CrowdControlKind::Stun, 2.0);
/// assert!(state.has_blocking_cc());
/// ```
#[cfg_attr(
    feature = "bevy",
    derive(bevy_ecs::component::Component, bevy_reflect::Reflect)
)]
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "bevy", reflect(Component))]
pub struct CrowdControlState {
    pub effects: Vec<ActiveCrowdControl>,
}

impl CrowdControlState {
    /// Returns `true` when no CC effect is currently active.
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    /// Returns `true` if any active effect blocks actions (movement + casting).
    ///
    /// Stun is the only fully blocking kind. Prefer [`Self::blocks_movement`] or
    /// [`Self::blocks_casting`] for the narrower gates.
    ///
    /// # Example
    /// ```rust,ignore
    /// if state.has_blocking_cc() { return; } // frozen
    /// ```
    pub fn has_blocking_cc(&self) -> bool {
        self.effects.iter().any(|effect| effect.kind.is_blocking())
    }

    /// True when stun or root is currently freezing movement.
    pub fn blocks_movement(&self) -> bool {
        self.effects
            .iter()
            .any(|effect| effect.kind.blocks_movement())
    }

    /// True when stun or silence is currently gagging casts.
    pub fn blocks_casting(&self) -> bool {
        self.effects
            .iter()
            .any(|effect| effect.kind.blocks_casting())
    }

    /// Refreshes (or inserts) a CC effect of the given kind.
    ///
    /// Refreshing — rather than stacking — keeps stun duration bounded even if
    /// multiple sources apply the same kind within a short window.
    ///
    /// # Example
    /// ```rust,ignore
    /// let mut state = CrowdControlState::default();
    /// state.apply(CrowdControlKind::Stun, 2.0);
    /// state.apply(CrowdControlKind::Stun, 1.0); // refreshes to 1.0s
    /// ```
    pub fn apply(&mut self, kind: CrowdControlKind, duration_seconds: f32) {
        if let Some(active) = self.effects.iter_mut().find(|effect| effect.kind == kind) {
            active.remaining_seconds = duration_seconds;
            active.total_seconds = duration_seconds;
            return;
        }
        self.effects.push(ActiveCrowdControl {
            kind,
            remaining_seconds: duration_seconds,
            total_seconds: duration_seconds,
        });
    }

    /// Advances every effect timer by `delta_seconds` and drops expired ones.
    ///
    /// Runs server-side each fixed tick. Clients only read the replicated
    /// snapshot, so they never call this.
    ///
    /// # Example
    /// ```rust,ignore
    /// state.tick(delta);
    /// ```
    pub fn tick(&mut self, delta_seconds: f32) {
        for effect in &mut self.effects {
            effect.remaining_seconds = (effect.remaining_seconds - delta_seconds).max(0.0);
        }
        self.effects.retain(|effect| effect.remaining_seconds > 0.0);
    }

    /// Returns the blocking effect with the longest remaining time, if any.
    ///
    /// Used by the CC bar UI to pick which effect to render when multiple
    /// blocking kinds coexist in the future.
    ///
    /// # Example
    /// ```rust,ignore
    /// if let Some(active) = state.longest_blocking() { render_bar(active); }
    /// ```
    pub fn longest_blocking(&self) -> Option<&ActiveCrowdControl> {
        self.effects
            .iter()
            .filter(|effect| effect.kind.is_blocking())
            .max_by(|left, right| {
                left.remaining_seconds
                    .partial_cmp(&right.remaining_seconds)
                    .expect("finite CC timer")
            })
    }

    /// The hard-control effect the world bar should show: longest remaining,
    /// then Stun > Silence > Root.
    pub fn longest(&self) -> Option<&ActiveCrowdControl> {
        self.effects.iter().max_by(|left, right| {
            left.remaining_seconds
                .total_cmp(&right.remaining_seconds)
                .then_with(|| {
                    left.kind
                        .world_bar_priority()
                        .cmp(&right.kind.world_bar_priority())
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_blocks_movement_but_not_all_actions() {
        let mut state = CrowdControlState::default();
        state.apply(CrowdControlKind::Root, 2.0);
        assert!(state.blocks_movement());
        assert!(!state.has_blocking_cc());
    }

    #[test]
    fn apply_inserts_new_effect() {
        let mut state = CrowdControlState::default();
        state.apply(CrowdControlKind::Stun, 2.0);
        assert_eq!(state.effects.len(), 1);
        assert!(state.has_blocking_cc());
    }

    #[test]
    fn apply_refreshes_existing_kind_instead_of_stacking() {
        let mut state = CrowdControlState::default();
        state.apply(CrowdControlKind::Stun, 2.0);
        state.apply(CrowdControlKind::Stun, 0.5);
        assert_eq!(state.effects.len(), 1);
        assert_eq!(state.effects[0].remaining_seconds, 0.5);
        assert_eq!(state.effects[0].total_seconds, 0.5);
    }

    #[test]
    fn tick_advances_and_drops_expired_effects() {
        let mut state = CrowdControlState::default();
        state.apply(CrowdControlKind::Stun, 1.0);
        state.tick(0.4);
        assert_eq!(state.effects.len(), 1);
        assert!((state.effects[0].remaining_seconds - 0.6).abs() < 1e-6);
        state.tick(0.6);
        assert!(state.is_empty());
        assert!(!state.has_blocking_cc());
    }

    #[test]
    fn longest_blocking_picks_max_remaining() {
        let mut state = CrowdControlState::default();
        state.apply(CrowdControlKind::Stun, 1.0);
        state.effects[0].remaining_seconds = 0.3;
        assert_eq!(
            state.longest_blocking().expect("present").remaining_seconds,
            0.3
        );
    }

    #[test]
    fn longest_blocking_returns_none_when_empty() {
        let state = CrowdControlState::default();
        assert!(state.longest_blocking().is_none());
    }

    #[test]
    fn silence_blocks_casting_but_not_movement() {
        let mut state = CrowdControlState::default();
        state.apply(CrowdControlKind::Silence, 2.0);
        assert!(state.blocks_casting());
        assert!(!state.blocks_movement());
        assert!(!state.has_blocking_cc());
    }

    #[test]
    fn longest_picks_root_when_it_is_the_only_hard_control() {
        let mut state = CrowdControlState::default();
        state.apply(CrowdControlKind::Root, 2.0);
        assert_eq!(
            state.longest().expect("present").kind,
            CrowdControlKind::Root
        );
        assert!(state.longest_blocking().is_none());
    }

    #[test]
    fn longest_tie_breaks_stun_over_root() {
        let mut state = CrowdControlState::default();
        state.apply(CrowdControlKind::Root, 1.0);
        state.apply(CrowdControlKind::Stun, 1.0);
        assert_eq!(
            state.longest().expect("present").kind,
            CrowdControlKind::Stun
        );
    }
}
