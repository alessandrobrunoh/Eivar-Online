//! Weapon ability system: gesti fissi dell'equipaggiamento (`BaseAbility`) +
//! Glifi incisi dal giocatore (`Essence`/`Modifier`/`AncientWord`) = spell
//! finale. Mirrors `crate::spells`/`crate::items` nello stile (trait +
//! registry + id), con quattro macro gemelle in `bevymmo-props-macro` per
//! ridurre il boilerplate di ogni nuovo pezzo di contenuto.

pub mod aim;
pub mod ancient_word;
pub mod base_ability;
pub mod blueprint;
pub mod cast_intent;
pub mod cooldowns;

pub mod events;
pub mod inscription;
pub mod known_glyphs;

pub mod resolve;
pub mod root_word;
pub mod slot;
pub mod weapon_abilities;

pub use aim::AbilityAim;
pub use ancient_word::{
    AncientWord, AncientWordEffect, AncientWordId, AncientWordMetadata, AncientWordRegistry,
    ArcAncientWord,
};
pub use base_ability::{
    AbilityCastMode, AbilityGeometry, AbilityId, AbilityParams, AbilityTag, AppliedControl,
    ArcBaseAbility, BaseAbility, BaseAbilityRegistry, ChannelMovementPolicy,
};
pub use blueprint::{AbilityBlueprint, ManifestationKind, ManifestationPayload};
pub use cast_intent::{
    flush_queued_release, movement_lock_for_ability, queue_release_until_observed,
    weapon_cast_intent, WeaponCastIntent,
};
pub use cooldowns::AbilityCooldowns;

pub use events::WeaponCastRequest;
pub use inscription::{
    AbilityInscription, ArmorInscription, ItemInscription, KitInscription, RuneProfile,
    SecondaryWord, SlotInscription, WeaponInscription,
};
pub use root_word::{
    ArcRootWord, RootWord, RootWordEffect, RootWordId, RootWordMetadata, RootWordRegistry,
};

pub use known_glyphs::KnownAncientLanguage;

pub use resolve::{
    cast_ability_preview, cast_armor_inscribed_ability, cast_root_inscribed_slot, resolve_ability,
    resolve_armor_inscribed_ability, resolve_root_inscribed_slot, CastBlockedReason, SlotPreview,
};
pub use slot::AbilitySlot;
pub use weapon_abilities::{
    resolve_active_ability, resolve_armor_ability, AbilityLoadout, AbilitySelection,
    WeaponAbilities,
};
