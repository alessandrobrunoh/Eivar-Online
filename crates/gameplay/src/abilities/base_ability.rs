//! `BaseAbility` — il "gesto" fisico di un'arma. Statico, mai creato dai
//! Glifi (principio cardine del sistema: l'equipaggiamento determina COME
//! un'abilità viene eseguita, l'Incisione determina COSA manifesta).
//!
//! A differenza di `Essence`/`Modifier`/`AncientWord`, una `BaseAbility` è
//! puro dato (nessuna logica che varia da istanza a istanza al di fuori di
//! `default_manifestation`, che ha un default generico derivato dalla
//! geometria) — per questo `#[base_ability(...)]` può generare l'intero
//! `impl BaseAbility for X` senza bisogno di un trait di comportamento
//! separato, a differenza delle altre tre macro gemelle.

use crate::EntityId;
use glam::Vec3;
use std::borrow::Cow;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::abilities::blueprint::AbilityBlueprint;
use crate::effects::EffectSpec;
use crate::registry::Registry;
use crate::spells::context::{AoeShape, SpellCastContext, TargetingMode};

/// Raggio d'impatto di una palla lanciata da un gesto `Projectile`.
pub const PROJECTILE_HIT_RADIUS: f32 = 1.0;

/// Semi-larghezza del "corridoio" davanti al lanciatore entro cui un gesto
/// `Projectile` senza bersaglio selezionato aggancia la prima entità (§ "una
/// palla davanti a sé": si mira guardando, non cliccando).
pub const FORWARD_LANE_HALF_WIDTH: f32 = 1.5;

/// Tag invisibili che determinano quali Modificatori/Parole Antiche
/// un'abilità può accettare (es. "Espandere richiede Area").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AbilityTag {
    Melee,
    Ranged,
    Projectile,
    Area,
    Ground,
    SingleTarget,
    SelfTarget,
    RepeatCompatible,
    PersistentCompatible,
    EchoCompatible,
}

/// Forma geometrica dell'impatto — condivisa fra il calcolo dell'effetto e
/// il visual (`impact_vfx` disegna esattamente questa forma).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AbilityGeometry {
    Cone { radius: f32, angle_deg: f32 },
    Circle { radius: f32 },
    Projectile { speed: f32 },
    SelfBuff { duration_seconds: f32 },
}

impl AbilityGeometry {
    /// How this gesture consumes the HUD's selected target and cursor point.
    ///
    /// The client always *has* both; only some geometries *use* them. Mapping
    /// onto [`TargetingMode`] is what stops a leftover selected dummy from
    /// failing a self-centered cleave as "out of range".
    pub fn targeting_mode(self) -> TargetingMode {
        match self {
            Self::Cone { .. } | Self::SelfBuff { .. } => TargetingMode::SelfCentered,
            Self::Circle { .. } => TargetingMode::GroundAoe,
            Self::Projectile { .. } => TargetingMode::DirectionalLine,
        }
    }

    /// Selected-unit id to send with the reducer. Cones, ground circles and
    /// self-buffs ignore a selected entity; attaching one would range-check it.
    pub fn selected_entity_payload(self, selected: Option<u64>) -> Option<u64> {
        match self {
            Self::Projectile { .. } => selected,
            Self::Cone { .. } | Self::Circle { .. } | Self::SelfBuff { .. } => None,
        }
    }
}

/// How an ability executes when activated.
///
/// Determines whether the ability fires immediately, has a wind-up period during
/// which movement cancels it, or channels repeated effects while held.
/// This is the weapon equivalent of [`crate::spells::context::CastKind`],
/// but lives on the ability definition rather than a separate spell config.
// No `Eq`: `Channeling` carries an `f32`, which has none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AbilityCastMode {
    /// Effect fires on press, no wind-up.
    Instant,
    /// Wind-up: click starts the cast; it fires when `cast_time` elapses.
    /// Movement always interrupts.
    CastTime,
    /// Repeated effect while held. Movement interrupts iff
    /// `movement_policy` is `InterruptOnMove`.
    Channeling {
        /// Seconds between each channel tick (e.g. 0.25 for 4 ticks/sec).
        tick_interval_seconds: f32,
        /// Whether movement cancels the channel.
        movement_policy: ChannelMovementPolicy,
    },
}

