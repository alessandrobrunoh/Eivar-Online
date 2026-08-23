//! `CardBuilder` — Builder pattern for spawning a standard Card.
//!
//! Building a Bevy UI `Node` tree by hand is verbose and easy to get
//! inconsistently styled across panels. The builder centralizes the header /
//! footer / padding / theme / exclusivity so every future panel passes through
//! a single call site.

use std::borrow::Cow;

use bevy::prelude::*;

use super::components::{
    CardBody, CardExclusivityPolicy, CardFooter, CardHeader, CardHeaderDragHandle, CardKind,
    CardPositioning, CardWindow, CloseCardButton, DraggableCard,
};
use crate::ui::button::{spawn_bar_child, BarButtonKind};
use crate::ui::scrollbar::spawn_scroll_view;
use crate::ui::theme::{ornate_panel_image, UiTheme};

const CARD_FRAME_PANEL_PATH: &str = "ui/hud/panel_large_left.png";
/// Neutral bar used for Close / Unequip / vendor rows.
pub const ORNATE_BAR_NEUTRAL_PATH: &str = "ui/hud/bar_neutral_right_01.png";
/// Confirm bar used for Equip and other affirmative actions.
pub const ORNATE_BAR_CONFIRM_PATH: &str = "ui/hud/bar_blue_left_01.png";
/// Horizontal / vertical inset that keeps bar-end ornaments from stretching.
const BAR_SLICE: [f32; 2] = [24.0, 10.0];

/// Texture used by the decorative Card frame.
///
/// The extracted artwork currently contains a complete panel rather than clean
/// independent 9-slice pieces. Keeping the complete panel intact avoids
/// stretching its asymmetric corner decorations into the card content.
///
/// Tests that spawn without an `AssetServer` can use [`CardFrameAssets::default`]
/// (`Handle::default()` for both images).
#[derive(Clone, Default)]
pub struct CardFrameAssets {
    pub panel: Handle<Image>,
    pub bar: Handle<Image>,
}

impl CardFrameAssets {
    /// Loads the complete extracted panel and the matching action bar.
    pub fn load(asset_server: &AssetServer) -> Self {
        Self {
            panel: asset_server.load(CARD_FRAME_PANEL_PATH),
            bar: asset_server.load(ORNATE_BAR_NEUTRAL_PATH),
        }
    }
}

/// 9-sliced ornate bar used by Close, Equip / Unequip, and vendor rows.
pub fn ornate_bar_image(image: Handle<Image>) -> ImageNode {
    ImageNode::new(image).with_mode(NodeImageMode::Sliced(TextureSlicer {
        border: BorderRect::axes(BAR_SLICE[0], BAR_SLICE[1]),
        center_scale_mode: SliceScaleMode::Stretch,
        sides_scale_mode: SliceScaleMode::Stretch,
        max_corner_scale: 1.0,
    }))
}

/// Default card geometry. Callers override via [`CardBuilder::width`] /
/// [`CardBuilder::height`].
pub const DEFAULT_CARD_WIDTH: f32 = 520.0;
pub const DEFAULT_CARD_HEIGHT: f32 = 360.0;
const HEADER_HEIGHT: f32 = 44.0;

/// Font size for a card's header title.
///
/// Deliberately not `theme.title_font_size`: that one is sized for a full
/// screen title (40 px) and does not fit a 44 px card header — a two-word item
/// name wrapped onto a second line and spilled over the close button.
const CARD_TITLE_FONT_SIZE: f32 = 22.0;

/// The title has to fit the header on a single line. Checked at compile time
/// rather than in a `#[test]`: both sides are constants, so a runtime assert
/// adds nothing (and clippy rightly flags it).
const _: () = assert!(CARD_TITLE_FONT_SIZE < HEADER_HEIGHT);
const INNER_PADDING: f32 = 14.0;
/// Inset for content inside the ornate `panel_large_left` frame. The gold
/// corners occupy ~80 px; anything smaller draws slots on top of them.
pub const FRAME_INNER_PADDING: f32 = 64.0;
const HEADER_BOTTOM_GAP: f32 = 12.0;
/// Breathing room and hairline rule the footer draws above its own widgets.
const FOOTER_PADDING_TOP: f32 = 8.0;
const FOOTER_RULE: f32 = 1.0;
/// Gap between a `CardPositioning::Right` card and the right edge of the viewport.
const RIGHT_EDGE_GAP: f32 = 24.0;
/// Gap between a `CardPositioning::Left` card and the left edge of the viewport.
const LEFT_EDGE_GAP: f32 = 40.0;
/// Leaves the ability hotbar readable under a right-docked card.
pub const HOTBAR_CLEARANCE: f32 = 150.0;
const TOP_EDGE_GAP: f32 = 28.0;

