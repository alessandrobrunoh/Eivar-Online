//! Lunge — Sword secondary ability (W).
//!
//! A lightning-fast forward thrust that strikes a single target with precision.
//! The precise stab can briefly stagger an opponent.

use bevymmo_props_macro::base_ability;

use crate::abilities::BaseAbilityRegistry;

#[base_ability(
    id = "lunge",
    name = "Lunge",
    tags = [Melee, SingleTarget, RepeatCompatible],
    range = 5.5,
    geometry = cone(radius = 3.5, angle_deg = 25.0),
    potency = 145.0,
    cast_time = 0.1,
    cooldown = 2.2,
    mana_cost = 7.0,
    control = Stun,
    control_duration = 0.4,
    animation = "sword_lunge",
    impact_vfx = "lunge_impact",
    icon = "abilities/icons/lunge.png",
)]
pub struct Lunge;

/// Adds this content package to the base-ability registry.
pub fn register(registry: &mut BaseAbilityRegistry) {
    Lunge::register(registry);
}
