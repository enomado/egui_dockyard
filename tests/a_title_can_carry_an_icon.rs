//! A tab's title is a row of atoms, so it can carry an icon as well as a name.
//!
//! # Why this is a test file of its own
//!
//! A title used to be a `WidgetText`, and text is the one thing a galley can hold: an icon in a
//! tab bar was not something a consumer could ask for, however the tab was styled. `MIN_SQUEEZED_TEXT`
//! said so in as many words — "the browsers this follows squeeze a tab down to its favicon; we have
//! no favicon, so the text itself is the last thing to go".
//!
//! Now a title is `egui::Atoms`, and what this file states is what a picture of a tab bar cannot:
//!
//! * an icon is **drawn**, and drawn *before* the name — the order the consumer gave;
//! * the tab is **measured** with the icon in it. This is the half that no screenshot would catch
//!   and that a painting-only implementation would fail: an icon drawn but not measured lands on
//!   top of a name that was given the whole tab, and the two overlap;
//! * a bar too full for its names **keeps the icons**, which is what "the icon is the last thing to
//!   go" means once the room runs out — the favicon behaviour the comment above described;
//! * a side strip carries the icon **upright** while it turns the name a quarter turn. A rotation
//!   applied to the whole title would be invisible in a count of what was painted and obvious on
//!   screen, so the icon here is deliberately oblong: turned, it would come back the other way up.
//!
//! The icon is a texture that no loader has to fetch (`TextureId::User`), so these scenes stay
//! headless — what is asserted is the layout, not the decoding of any image format.

use egui::load::SizedTexture;
use egui::{
    Atoms, CentralPanel, Color32, Context, Id, Image, LayerId, Pos2, RawInput, Rect, Shape,
    TextureId, Ui, Vec2,
};
use egui_dockyard::{
    DockArea, DockLayout, DockState, NodeId, NodePath, Style, SurfaceIndex, TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_title_can_carry_an_icon";

/// Half a device pixel: rectangles are snapped to whole pixels, so an exact comparison would be
/// reporting the snapping rather than the property.
const TOLERANCE: f32 = 0.5;

/// The texture every icon in this file is drawn from. `User`, so nothing has to be loaded.
const ICON: TextureId = TextureId::User(7);

/// Deliberately **not** square, and shorter than a line of text is tall.
///
/// Oblong is what makes the strip's assertion possible at all: an icon turned with the name would
/// come back 8 wide and 16 tall, and no count of what was painted would notice.
const ICON_SIZE: Vec2 = Vec2::new(16.0, 8.0);

/// What stands between two atoms of a title — [`egui::style::Spacing::icon_spacing`], which is
/// what `measure_title` asks the `Ui` for.
fn gap() -> f32 {
    egui::Style::default().spacing.icon_spacing
}

fn icon() -> Image<'static> {
    Image::from_texture(SizedTexture::new(ICON, ICON_SIZE))
}

/// Long enough that a handful of them cannot share one bar uncut.
fn long_name(index: usize) -> String {
    format!("Panel number {index} with a name of some length")
}

/// The natural size of an icon that has none of its own — a square the size an icon set is usually
/// drawn at, and taller than either the line or the em, so that the room it is offered is what
/// decides how big it lands.
const DRAWN_AT: Vec2 = Vec2::splat(24.0);

/// Names its tabs, with an icon in front of each one or without, so that the same scene can be
/// asked both questions and the difference between the answers is the icon itself.
#[derive(Clone)]
struct Viewer {
    icons: bool,
    /// A colour the consumer asked for, rather than the default the dock is free to replace.
    tint: Option<Color32>,
    /// An icon that takes the room it is offered, the way a *loaded* one does.
    ///
    /// [`Image::from_texture`] is [`egui::ImageFit::Exact`] — egui assumes a consumer handing over
    /// a sized texture means that size — so the icons above are the one kind that never asks how
    /// much room it has. A set loaded from bytes or a uri (which is every real icon set, ours
    /// included) is `Fraction` and scales to the offer, and that is the kind whose size the dock
    /// decides. `fit_to_fraction` puts a texture on those terms without needing a loader.
    fitting: bool,
}

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> Atoms<'static> {
        if self.icons {
            let base = if self.fitting {
                Image::from_texture(SizedTexture::new(ICON, DRAWN_AT))
                    .fit_to_fraction(Vec2::splat(1.0))
            } else {
                icon()
            };
            let icon = match self.tint {
                Some(tint) => base.tint(tint),
                None => base,
            };
            Atoms::new((icon, tab.clone()))
        } else {
            Atoms::new(tab.clone())
        }
    }

    fn ui(&mut self, ui: &mut Ui, tab: &Self::Tab) {
        ui.label(tab.as_str());
    }
}

