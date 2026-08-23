//! Persistent domain-event recording, deliberately separate from simulation.

use std::time::Duration;

use spacetimedb::{reducer, ReducerContext, Table};

use crate::tables::{
    account, domain_event, domain_event_config, DomainEvent, DomainEventConfig, DomainEventKind,
};

pub const DEFAULT_RETENTION_SECONDS: u64 = 24 * 60 * 60;
pub const DEFAULT_DAMAGE_THRESHOLD: f32 = 1.0;
pub const CLEANUP_INTERVAL_SECONDS: u64 = 60;

/// Toggle raw event recording without changing simulation results.
///
/// This is deliberately admin-only: clients must not be able to suppress or
/// selectively erase the audit history.
#[reducer]
pub fn set_logging(ctx: &ReducerContext, enabled: bool) -> Result<(), String> {
    let session = crate::reducers::account::caller_session(ctx)?;
    let account = ctx
        .db
        .account()
        .id()
        .find(&session.account_id)
        .ok_or_else(|| "account not found".to_string())?;
    if account.role != crate::tables::RoleRow::Admin {
        return Err("admin role required".to_string());
    }

    let config = ctx
        .db
        .domain_event_config()
        .id()
        .find(&0)
        .unwrap_or(DomainEventConfig {
            id: 0,
            enabled: true,
            damage_threshold: DEFAULT_DAMAGE_THRESHOLD,
            retention_seconds: DEFAULT_RETENTION_SECONDS,
        });
    ctx.db
        .domain_event_config()
        .id()
        .update(DomainEventConfig { enabled, ..config });
    Ok(())
}

pub fn logging_enabled(ctx: &ReducerContext) -> bool {
    ctx.db
        .domain_event_config()
        .id()
        .find(&0)
        .map(|config| config.enabled)
        .unwrap_or(true)
}

pub fn record_damage(
    ctx: &ReducerContext,
    attacker: Option<u64>,
    target: u64,
    amount: f32,
    ability_id: Option<String>,
    killed: bool,
) {
    if !logging_enabled(ctx) {
        return;
    }
    let threshold = ctx
        .db
        .domain_event_config()
        .id()
        .find(&0)
        .map(|config| config.damage_threshold)
        .unwrap_or(DEFAULT_DAMAGE_THRESHOLD);
    if !killed && amount < threshold {
        return;
    }
    ctx.db.domain_event().insert(DomainEvent {
        id: 0,
        occurred_at: ctx.timestamp,
        kind: DomainEventKind::DamageDealt,
        actor_entity_id: attacker,
        target_entity_id: Some(target),
        amount: Some(amount),
        source_id: ability_id,
        killer_entity_id: None,
        payload: None,
    });
}

pub fn record_death(ctx: &ReducerContext, target: u64, killer: Option<u64>, is_player: bool) {
    if !logging_enabled(ctx) {
        return;
    }
    ctx.db.domain_event().insert(DomainEvent {
        id: 0,
        occurred_at: ctx.timestamp,
        kind: if is_player {
            DomainEventKind::PlayerDied
        } else {
            DomainEventKind::EntityDied
        },
        actor_entity_id: killer,
        target_entity_id: Some(target),
        amount: None,
        source_id: None,
        killer_entity_id: killer,
        payload: None,
    });
}

pub fn record_cast(ctx: &ReducerContext, entity_id: u64, spell_id: String) {
    if !logging_enabled(ctx) {
        return;
    }
    ctx.db.domain_event().insert(DomainEvent {
        id: 0,
        occurred_at: ctx.timestamp,
        kind: DomainEventKind::SpellCast,
        actor_entity_id: Some(entity_id),
        target_entity_id: None,
        amount: None,
        source_id: Some(spell_id),
        killer_entity_id: None,
        payload: None,
    });
}

/// Scheduled retention pass; never runs once per simulation frame.
#[reducer]
pub fn prune(ctx: &ReducerContext, _schedule: crate::tables::DomainEventCleanupSchedule) {
    let retention = ctx
        .db
        .domain_event_config()
        .id()
        .find(&0)
        .map(|config| config.retention_seconds)
        .unwrap_or(DEFAULT_RETENTION_SECONDS);
    let expired: Vec<u64> = ctx
        .db
        .domain_event()
        .iter()
        .filter(|event| {
            ctx.timestamp
                .duration_since(event.occurred_at)
                .map(|age| age.as_secs() >= retention)
                .unwrap_or(false)
        })
        .map(|event| event.id)
        .collect();
    for id in expired {
        ctx.db.domain_event().id().delete(&id);
    }
}

pub fn schedule() -> spacetimedb::ScheduleAt {
    spacetimedb::ScheduleAt::Interval(Duration::from_secs(CLEANUP_INTERVAL_SECONDS).into())
}