/// Default cast mode derived from `cast_time`: positive → CastTime, zero → Instant.
impl Default for AbilityCastMode {
    fn default() -> Self {
        AbilityCastMode::Instant
    }
}

impl AbilityCastMode {
    /// Infer cast mode from raw `cast_time` value.
    /// Preserves existing behaviour: any positive cast_time means CastTime.
    pub fn from_cast_time(cast_time: f32) -> Self {
        if cast_time > 0.0 {
            AbilityCastMode::CastTime
        } else {
            AbilityCastMode::Instant
        }
    }

    /// Whether this mode requires a persisted `cast_state`.
    pub fn is_instant(&self) -> bool {
        matches!(self, AbilityCastMode::Instant)
    }

    /// Whether this mode repeats while the key is held.
    pub fn is_channeling(&self) -> bool {
        matches!(self, AbilityCastMode::Channeling { .. })
    }

    /// Whether leftover walking should be dropped when this mode begins.
    ///
    /// Instant fires on the same tick and can be used on the move. CastTime and
    /// Channeling open a persisted wind-up: leftover chase would cancel
    /// CastTime (always) and InterruptOnMove channels, so NPCs plant until the
    /// ability finishes.
    pub fn holds_still(&self) -> bool {
        !self.is_instant()
    }

    /// Total duration for the progress bar (cast_time or max channel time).
    /// For channeling this is caller-defined; for CastTime it is the ability's cast_time.
    pub fn required_seconds(&self, cast_time: f32) -> f32 {
        match self {
            AbilityCastMode::Instant => 0.0,
            AbilityCastMode::CastTime => cast_time,
            AbilityCastMode::Channeling { .. } => cast_time.max(0.1), // minimum visible window
        }
    }
}

/// Channeling movement interrupt policy for weapon abilities.
/// Mirrors [`crate::spells::context::ChannelMovementPolicy`] so the unified
/// cast state can carry either source's policy without conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelMovementPolicy {
    /// Movement cancels channeling (default for offensive abilities).
    #[default]
    InterruptOnMove,
    /// Movement allowed; only release / re-press / death terminates channeling.
    AllowMovement,
}