/// Vertical space a framed card spends on itself: the frame inset above and
/// below, the header, and the footer's padding and rule.
///
/// A card that sizes itself to its content adds its body height to this rather
/// than re-deriving the constants at the call site — they are private, and the
/// frame inset in particular (64 px, to clear the panel's gold corners) is easy
/// to forget and lands the body on top of the artwork when it is.
/// `footer_content_height` is the height of the widget the caller puts in the
/// footer, or `0.0` for a card without one.
pub fn framed_chrome_height(footer_content_height: f32) -> f32 {
    let footer = if footer_content_height > 0.0 {
        FOOTER_PADDING_TOP + FOOTER_RULE + footer_content_height
    } else {
        0.0
    };
    FRAME_INNER_PADDING * 2.0 + HEADER_HEIGHT + HEADER_BOTTOM_GAP + footer
}

/// Layout variant for the close button inside the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CardLayout {
    /// Header shows only the title; no close button.
    #[default]
    NoClose,
    /// Header shows the title on the left and a `Close` button on the right.
    WithClose,
}

/// Closure that spawns the body or footer children of a card.
type CardContentSpawner<'a> = Box<dyn FnOnce(&mut ChildSpawnerCommands) + 'a>;

/// Builder for a standard Card.
pub struct CardBuilder<'a> {
    kind: CardKind,
    title: Cow<'a, str>,
    width: Val,
    height: Val,
    layout: CardLayout,
    show_header: bool,
    exclusivity: CardExclusivityPolicy,
    positioning: CardPositioning,
    draggable: bool,
    scrollable: bool,
    frame: Option<CardFrameAssets>,
    body: CardContentSpawner<'a>,
    footer: Option<CardContentSpawner<'a>>,
}

impl<'a> CardBuilder<'a> {
    /// Starts a new card. Title is shown in the header.
    ///
    /// Gameplay cards must call [`CardBuilder::frame`] with
    /// [`CardFrameAssets::load`] so they get the ornate panel rather than the
    /// legacy gray rectangle. Tests without an `AssetServer` may omit it or
    /// pass [`CardFrameAssets::default`].
    pub fn new(kind: CardKind, title: impl Into<Cow<'a, str>>) -> Self {
        Self {
            kind,
            title: title.into(),
            width: Val::Px(DEFAULT_CARD_WIDTH),
            height: Val::Px(DEFAULT_CARD_HEIGHT),
            layout: CardLayout::NoClose,
            show_header: true,
            exclusivity: CardExclusivityPolicy::default(),
            positioning: CardPositioning::Center,
            draggable: false,
            scrollable: false,
            frame: None,
            body: Box::new(|_| {}),
            footer: None,
        }
    }

    /// Overrides the card width.
    pub fn width(mut self, width: Val) -> Self {
        self.width = width;
        self
    }

    /// Overrides the card height.
    pub fn height(mut self, height: Val) -> Self {
        self.height = height;
        self
    }

    /// Sets the card positioning (Center or Right).
    pub fn positioning(mut self, positioning: CardPositioning) -> Self {
        self.positioning = positioning;
        self
    }

    /// Enables dragging the card window around by holding its header.
    pub fn draggable(mut self) -> Self {
        self.draggable = true;
        self
    }

    /// Applies a resizable decorative frame to the card.
    pub fn frame(mut self, frame: CardFrameAssets) -> Self {
        self.frame = Some(frame);
        self
    }

    /// Puts the body inside a scroll view (mouse wheel + draggable thumb).
    ///
    /// Without this, a body taller than the card does not clip: it draws
    /// straight over the world outside the card's own background, and whatever
    /// falls past the footer is simply unreachable.
    pub fn scrollable(mut self) -> Self {
        self.scrollable = true;
        self
    }

    /// Hides the title header, leaving the decorative frame and body visible.
    pub fn headerless(mut self) -> Self {
        self.show_header = false;
        self
    }

