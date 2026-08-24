//! Client-only modular ability hotbar.
//!
//! Data-driven UI showing all active ability slots across weapon, helmet,
//! chestplate, and shoes as a bottom-centered row of circular rings.
//! Entries are derived from equipped items via `resolve_active_ability`.

use bevy::prelude::*;
use std::collections::HashMap;
use std::f32::consts::TAU;

use bevymmo_client::local_player::LocalPlayer;
use bevymmo_client::server_feed::SpellCooldownState;
use bevymmo_client::user_settings::{GameSettingsResource, KeyAction};
use bevymmo_gameplay::abilities::{
    resolve_active_ability, AbilityId, AbilitySlot, BaseAbilityRegistry,
};
use bevymmo_gameplay::items::components::Equipment;
use bevymmo_gameplay::items::registry::ItemRegistry;
use bevymmo_network::network::protocol::NetworkEntityId;

use crate::game_state::{in_gameplay, not_in_gameplay};
use crate::spells::input::WEAPON_HUD_BINDINGS;
use crate::ui::theme::UiTheme;

/// Ornate frame drawn on top of every hotbar slot.
const SPELL_BORDER_PATH: &str = "ui/spells/spell_border.png";
/// `{ability_id}` is substituted — drop a PNG at this path to show an icon
/// when the ability itself does not declare `icon`.
const SPELL_ICON_PATH: &str = "abilities/icons/{ability_id}.png";
const SPELL_SLOT_SIZE: f32 = 76.0;
/// Matches the inner hole of `spell_border.png` (~12.8% of the square).
/// Slightly under that so the icon tucks under the ring; the border draws on top.
const SPELL_ICON_INSET: f32 = 8.0;
const SPELL_SLOT_GAP: f32 = 10.0;
const SPELL_CENTER_FONT_SIZE: f32 = 14.0;
const SPELL_KEY_FONT_SIZE: f32 = 12.0;
const SPELL_ICON_READY: Color = Color::WHITE;
const SPELL_ICON_COOLDOWN: Color = Color::srgb(0.42, 0.44, 0.52);
const SPELL_ICON_UNAFFORDABLE: Color = Color::srgb(0.22, 0.36, 0.62);
const SPELL_CLOCK_DARK: Color = Color::srgba(0.02, 0.03, 0.07, 0.78);

/// What a HUD cooldown countdown is keyed by.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HudCooldownKey {
    Ability(AbilityId),
}

#[derive(Message, Debug, Clone, PartialEq)]
pub struct SpellHudCooldownStarted {
    pub key: HudCooldownKey,
    pub cooldown_seconds: f32,
}

#[derive(Clone, Copy, Debug)]
struct HudCooldown {
    remaining: f32,
    total: f32,
}

impl HudCooldown {
    fn ratio(&self) -> f32 {
        if self.total <= 0.0 {
            0.0
        } else {
            (self.remaining / self.total).clamp(0.0, 1.0)
        }
    }
}

#[derive(Resource, Default)]
pub struct SpellHudState {
    cooldowns: HashMap<HudCooldownKey, HudCooldown>,
}

/// Describes one hotbar column read from equipment + settings.
#[derive(Component, Clone)]
struct SpellHudEntry {
    /// What this entry's countdown is keyed by — `None` for empty slots.
    cooldown_key: Option<HudCooldownKey>,
    display_name: String,
    key_label: String,
    /// Base ability mana cost. Empty slots are 0 (always affordable).
    mana_cost: f32,
    /// Bevy asset path for the slot icon. `None` for empty slots or abilities
    /// that never selected an icon.
    icon_path: Option<String>,
}

#[derive(Component)]
struct SpellHudIcon;

#[derive(Component)]
struct SpellHudClock;

