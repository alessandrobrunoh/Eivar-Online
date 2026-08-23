//! ECS marker components and value types used by the standard Card UI.
//!
//! These markers let the global Card systems ([`super::systems`]) find the
//! Card root, its close button, and decide exclusivity without knowing about
//! any specific panel.

use bevy::prelude::*;

/// Root marker of a Card panel.
///
/// Each open panel is exactly one `CardWindow`. Carries its [`CardKind`] for
/// diagnostics / future focus management and its [`CardExclusivityPolicy`]
/// so the global [`super::systems::enforce_card_exclusivity`] system can decide
/// what other cards must be closed when this one opens.
#[derive(Component, Debug)]
pub struct CardWindow {
    pub kind: CardKind,
    pub exclusivity: CardExclusivityPolicy,
}

/// Initial anchor position for a Card window when spawned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CardPositioning {
    #[default]
    Center,
    Left,
    Right,
}

/// Marker component for Card windows that can be dragged around by their header.
#[derive(Component, Debug, Default)]
pub struct DraggableCard;

/// State component attached to a Card while it is actively being dragged.
#[derive(Component, Debug)]
pub struct CardDraggingState {
    pub drag_start_cursor: Vec2,
    pub drag_start_left: f32,
    pub drag_start_top: f32,
}

/// Identifies the type of card. Used today only for diagnostics; reserved for
/// per-kind behaviors (ESC closes only the topmost, focused kind, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    Inventory,
    Market,
    ItemDetail,
    CharacterSheet,
    Settings,
    Loot,
    Generic,
}

/// Policy Object pattern: every card declares how it interacts with other
/// open cards, instead of hardcoding pairs of panel types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CardExclusivityPolicy {
    /// Opening this card closes every other currently open non-`Coexist` card,
    /// and any later `Exclusive` card will in turn replace this one.
    ///
    /// Use this for top-level panels that should never overlap (main
    /// inventory, market, character sheet).
    #[default]
    Exclusive,

    /// This card may float alongside other open cards.
    ///
    /// Typical use: a detail/inspector card (item detail, tooltip) that must
    /// stay visible while its parent panel is still open.
    Coexist,
}

/// Header region of a Card. Holds the title and (optionally) the close button.
/// Spawned automatically by [`super::builder::CardBuilder`].
#[derive(Component, Debug)]
pub struct CardHeader;

/// Marker component for the header area used as a drag handle.
#[derive(Component, Debug)]
pub struct CardHeaderDragHandle;

/// Body region of a Card. Caller-supplied children go in here.
#[derive(Component, Debug)]
pub struct CardBody;

/// Optional footer region of a Card (action buttons row).
#[derive(Component, Debug)]
pub struct CardFooter;

/// Close button carried inside a Card header.
///
/// `kind` mirrors the parent card's kind so the global close system can despawn
/// the owning `CardWindow` without traversing the parent chain in the common
/// case (we still fall back to parent traversal for robustness).
#[derive(Component, Debug)]
pub struct CloseCardButton {
    pub kind: CardKind,
}