    /// Adds a `Close` button to the header (sets layout to [`CardLayout::WithClose`]).
    pub fn closeable(mut self) -> Self {
        self.layout = CardLayout::WithClose;
        self
    }

    /// Marks this card as `Exclusive` (closes other non-`Coexist` cards on open).
    pub fn exclusive(mut self) -> Self {
        self.exclusivity = CardExclusivityPolicy::Exclusive;
        self
    }

    /// Marks this card as `Coexist` (can stay open alongside other cards).
    pub fn coexist(mut self) -> Self {
        self.exclusivity = CardExclusivityPolicy::Coexist;
        self
    }

    /// Supplies the body content.
    pub fn with_body<F>(mut self, body: F) -> Self
    where
        F: FnOnce(&mut ChildSpawnerCommands) + 'a,
    {
        self.body = Box::new(body);
        self
    }

    /// Supplies an optional footer.
    pub fn with_footer<F>(mut self, footer: F) -> Self
    where
        F: FnOnce(&mut ChildSpawnerCommands) + 'a,
    {
        self.footer = Some(Box::new(footer));
        self
    }

    /// Spawns the card into the world and returns the root `CardWindow` entity.
    pub fn spawn(self, commands: &mut Commands<'_, '_>, theme: &UiTheme) -> Entity {
        let Self {
            kind,
            title,
            width,
            height,
            layout,
            show_header,
            exclusivity,
            positioning,
            draggable,
            scrollable,
            frame,
            body,
            footer,
        } = self;

        let framed = frame.is_some();
        let header_style = HeaderStyle::from_theme(theme, framed);

        // Cards are placed relative to the viewport, never to a fixed
        // resolution: a 50% inset plus a negative half-size margin. Centring
        // against a hardcoded 1920x1080
        // put every card partly or fully off-screen at the default 800x600
        // window (`bins/game/src/main.rs`).
        //
        // Sizes whose extent is not known at build time (anything but
        // `Val::Px`) fall back to `margin: auto` centring, which does not need
        // the size.
        let (top, bottom, margin_top) = match (positioning, height) {
            (_, Val::Px(h)) => (Val::Percent(50.0), Val::Auto, Val::Px(-h * 0.5)),
            (CardPositioning::Right | CardPositioning::Left, _) => {
                (Val::Px(TOP_EDGE_GAP), Val::Px(HOTBAR_CLEARANCE), Val::Auto)
            }
            _ => (Val::Px(0.0), Val::Px(0.0), Val::Auto),
        };
        let (left, right, margin_left) = match positioning {
            CardPositioning::Center => match width {
                Val::Px(w) => (Val::Percent(50.0), Val::Auto, Val::Px(-w * 0.5)),
                _ => (Val::Px(0.0), Val::Px(0.0), Val::Auto),
            },
            CardPositioning::Right => (Val::Auto, Val::Px(RIGHT_EDGE_GAP), Val::Auto),
            CardPositioning::Left => (Val::Px(LEFT_EDGE_GAP), Val::Auto, Val::Auto),
        };
        let margin = UiRect {
            left: margin_left,
            right: Val::Auto,
            top: margin_top,
            bottom: Val::Auto,
        };

        let mut card_root_cmd = commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                width,
                height,
                left,
                right,
                top,
                bottom,
                margin,
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(if framed {
                    FRAME_INNER_PADDING
                } else {
                    INNER_PADDING
                })),
                row_gap: Val::Px(HEADER_BOTTOM_GAP),
                border: UiRect::all(if framed { Val::Px(0.0) } else { Val::Px(1.5) }),
                border_radius: BorderRadius::all(Val::Px(10.0)),
                overflow: Overflow::clip(),
                ..default()
            },
            // The complete panel texture already contains its own dark center.
            // A root background here would also fill the transparent pixels
            // around the decorative corners.
            BackgroundColor(if framed { Color::NONE } else { theme.panel_bg }),
            BorderColor {
                top: Color::srgba(0.35, 0.38, 0.45, 0.6),
                right: Color::srgba(0.35, 0.38, 0.45, 0.6),
                bottom: Color::srgba(0.35, 0.38, 0.45, 0.6),
                left: Color::srgba(0.35, 0.38, 0.45, 0.6),
            },
            CardWindow { kind, exclusivity },
            Button,
        ));

        if draggable {
            card_root_cmd.insert(DraggableCard);
        }

        if let Some(frame) = frame {
            card_root_cmd.with_children(|card| spawn_card_frame(card, frame));
        }

        // The body content is filled in *after* this block: a scrollable body
        // needs `&mut Commands`, which `card_root_cmd` is holding borrowed.
        let mut body_container = Entity::PLACEHOLDER;
        card_root_cmd.with_children(|card_root| {
            if show_header {
                spawn_header(card_root, kind, layout, &title, &header_style);
            }

            body_container = card_root
                .spawn((
                    Node {
                        width: Val::Percent(100.0),
                        flex_grow: 1.0,
                        // Without this the body reports its full content height
                        // to the flex layout and pushes the footer out of the
                        // card instead of scrolling inside it.
                        min_height: Val::Px(0.0),
                        flex_direction: FlexDirection::Column,
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    CardBody,
                ))
                .id();

            if let Some(footer_fn) = footer {
                card_root
                    .spawn((
                        Node {
                            width: Val::Percent(100.0),
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(8.0),
                            padding: UiRect::top(Val::Px(FOOTER_PADDING_TOP)),
                            border: UiRect::top(Val::Px(FOOTER_RULE)),
                            flex_shrink: 0.0,
                            ..default()
                        },
                        BorderColor {
                            top: Color::srgba(1.0, 1.0, 1.0, 0.1),
                            right: Color::NONE,
                            bottom: Color::NONE,
                            left: Color::NONE,
                        },
                        CardFooter,
                    ))
                    .with_children(footer_fn);
            }
        });
        let card_id = card_root_cmd.id();

        if scrollable {
            spawn_scroll_view(commands, body_container, theme, |commands| {
                commands.spawn(Node::default()).with_children(body).id()
            });
        } else {
            commands.entity(body_container).with_children(body);
        }

        card_id
    }
}

