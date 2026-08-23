//! Authoritative resolution of the shared effect vocabulary.
//!
//! The queue is transaction-local. SpacetimeDB stores only the resulting state
//! and replicated events; intermediate effects are never persisted or sent to
//! clients.

use std::collections::VecDeque;

use bevymmo_domain::effects::{EffectBundle, EffectSpec, QueuedEffect};
use spacetimedb::ReducerContext;

/// Result of resolving one effect. The first slice intentionally keeps the
/// outcome small; richer outcomes can be added when resistance/cleanse rules
/// need to expose them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectOutcome {
    Applied,
    Ignored,
}

/// Resolve one effect through the authoritative server helpers.
///
/// Keeping this as the only semantic entry point lets projectile, AoE, spell
/// and periodic paths converge without moving database concerns into the shared
/// gameplay crate.
pub fn resolve_effect(ctx: &ReducerContext, effect: QueuedEffect) -> EffectOutcome {
    let target = effect.context.target.get();
    let source = effect
        .context
        .source
        .or(effect.context.original_caster)
        .map(|entity| entity.get());

    match effect.spec {
        EffectSpec::Damage(damage) => {
            crate::sim::combat::apply_damage(
                ctx,
                target,
                source,
                damage.amount,
                effect.context.ability_id.map(|id| id.as_str().to_string()),
            );
            EffectOutcome::Applied
        }
        EffectSpec::Heal(heal) => {
            if crate::sim::combat::heal_allowed_for(ctx, target, source) {
                crate::sim::combat::apply_healing(ctx, target, heal.amount);
                EffectOutcome::Applied
            } else {
                EffectOutcome::Ignored
            }
        }
        EffectSpec::ApplyStatus(status) => {
            if crate::sim::combat::hostile_effect_blocked(ctx, target, source) {
                EffectOutcome::Ignored
            } else {
                crate::sim::status::apply(
                    ctx,
                    target,
                    source,
                    &status.status_id,
                    status.duration_override_seconds,
                    status.potency,
                );
                EffectOutcome::Applied
            }
        }
        EffectSpec::Cleanse(cleanse) => {
            crate::sim::status::cleanse(ctx, target, cleanse);
            EffectOutcome::Applied
        }
        EffectSpec::Purge(purge) => {
            crate::sim::status::purge(ctx, target, purge);
            EffectOutcome::Applied
        }
    }
}

/// Expand a bundle into a deterministic queue and resolve it in emission order.
pub fn resolve_bundle(
    ctx: &ReducerContext,
    action_sequence: u64,
    bundle: EffectBundle,
) -> Vec<EffectOutcome> {
    let mut queue = VecDeque::with_capacity(bundle.effects.len());
    for (bundle_index, spec) in bundle.effects.into_iter().enumerate() {
        queue.push_back(QueuedEffect {
            action_sequence,
            bundle_index: bundle_index as u32,
            context: bundle.context.clone(),
            spec,
        });
    }

    let mut outcomes = Vec::with_capacity(queue.len());
    while let Some(effect) = queue.pop_front() {
        outcomes.push(resolve_effect(ctx, effect));
    }
    outcomes
}
