//! The account's own characters, listed on the character-select screen.
//!
//! Embedded inside `crate::ui::main_menu`'s character-select screen, above
//! the "create a new character" field — [`crate::ui::main_menu::setup_main_menu`]
//! calls [`spawn_roster_list`] to place it. A separate module because the
//! list rebuilds reactively off [`CharacterRoster`], which changes on its own
//! schedule (row insert/remove) independent of the rest of the screen.
//!
//! Rows are selectable; [`SelectedRosterEntry`] is what ENTER WORLD reads.
//! Per-row Play is intentionally absent so there is a single connect path.

use bevy::prelude::*;

use bevymmo_client::stdb::{CharacterRoster, RosterCharacter};

use crate::game_state::{DeleteCharacterRequest, MAX_CHARACTERS_PER_ACCOUNT};
use crate::ui::theme::UiTheme;

const TITLE_TEXT: &str = "CHARACTERS";
const TITLE_COLOR: Color = Color::srgb(0.85, 0.72, 0.42);
const TITLE_FONT_SIZE: f32 = 24.0;
const NAME_FONT_SIZE: f32 = 20.0;
const CREATE_LABEL: &str = "Create New Character";

/// Full-width roster row. Inner 52px portrait + 16px padding on each side.
const ROW_HEIGHT: f32 = 84.0;
const ROW_PAD: f32 = 16.0;
const ROW_RADIUS: f32 = 8.0;
const ROW_BORDER_WIDTH: f32 = 1.0;
const PORTRAIT_SIZE: f32 = 52.0;
const ROW_GAP: f32 = 10.0;

const ROW_BG: Color = Color::srgba(0.05, 0.07, 0.10, 0.94);
const ROW_BG_SELECTED: Color = Color::srgba(0.08, 0.13, 0.20, 0.96);
const ROW_BORDER: Color = Color::srgb(0.62, 0.50, 0.28);
const ROW_BORDER_HOVER: Color = Color::srgb(0.78, 0.64, 0.36);
const ROW_BORDER_SELECTED: Color = Color::srgb(0.32, 0.64, 0.92);
const PORTRAIT_BG: Color = Color::srgb(0.07, 0.08, 0.11);
const PORTRAIT_RING_PATH: &str = "ui/spells/spell_ring_silver.png";
const DELETE_BG: Color = Color::srgb(0.12, 0.10, 0.12);
const DELETE_BG_HOVER: Color = Color::srgb(0.28, 0.12, 0.12);
const DELETE_BG_PRESSED: Color = Color::srgb(0.18, 0.08, 0.08);
const DELETE_BUTTON_SIZE: f32 = 32.0;
const DELETE_CONFIRM_WIDTH: f32 = 88.0;

/// Which roster row ENTER WORLD should join (or the create-name path).
#[derive(Resource, Clone, Debug, PartialEq, Eq, Default)]
pub enum SelectedRosterEntry {
    #[default]
    None,
    Existing(String),
    Create,
}

/// Marker: the column that holds the title plus one row per character.
/// Rebuilt whole whenever [`CharacterRoster`] changes — the roster is at most
/// [`crate::game_state::MAX_CHARACTERS_PER_ACCOUNT`] entries, so a full
/// rebuild is cheaper than diffing.
#[derive(Component)]
pub struct RosterList;

#[derive(Component)]
struct RosterTitle;

/// Click target for a roster row. Carries enough to update
/// [`SelectedRosterEntry`] without looking the character up again.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
enum RosterEntryKind {
    Existing(String),
    Create,
}

/// "Delete" button for one roster row, in one of two states. Starts `Idle`;
/// a first click flips it to `Confirming` and changes its label instead of
/// deleting immediately — an accidental click must not destroy a character.
/// A second click while `Confirming` sends the request. Rebuilding the list
/// (e.g. because a *different* row changed) naturally resets any row still
/// `Idle`, since every row is despawned and respawned from scratch.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum RosterDeleteButton {
    Idle(bevymmo_client::stdb::Uuid),
    Confirming(bevymmo_client::stdb::Uuid),
}