fn spawn_card_frame(parent: &mut ChildSpawnerCommands, frame: CardFrameAssets) {
    parent.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            top: Val::Px(0.0),
            bottom: Val::Px(0.0),
            ..default()
        },
        ornate_panel_image(frame.panel),
        // Absolute overlay would otherwise win hit-testing over the header.
        Pickable::IGNORE,
    ));
}

fn spawn_header(
    parent: &mut ChildSpawnerCommands,
    kind: CardKind,
    layout: CardLayout,
    title: &str,
    style: &HeaderStyle,
) {
    let mut header_cmd = parent.spawn((
        Button,
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(HEADER_HEIGHT),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            padding: UiRect::axes(Val::Px(12.0), Val::Px(6.0)),
            border_radius: BorderRadius::all(Val::Px(6.0)),
            ..default()
        },
        // The ornate panel already provides the header field; a gray wash
        // on top of it reads as the old flat card chrome.
        BackgroundColor(if style.framed {
            Color::NONE
        } else {
            Color::srgba(1.0, 1.0, 1.0, 0.06)
        }),
        CardHeader,
    ));

    // The whole header is the drag handle, so it carries the marker whether or
    // not the card is draggable-decorated. There used to be a `≡` glyph here
    // too; Bevy's built-in font is an ASCII subset, so it rendered as a blank
    // box rather than a grip.
    header_cmd.insert(CardHeaderDragHandle);

    header_cmd.with_children(|header| {
        header.spawn((
            Text::new(title.to_string()),
            TextFont {
                font_size: FontSize::Px(style.title_font_size),
                ..default()
            },
            TextColor(style.text_color),
            // A long item name must be cut, never wrapped: the header has a
            // fixed height, so a second line draws straight over the body and
            // over the close button.
            TextLayout {
                linebreak: LineBreak::NoWrap,
                ..default()
            },
            Node {
                flex_shrink: 1.0,
                overflow: Overflow::clip_x(),
                ..default()
            },
            // Bevy reports Interaction on the picked leaf. Ignore the title
            // so the header Button keeps the hit, and still carry Interaction
            // so a title press can walk up to [`CardHeaderDragHandle`].
            Pickable::IGNORE,
            Interaction::default(),
        ));

        if layout == CardLayout::WithClose {
            spawn_close_button(header, kind, style);
        }
    });
}

fn spawn_close_button(header: &mut ChildSpawnerCommands, kind: CardKind, style: &HeaderStyle) {
    spawn_bar_child(
        header,
        "Close",
        style.button_font_size * 0.55,
        style.button_text_color,
        Val::Px(92.0),
        Val::Px(30.0),
        BarButtonKind::Neutral,
        CloseCardButton { kind },
    );
}

