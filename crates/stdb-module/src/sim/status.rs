//! Authoritative status lifecycle and materialization into runtime child state.

use std::sync::OnceLock;

use bevymmo_domain::effects::{
    CleanseEffect, PeriodicEffect as DomainPeriodicEffect, PurgeEffect, RefreshPolicy,
    StackPolicy as DomainStackPolicy, StackScope, StatusCategory, StatusFilter, StatusId,
    StatusRegistry, StatusSelection,
};
use spacetimedb::{ReducerContext, Table};

use crate::tables::{
    active_status, periodic_effect, stat_modifier, ActiveStatus, CrowdControlKindRow,
    ModifierKindRow, PeriodicEffect, StatModifier,
};

fn registry() -> &'static StatusRegistry {
    static REGISTRY: OnceLock<StatusRegistry> = OnceLock::new();
    REGISTRY.get_or_init(bevymmo_domain::content::statuses::default_statuses)
}

/// Applies a status definition and materializes its currently supported control
/// payload into the CC child table.
pub fn apply(
    ctx: &ReducerContext,
    target: u64,
    source: Option<u64>,
    status_id: &StatusId,
    duration_override_seconds: Option<f32>,
    potency: f32,
) {
    let Some(definition) = registry().get(status_id) else {
        log::warn!("unknown status `{}`", status_id.as_str());
        return;
    };

    let duration = duration_override_seconds
        .unwrap_or(definition.duration_seconds)
        .max(0.0);
    if duration <= 0.0 {
        return;
    }

    let existing = ctx
        .db
        .active_status()
        .on_entity()
        .filter(&target)
        .find(|row| {
            row.status_id == status_id.as_str()
                && (definition.stack_scope == StackScope::Global || row.source == source)
        });

    let control_kind = definition.control.map(control_kind);
    let mut stacks = 1_u16;
    let (active_status_id, remaining, total) = match existing {
        Some(row) => {
            let active_status_id = row.id;
            let (new_stacks, added_stack) = match definition.stacking {
                DomainStackPolicy::AddStacks => {
                    let prev = row.stacks;
                    let next = prev.saturating_add(1).min(definition.max_stacks.max(1));
                    (next, next > prev)
                }
                _ => (row.stacks.max(1), false),
            };

            let (remaining, total) = compute_refreshed_duration(
                row.remaining_seconds,
                row.total_seconds,
                duration,
                definition.refresh,
                added_stack,
            );

            ctx.db.active_status().id().update(ActiveStatus {
                stacks: new_stacks,
                potency,
                remaining_seconds: remaining,
                total_seconds: total,
                control_kind,
                ..row
            });
            stacks = new_stacks;
            (active_status_id, remaining, total)
        }
        None => {
            let inserted = ctx.db.active_status().insert(ActiveStatus {
                id: 0,
                entity_id: target,
                status_id: status_id.as_str().to_string(),
                source,
                stacks: 1,
                potency,
                remaining_seconds: duration,
                total_seconds: duration,
                control_kind,
            });
            (inserted.id, duration, duration)
        }
    };

    materialize_periodic(
        ctx,
        definition.periodic,
        target,
        source,
        active_status_id,
        remaining,
        stacks,
    );
    materialize_modifiers(
        ctx,
        definition.stat_modifiers,
        target,
        source,
        active_status_id,
        definition.category,
        remaining,
    );

    if let Some(kind) = operational_control_kind(control_kind) {
        crate::sim::crowd_control::materialize(
            ctx,
            target,
            source,
            kind,
            remaining,
            total,
            active_status_id,
        );
    }
}

pub fn cleanse(ctx: &ReducerContext, target: u64, effect: CleanseEffect) {
    remove_matching(
        ctx,
        target,
        effect.filter,
        effect.max_statuses,
        effect.selection,
        false,
    );
}

pub fn purge(ctx: &ReducerContext, target: u64, effect: PurgeEffect) {
    remove_matching(
        ctx,
        target,
        effect.filter,
        effect.max_statuses,
        effect.selection,
        true,
    );
}