/// Spawns the (initially empty) roster column, attached to `parent`.
/// [`rebuild_roster_list`] fills it in on the next frame that
/// [`CharacterRoster`] is populated.
pub fn spawn_roster_list(commands: &mut Commands, parent: Entity) -> Entity {
    let list = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Stretch,
                row_gap: Val::Px(ROW_GAP),
                flex_shrink: 0.0,
                ..default()
            },
            RosterList,
        ))
        .id();
    commands.entity(parent).add_child(list);
    list
}

pub struct CharacterRosterPlugin;

impl Plugin for CharacterRosterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SelectedRosterEntry>();
        app.add_systems(
            Update,
            (
                rebuild_roster_list,
                handle_roster_select,
                handle_roster_delete,
                apply_roster_row_visuals,
                tint_roster_delete_buttons,
            ),
        );
    }
}

fn rebuild_roster_list(
    mut commands: Commands,
    theme: Res<UiTheme>,
    asset_server: Res<AssetServer>,
    roster: Res<CharacterRoster>,
    mut selected: ResMut<SelectedRosterEntry>,
    list_query: Query<Entity, With<RosterList>>,
) {
    if !roster.is_changed() {
        return;
    }
    let Ok(list) = list_query.single() else {
        return;
    };

    retain_selection(&roster, &mut selected);

    commands.entity(list).despawn_related::<Children>();

    let mut characters: Vec<&RosterCharacter> = roster.iter().collect();
    characters.sort_by(|a, b| a.display_name.cmp(&b.display_name));

    spawn_roster_title(&mut commands, list);

    for character in &characters {
        spawn_roster_row(
            &mut commands,
            list,
            character,
            &selected,
            &theme,
            &asset_server,
        );
    }

    if roster.len() < MAX_CHARACTERS_PER_ACCOUNT {
        spawn_create_row(&mut commands, list, &selected, &theme, &asset_server);
    }
}

fn retain_selection(roster: &CharacterRoster, selected: &mut SelectedRosterEntry) {
    match selected {
        SelectedRosterEntry::Existing(name)
            if roster
                .iter()
                .any(|character| character.display_name == *name) => {}
        SelectedRosterEntry::Existing(_) => {
            *selected = SelectedRosterEntry::None;
        }
        SelectedRosterEntry::Create if roster.len() >= MAX_CHARACTERS_PER_ACCOUNT => {
            *selected = SelectedRosterEntry::None;
        }
        SelectedRosterEntry::Create | SelectedRosterEntry::None => {}
    }
}

fn spawn_roster_title(commands: &mut Commands, parent: Entity) {
    let title = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::new(Val::Px(0.0), Val::Px(0.0), Val::Px(4.0), Val::Px(8.0)),
                flex_shrink: 0.0,
                ..default()
            },
            RosterTitle,
        ))
        .id();
    commands.entity(parent).add_child(title);
    commands.entity(title).with_children(|title| {
        title.spawn((
            Text::new(TITLE_TEXT),
            TextFont {
                font_size: FontSize::Px(TITLE_FONT_SIZE),
                ..default()
            },
            TextColor(TITLE_COLOR),
        ));
    });
}

fn spawn_roster_row(
    commands: &mut Commands,
    parent: Entity,
    character: &RosterCharacter,
    selected: &SelectedRosterEntry,
    theme: &UiTheme,
    asset_server: &AssetServer,
) -> Entity {
    let kind = RosterEntryKind::Existing(character.display_name.clone());
    let row = spawn_row_shell(commands, parent, kind, selected);
    commands.entity(row).with_children(|row| {
        spawn_portrait_well(
            row,
            &portrait_letter(&character.display_name),
            theme,
            asset_server,
        );
        spawn_identity(row, character, theme);
        spawn_delete_button(row, character.character_id, theme);
    });
    row
}

