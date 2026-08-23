//! Componente e logica per la Scrollbar e lo ScrollView.

use bevy::{
    input::mouse::{MouseScrollUnit, MouseWheel},
    prelude::*,
    window::PrimaryWindow,
};

use crate::ui::scale::{physical_to_ui_px, window_to_ui_px};
use crate::ui::theme::UiTheme;

const SCROLLBAR_TRACK_WIDTH: f32 = 14.0;
const SCROLLBAR_MIN_THUMB: f32 = 24.0;
const SCROLLBAR_TRACK_COLOR: Color = Color::srgba(0.10, 0.08, 0.05, 0.92);
const SCROLLBAR_THUMB_COLOR: Color = Color::srgb(0.72, 0.56, 0.26);
const SCROLLBAR_THUMB_HOVER: Color = Color::srgb(0.86, 0.70, 0.34);
const SCROLLBAR_THUMB_ACTIVE: Color = Color::srgb(0.58, 0.44, 0.18);

pub struct ScrollbarPlugin;

impl Plugin for ScrollbarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                update_scroll_max,
                handle_mouse_scroll,
                handle_scrollbar_drag,
                apply_scroll_position,
                update_scrollbar_visuals,
            )
                .chain(),
        );
    }
}

/// Aggiunto al viewport (che clippa il contenuto).
#[derive(Component)]
pub struct ScrollView {
    pub content_entity: Entity,
    pub scrollbar_entity: Option<Entity>,
    pub track_entity: Option<Entity>,
    pub current_scroll: f32,
    pub max_scroll: f32,
}

/// Aggiunto al contenuto vero e proprio.
#[derive(Component)]
pub struct ScrollContent;

/// Marker on the scrollbar track (parent of the thumb).
#[derive(Component)]
pub struct ScrollbarTrack;

/// Aggiunto al "thumb" della scrollbar per trascinarlo.
#[derive(Component)]
pub struct ScrollbarThumb {
    pub viewport_entity: Entity,
    pub is_dragging: bool,
    pub drag_start_y: f32,
    pub drag_start_scroll: f32,
}

/// Current offset of the first [`ScrollView`] under `root`, or `0.0` when the
/// subtree has none.
///
/// Panels that rebuild themselves wholesale — the inventory detail editor, the
/// loot bag — despawn the view and lose its position with it. Reading the
/// offset back out just before the despawn and feeding it to
/// [`spawn_scroll_view_scrolled`] is what keeps a list from snapping to the top
/// after every click.
pub fn descendant_scroll(
    root: Entity,
    children: &Query<&Children>,
    scroll_views: &Query<&ScrollView>,
) -> f32 {
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if let Ok(view) = scroll_views.get(entity) {
            return view.current_scroll;
        }
        if let Ok(child_list) = children.get(entity) {
            stack.extend(child_list.iter());
        }
    }
    0.0
}

/// Crea una ScrollView. Ritorna l'Entity del wrapper esterno.
pub fn spawn_scroll_view(
    commands: &mut Commands,
    parent: Entity,
    theme: &UiTheme,
    content_builder: impl FnOnce(&mut Commands) -> Entity,
) -> Entity {
    spawn_scroll_view_with_content(commands, parent, theme, content_builder).0
}

/// Like [`spawn_scroll_view`], but the viewport starts at `initial_scroll`
/// instead of the top. Used when a panel is rebuilt and must keep its place.
pub fn spawn_scroll_view_scrolled(
    commands: &mut Commands,
    parent: Entity,
    theme: &UiTheme,
    initial_scroll: f32,
    content_builder: impl FnOnce(&mut Commands) -> Entity,
) -> Entity {
    spawn_scroll_view_configured(commands, parent, theme, initial_scroll, content_builder).0
}

/// Crea una ScrollView e ritorna sia il wrapper sia l'entity del contenuto.
///
/// La variante pubblica standard ritorna solo il wrapper; questa serve ai
/// widget che devono aggiungere dinamicamente figli al contenuto scrollabile.
pub fn spawn_scroll_view_with_content(
    commands: &mut Commands,
    parent: Entity,
    theme: &UiTheme,
    content_builder: impl FnOnce(&mut Commands) -> Entity,
) -> (Entity, Entity) {
    spawn_scroll_view_configured(commands, parent, theme, 0.0, content_builder)
}

