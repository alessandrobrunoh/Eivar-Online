//! Dropdown / select widget.
//!
//! A labeled header shows the current value. Clicking it opens a list of
//! items; picking one emits [`DropdownChanged`] and closes the list.

use bevy::ecs::component::Component;
use bevy::ecs::message::Message;
use bevy::prelude::*;

use crate::ui::button::{
    apply_button_image, queue_bar_images, sliced_bar_image, BarButtonKind, UiButtonImages,
};
use crate::ui::settings::state::SettingChoice;
use crate::ui::theme::UiTheme;

/// One selectable option in the dropdown.
#[derive(Clone, Debug)]
pub struct DropdownItem {
    /// Display string, shown to the user.
    pub label: String,
    /// Opaque string identifier stored in the widget. The caller decodes it.
    pub value: String,
}

/// A dropdown widget: label + value, click opens a list of items.
#[derive(Component, Clone)]
pub struct Dropdown {
    /// Stable identifier used by the caller to dispatch the change.
    pub id: SettingChoice,
    pub items: Vec<DropdownItem>,
    /// Index of the currently selected item.
    pub selected: usize,
    /// Whether the item list is visible.
    pub open: bool,
}

/// Header button that toggles the list.
#[derive(Component)]
pub struct DropdownHeader;

/// Item list under the header. Visibility follows [`Dropdown::open`].
#[derive(Component)]
pub struct DropdownList;

/// One row in an open dropdown list.
#[derive(Component)]
pub struct DropdownOption {
    pub dropdown: Entity,
    pub index: usize,
}

/// Marker: text node showing the current value next to the label.
#[derive(Component)]
pub struct DropdownValueText;

/// Select control used by settings panels.
pub type Select = Dropdown;

/// Event emitted when the dropdown selection changes (click or programmatic).
#[derive(Message, Clone, Debug)]
pub struct DropdownChanged {
    pub id: SettingChoice,
    pub value: String,
}

/// Spawns a labeled dropdown and returns its root entity.
///
/// Layout:
/// `[ label ............ value ▾ ]`
/// and, when open, a column of items underneath.
pub fn spawn_dropdown(
    commands: &mut Commands,
    parent: Entity,
    id: SettingChoice,
    label: impl Into<String>,
    items: Vec<DropdownItem>,
    initial_value: &str,
    theme: &UiTheme,
) -> Entity {
    let selected = items
        .iter()
        .position(|i| i.value == initial_value)
        .unwrap_or(0);
    let current_label = items
        .get(selected)
        .map(|item| item.label.clone())
        .unwrap_or_default();

    let root = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                ..default()
            },
            Dropdown {
                id,
                items: items.clone(),
                selected,
                open: false,
            },
        ))
        .id();
    commands.entity(parent).add_child(root);

    let header = commands
        .spawn((
            Button,
            Node {
                width: Val::Percent(100.0),
                height: Val::Px(44.0),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                padding: UiRect::axes(Val::Px(28.0), Val::Px(6.0)),
                flex_shrink: 0.0,
                ..default()
            },
            sliced_bar_image(Handle::default()),
            UiButtonImages::placeholder(),
            DropdownHeader,
        ))
        .id();
    commands.entity(root).add_child(header);
    queue_bar_images(commands, header, BarButtonKind::Neutral);

    let label_entity = commands
        .spawn((
            Text::new(label.into()),
            TextFont {
                font_size: FontSize::Px(theme.input_font_size),
                ..default()
            },
            TextColor(theme.button_text_color),
        ))
        .id();
    commands.entity(header).add_child(label_entity);

    let value_entity = commands
        .spawn((
            Text::new(current_label),
            TextFont {
                font_size: FontSize::Px(theme.input_font_size),
                ..default()
            },
            TextColor(theme.muted_text_color),
            DropdownValueText,
        ))
        .id();
    commands.entity(header).add_child(value_entity);

    let list = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                display: Display::None,
                ..default()
            },
            DropdownList,
        ))
        .id();
    commands.entity(root).add_child(list);

    for (index, item) in items.into_iter().enumerate() {
        let option = commands
            .spawn((
                Button,
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(36.0),
                    justify_content: JustifyContent::FlexStart,
                    align_items: AlignItems::Center,
                    padding: UiRect::axes(Val::Px(28.0), Val::Px(4.0)),
                    flex_shrink: 0.0,
                    ..default()
                },
                sliced_bar_image(Handle::default()),
                UiButtonImages::placeholder(),
                DropdownOption {
                    dropdown: root,
                    index,
                },
            ))
            .id();
        commands.entity(list).add_child(option);
        queue_bar_images(commands, option, BarButtonKind::Neutral);
        let option_label = commands
            .spawn((
                Text::new(item.label),
                TextFont {
                    font_size: FontSize::Px(theme.input_font_size),
                    ..default()
                },
                TextColor(theme.button_text_color),
            ))
            .id();
        commands.entity(option).add_child(option_label);
    }

    root
}