/// Parametri numerici, prima o dopo l'applicazione dei Modificatori
/// (§14-24 del design: Espandere/Concentrare/Accelerare/Amplificare/
/// Prolungare agiscono su questi campi).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbilityParams {
    pub potency: f32,
    pub area: f32,
    pub range: f32,
    pub cast_time: f32,
    pub cooldown: f32,
    pub mana_cost: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AbilityId(Cow<'static, str>);

impl AbilityId {
    pub fn new(id: impl Into<Cow<'static, str>>) -> Self {
        Self(id.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&'static str> for AbilityId {
    fn from(value: &'static str) -> Self {
        Self::new(value)
    }
}

/// Direzione orizzontale normalizzata (l'altezza non conta: si gioca su un
/// piano). `Vec3::ZERO` se la direzione è degenere.
fn flat_direction(direction: Vec3) -> Vec3 {
    Vec3::new(direction.x, 0.0, direction.z).normalize_or_zero()
}

/// Scarto orizzontale da `origin` a `point`.
fn flat_offset(origin: Vec3, point: Vec3) -> Vec3 {
    Vec3::new(point.x - origin.x, 0.0, point.z - origin.z)
}

/// Riporta `target` entro `range` dal lanciatore, preservando l'altezza del target
/// (o interpolando la quota tra lanciatore e target se clampato oltre gittata).
/// `range <= 0.0` = nessun limite (il gesto si piazza dove vuole il giocatore).
fn clamp_to_range(origin: Vec3, target: Vec3, range: f32) -> Vec3 {
    if range <= 0.0 {
        return target;
    }
    let offset = flat_offset(origin, target);
    let distance = offset.length();
    if distance <= range {
        target
    } else {
        let direction = offset / distance;
        Vec3::new(
            origin.x + direction.x * range,
            origin.y + (target.y - origin.y) * (range / distance),
            origin.z + direction.z * range,
        )
    }
}

pub trait BaseAbility: Send + Sync + 'static {
    fn id(&self) -> AbilityId;
    fn display_name(&self) -> &'static str;
    fn tags(&self) -> &'static [AbilityTag];
    fn geometry(&self) -> AbilityGeometry;
    fn base_params(&self) -> AbilityParams;

    /// Builds the derived representation consumed by preview and execution.
    fn blueprint(&self) -> crate::abilities::AbilityBlueprint {
        crate::abilities::AbilityBlueprint::from_base_ability(self)
    }

    /// How this ability executes when activated.
    ///
    /// Default implementation derives from [`AbilityParams::cast_time`]:
    /// positive → [`AbilityCastMode::CastTime`], zero → [`AbilityCastMode::Instant`].
    /// Override for channeling abilities.
    fn cast_mode(&self) -> AbilityCastMode {
        AbilityCastMode::from_cast_time(self.base_params().cast_time)
    }
    /// Clip di animazione del personaggio — SEMPRE la stessa qualunque sia
    /// l'Essenza incisa (il gesto appartiene all'arma, non al Glifo).
    fn animation(&self) -> &'static str;
    /// Forma visiva dell'impatto (particellare/mesh), a cui l'Essenza
    /// applica solo colore/tema (vedi `EssenceVisualTheme`).
    fn impact_vfx(&self) -> &'static str;

    /// Bevy asset path under `assets/` for the HUD icon.
    ///
    /// Empty means the presentation layer shows no icon. `#[base_ability]`
    /// defaults this to `abilities/icons/{id}.png` when `icon` is omitted.
    fn icon(&self) -> &'static str {
        ""
    }

    /// Ritardo fra il lancio e l'impatto. > 0 significa "cerchio di
    /// preavviso a terra, poi lo schianto" (il Meteorite); il client legge
    /// lo stesso valore da qui per far durare il marker esattamente quanto
    /// l'attesa reale. Default: impatto immediato.
    fn impact_delay(&self) -> f32 {
        0.0
    }

    /// Stun applicato all'impatto, in secondi. Default 0.0 = il gesto non ha
    /// componente di controllo, solo danno.
    fn stun_seconds(&self) -> f32 {
        0.0
    }

    fn has_tag(&self, tag: AbilityTag) -> bool {
        self.tags().contains(&tag)
    }

    /// Dove il gesto manifesta il proprio effetto.
    ///
    /// - `Circle`: il punto indicato dal mouse, clampato a `params.range`
    ///   attorno al lanciatore (0.0 = nessun limite di gittata).
    /// - `Cone`: il lanciatore stesso, che è l'APICE del settore — la forma
    ///   vera e propria la porta [`Self::impact_shape`].
    /// - Altro: il lanciatore stesso.
    fn impact_center(&self, params: &AbilityParams, ctx: &SpellCastContext) -> Vec3 {
        match self.geometry() {
            AbilityGeometry::Circle { .. } => {
                clamp_to_range(ctx.caster_position, ctx.effective_center(), params.range)
            }
            AbilityGeometry::Cone { .. }
            | AbilityGeometry::Projectile { .. }
            | AbilityGeometry::SelfBuff { .. } => ctx.caster_position,
        }
    }

    /// Raggio effettivo dell'impatto ad area (0.0 per proiettili/self-buff).
    /// Per il cono è la gittata del settore misurata dall'apice.
    fn impact_radius(&self, params: &AbilityParams) -> f32 {
        match self.geometry() {
            AbilityGeometry::Cone { radius, .. } | AbilityGeometry::Circle { radius } => {
                params.area.max(radius)
            }
            AbilityGeometry::Projectile { .. } | AbilityGeometry::SelfBuff { .. } => 0.0,
        }
    }

    /// Forma coperta attorno a [`Self::impact_center`].
    ///
    /// È il terzo membro della terna centro/raggio/forma: chiunque debba
    /// sapere "quale superficie tocca questo gesto" — il server per applicare
    /// l'effetto, il client per disegnare l'anteprima di mira — la chiede qui
    /// invece di ricostruirla dalla geometria.
    fn impact_shape(&self, ctx: &SpellCastContext) -> AoeShape {
        match self.geometry() {
            AbilityGeometry::Cone { angle_deg, .. } => AoeShape::Cone {
                direction: flat_direction(ctx.caster_look_direction),
                angle_deg,
            },
            AbilityGeometry::Circle { .. }
            | AbilityGeometry::Projectile { .. }
            | AbilityGeometry::SelfBuff { .. } => AoeShape::Circle,
        }
    }

    /// Chi incassa la palla di un gesto `Projectile`.
    ///
    /// Il bersaglio selezionato vince, ma solo se è davvero davanti e a
    /// portata; altrimenti si aggancia la prima entità nel corridoio frontale
    /// (nessuna selezione richiesta — si spara dove si guarda).
    fn projectile_target(
        &self,
        params: &AbilityParams,
        ctx: &SpellCastContext,
    ) -> Option<EntityId> {
        let AbilityGeometry::Projectile { .. } = self.geometry() else {
            return None;
        };
        let range = params.range;
        let forward = flat_direction(ctx.caster_look_direction);
        if forward == Vec3::ZERO {
            return ctx.target_entity;
        }

        if let Some(selected) = ctx.target_entity {
            let in_front = ctx
                .potential_targets
                .iter()
                .find(|(entity, _)| *entity == selected)
                .map(|(_, position)| {
                    let offset = flat_offset(ctx.caster_position, *position);
                    let along = offset.dot(forward);
                    along > 0.0 && along <= range
                })
                .unwrap_or(false);
            if in_front {
                return Some(selected);
            }
        }

        ctx.potential_targets
            .iter()
            .filter(|(entity, _)| *entity != ctx.caster)
            .filter_map(|(entity, position)| {
                let offset = flat_offset(ctx.caster_position, *position);
                let along = offset.dot(forward);
                if along <= 0.0 || along > range {
                    return None;
                }
                if (offset - forward * along).length() > FORWARD_LANE_HALF_WIDTH {
                    return None;
                }
                Some((*entity, along))
            })
            .min_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(entity, _)| entity)
    }

    /// Piazza una regione con centro, raggio, forma e ritardo DI QUESTO gesto.
    ///
    /// Unico punto in cui la geometria di un'abilità si traduce in una
    /// `AoeSpawnRequest`: le Essenze che aggiungono un proprio effetto sopra
    /// la stessa area (Gelo → rallentamento, Terra → stagger) passano di qui
    /// invece di ricopiare centro/raggio/delay, così un cambio di forma le
    /// segue automaticamente.
    fn emit_area_effect(
        &self,
        params: &AbilityParams,
        ctx: &mut SpellCastContext,
        effects: Vec<EffectSpec>,
    ) {
        let delay = self.impact_delay();
        let center = self.impact_center(params, ctx);
        let radius = self.impact_radius(params);
        let shape = self.impact_shape(ctx);
        let spell_id = self.id().as_str().to_string();
        if effects
            .iter()
            .any(|spec| matches!(spec, EffectSpec::Heal(_)))
        {
            // Authoritative heal filtering drops hostiles; include the caster
            // so a Life wave can restore the wielder standing in their cone.
            ctx.emit_aoe(center, radius, shape, delay, delay, spell_id, effects);
        } else {
            ctx.emit_aoe_excluding_caster(center, radius, shape, delay, delay, spell_id, effects);
        }
    }

    /// Piazza l'impatto ad area del gesto con gli effetti già risolti
    /// (payload della Root Word). Centro, raggio e delay restano del gesto.
    fn emit_area_impact(
        &self,
        params: &AbilityParams,
        ctx: &mut SpellCastContext,
        effects: Vec<EffectSpec>,
    ) {
        self.emit_area_effect(params, ctx, effects);

        let center = self.impact_center(params, ctx);
        let spell_id = self.id().as_str().to_string();
        match self.geometry() {
            AbilityGeometry::Cone { .. } => {
                let direction = match self.impact_shape(ctx) {
                    AoeShape::Cone { direction, .. } => flat_direction(direction),
                    _ => flat_direction(ctx.caster_look_direction),
                };
                let end = center + direction * self.impact_radius(params);
                ctx.emit_visual(spell_id, center, end);
            }
            _ => ctx.emit_visual(spell_id, ctx.caster_position, center),
        }
    }

    /// Emette il payload nella forma dettata dalla geometria di questa abilità.
    fn emit_payload_for_geometry(
        &self,
        params: &AbilityParams,
        ctx: &mut SpellCastContext,
        effects: Vec<EffectSpec>,
    ) {
        match self.geometry() {
            AbilityGeometry::Cone { .. } | AbilityGeometry::Circle { .. } => {
                self.emit_area_impact(params, ctx, effects);
            }
            AbilityGeometry::Projectile { speed } => match self.projectile_target(params, ctx) {
                Some(target) => {
                    ctx.emit_projectile(target, speed, effects, PROJECTILE_HIT_RADIUS);
                    let target_position = ctx
                        .potential_targets
                        .iter()
                        .find(|(entity, _)| *entity == target)
                        .map(|(_, position)| *position)
                        .unwrap_or(ctx.caster_position);
                    ctx.emit_visual(
                        self.id().as_str().to_string(),
                        ctx.caster_position,
                        target_position,
                    );
                }
                None => {
                    let end = ctx.caster_position
                        + flat_direction(ctx.caster_look_direction) * params.range;
                    ctx.emit_visual(self.id().as_str().to_string(), ctx.caster_position, end);
                }
            },
            AbilityGeometry::SelfBuff { .. } => {
                for spec in effects {
                    ctx.emit_effect(ctx.caster, spec);
                }
            }
        }
    }

    /// Manifestazione usata quando lo slot non ha nessuna Essenza incisa
    /// (o quando quella incisa non è conosciuta). Ha un default generico
    /// derivato dalla geometria, per questo NON serve scrivere logica a
    /// mano per ogni `BaseAbility`.
    /// Optional cleanse emitted by a self-targeted ability.
    fn cleanse_effect(&self) -> Option<crate::effects::CleanseEffect> {
        None
    }

    /// Extra effects declared on the gesture itself. Inscribed casts ignore
    /// this and emit [`AbilityBlueprint::payload`] instead.
    fn additional_effects(&self) -> Vec<EffectSpec> {
        Vec::new()
    }

    /// Authoritative cast of an already-resolved blueprint: geometry from the
    /// gesture, payload from the Root Word.
    fn manifest_blueprint(&self, blueprint: &AbilityBlueprint, ctx: &mut SpellCastContext) {
        if let Some(cleanse) = self.cleanse_effect() {
            ctx.emit_cleanse(ctx.caster, cleanse);
            return;
        }
        self.emit_payload_for_geometry(&blueprint.params, ctx, blueprint.payload_effects());
    }

    fn default_manifestation(&self, params: &AbilityParams, ctx: &mut SpellCastContext) {
        let mut blueprint = self.blueprint();
        blueprint.params = *params;
        self.manifest_blueprint(&blueprint, ctx);
    }
}