fn spawn_scroll_view_configured(
    commands: &mut Commands,
    parent: Entity,
    _theme: &UiTheme,
    initial_scroll: f32,
    content_builder: impl FnOnce(&mut Commands) -> Entity,
) -> (Entity, Entity) {
    let wrapper = commands
        .spawn((Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            ..default()
        },))
        .id();

    commands.entity(parent).add_child(wrapper);

    // Viewport shares the row with the track. `width: 100%` plus a 12 px
    // sibling overflowed the wrapper and the card's `overflow: clip` ate
    // the scrollbar entirely — that is why the inventory bar was invisible.
    let viewport = commands
        .spawn((
            Node {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_width: Val::Px(0.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                overflow: Overflow::clip_y(),
                ..default()
            },
            Interaction::default(), // per intercettare l'hover del mouse wheel
        ))
        .id();

    commands.entity(wrapper).add_child(viewport);

    // Il Contenuto
    let content = content_builder(commands);
    commands.entity(content).insert(ScrollContent);
    // Width stays 100% of the viewport so grids can centre; `top` is what
    // `apply_scroll_position` writes each frame.
    commands.entity(content).insert(Node {
        width: Val::Percent(100.0),
        top: Val::Px(0.0),
        flex_direction: FlexDirection::Column,
        position_type: PositionType::Relative,
        ..default()
    });
    commands.entity(viewport).add_child(content);

    // Track. Starts collapsed: `max_scroll` is 0 until layout, and a
    // Hidden track would still reserve 14 px.
    let track = commands
        .spawn((
            Node {
                width: Val::Px(SCROLLBAR_TRACK_WIDTH),
                height: Val::Percent(100.0),
                flex_shrink: 0.0,
                margin: UiRect::left(Val::Px(6.0)),
                position_type: PositionType::Relative,
                border: UiRect::all(Val::Px(1.0)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(SCROLLBAR_TRACK_COLOR),
            BorderColor::all(Color::srgba(0.55, 0.42, 0.18, 0.7)),
            ScrollbarTrack,
            Visibility::Hidden,
        ))
        .id();

    // Thumb
    let thumb = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(1.0),
                right: Val::Px(1.0),
                top: Val::Px(0.0),
                width: Val::Auto,
                height: Val::Px(40.0),
                ..default()
            },
            BackgroundColor(SCROLLBAR_THUMB_COLOR),
            ScrollbarThumb {
                viewport_entity: viewport,
                is_dragging: false,
                drag_start_y: 0.0,
                drag_start_scroll: 0.0,
            },
            Interaction::default(),
        ))
        .id();

    commands.entity(track).add_child(thumb);
    commands.entity(wrapper).add_child(track);

    commands.entity(viewport).insert(ScrollView {
        content_entity: content,
        scrollbar_entity: Some(thumb),
        track_entity: Some(track),
        current_scroll: initial_scroll.max(0.0),
        max_scroll: 0.0,
    });

    (wrapper, content)
}

fn update_scroll_max(
    mut view_q: Query<(&mut ScrollView, &ComputedNode)>,
    content_q: Query<&ComputedNode, With<ScrollContent>>,
) {
    for (mut view, view_node) in view_q.iter_mut() {
        if let Ok(content_node) = content_q.get(view.content_entity) {
            let view_height = view_node.size().y;
            let content_height = content_node.size().y;
            view.max_scroll = (content_height - view_height).max(0.0);
            view.current_scroll = view.current_scroll.clamp(0.0, view.max_scroll);
        }
    }
}

fn handle_mouse_scroll(
    mut mouse_wheel_events: MessageReader<MouseWheel>,
    mut query: Query<(&mut ScrollView, &Interaction)>,
) {
    for event in mouse_wheel_events.read() {
        for (mut scroll_view, interaction) in query.iter_mut() {
            if *interaction == Interaction::Hovered {
                let dy = match event.unit {
                    MouseScrollUnit::Line => event.y * 30.0,
                    MouseScrollUnit::Pixel => event.y,
                };
                scroll_view.current_scroll -= dy;
                scroll_view.current_scroll = scroll_view
                    .current_scroll
                    .clamp(0.0, scroll_view.max_scroll);
            }
        }
    }
}

fn handle_scrollbar_drag(
    mut query: Query<(&Interaction, &mut ScrollbarThumb, &mut BackgroundColor)>,
    mut view_q: Query<(&mut ScrollView, &ComputedNode)>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    ui_scale: Res<UiScale>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let cursor_y = window.cursor_position().map(|p| p.y).unwrap_or(0.0);

    for (interaction, mut thumb, mut bg) in query.iter_mut() {
        // Avvio drag
        if *interaction == Interaction::Pressed && mouse_input.just_pressed(MouseButton::Left) {
            thumb.is_dragging = true;
            thumb.drag_start_y = cursor_y;
            if let Ok((view, _)) = view_q.get(thumb.viewport_entity) {
                thumb.drag_start_scroll = view.current_scroll;
            }
        }

        // Rilascio drag
        if thumb.is_dragging && mouse_input.just_released(MouseButton::Left) {
            thumb.is_dragging = false;
        }

        // Colori
        if thumb.is_dragging || *interaction == Interaction::Pressed {
            *bg = BackgroundColor(SCROLLBAR_THUMB_ACTIVE);
        } else if *interaction == Interaction::Hovered {
            *bg = BackgroundColor(SCROLLBAR_THUMB_HOVER);
        } else {
            *bg = BackgroundColor(SCROLLBAR_THUMB_COLOR);
        }

        // Calcolo spostamento
        if thumb.is_dragging {
            // `cursor_y` è in px logici della finestra, `size()` in px fisici:
            // vanno riportati nello stesso spazio (UI-logico, quello dei
            // `Val::Px`), altrimenti il rapporto sbaglia del fattore di scala
            // del layout e il trascinamento della barra scorre più lento del
            // mouse.
            let dy = window_to_ui_px(Vec2::new(0.0, cursor_y - thumb.drag_start_y), &ui_scale).y;
            if let Ok((mut view, view_node)) = view_q.get_mut(thumb.viewport_entity) {
                // proporzione per tradurre spostamento del mouse in scroll
                let view_h = view_node.size().y * view_node.inverse_scale_factor();
                // thumb occupa min 20.0 px (es.)
                let max_thumb_travel = view_h - 20.0;
                if max_thumb_travel > 0.0 && view.max_scroll > 0.0 {
                    let scroll_per_px = view.max_scroll / max_thumb_travel;
                    view.current_scroll = thumb.drag_start_scroll + dy * scroll_per_px;
                    view.current_scroll = view.current_scroll.clamp(0.0, view.max_scroll);
                }
            }
        }
    }
}