fn spawn_create_row(
    commands: &mut Commands,
    parent: Entity,
    selected: &SelectedRosterEntry,
    theme: &UiTheme,
    asset_server: &AssetServer,
) -> Entity {
    let row = spawn_row_shell(commands, parent, RosterEntryKind::Create, selected);
    commands.entity(row).with_children(|row| {
        spawn_portrait_well(row, "+", theme, asset_server);
        spawn_create_label(row);
    });
    row
}

fn spawn_row_shell(
    commands: &mut Commands,
    parent: Entity,
    kind: RosterEntryKind,
    selected: &SelectedRosterEntry,
) -> Entity {
    let is_selected = entry_is_selected(&kind, selected);
    let (bg, border) = row_chrome(is_selected, false);
    let row = commands
        .spawn((
            Button,
            Interaction::None,
            Node {
                width: Val::Percent(100.0),
                height: Val::Px(ROW_HEIGHT),
                min_height: Val::Px(ROW_HEIGHT),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                padding: UiRect::all(Val::Px(ROW_PAD)),
                column_gap: Val::Px(12.0),
                border: UiRect::all(Val::Px(ROW_BORDER_WIDTH)),
                border_radius: BorderRadius::all(Val::Px(ROW_RADIUS)),
                flex_shrink: 0.0,
                overflow: Overflow::clip(),
                ..default()
            },
            bg,
            border,
            kind,
        ))
        .id();
    commands.entity(parent).add_child(row);
    row
}

fn spawn_portrait_well(
    parent: &mut ChildSpawnerCommands,
    letter: &str,
    theme: &UiTheme,
    asset_server: &AssetServer,
) {
    parent
        .spawn((
            Node {
                width: Val::Px(PORTRAIT_SIZE),
                height: Val::Px(PORTRAIT_SIZE),
                min_width: Val::Px(PORTRAIT_SIZE),
                min_height: Val::Px(PORTRAIT_SIZE),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                flex_shrink: 0.0,
                ..default()
            },
            ImageNode::new(asset_server.load(PORTRAIT_RING_PATH)).with_mode(NodeImageMode::Stretch),
            BackgroundColor(PORTRAIT_BG),
        ))
        .with_children(|well| {
            well.spawn((
                Text::new(letter.to_string()),
                TextFont {
                    font_size: FontSize::Px(theme.input_font_size + 4.0),
                    ..default()
                },
                TextColor(TITLE_COLOR),
            ));
        });
}