fn style() -> Style {
    Style::from_egui(&egui::Style::default())
}

fn path(node: NodeId) -> NodePath {
    NodePath::new(SurfaceIndex::main(), node)
}

/// One piece of text the frame painted, and where it was allowed to show.
#[derive(Clone, Debug)]
struct Painted {
    text: String,
    rect: Rect,
    /// What the painter lets through — the slot the title was given, which is what says which tab
    /// this piece belongs to and how much room that tab had.
    clip: Rect,
    /// The colour the dock handed the name — [`egui::epaint::TextShape::fallback_color`], which is
    /// the `color` argument `paint_title` was called with.
    color: Color32,
}

/// One icon the frame painted, and the colour it was painted in.
///
/// The colour is the rectangle's own fill, which is what a textured rectangle multiplies its
/// texture by — i.e. the image's tint.
#[derive(Clone, Copy, Debug)]
struct PaintedIcon {
    rect: Rect,
    tint: Color32,
}

/// What one frame painted: the names, and the icons beside them.
#[derive(Clone, Debug, Default)]
struct Frame {
    names: Vec<Painted>,
    icons: Vec<PaintedIcon>,
}

/// Every piece of text the dock's own layer painted this frame.
///
/// Read *inside* the pass, because `end_pass` flattens the layers and the layer a shape belongs to
/// is gone by the time the frame returns.
fn painted_text(ctx: &Context) -> Vec<Painted> {
    ctx.graphics(|graphics| {
        graphics
            .get(LayerId::background())
            .map(|list| {
                list.all_entries()
                    .filter_map(|entry| match &entry.shape {
                        Shape::Text(text) => Some(Painted {
                            text: text
                                .galley
                                .rows
                                .iter()
                                .flat_map(|placed| placed.row.glyphs.iter().map(|glyph| glyph.chr))
                                .collect(),
                            rect: entry.shape.visual_bounding_rect(),
                            clip: entry.clip_rect,
                            color: text.fallback_color,
                        }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Every icon the dock painted this frame, by rectangle.
///
/// Selected by the *texture it is filled from*, so the tab's own background — a filled rectangle
/// in the same layer — is never mistaken for one.
fn painted_icons(ctx: &Context) -> Vec<PaintedIcon> {
    ctx.graphics(|graphics| {
        graphics
            .get(LayerId::background())
            .map(|list| {
                list.all_entries()
                    .filter_map(|entry| match &entry.shape {
                        Shape::Rect(rect) => rect
                            .brush
                            .as_ref()
                            .is_some_and(|brush| brush.fill_texture_id == ICON)
                            .then_some(PaintedIcon {
                                rect: rect.rect,
                                tint: rect.fill,
                            }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// A few quiet frames, answering with what the last one painted: the geometry map has to settle
/// before the bar is sharing out a width it will keep.
fn frames(ctx: &Context, state: &mut DockState<String>, style: &Style, icons: bool) -> Frame {
    frames_with(
        ctx,
        state,
        style,
        &Viewer {
            icons,
            tint: None,
            fitting: false,
        },
    )
}

/// The same scene, driven by a viewer the caller built — used where what is being asked about is
/// the viewer's own request rather than the presence of an icon.
fn frames_with(
    ctx: &Context,
    state: &mut DockState<String>,
    style: &Style,
    viewer: &Viewer,
) -> Frame {
    let mut painted = Frame::default();
    for _ in 0..4 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            CentralPanel::default().show(ui, |ui| {
                DockArea::new(state)
                    .id(Id::new(DOCK_ID))
                    .style(style.clone())
                    .show_leaf_close_all_buttons(false)
                    .show_leaf_collapse_buttons(true)
                    .collapse_sideways(true)
                    .show_inside(ui, &mut viewer.clone());
            });
            painted = Frame {
                names: painted_text(ui.ctx()),
                icons: painted_icons(ui.ctx()),
            };
        });
        // `TexturesDelta` panics when dropped with deltas nobody applied, and there is no backend.
        output.textures_delta.clear();
    }
    painted
}

/// The strip of screen the leaf's tab bar occupies: the top `tab_bar.height` of the leaf.
fn bar_of(ctx: &Context, node: NodeId, style: &Style) -> Rect {
    let leaf = DockLayout::load(ctx, Id::new(DOCK_ID))
        .rect(path(node))
        .expect("the leaf was laid out");
    Rect::from_min_size(leaf.min, Vec2::new(leaf.width(), style.tab_bar.height))
}

fn rect_of(ctx: &Context, node: NodeId) -> Rect {
    DockLayout::load(ctx, Id::new(DOCK_ID))
        .rect(path(node))
        .expect("the node was laid out")
}

/// The titles shown inside `area`, selected by *where they were allowed to show* — a title is
/// drawn whole and clipped to its slot, so the clip is what says which bar it belongs to. The
/// leaf's body paints its own text in the same frame.
fn names_in(painted: &[Painted], area: Rect) -> Vec<Painted> {
    painted
        .iter()
        .filter(|item| area.expand(TOLERANCE).contains_rect(item.clip))
        .cloned()
        .collect()
}

fn icons_in(painted: &[PaintedIcon], area: Rect) -> Vec<PaintedIcon> {
    painted
        .iter()
        .filter(|icon| area.expand(TOLERANCE).contains_rect(icon.rect))
        .copied()
        .collect()
}

/// One leaf filling the screen, holding `count` tabs with names of some length.
fn a_leaf_of(count: usize) -> (DockState<String>, NodeId) {
    let tabs: Vec<String> = (0..count).map(long_name).collect();
    let state = DockState::new(tabs);
    let root = state.main_surface().root().unwrap();
    (state, root)
}

/// A title carrying an icon draws it, once, and in front of the name.
#[test]
fn an_icon_is_drawn_before_the_name() {
    let style = style();
    let (mut state, leaf) = a_leaf_of(1);

    let ctx = Context::default();
    let painted = frames(&ctx, &mut state, &style, true);
    let bar = bar_of(&ctx, leaf, &style);

    let icons = icons_in(&painted.icons, bar);
    assert_eq!(
        icons.len(),
        1,
        "one tab carrying one icon should paint exactly one, got {icons:?}"
    );
    let icon = icons[0].rect;
    assert!(
        (icon.size() - ICON_SIZE).length() <= TOLERANCE,
        "the icon should keep the size it was given, got {:?}",
        icon.size()
    );

    let names = names_in(&painted.names, bar);
    assert_eq!(names.len(), 1, "one tab, one name: {:?}", names);
    assert!(
        icon.right() <= names[0].rect.left() + TOLERANCE,
        "the icon was given first, so it belongs in front of the name: icon {icon:?}, name {:?}",
        names[0].rect
    );
}

/// The tab is measured with its icon in it: the same tab is wider by exactly the icon and the gap
/// behind it.
///
/// This is the half a screenshot cannot see. An icon that is drawn but not measured leaves the
/// name the whole tab, and the two are painted over each other.
#[test]
fn an_icon_widens_the_tab_that_carries_it() {
    let style = style();

    // Two scenes rather than two tabs: what is compared is the room one tab was given with an icon
    // and without, and a bar shares its width out between whatever tabs it has.
    let (mut plain_state, plain_leaf) = a_leaf_of(1);
    let plain_ctx = Context::default();
    let plain = frames(&plain_ctx, &mut plain_state, &style, false);
    let plain_bar = bar_of(&plain_ctx, plain_leaf, &style);

    let (mut with_state, with_leaf) = a_leaf_of(1);
    let with_ctx = Context::default();
    let with = frames(&with_ctx, &mut with_state, &style, true);
    let with_bar = bar_of(&with_ctx, with_leaf, &style);

    let plain_slot = names_in(&plain.names, plain_bar);
    let with_slot = names_in(&with.names, with_bar);
    assert_eq!(plain_slot.len(), 1, "one tab, one name: {plain_slot:?}");
    assert_eq!(with_slot.len(), 1, "one tab, one name: {with_slot:?}");

    let widened = with_slot[0].clip.width() - plain_slot[0].clip.width();
    let expected = ICON_SIZE.x + gap();
    assert!(
        (widened - expected).abs() <= TOLERANCE,
        "the tab should grow by the icon and the gap behind it ({expected}), grew by {widened}"
    );

    // And the name itself is not the thing that paid for it: the same name is laid out whole.
    assert_eq!(
        with_slot[0].text, plain_slot[0].text,
        "the name is the same either way"
    );
}

/// A bar too full for its names keeps every icon: the icon is what a squeezed tab is left with,
/// the way a browser keeps its favicon.
#[test]
fn a_squeezed_bar_keeps_every_icon() {
    let style = style();
    let (mut state, leaf) = a_leaf_of(8);

    let ctx = Context::default();
    let painted = frames(&ctx, &mut state, &style, true);
    let bar = bar_of(&ctx, leaf, &style);

    let names = names_in(&painted.names, bar);
    let cut = names
        .iter()
        .filter(|name| !name.clip.expand(1.0).contains_rect(name.rect))
        .count();
    assert!(
        cut > 0,
        "the scene has to be a squeezed bar for this test to say anything; nothing was cut"
    );

    let icons = icons_in(&painted.icons, bar);
    assert_eq!(
        icons.len(),
        8,
        "every tab keeps its icon however squeezed the bar is, got {} of 8",
        icons.len()
    );
}

/// A side strip turns the name a quarter turn and leaves the icon upright, below the name it
/// belongs to — the strip reads bottom to top, so that is where the title starts.
#[test]
fn a_strip_carries_the_icon_upright() {
    let style = style();

    // One leaf collapsed sideways beside another, so there is a strip to read at all.
    let mut state = DockState::new(vec!["open".to_owned()]);
    let open = state.main_surface().root().unwrap();
    let [_, strip] = state.split(
        path(open),
        egui_dockyard::Split::Left,
        0.5,
        egui_dockyard::Node::leaf("Tuning".to_owned()),
    );
    state.main_surface_mut().set_leaf_collapsed(strip, true);

    let ctx = Context::default();
    let painted = frames(&ctx, &mut state, &style, true);
    let strip_rect = rect_of(&ctx, strip);

    let icons = icons_in(&painted.icons, strip_rect);
    assert_eq!(
        icons.len(),
        1,
        "the strip names one tab, so it draws one icon, got {icons:?}"
    );
    let icon = icons[0].rect;
    assert!(
        (icon.size() - ICON_SIZE).length() <= TOLERANCE,
        "an icon stays upright while the name turns — turned, this one would come back {:?}",
        Vec2::new(ICON_SIZE.y, ICON_SIZE.x)
    );

    let names = names_in(&painted.names, strip_rect);
    assert_eq!(names.len(), 1, "the strip names its one tab: {names:?}");
    assert!(
        icon.center().y >= names[0].rect.center().y,
        "a strip reads bottom to top, so the first atom is the lowest: icon {icon:?}, name {:?}",
        names[0].rect
    );
}

/// An icon is painted in the colour its own tab gave the name beside it.
///
/// A monochrome icon that kept the colour it was drawn in would be a set that works on one theme
/// and disappears on the other, and would not follow a tab from inactive to active. The scene has
/// two tabs *because* one of them is the active one: the dock hands those two different text
/// colours, so an implementation that painted every icon in one constant — including the right
/// constant for the active tab — passes half of this and fails the other half.
#[test]
fn an_icon_takes_the_colour_of_its_name() {
    // The two states have to be *told apart* for the scene to be worth running: `Style::from_egui`
    // gives an active tab and an inactive one the same text colour, and against that a constant
    // would satisfy every comparison below. The final assertion holds this honest.
    let mut style = style();
    style.tab.active.text_color = Color32::from_rgb(240, 240, 240);
    style.tab.inactive.text_color = Color32::from_rgb(120, 90, 60);
    let (mut state, leaf) = a_leaf_of(2);

    let ctx = Context::default();
    let painted = frames(&ctx, &mut state, &style, true);
    let bar = bar_of(&ctx, leaf, &style);

    let mut icons = icons_in(&painted.icons, bar);
    let mut names = names_in(&painted.names, bar);
    assert_eq!(icons.len(), 2, "two tabs, two icons: {icons:?}");
    assert_eq!(names.len(), 2, "two tabs, two names: {names:?}");

    // Along the bar, so each icon is read against the name of the same tab.
    icons.sort_by(|a, b| a.rect.left().total_cmp(&b.rect.left()));
    names.sort_by(|a, b| a.rect.left().total_cmp(&b.rect.left()));

    for (icon, name) in icons.iter().zip(&names) {
        assert_eq!(
            icon.tint, name.color,
            "the icon of {:?} should be painted in the colour of its own name, got {:?} against {:?}",
            name.text, icon.tint, name.color
        );
    }

    assert_ne!(
        names[0].color, names[1].color,
        "this scene is only worth running while the active tab and the inactive one are named in \
         different colours — otherwise a constant would satisfy the loop above"
    );
}

/// An icon that has no size of its own is held to the **em** — the type size of the name beside
/// it — and not to the line box that name is laid out in.
///
/// # Why this one needs an icon of a different kind
///
/// Every scene above hands the dock an [`Image::from_texture`], which egui reads as
/// [`egui::ImageFit::Exact`]: it is the one kind of image that never asks how much room it has, so
/// the six tests written before this one could not see the sizing rule at all — they would go on
/// passing whatever the dock offered. Real icon sets are loaded from bytes or a uri, which is
/// `Fraction`, and *that* kind takes the room it is given. This scene puts a texture on those
/// terms so the property can be stated without a loader.
///
/// The em rather than the line is what keeps an icon from swelling to the height of its own tab: a
/// tab bar is a fixed height while the line box is most of it, so an icon given the line lands
/// with a couple of points of tab above and below it while the letters beside it have seven.
#[test]
fn an_icon_of_its_own_size_is_held_to_the_em() {
    let style = style();
    let (mut state, leaf) = a_leaf_of(2);

    let ctx = Context::default();
    let painted = frames_with(
        &ctx,
        &mut state,
        &style,
        &Viewer {
            icons: true,
            tint: None,
            fitting: true,
        },
    );
    let bar = bar_of(&ctx, leaf, &style);

    let font = egui::TextStyle::Button.resolve(&egui::Style::default());
    let em = font.size;
    let line = ctx.fonts_mut(|fonts| fonts.row_height(&font));

    let icons = icons_in(&painted.icons, bar);
    assert_eq!(icons.len(), 2, "two tabs, two icons: {icons:?}");
    for icon in &icons {
        assert!(
            (icon.rect.height() - em).abs() <= TOLERANCE,
            "an icon drawn at {DRAWN_AT:?} should come down to the em ({em}), not to the line \
             ({line}) and not to its own size: got {:?}",
            icon.rect.size()
        );
        // Square in, square out: an icon held by its height must not be stretched by the width it
        // was offered, which is the whole bar.
        assert!(
            (icon.rect.width() - icon.rect.height()).abs() <= TOLERANCE,
            "a square icon should stay square, got {:?}",
            icon.rect.size()
        );
    }

    assert!(
        line > em + 1.0,
        "this scene is only worth running while the line box ({line}) is taller than the em \
         ({em}) — otherwise the rule it states and the one it replaced are the same number"
    );
}

/// A tint the consumer asked for is left alone.
///
/// The rule above replaces the *default* tint, which is what "this icon has no colour of its own"
/// looks like. An icon handed an explicit colour — a multi-coloured logo, a status dot that is
/// meant to stay red — has one, and repainting it in the tab's text colour would be the dock
/// overruling its consumer.
#[test]
fn an_explicit_tint_is_left_alone() {
    let style = style();
    let (mut state, leaf) = a_leaf_of(2);

    let asked_for = Color32::from_rgb(200, 30, 90);
    let ctx = Context::default();
    let painted = frames_with(
        &ctx,
        &mut state,
        &style,
        &Viewer {
            icons: true,
            tint: Some(asked_for),
            fitting: false,
        },
    );
    let bar = bar_of(&ctx, leaf, &style);

    let icons = icons_in(&painted.icons, bar);
    assert_eq!(icons.len(), 2, "two tabs, two icons: {icons:?}");
    for icon in &icons {
        assert_eq!(
            icon.tint, asked_for,
            "the consumer asked for {asked_for:?}, so that is what should be painted"
        );
    }
}
