//! Sword weapon family.

// The outer module is the *family*, the inner one is the individual weapon that
// happens to share its name — `weapons::sword::sword::Sword` is the plain sword,
// and a longsword or a sabre would sit beside it. Renaming either half would
// make the path lie about one of the two.
#[allow(clippy::module_inception)]
pub mod sword;

use bevymmo_props_macro::weapon_family;

use crate::ability_definitions::blade_storm::BladeStorm;
use crate::ability_definitions::cleave::Cleave;
use crate::ability_definitions::lunge::Lunge;

#[weapon_family(
    id = "sword",
    name = "Spada",
    primary = [Cleave],
    secondary = [Lunge],
    ultimate = [BladeStorm],
)]
pub struct SwordFamily;

pub fn register(registry: &mut crate::items::WeaponFamilyRegistry) {
    SwordFamily::register(registry);
}