/// Spawns a [`Select`] control.
pub fn spawn_select(
    commands: &mut Commands,
    parent: Entity,
    id: SettingChoice,
    label: impl Into<String>,
    items: Vec<DropdownItem>,
    initial_value: &str,
    theme: &UiTheme,
) -> Entity {
    spawn_dropdown(commands, parent, id, label, items, initial_value, theme)
}

/// Click on the header toggles this dropdown and closes the others.
pub fn toggle_dropdown_on_header_click(
    headers: Query<(&Interaction, &ChildOf), (Changed<Interaction>, With<DropdownHeader>)>,
    mut dropdowns: Query<(Entity, &mut Dropdown)>,
) {
    let mut clicked = None;
    for (interaction, child_of) in &headers {
        if *interaction == Interaction::Pressed {
            clicked = Some(child_of.0);
        }
    }
    let Some(clicked) = clicked else {
        return;
    };
    for (entity, mut dropdown) in &mut dropdowns {
        if entity == clicked {
            dropdown.open = !dropdown.open;
        } else {
            dropdown.open = false;
        }
    }
}

/// Click on an item selects it, closes the list, and emits [`DropdownChanged`].
pub fn pick_dropdown_option(
    clicks: Query<(&Interaction, &DropdownOption), Changed<Interaction>>,
    mut dropdowns: Query<&mut Dropdown>,
    headers: Query<(Entity, &ChildOf), With<DropdownHeader>>,
    mut value_texts: Query<(&ChildOf, &mut Text), With<DropdownValueText>>,
    mut changed: MessageWriter<DropdownChanged>,
) {
    for (interaction, option) in &clicks {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Some((label, value, id)) =
            dropdowns
                .get_mut(option.dropdown)
                .ok()
                .and_then(|mut dropdown| {
                    if option.index >= dropdown.items.len() {
                        return None;
                    }
                    dropdown.selected = option.index;
                    dropdown.open = false;
                    Some((
                        dropdown.items[option.index].label.clone(),
                        dropdown.items[option.index].value.clone(),
                        dropdown.id,
                    ))
                })
        else {
            continue;
        };

        if let Some(header) = headers
            .iter()
            .find_map(|(entity, parent)| (parent.0 == option.dropdown).then_some(entity))
        {
            for (parent, mut text) in &mut value_texts {
                if parent.0 == header {
                    text.0 = label;
                    break;
                }
            }
        }

        changed.write(DropdownChanged { id, value });
    }
}

/// Escape closes any open dropdown without changing the selection.
///
/// Consumes the press so the settings overlay / pause menu do not also
/// close on the same key.
pub fn close_dropdowns_on_escape(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut dropdowns: Query<&mut Dropdown>,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    let mut closed = false;
    for mut dropdown in &mut dropdowns {
        if dropdown.open {
            dropdown.open = false;
            closed = true;
        }
    }
    if closed {
        keys.clear_just_pressed(KeyCode::Escape);
    }
}

/// A click that is not on a header or option closes every open list.
pub fn close_dropdowns_on_outside_click(
    mouse: Res<ButtonInput<MouseButton>>,
    headers: Query<&Interaction, With<DropdownHeader>>,
    options: Query<&Interaction, With<DropdownOption>>,
    mut dropdowns: Query<&mut Dropdown>,
) {
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    let on_widget = headers
        .iter()
        .chain(options.iter())
        .any(|interaction| matches!(*interaction, Interaction::Pressed | Interaction::Hovered));
    if on_widget {
        return;
    }
    for mut dropdown in &mut dropdowns {
        dropdown.open = false;
    }
}