#[derive(Resource, Default)]
struct SpellHudLayoutState {
    initialized: bool,
    /// `(ability_slot, ability_id, key_label, display_name, icon_path)` —
    /// rebuilds when any of these change (weapon swap, gear change, Incisione
    /// rewrite, icon reassignment).
    signature: Vec<(
        AbilitySlot,
        Option<AbilityId>,
        String,
        String,
        Option<String>,
    )>,
}

impl SpellHudState {
    pub fn is_on_cooldown(&self, key: &HudCooldownKey) -> bool {
        self.cooldowns
            .get(key)
            .is_some_and(|cooldown| cooldown.remaining > 0.0)
    }

    pub fn ability_on_cooldown(&self, id: &AbilityId) -> bool {
        self.is_on_cooldown(&HudCooldownKey::Ability(id.clone()))
    }

    fn remaining(&self, key: &HudCooldownKey) -> f32 {
        self.cooldowns
            .get(key)
            .map(|cooldown| cooldown.remaining)
            .unwrap_or(0.0)
    }

    fn ratio(&self, key: &HudCooldownKey) -> f32 {
        self.cooldowns
            .get(key)
            .map(HudCooldown::ratio)
            .unwrap_or(0.0)
    }

    fn begin(&mut self, key: HudCooldownKey, seconds: f32) {
        let seconds = seconds.max(0.0);
        self.cooldowns.insert(
            key,
            HudCooldown {
                remaining: seconds,
                total: seconds.max(0.001),
            },
        );
    }
}

fn spell_icon_path(id: &AbilityId) -> String {
    SPELL_ICON_PATH.replace("{ability_id}", id.as_str())
}

fn resolved_icon_path(declared: &str, id: &AbilityId) -> String {
    if declared.is_empty() {
        spell_icon_path(id)
    } else {
        declared.to_string()
    }
}

#[derive(Component)]
struct SpellHudRoot;

pub fn spell_hud_systems(app: &mut App) {
    app.init_resource::<SpellHudState>();
    app.init_resource::<SpellHudLayoutState>();
    app.add_message::<SpellHudCooldownStarted>();
    app.add_message::<SpellCooldownState>();
    app.add_systems(Startup, setup_spell_hud);
    app.add_systems(
        Update,
        (sync_spell_hud, adopt_server_cooldowns, update_spell_hud)
            .chain()
            .run_if(in_gameplay),
    );
    app.add_systems(Update, hide_spell_hud.run_if(not_in_gameplay));
}

fn setup_spell_hud(mut commands: Commands, _theme: Res<UiTheme>, asset_server: Res<AssetServer>) {
    let _border: Handle<Image> = asset_server.load(SPELL_BORDER_PATH);
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(86.0),
            width: Val::Percent(100.0),
            padding: UiRect::all(Val::Px(8.0)),
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::FlexEnd,
            column_gap: Val::Px(SPELL_SLOT_GAP),
            ..default()
        },
        BackgroundColor(Color::NONE),
        SpellHudRoot,
    ));
}

/// A single logical hotbar slot: which piece of equipment and which ability
/// slot within it, plus the `KeyAction` used to read the current binding label.
struct HotbarSlotDef {
    action: KeyAction,
    slot: AbilitySlot,
    /// Extracts the relevant `ItemInstance` from `Equipment`.
    equip_fn: fn(&Equipment) -> &Option<bevymmo_gameplay::items::instance::ItemInstance>,
}