pub type ArcBaseAbility = Arc<dyn BaseAbility>;

#[cfg_attr(feature = "bevy", derive(bevy_ecs::resource::Resource))]
#[derive(Default)]
pub struct BaseAbilityRegistry {
    abilities: Registry<AbilityId, ArcBaseAbility>,
}

impl BaseAbilityRegistry {
    pub fn register(&mut self, ability: ArcBaseAbility) {
        self.abilities.insert(ability.id(), ability);
    }
    pub fn get(&self, id: &AbilityId) -> Option<ArcBaseAbility> {
        self.abilities.get(id).cloned()
    }
    pub fn contains(&self, id: &AbilityId) -> bool {
        self.abilities.contains(id)
    }
    pub fn len(&self) -> usize {
        self.abilities.len()
    }
    pub fn is_empty(&self) -> bool {
        self.abilities.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyAbility;
    impl BaseAbility for DummyAbility {
        fn id(&self) -> AbilityId {
            AbilityId::new("dummy")
        }
        fn display_name(&self) -> &'static str {
            "Dummy"
        }
        fn tags(&self) -> &'static [AbilityTag] {
            &[AbilityTag::Melee, AbilityTag::Area]
        }
        fn geometry(&self) -> AbilityGeometry {
            AbilityGeometry::Circle { radius: 3.0 }
        }
        fn base_params(&self) -> AbilityParams {
            AbilityParams {
                potency: 100.0,
                area: 3.0,
                range: 0.0,
                cast_time: 0.5,
                cooldown: 5.0,
                mana_cost: 10.0,
            }
        }
        fn animation(&self) -> &'static str {
            "dummy_anim"
        }
        fn impact_vfx(&self) -> &'static str {
            "dummy_vfx"
        }
    }

    #[test]
    fn has_tag_reads_the_static_tag_list() {
        let ability = DummyAbility;
        assert!(ability.has_tag(AbilityTag::Area));
        assert!(!ability.has_tag(AbilityTag::Projectile));
    }

    #[test]
    fn icon_defaults_to_empty_when_not_overridden() {
        assert_eq!(DummyAbility.icon(), "");
    }

    #[test]
    fn register_and_lookup_by_id() {
        let mut registry = BaseAbilityRegistry::default();
        registry.register(Arc::new(DummyAbility));
        assert!(registry.contains(&AbilityId::new("dummy")));
        assert_eq!(registry.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Cast mode derivation tests
    // -----------------------------------------------------------------------

    struct InstantAbility;
    impl BaseAbility for InstantAbility {
        fn id(&self) -> AbilityId {
            AbilityId::new("instant")
        }
        fn display_name(&self) -> &'static str {
            "Instant"
        }
        fn tags(&self) -> &'static [AbilityTag] {
            &[]
        }
        fn geometry(&self) -> AbilityGeometry {
            AbilityGeometry::Circle { radius: 2.0 }
        }
        fn base_params(&self) -> AbilityParams {
            AbilityParams {
                potency: 50.0,
                area: 0.0,
                range: 2.0,
                cast_time: 0.0,
                cooldown: 1.0,
                mana_cost: 5.0,
            }
        }
        fn animation(&self) -> &'static str {
            "swing"
        }
        fn impact_vfx(&self) -> &'static str {
            "slash"
        }
    }

    struct CastTimeAbility;
    impl BaseAbility for CastTimeAbility {
        fn id(&self) -> AbilityId {
            AbilityId::new("cast_time")
        }
        fn display_name(&self) -> &'static str {
            "CastTime"
        }
        fn tags(&self) -> &'static [AbilityTag] {
            &[AbilityTag::Ranged]
        }
        fn geometry(&self) -> AbilityGeometry {
            AbilityGeometry::Projectile { speed: 30.0 }
        }
        fn base_params(&self) -> AbilityParams {
            AbilityParams {
                potency: 80.0,
                area: 0.0,
                range: 20.0,
                cast_time: 0.8,
                cooldown: 6.0,
                mana_cost: 15.0,
            }
        }
        fn animation(&self) -> &'static str {
            "draw"
        }
        fn impact_vfx(&self) -> &'static str {
            "bolt"
        }
    }

    struct ChannelingAbility;
    impl BaseAbility for ChannelingAbility {
        fn id(&self) -> AbilityId {
            AbilityId::new("channeling")
        }
        fn display_name(&self) -> &'static str {
            "Channeling"
        }
        fn tags(&self) -> &'static [AbilityTag] {
            &[AbilityTag::Area, AbilityTag::RepeatCompatible]
        }
        fn geometry(&self) -> AbilityGeometry {
            AbilityGeometry::Circle { radius: 4.0 }
        }
        fn base_params(&self) -> AbilityParams {
            AbilityParams {
                potency: 20.0,
                area: 4.0,
                range: 0.0,
                cast_time: 3.0,
                cooldown: 10.0,
                mana_cost: 30.0,
            }
        }
        fn animation(&self) -> &'static str {
            "channel"
        }
        fn impact_vfx(&self) -> &'static str {
            "beam"
        }
        fn cast_mode(&self) -> AbilityCastMode {
            AbilityCastMode::Channeling {
                tick_interval_seconds: 0.25,
                movement_policy: ChannelMovementPolicy::InterruptOnMove,
            }
        }
    }

    #[test]
    fn zero_cast_time_defaults_to_instant() {
        let ability = InstantAbility;
        assert_eq!(ability.cast_mode(), AbilityCastMode::Instant);
        assert!(ability.cast_mode().is_instant());
        assert!(!ability.cast_mode().holds_still());
    }

    #[test]
    fn positive_cast_time_defaults_to_cast_time() {
        let ability = CastTimeAbility;
        assert!(matches!(ability.cast_mode(), AbilityCastMode::CastTime));
        assert!(!ability.cast_mode().is_instant());
        assert!(ability.cast_mode().holds_still());
    }

    #[test]
    fn wind_up_and_channel_hold_still() {
        assert!(!AbilityCastMode::Instant.holds_still());
        assert!(AbilityCastMode::CastTime.holds_still());
        assert!(AbilityCastMode::Channeling {
            tick_interval_seconds: 0.2,
            movement_policy: ChannelMovementPolicy::InterruptOnMove,
        }
        .holds_still());
        assert!(AbilityCastMode::Channeling {
            tick_interval_seconds: 0.2,
            movement_policy: ChannelMovementPolicy::AllowMovement,
        }
        .holds_still());
    }

    #[test]
    fn explicit_channeling_overrides_cast_time_derivation() {
        let ability = ChannelingAbility;
        match ability.cast_mode() {
            AbilityCastMode::Channeling {
                tick_interval_seconds,
                ..
            } => {
                assert!((tick_interval_seconds - 0.25).abs() < f32::EPSILON);
            }
            other => panic!("expected Channeling, got {:?}", other),
        }
    }

    #[test]
    fn channeling_required_seconds_has_minimum() {
        let mode = AbilityCastMode::Channeling {
            tick_interval_seconds: 0.5,
            movement_policy: ChannelMovementPolicy::AllowMovement,
        };
        // Even with zero cast_time, channeling has a 0.1s minimum window.
        let required = mode.required_seconds(0.0);
        assert!((required - 0.1).abs() < f32::EPSILON);
        // Positive cast_time passes through.
        let required = mode.required_seconds(3.0);
        assert!((required - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn channeling_movement_policy_is_accessible() {
        // Verify that the movement policy can be extracted for storage in
        // CastState.channel_movement_interrupts, which is what the server's
        // advance_casts reads to decide whether to interrupt on movement.
        let interrupt_on_move = AbilityCastMode::Channeling {
            tick_interval_seconds: 0.25,
            movement_policy: ChannelMovementPolicy::InterruptOnMove,
        };
        let allow_movement = AbilityCastMode::Channeling {
            tick_interval_seconds: 0.25,
            movement_policy: ChannelMovementPolicy::AllowMovement,
        };

        // The server should store this as a bool.
        fn should_interrupt_on_move(mode: &AbilityCastMode) -> bool {
            match mode {
                AbilityCastMode::Channeling {
                    movement_policy, ..
                } => {
                    matches!(movement_policy, ChannelMovementPolicy::InterruptOnMove)
                }
                _ => true, // Non-channeling modes always interrupt (CastTime) or don't check (Instant)
            }
        }

        assert!(
            should_interrupt_on_move(&interrupt_on_move),
            "InterruptOnMove should return true"
        );
        assert!(
            !should_interrupt_on_move(&allow_movement),
            "AllowMovement should return false"
        );
    }

    #[test]
    fn clamp_to_range_preserves_target_height_when_in_range() {
        let origin = Vec3::new(0.0, 10.0, 0.0);
        let target = Vec3::new(3.0, 15.0, 4.0); // distance = 5.0
        let clamped = clamp_to_range(origin, target, 10.0);
        assert_eq!(clamped, target);
        assert_eq!(clamped.y, 15.0);
    }

    #[test]
    fn clamp_to_range_interpolates_height_when_clamped() {
        let origin = Vec3::new(0.0, 10.0, 0.0);
        let target = Vec3::new(0.0, 20.0, 20.0); // distance = 20.0
        let clamped = clamp_to_range(origin, target, 10.0); // halfway
        assert!((clamped.x - 0.0).abs() < 1e-4);
        assert!((clamped.y - 15.0).abs() < 1e-4);
        assert!((clamped.z - 10.0).abs() < 1e-4);
    }

    #[test]
    fn circle_impact_center_preserves_target_height_on_mountain() {
        use crate::stats::components::CombatStats;
        let ability = InstantAbility;
        let combat = CombatStats {
            attack_power: 10.0,
            armor: 0.0,
            threat_generation: 1.0,
        };
        let caster_pos = Vec3::new(0.0, 12.0, 0.0);
        let target_on_mountain = Vec3::new(1.0, 14.0, 1.0);
        let ctx = SpellCastContext::new(
            EntityId::new(1),
            caster_pos,
            &combat,
            Vec3::Z,
            Some(target_on_mountain),
            None,
            &[],
        );
        let center = ability.impact_center(&ability.base_params(), &ctx);
        assert_eq!(center, target_on_mountain);
        assert_eq!(center.y, 14.0);
    }

    #[test]
    fn a_heal_wave_includes_the_caster() {
        use crate::abilities::blueprint::ManifestationPayload;
        use crate::spells::context::AoeTargeting;
        use crate::stats::components::CombatStats;

        let ability = InstantAbility;
        let mut blueprint = ability.blueprint();
        blueprint.payload = ManifestationPayload::heal([]);
        let combat = CombatStats {
            attack_power: 10.0,
            armor: 0.0,
            threat_generation: 1.0,
        };
        let mut ctx = SpellCastContext::new(
            EntityId::new(1),
            Vec3::ZERO,
            &combat,
            Vec3::Z,
            None,
            None,
            &[],
        );
        ability.manifest_blueprint(&blueprint, &mut ctx);
        assert_eq!(ctx.pending_aoes.len(), 1);
        assert_eq!(ctx.pending_aoes[0].targeting, AoeTargeting::Everyone);
        assert!(matches!(
            ctx.pending_aoes[0].effects[0],
            crate::effects::EffectSpec::Heal(_)
        ));
    }

    #[test]
    fn a_damage_wave_excludes_the_caster() {
        use crate::spells::context::AoeTargeting;
        use crate::stats::components::CombatStats;

        let ability = InstantAbility;
        let combat = CombatStats {
            attack_power: 10.0,
            armor: 0.0,
            threat_generation: 1.0,
        };
        let mut ctx = SpellCastContext::new(
            EntityId::new(1),
            Vec3::ZERO,
            &combat,
            Vec3::Z,
            None,
            None,
            &[],
        );
        ability.default_manifestation(&ability.base_params(), &mut ctx);
        assert_eq!(ctx.pending_aoes.len(), 1);
        assert_eq!(ctx.pending_aoes[0].targeting, AoeTargeting::ExcludeCaster);
    }

    #[test]
    fn cones_and_self_buffs_are_self_centered() {
        assert_eq!(
            AbilityGeometry::Cone {
                radius: 5.0,
                angle_deg: 85.0
            }
            .targeting_mode(),
            TargetingMode::SelfCentered
        );
        assert_eq!(
            AbilityGeometry::SelfBuff {
                duration_seconds: 1.0
            }
            .targeting_mode(),
            TargetingMode::SelfCentered
        );
    }

    #[test]
    fn circles_are_ground_aoe_and_projectiles_are_directional() {
        assert_eq!(
            AbilityGeometry::Circle { radius: 5.5 }.targeting_mode(),
            TargetingMode::GroundAoe
        );
        assert_eq!(
            AbilityGeometry::Projectile { speed: 24.0 }.targeting_mode(),
            TargetingMode::DirectionalLine
        );
    }

    #[test]
    fn only_projectiles_send_a_selected_entity() {
        let selected = Some(7);
        assert_eq!(
            AbilityGeometry::Projectile { speed: 24.0 }.selected_entity_payload(selected),
            selected
        );
        assert_eq!(
            AbilityGeometry::Cone {
                radius: 5.0,
                angle_deg: 85.0
            }
            .selected_entity_payload(selected),
            None
        );
        assert_eq!(
            AbilityGeometry::Circle { radius: 5.5 }.selected_entity_payload(selected),
            None
        );
        assert_eq!(
            AbilityGeometry::SelfBuff {
                duration_seconds: 1.0
            }
            .selected_entity_payload(selected),
            None
        );
    }
}