/// Bundles the theme values used by the header so helper functions stay
/// under clippy's argument-count threshold.
struct HeaderStyle {
    title_font_size: f32,
    button_font_size: f32,
    text_color: Color,
    button_text_color: Color,
    framed: bool,
}

impl HeaderStyle {
    fn from_theme(theme: &UiTheme, framed: bool) -> Self {
        Self {
            title_font_size: CARD_TITLE_FONT_SIZE,
            button_font_size: theme.button_font_size,
            text_color: theme.text_color,
            button_text_color: theme.button_text_color,
            framed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_theme() -> UiTheme {
        UiTheme::default()
    }

    #[test]
    fn spawn_creates_one_card_window_with_header_and_body() {
        let mut app = App::new();
        let theme = test_theme();
        let mut commands = app.world_mut().commands();

        let card_entity = CardBuilder::new(CardKind::Generic, "Test")
            .with_body(|body| {
                body.spawn(Text::new("hello"));
            })
            .spawn(&mut commands, &theme);

        app.update();

        let world = app.world();
        let window = world
            .get::<CardWindow>(card_entity)
            .expect("card root has CardWindow");
        assert_eq!(window.kind, CardKind::Generic);
        assert_eq!(window.exclusivity, CardExclusivityPolicy::Exclusive);

        let world = app.world_mut();
        let mut headers = world.query::<&CardHeader>();
        assert_eq!(headers.iter(world).count(), 1);

        let world = app.world_mut();
        let mut bodies = world.query::<&CardBody>();
        assert_eq!(bodies.iter(world).count(), 1);

        let world = app.world_mut();
        let mut footers = world.query::<&CardFooter>();
        assert_eq!(footers.iter(world).count(), 0);
    }

    /// The regression: a body taller than the card drew straight over the world
    /// outside the card background, and everything past the footer was
    /// unreachable. A scrollable card must put its content behind a viewport.
    #[test]
    fn scrollable_card_wraps_its_body_in_a_scroll_view() {
        use crate::ui::scrollbar::{ScrollContent, ScrollView};

        let mut app = App::new();
        let theme = test_theme();
        let mut commands = app.world_mut().commands();

        CardBuilder::new(CardKind::ItemDetail, "Tall")
            .scrollable()
            .with_body(|body| {
                body.spawn(Text::new("line"));
            })
            .spawn(&mut commands, &theme);

        app.update();

        let world = app.world_mut();
        let mut views = world.query::<&ScrollView>();
        assert_eq!(views.iter(world).count(), 1, "one viewport per scroll body");

        let world = app.world_mut();
        let mut contents = world.query::<&ScrollContent>();
        assert_eq!(contents.iter(world).count(), 1);
    }

    /// A plain card must stay exactly as it was: no viewport, body content
    /// parented straight to `CardBody`.
    #[test]
    fn a_non_scrollable_card_has_no_scroll_view() {
        use crate::ui::scrollbar::ScrollView;

        let mut app = App::new();
        let theme = test_theme();
        let mut commands = app.world_mut().commands();

        CardBuilder::new(CardKind::Generic, "Plain")
            .with_body(|body| {
                body.spawn(Text::new("line"));
            })
            .spawn(&mut commands, &theme);

        app.update();

        let world = app.world_mut();
        let mut views = world.query::<&ScrollView>();
        assert_eq!(views.iter(world).count(), 0);
    }

    /// The header has a fixed height, so a title that wraps draws over the body
    /// and over the close button — as "Magic Staff" did at the 40 px screen
    /// title size. It must be laid out no-wrap, at a size that fits.
    #[test]
    fn header_title_never_wraps_and_fits_the_header() {
        let mut app = App::new();
        let theme = test_theme();
        let mut commands = app.world_mut().commands();

        CardBuilder::new(CardKind::ItemDetail, "A Very Long Item Name Indeed")
            .closeable()
            .spawn(&mut commands, &theme);

        app.update();

        let world = app.world_mut();
        let mut titles = world.query::<(&Text, &TextLayout)>();
        let (_, layout) = titles
            .iter(world)
            .find(|(text, _)| text.0.starts_with("A Very Long"))
            .expect("header title spawned");
        assert_eq!(layout.linebreak, LineBreak::NoWrap);
    }

    #[test]
    fn closeable_adds_close_button() {
        let mut app = App::new();
        let theme = test_theme();
        let mut commands = app.world_mut().commands();

        CardBuilder::new(CardKind::Generic, "Test")
            .closeable()
            .spawn(&mut commands, &theme);

        app.update();

        let world = app.world_mut();
        let mut close_buttons = world.query::<&CloseCardButton>();
        assert_eq!(close_buttons.iter(world).count(), 1);
    }

    #[test]
    fn close_button_uses_sliced_bar_art() {
        let mut app = App::new();
        let theme = test_theme();
        let mut commands = app.world_mut().commands();

        CardBuilder::new(CardKind::Generic, "Test")
            .closeable()
            .spawn(&mut commands, &theme);

        app.update();

        let world = app.world_mut();
        let mut query = world.query::<(
            &CloseCardButton,
            &ImageNode,
            &crate::ui::button::UiButtonImages,
        )>();
        let (_, image, _) = query
            .iter(world)
            .next()
            .expect("close button carries bar visuals");
        assert!(matches!(image.image_mode, NodeImageMode::Sliced(_)));
    }

    /// Regression: centring used to be computed against a hardcoded 1920x1080,
    /// so at the default 800x600 window every card spawned off-screen. The
    /// placement must be expressed in viewport-relative terms instead.
    #[test]
    fn centered_card_is_positioned_relative_to_the_viewport() {
        let mut app = App::new();
        let theme = test_theme();
        let mut commands = app.world_mut().commands();

        let entity = CardBuilder::new(CardKind::Generic, "Test")
            .width(Val::Px(400.0))
            .height(Val::Px(300.0))
            .spawn(&mut commands, &theme);

        app.update();

        let node = app.world().get::<Node>(entity).expect("card node");
        assert_eq!(node.left, Val::Percent(50.0));
        assert_eq!(node.top, Val::Percent(50.0));
        assert_eq!(node.margin.left, Val::Px(-200.0));
        assert_eq!(node.margin.top, Val::Px(-150.0));
    }

    #[test]
    fn right_positioned_card_anchors_to_the_right_edge() {
        let mut app = App::new();
        let theme = test_theme();
        let mut commands = app.world_mut().commands();

        let entity = CardBuilder::new(CardKind::Inventory, "Inventory")
            .width(Val::Px(400.0))
            .height(Val::Px(300.0))
            .positioning(CardPositioning::Right)
            .spawn(&mut commands, &theme);

        app.update();

        let node = app.world().get::<Node>(entity).expect("card node");
        assert_eq!(node.right, Val::Px(RIGHT_EDGE_GAP));
        assert_eq!(node.left, Val::Auto);
        // Vertically centred like any other card.
        assert_eq!(node.top, Val::Percent(50.0));
        assert_eq!(node.margin.top, Val::Px(-150.0));
    }

    #[test]
    fn left_positioned_card_anchors_to_the_left_edge() {
        let mut app = App::new();
        let theme = test_theme();
        let mut commands = app.world_mut().commands();

        let entity = CardBuilder::new(CardKind::Inventory, "Inventory")
            .width(Val::Px(400.0))
            .height(Val::Px(300.0))
            .positioning(CardPositioning::Left)
            .spawn(&mut commands, &theme);

        app.update();

        let node = app.world().get::<Node>(entity).expect("card node");
        assert_eq!(node.left, Val::Px(LEFT_EDGE_GAP));
        assert_eq!(node.right, Val::Auto);
        // Vertically centred like any other card.
        assert_eq!(node.top, Val::Percent(50.0));
        assert_eq!(node.margin.top, Val::Px(-150.0));
    }

    #[test]
    fn framed_card_pads_content_inside_the_ornate_border() {
        let mut app = App::new();
        let theme = test_theme();
        let mut commands = app.world_mut().commands();

        let entity = CardBuilder::new(CardKind::Inventory, "Inventory")
            .frame(CardFrameAssets::default())
            .spawn(&mut commands, &theme);

        app.update();

        let node = app.world().get::<Node>(entity).expect("card node");
        assert_eq!(node.padding.left, Val::Px(FRAME_INNER_PADDING));
        assert_eq!(node.padding.right, Val::Px(FRAME_INNER_PADDING));
        assert_eq!(node.padding.top, Val::Px(FRAME_INNER_PADDING));
        assert_eq!(node.padding.bottom, Val::Px(FRAME_INNER_PADDING));
    }

    #[test]
    fn auto_height_right_card_clears_the_hotbar() {
        let mut app = App::new();
        let theme = test_theme();
        let mut commands = app.world_mut().commands();

        let entity = CardBuilder::new(CardKind::Inventory, "Inventory")
            .width(Val::Px(400.0))
            .height(Val::Auto)
            .positioning(CardPositioning::Right)
            .spawn(&mut commands, &theme);

        app.update();

        let node = app.world().get::<Node>(entity).expect("card node");
        assert_eq!(node.top, Val::Px(TOP_EDGE_GAP));
        assert_eq!(node.bottom, Val::Px(HOTBAR_CLEARANCE));
        assert_eq!(node.right, Val::Px(RIGHT_EDGE_GAP));
    }

    #[test]
    fn coexist_flag_is_preserved_on_card_window() {
        let mut app = App::new();
        let theme = test_theme();
        let mut commands = app.world_mut().commands();

        let entity = CardBuilder::new(CardKind::ItemDetail, "Detail")
            .coexist()
            .spawn(&mut commands, &theme);

        app.update();

        let world = app.world();
        let window = world.get::<CardWindow>(entity).expect("card window");
        assert_eq!(window.exclusivity, CardExclusivityPolicy::Coexist);
    }

    #[test]
    fn draggable_framed_card_keeps_a_header_drag_handle() {
        let mut app = App::new();
        let theme = test_theme();
        let mut commands = app.world_mut().commands();

        let entity = CardBuilder::new(CardKind::ItemDetail, "Staffa da Mago")
            .frame(CardFrameAssets::default())
            .draggable()
            .closeable()
            .spawn(&mut commands, &theme);

        app.update();

        assert!(
            app.world().get::<DraggableCard>(entity).is_some(),
            "item detail must stay .draggable()"
        );

        let world = app.world_mut();
        let mut handles = world.query::<(&CardHeaderDragHandle, &Node)>();
        let (_, header) = handles.iter(world).next().expect("header drag handle");
        assert_eq!(header.width, Val::Percent(100.0));
        assert_eq!(header.height, Val::Px(HEADER_HEIGHT));

        let world = app.world_mut();
        let mut frames = world.query::<(&ImageNode, &Node, &Pickable)>();
        assert!(
            frames.iter(world).any(|(_, node, pickable)| {
                node.position_type == PositionType::Absolute
                    && node.left == Val::Px(0.0)
                    && node.right == Val::Px(0.0)
                    && *pickable == Pickable::IGNORE
            }),
            "ornate frame must not steal header hits"
        );

        let world = app.world_mut();
        let mut titles = world.query::<(&Text, &Pickable)>();
        let (_, pickable) = titles
            .iter(world)
            .find(|(text, _)| text.0 == "Staffa da Mago")
            .expect("header title");
        assert_eq!(*pickable, Pickable::IGNORE);
    }

    #[test]
    fn framed_card_header_is_transparent() {
        let mut app = App::new();
        let theme = test_theme();
        let mut commands = app.world_mut().commands();

        CardBuilder::new(CardKind::ItemDetail, "Staff")
            .frame(CardFrameAssets::default())
            .spawn(&mut commands, &theme);

        app.update();

        let world = app.world_mut();
        let mut headers = world.query::<(&CardHeader, &BackgroundColor)>();
        let (_, bg) = headers.iter(world).next().expect("header");
        assert_eq!(bg.0, Color::NONE);
    }

    #[test]
    fn close_button_uses_bar_image_not_a_flat_fill() {
        let mut app = App::new();
        let theme = test_theme();
        let mut commands = app.world_mut().commands();

        CardBuilder::new(CardKind::ItemDetail, "Staff")
            .frame(CardFrameAssets::default())
            .closeable()
            .spawn(&mut commands, &theme);

        app.update();

        let world = app.world_mut();
        let mut close_buttons = world.query::<(&CloseCardButton, &ImageNode, &BackgroundColor)>();
        let (_, image, fill) = close_buttons.iter(world).next().expect("close button");
        assert!(
            matches!(image.image_mode, NodeImageMode::Sliced(_)),
            "close button must be 9-sliced bar art"
        );
        assert_eq!(
            fill.0,
            Color::NONE,
            "close button must not keep a flat BackgroundColor fill"
        );
    }
}