/// All 6 hotbar columns in display order: three weapon slots and one active
/// ability for each armor piece. Weapon actions are [`WEAPON_HUD_BINDINGS`]
/// so the printed keys are the ones `cast_abilities_on_key` actually reads.
const HOTBAR_SLOTS: [HotbarSlotDef; 6] = [
    HotbarSlotDef {
        action: WEAPON_HUD_BINDINGS[0].0,
        slot: WEAPON_HUD_BINDINGS[0].1,
        equip_fn: |e| &e.weapon,
    },
    HotbarSlotDef {
        action: WEAPON_HUD_BINDINGS[1].0,
        slot: WEAPON_HUD_BINDINGS[1].1,
        equip_fn: |e| &e.weapon,
    },
    HotbarSlotDef {
        action: WEAPON_HUD_BINDINGS[2].0,
        slot: WEAPON_HUD_BINDINGS[2].1,
        equip_fn: |e| &e.weapon,
    },
    HotbarSlotDef {
        action: KeyAction::CastHelmet,
        slot: AbilitySlot::Primary,
        equip_fn: |e| &e.helmet,
    },
    HotbarSlotDef {
        action: KeyAction::CastChestplate,
        slot: AbilitySlot::Primary,
        equip_fn: |e| &e.armor,
    },
    HotbarSlotDef {
        action: KeyAction::CastBoots,
        slot: AbilitySlot::Primary,
        equip_fn: |e| &e.shoes,
    },
];

/// Resolves the active ability for one equipped item + ability-slot pair.
///
/// Returns `(AbilityId, display_name, mana_cost, icon_path)` if the item exists,
/// has an ability loadout, and a valid ability can be resolved through its
/// selection.
fn resolve_equipment_entry(
    equipped: &Option<bevymmo_gameplay::items::instance::ItemInstance>,
    slot: AbilitySlot,
    item_registry: &ItemRegistry,
    ability_registry: &BaseAbilityRegistry,
) -> Option<(AbilityId, String, f32, String)> {
    let instance = equipped.as_ref()?;
    let item = item_registry.get(&instance.item_id)?;
    let loadout = item.ability_loadout()?;
    let ability_id = if matches!(
        item.config().category,
        bevymmo_gameplay::items::definition::ItemCategory::Armor
            | bevymmo_gameplay::items::definition::ItemCategory::Accessory
    ) {
        bevymmo_gameplay::abilities::resolve_armor_ability(loadout, &instance.ability_selection)?
    } else {
        resolve_active_ability(slot, loadout, &instance.ability_selection)?
    };
    let ability = ability_registry.get(ability_id)?;
    Some((
        ability_id.clone(),
        ability.display_name().to_string(),
        ability.base_params().mana_cost,
        resolved_icon_path(ability.icon(), ability_id),
    ))
}

#[allow(clippy::too_many_arguments)]
fn sync_spell_hud(
    mut commands: Commands,
    theme: Res<UiTheme>,
    asset_server: Res<AssetServer>,
    settings: Res<GameSettingsResource>,
    mut layout_state: ResMut<SpellHudLayoutState>,
    player_query: Query<&Equipment, With<LocalPlayer>>,
    item_registry: Res<ItemRegistry>,
    ability_registry: Res<BaseAbilityRegistry>,
    hud_query: Query<Entity, With<SpellHudRoot>>,
) {
    let Ok(equipment) = player_query.single() else {
        return;
    };
    let Ok(root_entity) = hud_query.single() else {
        return;
    };

    let mut signature = Vec::new();
    let mut entries = Vec::new();

    for def in &HOTBAR_SLOTS {
        let resolved = resolve_equipment_entry(
            (def.equip_fn)(equipment),
            def.slot,
            &item_registry,
            &ability_registry,
        );

        let (cooldown_key, display_name, mana_cost, icon_path) = match &resolved {
            Some((id, name, cost, icon)) => (
                Some(HudCooldownKey::Ability(id.clone())),
                name.clone(),
                *cost,
                Some(icon.clone()),
            ),
            None => (None, "Empty".to_string(), 0.0, None),
        };
        let key_label = display_key_label(&settings.0.keybinds.get(def.action).label());

        signature.push((
            def.slot,
            resolved.as_ref().map(|(id, _, _, _)| id.clone()),
            key_label.clone(),
            display_name.clone(),
            icon_path.clone(),
        ));
        entries.push(SpellHudEntry {
            cooldown_key,
            display_name,
            key_label,
            mana_cost,
            icon_path,
        });
    }

    if layout_state.initialized && layout_state.signature == signature {
        return;
    }
    layout_state.initialized = true;
    layout_state.signature = signature;

    commands.entity(root_entity).despawn_related::<Children>();

    let border = asset_server.load(SPELL_BORDER_PATH);
    commands.entity(root_entity).with_children(|parent| {
        for entry in entries {
            spawn_hotbar_slot(parent, &theme, &asset_server, &border, entry);
        }
    });
}

