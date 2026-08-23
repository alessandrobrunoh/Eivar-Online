//! Static status definitions and the registry shared by content, client and server.

use std::borrow::Cow;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::registry::Registry;
use crate::stats::events::{ModifierOp, StatField};

use super::spec::EffectSpec;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StatusId(Cow<'static, str>);

impl StatusId {
    pub fn new(id: impl Into<Cow<'static, str>>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&'static str> for StatusId {
    fn from(value: &'static str) -> Self {
        Self::new(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatusCategory {
    Buff,
    Debuff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StackPolicy {
    None,
    Refresh,
    AddStacks,
    Strongest,
    Replace,
    Independent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StackScope {
    Global,
    PerSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefreshPolicy {
    None,
    RefreshAll,
    RefreshNewStackOnly,
    Extend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DispelPolicy {
    NotDispellable,
    RemoveWholeStatus,
    RemoveStacks,
}

/// Hard-control payload on a status definition. Slow is a stat modifier, not a
/// control kind, so this is [`crate::crowd_control::CrowdControlKind`].
pub type ControlSpec = crate::crowd_control::CrowdControlKind;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatModifierSpec {
    pub field: StatField,
    pub operation: ModifierOp,
    pub value: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PeriodicEffect {
    Damage { amount: f32 },
    Heal { amount: f32 },
}

impl PeriodicEffect {
    pub fn as_effect(&self) -> EffectSpec {
        match self {
            Self::Damage { amount } => {
                EffectSpec::Damage(super::spec::DamageEffect { amount: *amount })
            }
            Self::Heal { amount } => EffectSpec::Heal(super::spec::HealEffect { amount: *amount }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeriodicSpec {
    pub interval_seconds: f32,
    pub effect: PeriodicEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusPresentation {
    pub icon: &'static str,
    pub short_name: &'static str,
}

impl Default for StatusPresentation {
    fn default() -> Self {
        Self {
            icon: "status_default",
            short_name: "Status",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatusDefinition {
    pub id: StatusId,
    pub category: StatusCategory,
    pub duration_seconds: f32,
    pub cleanseable: bool,
    pub purgeable: bool,
    pub stacking: StackPolicy,
    pub stack_scope: StackScope,
    pub max_stacks: u16,
    pub refresh: RefreshPolicy,
    pub dispel: DispelPolicy,
    pub periodic: Option<PeriodicSpec>,
    pub stat_modifiers: &'static [StatModifierSpec],
    pub control: Option<ControlSpec>,
    pub presentation: StatusPresentation,
}

pub trait Status: Send + Sync + 'static {
    fn definition() -> StatusDefinition
    where
        Self: Sized;

    fn status_id() -> StatusId
    where
        Self: Sized,
    {
        Self::definition().id
    }
}

pub type ArcStatus = Arc<StatusDefinition>;

#[cfg_attr(feature = "bevy", derive(bevy_ecs::resource::Resource))]
#[derive(Default, Clone)]
pub struct StatusRegistry {
    definitions: Registry<StatusId, ArcStatus>,
}

/// Replicated status snapshot used by client presentation and future status UI.
#[cfg_attr(feature = "bevy", derive(bevy_ecs::component::Component))]
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveStatusSnapshot {
    pub instance_id: u64,
    pub status_id: String,
    pub source: Option<crate::EntityId>,
    pub stacks: u16,
    pub potency: f32,
    pub remaining_seconds: f32,
    pub total_seconds: f32,
}

/// All active semantic statuses on one entity.
#[cfg_attr(feature = "bevy", derive(bevy_ecs::component::Component))]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActiveStatuses {
    pub statuses: Vec<ActiveStatusSnapshot>,
}

impl StatusRegistry {
    pub fn register(&mut self, definition: StatusDefinition) {
        self.definitions
            .insert(definition.id.clone(), Arc::new(definition));
    }

    pub fn get(&self, id: &StatusId) -> Option<ArcStatus> {
        self.definitions.get(id).cloned()
    }

    pub fn contains(&self, id: &StatusId) -> bool {
        self.definitions.contains(id)
    }

    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_damage_is_converted_to_the_normal_damage_effect() {
        let periodic = PeriodicEffect::Damage { amount: 10.0 };

        assert_eq!(
            periodic.as_effect(),
            EffectSpec::Damage(super::super::spec::DamageEffect { amount: 10.0 })
        );
    }

    #[test]
    fn registry_replaces_definitions_with_the_same_id() {
        let mut registry = StatusRegistry::default();
        let definition = StatusDefinition {
            id: StatusId::new("stun"),
            category: StatusCategory::Debuff,
            duration_seconds: 2.0,
            cleanseable: true,
            purgeable: false,
            stacking: StackPolicy::Refresh,
            stack_scope: StackScope::Global,
            max_stacks: 1,
            refresh: RefreshPolicy::RefreshAll,
            dispel: DispelPolicy::RemoveWholeStatus,
            periodic: None,
            stat_modifiers: &[],
            control: Some(ControlSpec::Stun),
            presentation: StatusPresentation::default(),
        };

        registry.register(definition);
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry
                .get(&StatusId::new("stun"))
                .unwrap()
                .duration_seconds,
            2.0
        );
    }
}