fn remove_matching(
    ctx: &ReducerContext,
    target: u64,
    filter: StatusFilter,
    max_statuses: Option<u16>,
    selection: StatusSelection,
    purge: bool,
) {
    let mut matching: Vec<_> = ctx
        .db
        .active_status()
        .on_entity()
        .filter(&target)
        .filter(|row| {
            let Some(definition) = registry().get(&StatusId::new(row.status_id.clone())) else {
                return false;
            };
            let category_matches = match filter {
                StatusFilter::Buffs => definition.category == StatusCategory::Buff,
                StatusFilter::Debuffs => definition.category == StatusCategory::Debuff,
                StatusFilter::All => true,
            };
            category_matches
                && if purge {
                    definition.purgeable
                } else {
                    definition.cleanseable
                }
        })
        .collect();

    matching.sort_by(|left, right| match selection {
        StatusSelection::Oldest => left.id.cmp(&right.id),
        StatusSelection::Newest => right.id.cmp(&left.id),
        StatusSelection::ShortestRemaining => left
            .remaining_seconds
            .total_cmp(&right.remaining_seconds)
            .then_with(|| left.id.cmp(&right.id)),
    });

    let limit = max_statuses.unwrap_or(u16::MAX) as usize;
    for status in matching.into_iter().take(limit) {
        remove_status_instance(ctx, status);
    }
}

pub(crate) fn remove_status_instance(ctx: &ReducerContext, status: ActiveStatus) {
    ctx.db.active_status().id().delete(status.id);
    remove_owned_periodics(ctx, status.id);
    remove_owned_modifiers(ctx, status.id);
    crate::sim::combat::recalculate_effective_stats(ctx, status.entity_id);
    crate::sim::crowd_control::remove_owned(ctx, status.id);
}

/// Expires semantic status rows and copies remaining time onto owned control
/// children. Status-owned periodic schedules are removed before the semantic
/// row disappears.
pub fn step(ctx: &ReducerContext, dt: f32) {
    let mut updated = Vec::new();
    let mut expired = Vec::new();
    let mut control_sync = Vec::new();

    for status in ctx.db.active_status().iter() {
        let remaining = status.remaining_seconds - dt;
        if remaining <= 0.0 {
            expired.push(status);
        } else {
            if operational_control_kind(status.control_kind).is_some() {
                control_sync.push((status.id, remaining, status.total_seconds));
            }
            updated.push(ActiveStatus {
                remaining_seconds: remaining,
                ..status
            });
        }
    }

    for status in updated {
        ctx.db.active_status().id().update(status);
    }
    for (origin, remaining, total) in control_sync {
        crate::sim::crowd_control::sync_timer(ctx, origin, remaining, total);
    }
    for status in expired {
        remove_status_instance(ctx, status);
    }
}

fn materialize_periodic(
    ctx: &ReducerContext,
    periodic: Option<bevymmo_domain::effects::PeriodicSpec>,
    target: u64,
    source: Option<u64>,
    status_instance_id: u64,
    duration: f32,
    stacks: u16,
) {
    let Some(periodic) = periodic else {
        remove_owned_periodics(ctx, status_instance_id);
        return;
    };
    if periodic.interval_seconds <= 0.0 || duration <= 0.0 {
        remove_owned_periodics(ctx, status_instance_id);
        return;
    }

    let stack_mul = stacks.max(1) as f32;
    let amount_per_tick = match periodic.effect {
        DomainPeriodicEffect::Damage { amount, .. } => -amount.abs() * stack_mul,
        DomainPeriodicEffect::Heal { amount } => amount.abs() * stack_mul,
    };
    if amount_per_tick == 0.0 {
        remove_owned_periodics(ctx, status_instance_id);
        return;
    }

    let existing = ctx
        .db
        .periodic_effect()
        .iter()
        .find(|row| row.origin_status_instance_id == Some(status_instance_id));

    match existing {
        Some(row) => {
            ctx.db.periodic_effect().id().update(PeriodicEffect {
                amount_per_tick,
                remaining_seconds: duration,
                ..row
            });
        }
        None => {
            ctx.db.periodic_effect().insert(PeriodicEffect {
                id: 0,
                entity_id: target,
                source,
                amount_per_tick,
                tick_interval_seconds: periodic.interval_seconds,
                origin_status_instance_id: Some(status_instance_id),
                since_last_tick: 0.0,
                remaining_seconds: duration,
            });
        }
    }
}

