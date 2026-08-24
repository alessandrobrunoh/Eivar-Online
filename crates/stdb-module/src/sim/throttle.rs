//! Running part of the tick less often than the tick itself.
//!
//! The tick is 20 Hz because movement and combat need to be. Some of what it
//! calls does not: mana regeneration and presence expiry produce the same
//! result run once a second with a second's worth of `dt`, and cost a
//! twentieth as much.
//!
//! That matters most for the writes. `entity_stats` is a `public` table, so
//! every row it updates is a delta replicated to every subscribed client —
//! regenerating mana at 20 Hz meant every entity below full mana, anywhere in
//! the world, produced twenty updates a second for every connected player.
//!
//! # Why this is sound for the callers that use it
//!
//! Only for work that is linear in `dt`. `regenerated_mana` is
//! `(current + rate * dt).clamp(0, max)`, so one call with `dt = 1.0` lands
//! exactly where twenty calls with `dt = 0.05` would, including at the cap.
//! Anything with a per-call step, a threshold crossed mid-interval, or a
//! visible animation must keep running every tick.

use std::sync::atomic::{AtomicU32, Ordering};

/// Accumulates `dt` and reports when a period's worth has gone by.
///
/// State lives in an atomic rather than a table because it is pure
/// bookkeeping: losing it on a republish costs one short interval, and a
/// column would be one more write per tick, which is the cost this exists to
/// avoid. `crate::tick` already relies on module statics surviving between
/// ticks for its seeding flags.
pub struct Throttle {
    /// Microseconds, not milliseconds: at 20 Hz, rounding `dt` to whole
    /// milliseconds drifts by up to half a millisecond per tick, which is a
    /// percent of a one-second period. `dt` is capped at
    /// `MAX_STEP_SECONDS`, so this cannot overflow within a period.
    accumulated_micros: AtomicU32,
    period_micros: u32,
}

impl Throttle {
    pub const fn from_millis(period_millis: u32) -> Self {
        Self {
            accumulated_micros: AtomicU32::new(0),
            period_micros: period_millis.saturating_mul(1_000),
        }
    }

    /// Adds `dt` to the accumulator.
    ///
    /// Returns `Some(elapsed)` — the real time accumulated, in seconds — when
    /// a period is due, and resets. Returns `None` otherwise, and the caller
    /// should do nothing at all this tick.
    ///
    /// The elapsed value is what accumulated, not the nominal period, so a
    /// caller that multiplies by a rate stays correct across a stalled tick.
    pub fn due(&self, dt: f32) -> Option<f32> {
        if !dt.is_finite() || dt <= 0.0 {
            return None;
        }
        let add = (dt * 1_000_000.0) as u32;
        let total = self
            .accumulated_micros
            .load(Ordering::Relaxed)
            .saturating_add(add);

        if total < self.period_micros {
            self.accumulated_micros.store(total, Ordering::Relaxed);
            return None;
        }
        self.accumulated_micros.store(0, Ordering::Relaxed);
        Some(total as f32 / 1_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One tick at the module's 20 Hz.
    const TICK: f32 = 0.05;

    #[test]
    fn stays_quiet_until_the_period_is_due() {
        let throttle = Throttle::from_millis(1_000);
        // 19 ticks is 950ms — not yet a second.
        for _ in 0..19 {
            assert!(throttle.due(TICK).is_none());
        }
        assert!(throttle.due(TICK).is_some(), "the 20th tick completes 1s");
    }

    #[test]
    fn reports_the_time_that_actually_accumulated() {
        let throttle = Throttle::from_millis(1_000);
        for _ in 0..19 {
            throttle.due(TICK);
        }
        let elapsed = throttle.due(TICK).expect("due");
        assert!(
            (elapsed - 1.0).abs() < 0.01,
            "20 x 50ms should report about a second, got {elapsed}"
        );
    }

    #[test]
    fn the_accumulator_resets_so_periods_do_not_drift_together() {
        let throttle = Throttle::from_millis(1_000);
        for _ in 0..20 {
            throttle.due(TICK);
        }
        // Immediately after firing, the next tick must not fire again.
        assert!(throttle.due(TICK).is_none());
    }

    #[test]
    fn a_single_long_stall_fires_once_with_the_whole_span() {
        let throttle = Throttle::from_millis(1_000);
        // `MAX_STEP_SECONDS` caps a real `dt` at 0.25, but the throttle must
        // not misbehave if that ever changes.
        let elapsed = throttle.due(3.0).expect("a 3s step is overdue");
        assert!((elapsed - 3.0).abs() < 0.01);
        // And it does not then owe two more firings.
        assert!(throttle.due(TICK).is_none());
    }

    #[test]
    fn a_zero_or_negative_step_never_fires() {
        let throttle = Throttle::from_millis(1_000);
        assert!(throttle.due(0.0).is_none());
        assert!(throttle.due(-1.0).is_none());
        assert!(throttle.due(f32::NAN).is_none());
        // ...and none of those polluted the accumulator.
        for _ in 0..19 {
            assert!(throttle.due(TICK).is_none());
        }
        assert!(throttle.due(TICK).is_some());
    }

    #[test]
    fn regeneration_over_one_period_matches_tick_by_tick() {
        // The property the whole module rests on: `regenerated_mana` is linear
        // in `dt`, so batching a period's worth is not an approximation.
        use bevymmo_domain::stats::formulas::regenerated_mana;

        let (max, rate) = (100.0, 5.0);
        let mut per_tick = 0.0;
        for _ in 0..20 {
            per_tick = regenerated_mana(per_tick, max, rate, TICK);
        }
        let batched = regenerated_mana(0.0, max, rate, 1.0);
        assert!((per_tick - batched).abs() < 0.001);
    }

    #[test]
    fn batching_still_matches_when_the_pool_fills_mid_period() {
        use bevymmo_domain::stats::formulas::regenerated_mana;

        // Starts 1 mana short of full with a rate that overshoots: both paths
        // must land exactly on the cap, not past it.
        let (max, rate) = (100.0, 50.0);
        let mut per_tick = 99.0;
        for _ in 0..20 {
            per_tick = regenerated_mana(per_tick, max, rate, TICK);
        }
        let batched = regenerated_mana(99.0, max, rate, 1.0);
        assert_eq!(per_tick, max);
        assert_eq!(batched, max);
    }
}