/// Ornate bar hover/active: open header and selected option stay highlighted.
pub fn sync_dropdown_bar_visuals(
    dropdowns: Query<&Dropdown>,
    mut headers: Query<
        (&ChildOf, &Interaction, &UiButtonImages, &mut ImageNode),
        With<DropdownHeader>,
    >,
    mut options: Query<
        (
            &DropdownOption,
            &Interaction,
            &UiButtonImages,
            &mut ImageNode,
        ),
        Without<DropdownHeader>,
    >,
) {
    for (parent, interaction, images, mut image) in &mut headers {
        let open = dropdowns
            .get(parent.0)
            .map(|dropdown| dropdown.open)
            .unwrap_or(false);
        let shown = if open {
            Interaction::Hovered
        } else {
            *interaction
        };
        apply_button_image(shown, &mut image, images);
    }
    for (option, interaction, images, mut image) in &mut options {
        let selected = dropdowns
            .get(option.dropdown)
            .map(|dropdown| dropdown.selected == option.index)
            .unwrap_or(false);
        let shown = if selected {
            Interaction::Hovered
        } else {
            *interaction
        };
        apply_button_image(shown, &mut image, images);
    }
}

/// Copies [`Dropdown::open`] onto the list node's `Display`.
pub fn sync_dropdown_open_state(
    dropdowns: Query<(&Dropdown, &Children), Changed<Dropdown>>,
    mut lists: Query<&mut Node, With<DropdownList>>,
) {
    for (dropdown, children) in &dropdowns {
        for child in children {
            if let Ok(mut node) = lists.get_mut(*child) {
                node.display = if dropdown.open {
                    Display::Flex
                } else {
                    Display::None
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::UiTheme;
    use bevy::ecs::message::{MessageCursor, Messages};

    fn items() -> Vec<DropdownItem> {
        vec![
            DropdownItem {
                label: "Windowed".into(),
                value: "windowed".into(),
            },
            DropdownItem {
                label: "Borderless".into(),
                value: "borderless".into(),
            },
            DropdownItem {
                label: "Exclusive".into(),
                value: "exclusive".into(),
            },
        ]
    }

    fn spawn_widget(world: &mut World, initial: &str) -> Entity {
        let parent = world.spawn(Node::default()).id();
        let initial = initial.to_string();
        let root = world.resource_scope(|world, theme: Mut<UiTheme>| {
            let mut commands = world.commands();
            spawn_dropdown(
                &mut commands,
                parent,
                SettingChoice::WindowMode,
                "Window Mode",
                items(),
                &initial,
                &theme,
            )
        });
        world.flush();
        root
    }

    #[test]
    fn spawn_shows_the_initial_value_label() {
        let mut world = World::new();
        world.insert_resource(UiTheme::default());
        spawn_widget(&mut world, "borderless");

        let text = world
            .query_filtered::<&Text, With<DropdownValueText>>()
            .single(&world)
            .expect("value text");
        assert_eq!(text.0, "Borderless");
        assert!(
            world
                .query_filtered::<&ImageNode, With<DropdownHeader>>()
                .iter(&world)
                .next()
                .is_some(),
            "dropdown header must use ornate bar art"
        );
    }

    fn press(app: &mut App, entity: Entity) {
        app.world_mut()
            .entity_mut(entity)
            .insert(Interaction::Pressed);
    }

    #[test]
    fn clicking_an_item_emits_the_new_value_and_closes() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<ButtonInput<MouseButton>>();
        app.insert_resource(UiTheme::default());
        app.add_message::<DropdownChanged>();
        app.add_systems(
            Update,
            (
                toggle_dropdown_on_header_click,
                pick_dropdown_option,
                sync_dropdown_open_state,
            )
                .chain(),
        );

        let root = spawn_widget(app.world_mut(), "windowed");
        let header = app
            .world_mut()
            .query_filtered::<Entity, With<DropdownHeader>>()
            .single(app.world())
            .expect("header");
        press(&mut app, header);
        app.update();
        assert!(app.world().get::<Dropdown>(root).unwrap().open);

        let option = app
            .world_mut()
            .query::<(Entity, &DropdownOption)>()
            .iter(app.world())
            .find_map(|(entity, option)| (option.index == 1).then_some(entity))
            .expect("borderless option");
        press(&mut app, option);
        app.update();

        let dropdown = app.world().get::<Dropdown>(root).unwrap();
        assert!(!dropdown.open);
        assert_eq!(dropdown.selected, 1);

        let text = app
            .world_mut()
            .query_filtered::<&Text, With<DropdownValueText>>()
            .single(app.world())
            .expect("value text");
        assert_eq!(text.0, "Borderless");

        let mut cursor = MessageCursor::<DropdownChanged>::default();
        let events: Vec<_> = {
            let messages = app.world().resource::<Messages<DropdownChanged>>();
            cursor.read(messages).cloned().collect()
        };
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, SettingChoice::WindowMode);
        assert_eq!(events[0].value, "borderless");
    }
}
