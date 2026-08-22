//! Slow: a speed-reduction debuff. Not hard crowd control.

use bevymmo_props_macro::status;

use crate::effects::StatusRegistry;

#[status(
    id = "slow",
    name = "Slow",
    icon = "status_slow",
    category = Debuff,
    duration = 3.0,
    cleanseable = true,
    purgeable = true,
    stacking = Refresh,
    refresh = RefreshAll,
    modifier(
        stat = Speed,
        operation = Multiply,
        value = 0.5
    )
)]
pub struct Slow;

pub fn register(registry: &mut StatusRegistry) {
    Slow::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Status, StatusCategory};
    use crate::stats::events::{ModifierOp, StatField};

    #[test]
    fn slow_is_a_cleanseable_speed_reduction_debuff_without_hard_control() {
        let definition = Slow::definition();

        assert_eq!(definition.category, StatusCategory::Debuff);
        assert!(definition.cleanseable);
        assert!(definition.purgeable);
        assert_eq!(definition.control, None);
        assert_eq!(definition.duration_seconds, 3.0);
        assert_eq!(definition.stat_modifiers.len(), 1);
        assert_eq!(definition.stat_modifiers[0].field, StatField::Speed);
        assert_eq!(definition.stat_modifiers[0].operation, ModifierOp::Multiply);
        assert_eq!(definition.stat_modifiers[0].value, 0.5);
    }
}