fn spawn_hotbar_slot(
    parent: &mut ChildSpawnerCommands,
    theme: &UiTheme,
    asset_server: &AssetServer,
    border: &Handle<Image>,
    entry: SpellHudEntry,
) {
    let icon = entry
        .icon_path
        .as_ref()
        .map(|path| asset_server.load(path.clone()));

    parent
        .spawn((
            Node {
                width: Val::Px(SPELL_SLOT_SIZE),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(2.0),
                ..default()
            },
            BackgroundColor(Color::NONE),
            entry.clone(),
        ))
        .with_children(|slot| {
            slot.spawn((
                Node {
                    width: Val::Px(SPELL_SLOT_SIZE),
                    height: Val::Px(SPELL_SLOT_SIZE),
                    position_type: PositionType::Relative,
                    ..default()
                },
                BackgroundColor(Color::NONE),
            ))
            .with_children(|face| {
                if let Some(icon) = icon {
                    face.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(SPELL_ICON_INSET),
                            right: Val::Px(SPELL_ICON_INSET),
                            top: Val::Px(SPELL_ICON_INSET),
                            bottom: Val::Px(SPELL_ICON_INSET),
                            border_radius: BorderRadius::all(Val::Percent(50.0)),
                            overflow: Overflow::clip(),
                            ..default()
                        },
                        ImageNode {
                            image: icon,
                            color: SPELL_ICON_READY,
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                        SpellHudIcon,
                        entry.clone(),
                    ));
                }

                face.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(SPELL_ICON_INSET),
                        right: Val::Px(SPELL_ICON_INSET),
                        top: Val::Px(SPELL_ICON_INSET),
                        bottom: Val::Px(SPELL_ICON_INSET),
                        border_radius: BorderRadius::all(Val::Percent(50.0)),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    BackgroundGradient(Vec::new()),
                    Visibility::Hidden,
                    SpellHudClock,
                    entry.clone(),
                ));

                face.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(0.0),
                        right: Val::Px(0.0),
                        top: Val::Px(0.0),
                        bottom: Val::Px(0.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|label| {
                    label.spawn((
                        Text(String::new()),
                        TextFont {
                            font_size: FontSize::Px(SPELL_CENTER_FONT_SIZE),
                            ..default()
                        },
                        TextColor(theme.text_color),
                        TextLayout::justify(Justify::Center),
                        Name::new("hotbar-cooldown"),
                        Visibility::Hidden,
                        entry.clone(),
                    ));
                });

                face.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(0.0),
                        right: Val::Px(0.0),
                        top: Val::Px(0.0),
                        bottom: Val::Px(0.0),
                        ..default()
                    },
                    ImageNode::new(border.clone()).with_mode(NodeImageMode::Stretch),
                    Pickable::IGNORE,
                ));
            });

            slot.spawn((
                Text(entry.key_label.clone()),
                TextFont {
                    font_size: FontSize::Px(SPELL_KEY_FONT_SIZE),
                    ..default()
                },
                TextColor(theme.muted_text_color),
                TextLayout::justify(Justify::Center),
            ));
        });
}