fn remove_owned_periodics(ctx: &ReducerContext, status_instance_id: u64) {
    let ids: Vec<_> = ctx
        .db
        .periodic_effect()
        .iter()
        .filter(|row| row.origin_status_instance_id == Some(status_instance_id))
        .map(|row| row.id)
        .collect();
    for id in ids {
        ctx.db.periodic_effect().id().delete(id);
    }
}

fn materialize_modifiers(
    ctx: &ReducerContext,
    modifiers: &[bevymmo_domain::effects::StatModifierSpec],
    target: u64,
    source: Option<u64>,
    status_instance_id: u64,
    category: StatusCategory,
    duration: f32,
) {
    remove_owned_modifiers(ctx, status_instance_id);
    let kind = match category {
        StatusCategory::Buff => ModifierKindRow::Buff,
        StatusCategory::Debuff => ModifierKindRow::Debuff,
    };

    for modifier in modifiers {
        let is_multiplicative = match modifier.operation {
            bevymmo_domain::stats::events::ModifierOp::Add => false,
            bevymmo_domain::stats::events::ModifierOp::Multiply => true,
            bevymmo_domain::stats::events::ModifierOp::Override => {
                log::warn!("status modifier Override is not supported by the current row schema");
                continue;
            }
        };
        ctx.db.stat_modifier().insert(StatModifier {
            id: 0,
            entity_id: target,
            source,
            field: format!("{:?}", modifier.field),
            is_multiplicative,
            amount: modifier.value,
            kind,
            remaining_seconds: Some(duration),
            origin_status_instance_id: Some(status_instance_id),
        });
    }
    if !modifiers.is_empty() {
        crate::sim::combat::recalculate_effective_stats(ctx, target);
    }
}

fn remove_owned_modifiers(ctx: &ReducerContext, status_instance_id: u64) {
    let ids: Vec<_> = ctx
        .db
        .stat_modifier()
        .iter()
        .filter(|row| row.origin_status_instance_id == Some(status_instance_id))
        .map(|row| row.id)
        .collect();
    for id in ids {
        ctx.db.stat_modifier().id().delete(id);
    }
}

fn control_kind(control: bevymmo_domain::effects::ControlSpec) -> CrowdControlKindRow {
    match control {
        bevymmo_domain::crowd_control::CrowdControlKind::Stun => CrowdControlKindRow::Stun,
        bevymmo_domain::crowd_control::CrowdControlKind::Root => CrowdControlKindRow::Root,
        bevymmo_domain::crowd_control::CrowdControlKind::Silence => CrowdControlKindRow::Silence,
    }
}

/// Hard-control kinds that own a `crowd_control` child. Slow is a speed
/// modifier only; materializing it as CC would open a gate that does nothing
/// and a row the client already drops.
pub(crate) fn operational_control_kind(
    kind: Option<CrowdControlKindRow>,
) -> Option<CrowdControlKindRow> {
    match kind {
        Some(CrowdControlKindRow::Slow) | None => None,
        some @ Some(_) => some,
    }
}

