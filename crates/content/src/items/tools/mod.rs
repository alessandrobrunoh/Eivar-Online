//! Gathering tools.

pub mod axe;
pub mod hammer;

use crate::items::ItemRegistry;

pub fn register(registry: &mut ItemRegistry) {
    axe::simple::register(registry);
    hammer::simple::register(registry);
}
