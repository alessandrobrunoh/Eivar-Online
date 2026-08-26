//! Aegis — a helmet ability that grants a temporary pure shield.

use bevymmo_props_macro::base_ability;

use crate::abilities::BaseAbilityRegistry;

#[base_ability(
    id = "aegis",
    name = "Aegis",
    tags = [SelfTarget],
    range = 0.0,
    geometry = circle(radius = 0.0),
    potency = 1000.0,
    cast_time = 0.0,
    cooldown = 10.0,
    mana_cost = 0.0,
    animation = "aegis",
    impact_vfx = "aegis",
)]
pub struct Aegis;

pub fn register(registry: &mut BaseAbilityRegistry) {
    Aegis::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::BaseAbility;

    #[test]
    fn grants_the_authored_shield_potency() {
        assert_eq!(Aegis.base_params().potency, 1000.0);
        assert!(Aegis.has_tag(crate::abilities::AbilityTag::SelfTarget));
    }
}