fn apply_scroll_position(
    mut view_q: Query<&ScrollView>,
    mut content_q: Query<&mut Node, With<ScrollContent>>,
) {
    for view in view_q.iter_mut() {
        if let Ok(mut node) = content_q.get_mut(view.content_entity) {
            node.top = Val::Px(-view.current_scroll);
        }
    }
}

fn update_scrollbar_visuals(
    view_q: Query<(&ScrollView, &ComputedNode)>,
    mut thumb_q: Query<&mut Node, (With<ScrollbarThumb>, Without<ScrollbarTrack>)>,
    mut track_q: Query<
        (&mut Node, &mut Visibility),
        (With<ScrollbarTrack>, Without<ScrollbarThumb>),
    >,
) {
    for (view, view_node) in &view_q {
        let Some(track_ent) = view.track_entity else {
            continue;
        };
        let Ok((mut track_node, mut track_visibility)) = track_q.get_mut(track_ent) else {
            continue;
        };

        if view.max_scroll <= 0.0 {
            track_node.display = Display::None;
            *track_visibility = Visibility::Hidden;
            continue;
        }

        track_node.display = Display::Flex;
        *track_visibility = Visibility::Inherited;

        let Some(thumb_ent) = view.scrollbar_entity else {
            continue;
        };
        let Ok(mut thumb_node) = thumb_q.get_mut(thumb_ent) else {
            continue;
        };

        let view_h = physical_to_ui_px(view_node.size(), view_node).y;
        let content_h = view_h + view.max_scroll;
        let proportion = view_h / content_h.max(1.0);
        let thumb_h = (view_h * proportion).max(SCROLLBAR_MIN_THUMB);

        thumb_node.height = Val::Px(thumb_h);

        let max_thumb_travel = (view_h - thumb_h).max(0.0);
        let scroll_percent = view.current_scroll / view.max_scroll;
        thumb_node.top = Val::Px(scroll_percent * max_thumb_travel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_leaves_room_for_the_track() {
        let mut app = App::new();
        app.init_resource::<UiTheme>();
        let theme = UiTheme::default();
        let mut commands = app.world_mut().commands();
        let parent = commands.spawn(Node::default()).id();
        spawn_scroll_view(&mut commands, parent, &theme, |commands| {
            commands.spawn(Node::default()).id()
        });
        app.update();

        let world = app.world_mut();
        let mut views = world.query::<(&ScrollView, &Node)>();
        let (_, viewport) = views.iter(world).next().expect("viewport");
        assert_eq!(
            viewport.flex_grow, 1.0,
            "viewport must shrink so the track is not clipped"
        );
        assert_eq!(viewport.min_width, Val::Px(0.0));
        assert_ne!(viewport.width, Val::Percent(100.0));
    }

    #[test]
    fn spawn_creates_a_track_and_thumb() {
        let mut app = App::new();
        app.init_resource::<UiTheme>();
        let theme = UiTheme::default();
        let mut commands = app.world_mut().commands();
        let parent = commands.spawn(Node::default()).id();
        spawn_scroll_view(&mut commands, parent, &theme, |commands| {
            commands.spawn(Node::default()).id()
        });
        app.update();

        let world = app.world_mut();
        let mut thumbs = world.query::<&ScrollbarThumb>();
        assert_eq!(thumbs.iter(world).count(), 1);

        let world = app.world_mut();
        let mut tracks = world.query::<(&ScrollbarTrack, &Node, &Visibility)>();
        let (_, track_node, visibility) = tracks.iter(world).next().expect("track");
        assert_eq!(
            track_node.display,
            Display::None,
            "unscrollable content must not reserve track width"
        );
        assert_eq!(*visibility, Visibility::Hidden);

        let world = app.world_mut();
        let mut views = world.query::<&ScrollView>();
        let view = views.iter(world).next().expect("view");
        assert!(view.scrollbar_entity.is_some());
        assert!(view.track_entity.is_some());
    }
}