/// Computes the new `(remaining_seconds, total_seconds)` when a status is
/// re-applied, according to its [`RefreshPolicy`].
fn compute_refreshed_duration(
    current_remaining: f32,
    current_total: f32,
    new_duration: f32,
    policy: RefreshPolicy,
    added_stack: bool,
) -> (f32, f32) {
    match policy {
        RefreshPolicy::None => {
            // Do not extend: keep whatever time is left.
            (current_remaining, current_total)
        }
        RefreshPolicy::RefreshAll => {
            // Full reset to the new duration.
            (new_duration, new_duration)
        }
        RefreshPolicy::RefreshNewStackOnly => {
            if added_stack {
                // A stack was actually gained: reset the timer.
                (new_duration, new_duration)
            } else {
                // Already at max stacks or non-stacking policy: no refresh.
                (current_remaining, current_total)
            }
        }
        RefreshPolicy::Extend => {
            // Add the new duration on top of what remains.
            let extended = current_remaining + new_duration;
            (extended, extended)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_none_preserves_existing_time() {
        let (remaining, total) =
            compute_refreshed_duration(3.0, 5.0, 5.0, RefreshPolicy::None, false);
        assert_eq!(remaining, 3.0);
        assert_eq!(total, 5.0);
    }

    #[test]
    fn refresh_all_resets_to_full_new_duration() {
        let (remaining, total) =
            compute_refreshed_duration(1.0, 5.0, 8.0, RefreshPolicy::RefreshAll, false);
        assert_eq!(remaining, 8.0);
        assert_eq!(total, 8.0);
    }

    #[test]
    fn refresh_new_stack_only_resets_when_stack_added() {
        let (remaining, total) =
            compute_refreshed_duration(2.0, 5.0, 5.0, RefreshPolicy::RefreshNewStackOnly, true);
        assert_eq!(remaining, 5.0);
        assert_eq!(total, 5.0);
    }

    #[test]
    fn refresh_new_stack_only_ignores_when_no_stack_added() {
        let (remaining, total) =
            compute_refreshed_duration(2.0, 5.0, 5.0, RefreshPolicy::RefreshNewStackOnly, false);
        assert_eq!(remaining, 2.0);
        assert_eq!(total, 5.0);
    }

    #[test]
    fn refresh_extend_adds_new_duration_to_remaining() {
        let (remaining, total) =
            compute_refreshed_duration(3.0, 5.0, 5.0, RefreshPolicy::Extend, false);
        assert_eq!(remaining, 8.0);
        assert_eq!(total, 8.0);
    }

    #[test]
    fn refresh_extend_with_almost_expired_status() {
        let (remaining, total) =
            compute_refreshed_duration(0.1, 5.0, 5.0, RefreshPolicy::Extend, false);
        assert!((remaining - 5.1).abs() < 1e-6);
        assert!((total - 5.1).abs() < 1e-6);
    }

    #[test]
    fn refresh_none_control_child_keeps_status_remaining_not_incoming() {
        let incoming = 8.0;
        let (remaining, total) =
            compute_refreshed_duration(3.0, 5.0, incoming, RefreshPolicy::None, false);
        assert_eq!(remaining, 3.0);
        assert_eq!(total, 5.0);
        assert_ne!(remaining, incoming);
        assert_eq!(
            crate::sim::crowd_control::plan_materialize_control(remaining),
            crate::sim::crowd_control::MaterializeAction::Write {
                remaining_seconds: 3.0
            }
        );
    }

    #[test]
    fn refresh_extend_control_child_uses_extended_remaining() {
        let (remaining, total) =
            compute_refreshed_duration(3.0, 5.0, 5.0, RefreshPolicy::Extend, false);
        assert_eq!(
            crate::sim::crowd_control::plan_materialize_control(remaining),
            crate::sim::crowd_control::MaterializeAction::Write {
                remaining_seconds: 8.0
            }
        );
        assert_eq!(total, 8.0);
    }

    #[test]
    fn refresh_all_control_child_resets_to_incoming() {
        let incoming = 8.0;
        let (remaining, total) =
            compute_refreshed_duration(1.0, 5.0, incoming, RefreshPolicy::RefreshAll, false);
        assert_eq!(
            crate::sim::crowd_control::plan_materialize_control(remaining),
            crate::sim::crowd_control::MaterializeAction::Write {
                remaining_seconds: incoming
            }
        );
        assert_eq!(total, incoming);
    }

    #[test]
    fn slow_does_not_materialize_a_crowd_control_child() {
        assert_eq!(
            operational_control_kind(Some(CrowdControlKindRow::Slow)),
            None
        );
        assert_eq!(
            operational_control_kind(Some(CrowdControlKindRow::Stun)),
            Some(CrowdControlKindRow::Stun)
        );
        assert_eq!(
            operational_control_kind(Some(CrowdControlKindRow::Root)),
            Some(CrowdControlKindRow::Root)
        );
        assert_eq!(
            operational_control_kind(Some(CrowdControlKindRow::Silence)),
            Some(CrowdControlKindRow::Silence)
        );
        assert_eq!(operational_control_kind(None), None);
    }
}