/// Overwrites local cooldown guesses with authoritative server values.
fn adopt_server_cooldowns(
    mut state: ResMut<SpellHudState>,
    mut incoming: MessageReader<SpellCooldownState>,
    local_player: Query<&NetworkEntityId, With<LocalPlayer>>,
) {
    let Ok(local) = local_player.single() else {
        incoming.clear();
        return;
    };

    for message in incoming.read() {
        if message.entity_id != local.0 {
            continue;
        }
        let key = HudCooldownKey::Ability(AbilityId::new(message.ability_id.clone()));
        if message.is_ready() {
            state.cooldowns.remove(&key);
        } else {
            let remaining = message.remaining_seconds.max(0.0);
            let total = if message.duration_seconds > 0.0 {
                message.duration_seconds
            } else {
                remaining.max(0.001)
            };
            state
                .cooldowns
                .insert(key, HudCooldown { remaining, total });
        }
    }
}

fn update_spell_hud(
    time: Res<Time>,
    mut elapsed_since_label_update: Local<f32>,
    mut state: ResMut<SpellHudState>,
    mut cooldown_started: MessageReader<SpellHudCooldownStarted>,
    mut roots: Query<&mut Node, With<SpellHudRoot>>,
    mut texts: Query<(&SpellHudEntry, &mut Text, &mut Visibility), With<Name>>,
    mut icons: Query<(&SpellHudEntry, &mut ImageNode), With<SpellHudIcon>>,
    mut clocks: Query<
        (&SpellHudEntry, &mut BackgroundGradient, &mut Visibility),
        (With<SpellHudClock>, Without<Name>),
    >,
    local_vitals: Query<&bevymmo_gameplay::stats::components::VitalStats, With<LocalPlayer>>,
) {
    let mut has_new_cooldown = false;
    for message in cooldown_started.read() {
        has_new_cooldown = true;
        state.begin(message.key.clone(), message.cooldown_seconds);
    }

    let delta = time.delta_secs();
    state.cooldowns.retain(|_, cooldown| {
        cooldown.remaining = (cooldown.remaining - delta).max(0.0);
        cooldown.remaining > 0.0
    });

    if let Ok(mut root) = roots.single_mut() {
        root.display = Display::Flex;
    }

    *elapsed_since_label_update += delta;
    let should_update_labels = has_new_cooldown || *elapsed_since_label_update >= 0.05;
    if !should_update_labels {
        return;
    }
    *elapsed_since_label_update = 0.0;

    for (entry, mut text, mut vis) in texts.iter_mut() {
        let remaining = entry
            .cooldown_key
            .as_ref()
            .map(|key| state.remaining(key))
            .unwrap_or(0.0);
        let next = format_cooldown_text(entry, remaining);
        if text.0 != next {
            text.0 = next;
        }
        *vis = if remaining > 0.0 && entry.cooldown_key.is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }

    let current_mana = local_vitals
        .single()
        .map(|vital| vital.current_mana)
        .unwrap_or(f32::MAX);
    for (entry, mut image) in icons.iter_mut() {
        let cooling = entry
            .cooldown_key
            .as_ref()
            .is_some_and(|key| state.remaining(key) > 0.0);
        let unaffordable =
            !bevymmo_gameplay::stats::formulas::can_afford_mana(current_mana, entry.mana_cost);
        image.color = spell_icon_color(cooling, unaffordable);
    }

    for (entry, mut gradient, mut vis) in clocks.iter_mut() {
        let ratio = entry
            .cooldown_key
            .as_ref()
            .map(|key| state.ratio(key))
            .unwrap_or(0.0);
        if ratio <= 0.0 {
            *vis = Visibility::Hidden;
            gradient.0.clear();
            continue;
        }
        *vis = Visibility::Inherited;
        *gradient = clock_overlay(ratio);
    }
}

fn hide_spell_hud(mut roots: Query<&mut Node, With<SpellHudRoot>>) {
    if let Ok(mut root) = roots.single_mut() {
        root.display = Display::None;
    }
}

/// Formats the caption printed on a keybind chip (`Digit1` → `1`).
fn spell_icon_color(cooling: bool, unaffordable: bool) -> Color {
    if cooling {
        SPELL_ICON_COOLDOWN
    } else if unaffordable {
        SPELL_ICON_UNAFFORDABLE
    } else {
        SPELL_ICON_READY
    }
}