fn spawn_identity(parent: &mut ChildSpawnerCommands, character: &RosterCharacter, theme: &UiTheme) {
    parent
        .spawn(Node {
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: Val::Px(0.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::Center,
            row_gap: Val::Px(2.0),
            overflow: Overflow::clip(),
            ..default()
        })
        .with_children(|details| {
            details.spawn((
                Text::new(character.display_name.clone()),
                TextFont {
                    font_size: FontSize::Px(NAME_FONT_SIZE),
                    ..default()
                },
                TextColor(theme.text_color),
                TextLayout {
                    linebreak: LineBreak::NoWrap,
                    ..default()
                },
            ));
            if character.online {
                details.spawn((
                    Text::new("Online".to_string()),
                    TextFont {
                        font_size: FontSize::Px(13.0),
                        ..default()
                    },
                    TextColor(Color::srgb(0.55, 0.85, 0.55)),
                ));
            }
        });
}

fn spawn_create_label(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn(Node {
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: Val::Px(0.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::FlexStart,
            align_items: AlignItems::Center,
            overflow: Overflow::clip(),
            ..default()
        })
        .with_children(|details| {
            details.spawn((
                Text::new(CREATE_LABEL),
                TextFont {
                    font_size: FontSize::Px(NAME_FONT_SIZE),
                    ..default()
                },
                TextColor(Color::srgb(0.82, 0.74, 0.52)),
                TextLayout {
                    linebreak: LineBreak::NoWrap,
                    ..default()
                },
            ));
        });
}

fn spawn_delete_button(
    parent: &mut ChildSpawnerCommands,
    character_id: bevymmo_client::stdb::Uuid,
    theme: &UiTheme,
) {
    parent
        .spawn((
            Button,
            Interaction::None,
            Node {
                width: Val::Px(DELETE_BUTTON_SIZE),
                height: Val::Px(DELETE_BUTTON_SIZE),
                min_width: Val::Px(DELETE_BUTTON_SIZE),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                flex_shrink: 0.0,
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(DELETE_BG),
            BorderColor::all(ROW_BORDER),
            RosterDeleteButton::Idle(character_id),
        ))
        .with_children(|button| {
            button.spawn((
                Text::new("X"),
                TextFont {
                    font_size: FontSize::Px(theme.button_font_size * 0.7),
                    ..default()
                },
                TextColor(theme.button_text_color),
            ));
        });
}

fn row_chrome(selected: bool, hovered: bool) -> (BackgroundColor, BorderColor) {
    let bg = if selected { ROW_BG_SELECTED } else { ROW_BG };
    let border = if selected {
        ROW_BORDER_SELECTED
    } else if hovered {
        ROW_BORDER_HOVER
    } else {
        ROW_BORDER
    };
    (BackgroundColor(bg), BorderColor::all(border))
}

fn entry_is_selected(kind: &RosterEntryKind, selected: &SelectedRosterEntry) -> bool {
    match (kind, selected) {
        (RosterEntryKind::Existing(name), SelectedRosterEntry::Existing(selected_name)) => {
            name == selected_name
        }
        (RosterEntryKind::Create, SelectedRosterEntry::Create) => true,
        _ => false,
    }
}

fn portrait_letter(name: &str) -> String {
    match name.chars().next() {
        Some(c) => c.to_uppercase().collect(),
        None => "?".to_string(),
    }
}

fn handle_roster_select(
    mut selected: ResMut<SelectedRosterEntry>,
    rows: Query<(&Interaction, &RosterEntryKind), Changed<Interaction>>,
) {
    for (interaction, kind) in rows.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        *selected = match kind {
            RosterEntryKind::Existing(name) => SelectedRosterEntry::Existing(name.clone()),
            RosterEntryKind::Create => SelectedRosterEntry::Create,
        };
    }
}

fn apply_roster_row_visuals(
    selected: Res<SelectedRosterEntry>,
    mut rows: Query<(
        &RosterEntryKind,
        &Interaction,
        &mut BackgroundColor,
        &mut BorderColor,
    )>,
) {
    for (kind, interaction, mut bg, mut border) in &mut rows {
        let hovered = matches!(*interaction, Interaction::Hovered | Interaction::Pressed);
        let (next_bg, next_border) = row_chrome(entry_is_selected(kind, &selected), hovered);
        *bg = next_bg;
        *border = next_border;
    }
}

fn tint_roster_delete_buttons(
    mut buttons: Query<(&Interaction, &mut BackgroundColor), With<RosterDeleteButton>>,
) {
    for (interaction, mut bg) in &mut buttons {
        bg.0 = match *interaction {
            Interaction::Pressed => DELETE_BG_PRESSED,
            Interaction::Hovered => DELETE_BG_HOVER,
            Interaction::None => DELETE_BG,
        };
    }
}

fn handle_roster_delete(
    mut delete_request: ResMut<DeleteCharacterRequest>,
    mut buttons: Query<
        (&Interaction, &mut RosterDeleteButton, &Children, &mut Node),
        Changed<Interaction>,
    >,
    mut labels: Query<&mut Text>,
) {
    for (interaction, mut state, children, mut node) in buttons.iter_mut() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let next = match *state {
            RosterDeleteButton::Idle(id) => RosterDeleteButton::Confirming(id),
            RosterDeleteButton::Confirming(id) => {
                delete_request.0 = Some(id);
                RosterDeleteButton::Idle(id)
            }
        };
        let (label, width) = match next {
            RosterDeleteButton::Idle(_) => ("X", DELETE_BUTTON_SIZE),
            RosterDeleteButton::Confirming(_) => ("Confirm?", DELETE_CONFIRM_WIDTH),
        };
        node.width = Val::Px(width);
        for child in children.iter() {
            if let Ok(mut text) = labels.get_mut(child) {
                text.0 = label.to_string();
            }
        }
        *state = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevymmo_client::stdb::Uuid;

    fn test_character(name: &str, online: bool) -> RosterCharacter {
        RosterCharacter {
            character_id: Uuid::from_u128(1),
            display_name: name.to_string(),
            online,
        }
    }

    fn spawn_row_app(character: &RosterCharacter, selected: SelectedRosterEntry) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_asset::<Image>();
        app.init_resource::<UiTheme>();
        app.insert_resource(selected);
        let theme = UiTheme::default();
        let parent = app.world_mut().spawn(Node::default()).id();
        let selection = app.world().resource::<SelectedRosterEntry>().clone();
        let asset_server = app.world().resource::<AssetServer>().clone();
        let row = {
            let mut commands = app.world_mut().commands();
            spawn_roster_row(
                &mut commands,
                parent,
                character,
                &selection,
                &theme,
                &asset_server,
            )
        };
        app.world_mut().flush();
        (app, row)
    }

    #[test]
    fn row_geometry_fits_portrait_and_even_padding() {
        assert!((76.0..=84.0).contains(&ROW_HEIGHT));
        assert_eq!(ROW_PAD, 16.0);
        assert!((50.0..=54.0).contains(&PORTRAIT_SIZE));
        assert_eq!(ROW_RADIUS, 8.0);
        // Compile-time: these are relationships between constants, so a
        // violation should fail the build rather than wait for a test run.
        const {
            assert!(ROW_HEIGHT >= PORTRAIT_SIZE + ROW_PAD * 2.0)
        };
        const { assert!(DELETE_BUTTON_SIZE <= ROW_HEIGHT - ROW_PAD * 2.0) };
    }

    #[test]
    fn roster_row_is_full_width_without_play() {
        let character = test_character("Galvdon", true);
        let (mut app, row) = spawn_row_app(&character, SelectedRosterEntry::None);
        let node = app.world().get::<Node>(row).expect("row node");
        assert_eq!(node.width, Val::Percent(100.0));
        assert_eq!(node.height, Val::Px(ROW_HEIGHT));
        assert_eq!(node.padding, UiRect::all(Val::Px(ROW_PAD)));
        assert_eq!(node.align_items, AlignItems::Center);
        assert_eq!(
            app.world().get::<RosterEntryKind>(row),
            Some(&RosterEntryKind::Existing("Galvdon".into()))
        );
        assert!(app.world().get::<Button>(row).is_some());

        let mut play_labels = 0;
        let mut texts = app.world_mut().query::<&Text>();
        for text in texts.iter(app.world()) {
            if text.0 == "Play" {
                play_labels += 1;
            }
        }
        assert_eq!(play_labels, 0, "Play must not live on the roster row");
    }

    #[test]
    fn roster_row_shows_initial_and_online() {
        let character = test_character("Galvdon", true);
        let (mut app, _) = spawn_row_app(&character, SelectedRosterEntry::None);
        let mut texts = app.world_mut().query::<&Text>();
        let labels: Vec<String> = texts.iter(app.world()).map(|text| text.0.clone()).collect();
        assert!(labels.iter().any(|label| label == "G"));
        assert!(labels.iter().any(|label| label == "Galvdon"));
        assert!(labels.iter().any(|label| label == "Online"));
        assert!(labels.iter().any(|label| label == "X"));
    }

    #[test]
    fn clicking_a_character_row_selects_it() {
        let mut app = App::new();
        app.init_resource::<SelectedRosterEntry>();
        app.add_systems(Update, handle_roster_select);
        app.world_mut().spawn((
            Interaction::Pressed,
            RosterEntryKind::Existing("Galvdon".into()),
        ));
        app.update();
        assert_eq!(
            *app.world().resource::<SelectedRosterEntry>(),
            SelectedRosterEntry::Existing("Galvdon".into())
        );
    }

    #[test]
    fn clicking_create_selects_create() {
        let mut app = App::new();
        app.init_resource::<SelectedRosterEntry>();
        app.add_systems(Update, handle_roster_select);
        app.world_mut()
            .spawn((Interaction::Pressed, RosterEntryKind::Create));
        app.update();
        assert_eq!(
            *app.world().resource::<SelectedRosterEntry>(),
            SelectedRosterEntry::Create
        );
    }

    #[test]
    fn delete_requires_a_second_confirming_click() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_asset::<Image>();
        app.init_resource::<UiTheme>();
        app.init_resource::<DeleteCharacterRequest>();
        app.add_systems(Update, handle_roster_delete);

        let character = test_character("Galvdon", false);
        let theme = UiTheme::default();
        let parent = app.world_mut().spawn(Node::default()).id();
        let selection = SelectedRosterEntry::None;
        let asset_server = app.world().resource::<AssetServer>().clone();
        {
            let mut commands = app.world_mut().commands();
            spawn_roster_row(
                &mut commands,
                parent,
                &character,
                &selection,
                &theme,
                &asset_server,
            );
        }
        app.world_mut().flush();

        let delete = app
            .world_mut()
            .query_filtered::<Entity, With<RosterDeleteButton>>()
            .single(app.world())
            .expect("delete button");
        app.world_mut()
            .entity_mut(delete)
            .insert(Interaction::Pressed);
        app.update();

        assert!(
            app.world().resource::<DeleteCharacterRequest>().0.is_none(),
            "first click must only arm confirm"
        );
        assert!(matches!(
            app.world().get::<RosterDeleteButton>(delete),
            Some(RosterDeleteButton::Confirming(_))
        ));
        let node = app.world().get::<Node>(delete).expect("delete node");
        assert_eq!(node.width, Val::Px(DELETE_CONFIRM_WIDTH));

        app.world_mut().entity_mut(delete).insert(Interaction::None);
        app.update();
        app.world_mut()
            .entity_mut(delete)
            .insert(Interaction::Pressed);
        app.update();

        assert_eq!(
            app.world().resource::<DeleteCharacterRequest>().0,
            Some(character.character_id)
        );
    }

    #[test]
    fn empty_roster_rebuilds_title_and_create_row() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_asset::<Image>();
        app.init_resource::<UiTheme>();
        app.init_resource::<CharacterRoster>();
        app.init_resource::<SelectedRosterEntry>();
        app.add_systems(Update, rebuild_roster_list);

        let list = app.world_mut().spawn(RosterList).id();
        app.update();

        let children = app
            .world()
            .entity(list)
            .get::<Children>()
            .expect("children");
        assert_eq!(children.len(), 2, "title + create row");
        let title = children.first().copied().expect("title");
        let create = children.get(1).copied().expect("create row");
        assert!(app.world().entity(title).contains::<RosterTitle>());
        assert_eq!(
            app.world().entity(create).get::<RosterEntryKind>(),
            Some(&RosterEntryKind::Create)
        );
    }

    #[test]
    fn retain_selection_drops_missing_names_and_full_create() {
        let roster = CharacterRoster::default();
        let mut selected = SelectedRosterEntry::Existing("Galvdon".into());
        retain_selection(&roster, &mut selected);
        assert_eq!(selected, SelectedRosterEntry::None);

        let mut selected = SelectedRosterEntry::Create;
        retain_selection(&roster, &mut selected);
        assert_eq!(selected, SelectedRosterEntry::Create);
    }

    #[test]
    fn selected_row_uses_blue_border() {
        let (bg, border) = row_chrome(true, false);
        assert_eq!(bg.0, ROW_BG_SELECTED);
        assert_eq!(border, BorderColor::all(ROW_BORDER_SELECTED));
        let (_, idle) = row_chrome(false, false);
        assert_eq!(idle, BorderColor::all(ROW_BORDER));
    }
}