fn display_key_label(label: &str) -> String {
    label
        .strip_prefix("Digit")
        .or_else(|| label.strip_prefix("Key"))
        .unwrap_or(label)
        .to_string()
}

fn format_cooldown_text(entry: &SpellHudEntry, remaining_seconds: f32) -> String {
    if entry.cooldown_key.is_none() || entry.display_name == "Empty" {
        return String::new();
    }
    if remaining_seconds > 0.0 {
        format!("{remaining_seconds:.1}")
    } else {
        String::new()
    }
}

/// Clock wipe: remaining `ratio` (1 = just cast, 0 = ready).
///
/// Bevy's conic `0` is already 12 o'clock and increases clockwise
/// (`atan2(-x, y)` in UI space). Elapsed time is revealed from 12 clockwise;
/// the leftover pie stays dark.
fn clock_overlay(ratio: f32) -> BackgroundGradient {
    let remaining = ratio.clamp(0.0, 1.0);
    let elapsed = (1.0 - remaining) * TAU;
    BackgroundGradient::from(
        ConicGradient::new(
            UiPosition::CENTER,
            vec![
                AngularColorStop::new(Color::NONE, 0.0),
                AngularColorStop::new(Color::NONE, elapsed),
                AngularColorStop::new(SPELL_CLOCK_DARK, elapsed),
                AngularColorStop::new(SPELL_CLOCK_DARK, TAU),
            ],
        )
        .with_start(0.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ability_entry(id: &'static str, name: &str, key: &str) -> SpellHudEntry {
        SpellHudEntry {
            cooldown_key: Some(HudCooldownKey::Ability(AbilityId::new(id))),
            display_name: name.to_string(),
            key_label: key.to_string(),
            mana_cost: 0.0,
            icon_path: Some(resolved_icon_path("", &AbilityId::new(id))),
        }
    }

    fn empty_entry(key: &str) -> SpellHudEntry {
        SpellHudEntry {
            cooldown_key: None,
            display_name: "Empty".to_string(),
            key_label: key.to_string(),
            mana_cost: 0.0,
            icon_path: None,
        }
    }

    #[test]
    fn technical_key_names_become_player_facing_labels() {
        assert_eq!(display_key_label("Digit1"), "1");
        assert_eq!(display_key_label("KeyD"), "D");
        assert_eq!(display_key_label("PageUp"), "PageUp");
    }

    #[test]
    fn cooldown_text_formats_all_states() {
        let entry = ability_entry("bolt", "Arcane Bolt", "1");
        assert_eq!(format_cooldown_text(&entry, 0.0), "");
        assert_eq!(format_cooldown_text(&entry, 2.5), "2.5");
        assert_eq!(format_cooldown_text(&entry, 0.09), "0.1");
        assert_eq!(format_cooldown_text(&empty_entry("1"), 0.0), "");
    }

    #[test]
    fn empty_slot_has_no_ready_label() {
        let entry = empty_entry("D");
        assert_eq!(format_cooldown_text(&entry, 0.0), "");
        assert_eq!(format_cooldown_text(&entry, 99.0), "");
    }

    #[test]
    fn spell_icon_path_uses_the_ability_id() {
        assert_eq!(
            spell_icon_path(&AbilityId::new("cleave")),
            "abilities/icons/cleave.png"
        );
        assert_eq!(
            spell_icon_path(&AbilityId::new("lunge")),
            "abilities/icons/lunge.png"
        );
    }

    #[test]
    fn declared_icon_wins_over_the_id_convention() {
        assert_eq!(
            resolved_icon_path("items/icons/sword.png", &AbilityId::new("cleave")),
            "items/icons/sword.png"
        );
        assert_eq!(
            resolved_icon_path("", &AbilityId::new("cleave")),
            "abilities/icons/cleave.png"
        );
    }

    #[test]
    fn clock_overlay_covers_the_full_face_at_cast() {
        let gradient = clock_overlay(1.0);
        assert_eq!(gradient.0.len(), 1);
        let Gradient::Conic(conic) = &gradient.0[0] else {
            panic!("clock overlay is a conic gradient");
        };
        assert!(
            conic.start.abs() < 0.001,
            "Bevy conic 0 is 12 o'clock; do not offset to 9 o'clock"
        );
        assert_eq!(conic.stops.len(), 4);
    }

    #[test]
    fn clock_overlay_reveals_clockwise_from_noon() {
        let gradient = clock_overlay(0.5);
        let Gradient::Conic(conic) = &gradient.0[0] else {
            panic!("clock overlay is a conic gradient");
        };
        // Half elapsed: clear from 12 to 6, dark from 6 to 12.
        assert!((conic.stops[1].angle.unwrap() - std::f32::consts::PI).abs() < 0.001);
        assert_eq!(conic.stops[0].color, Color::NONE);
        assert_eq!(conic.stops[2].color, SPELL_CLOCK_DARK);
        const { assert!(SPELL_ICON_INSET < 10.0) };
    }

    #[test]
    fn nine_hotbar_slots_defined() {
        assert_eq!(HOTBAR_SLOTS.len(), 6);
        // Weapon 3 + one active ability per armor piece.
        assert_eq!(HOTBAR_SLOTS[0].action, KeyAction::CastPrimary);
        assert_eq!(HOTBAR_SLOTS[2].action, KeyAction::CastUltimate);
        assert_eq!(HOTBAR_SLOTS[3].action, KeyAction::CastHelmet);
        assert_eq!(HOTBAR_SLOTS[4].action, KeyAction::CastChestplate);
        assert_eq!(HOTBAR_SLOTS[5].action, KeyAction::CastBoots);
    }

    #[test]
    fn weapon_hud_keys_match_the_cast_system() {
        // HUD advertising Digit1 while input only listens to KeyQ is how
        // Charge casts started and never released. These two tables must
        // name the same actions for the same slots.
        for (i, (action, slot)) in WEAPON_HUD_BINDINGS.iter().enumerate() {
            assert_eq!(HOTBAR_SLOTS[i].action, *action);
            assert_eq!(HOTBAR_SLOTS[i].slot, *slot);
        }
    }

    #[test]
    fn ability_cooldown_tracking_works() {
        let mut state = SpellHudState::default();
        let id = AbilityId::new("bolt");
        assert!(!state.ability_on_cooldown(&id));

        state.begin(HudCooldownKey::Ability(id.clone()), 3.0);
        assert!(state.ability_on_cooldown(&id));
        assert!((state.ratio(&HudCooldownKey::Ability(id.clone())) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn unaffordable_icon_is_distinct_from_ready_and_cooldown() {
        assert_eq!(spell_icon_color(false, false), SPELL_ICON_READY);
        assert_eq!(spell_icon_color(true, false), SPELL_ICON_COOLDOWN);
        assert_eq!(spell_icon_color(false, true), SPELL_ICON_UNAFFORDABLE);
        assert_eq!(
            spell_icon_color(true, true),
            SPELL_ICON_COOLDOWN,
            "cooldown wins over mana so the clock remains readable"
        );
        assert_ne!(SPELL_ICON_UNAFFORDABLE, SPELL_ICON_COOLDOWN);
        assert_ne!(SPELL_ICON_UNAFFORDABLE, SPELL_ICON_READY);
    }

    #[test]
    fn hud_cooldown_key_equality() {
        let a = HudCooldownKey::Ability(AbilityId::new("fireball"));
        let b = HudCooldownKey::Ability(AbilityId::new("fireball"));
        let c = HudCooldownKey::Ability(AbilityId::new("icebolt"));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
