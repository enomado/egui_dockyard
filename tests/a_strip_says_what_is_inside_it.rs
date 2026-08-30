//! What a strip draws besides its arrow: the names of the tabs it stands for.
//!
//! # Why this is a test file of its own
//!
//! `collapse_sideways` and stowing both end in the same picture — a rectangle one tab bar wide
//! with a single arrow at the top of it — and both were shipped drawing nothing else. That was a
//! deliberate boundary (`PLAN_a_side_can_be_stowed.md`, decision 2: per-leaf marks "would need
//! vertical text or icons in a strip one tab bar wide"), and it is the boundary this file removes.
//!
//! What it states, and what none of the layout tests next door can:
//!
//! * a strip carries the **names** of what is inside it — its own tabs for a collapsed leaf,
//!   every leaf's tabs for a side stowed as a unit;
//! * the names stay **inside the strip**, however long they are. This is the one property the
//!   rotation can get wrong in a way that no assertion about the tree would notice: a quarter
//!   turn the wrong way round still draws every name, just off the edge of the panel;
//! * a click on a name brings the panel back **showing that tab** — asserted on a tab that was
//!   *not* active, so an implementation that merely expands cannot pass;
//! * the arrow keeps its own meaning: come back as you were, with the tab you left showing.
//!
//! The scene for the stowed side is three leaves, for the same reason the gesture's scene in
//! `a_side_can_be_stowed.rs` is: with two, a rule that only reached one leaf would look right.

use egui::{
    CentralPanel, Context, Event, Id, LayerId, Modifiers, PointerButton, Pos2, RawInput, Rect,
    Shape, Ui, Vec2, WidgetText,
};
use egui_dockyard::{
    DockArea, DockLayout, DockState, Node, NodeId, NodePath, Split, Style, SurfaceIndex, TabIndex,
    TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_strip_says_what_is_inside_it";

/// Half a device pixel: every boundary is snapped to whole pixels, so an exact comparison would
/// be reporting the snapping rather than the property.
const TOLERANCE: f32 = 0.5;

/// The mark a strip draws in place of the names it had no room for. A *truncated name* also ends
/// in this character, which is why every assertion about the mark compares the whole text.
const ELLIPSIS: &str = "…";

struct Viewer;

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> WidgetText {
        tab.clone().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &Self::Tab) {
        ui.label(tab.as_str());
    }
}

fn tab(name: &str) -> String {
    name.to_owned()
}

fn style() -> Style {
    Style::from_egui(&egui::Style::default())
}

fn path(node: NodeId) -> NodePath {
    NodePath::new(SurfaceIndex::main(), node)
}

/// One piece of text the frame painted, and where it landed.
#[derive(Clone, Debug)]
struct Painted {
    /// The glyphs actually laid out. Names are laid out *whole* now and cut by the clip rather
    /// than by the text layout, so this is the full name even when only part of it is on screen —
    /// which is why "was this cut?" is `rect` against `clip` below, and not a search for an
    /// ellipsis in here.
    text: String,
    /// Where the glyphs are, whether or not they are visible.
    rect: Rect,
    /// What the painter would let through. A name longer than its slot has a `clip` narrower than
    /// its `rect`: that difference *is* the cut.
    clip: Rect,
    /// Radians clockwise. Zero for ordinary horizontal text, so "was this turned?" is a question
    /// about the shape rather than about its shape's proportions.
    angle: f32,
}

impl Painted {
    /// Whether the strip showed less of this name than the name has.
    ///
    /// With a pixel of slack, since a slot and the name it holds can be given the same length and
    /// still come back a hair apart once both are snapped to the pixel grid. A name that was
    /// actually cut misses by tens of pixels, not by fractions.
    fn cut(&self) -> bool {
        !self.clip.expand(1.0).contains_rect(self.rect)
    }
}

/// Every piece of text the dock's own layer painted this frame.
///
/// Read *inside* the pass, because `end_pass` flattens the layers and the layer a shape belongs
/// to is gone by the time the frame returns.
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
                            angle: text.angle,
                        }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// What one frame painted: the names, and the fades that say a name was cut.
///
/// Both are read *inside* the pass — `end_pass` empties the lists, and reading a frame's shapes
/// after it has returned answers with nothing at all. (This test read the fades afterwards at
/// first, and "no fade was painted" was indistinguishable from "the feature does not work".)
#[derive(Clone, Debug, Default)]
struct Frame {
    names: Vec<Painted>,
    fades: Vec<Rect>,
}

/// One headless frame carrying `events`, answering with what it painted.
fn frame(ctx: &Context, state: &mut DockState<String>, style: &Style, events: Vec<Event>) -> Frame {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
        events,
        ..Default::default()
    };
    let mut painted = Frame::default();
    let mut output = ctx.run_ui(input, |ui| {
        CentralPanel::default().show(ui, |ui| {
            DockArea::new(state)
                .id(Id::new(DOCK_ID))
                .style(style.clone())
                .show_leaf_collapse_buttons(true)
                .collapse_sideways(true)
                .show_inside(ui, &mut Viewer);
        });
        painted = Frame {
            names: painted_text(ui.ctx()),
            fades: painted_fades(ui.ctx()),
        };
    });
    // `TexturesDelta` panics when dropped with deltas nobody applied, and there is no backend here.
    output.textures_delta.clear();
    painted
}

/// Every fade the dock painted this frame, by rectangle.
///
/// A fade is a mesh that runs from fully transparent to fully opaque — that is how a name says it
/// was cut, egui having no text mask to do it with. Selected by the *colours of its vertices*
/// rather than by being a mesh at all, so an ordinary filled shape could never be mistaken for
/// one.
fn painted_fades(ctx: &Context) -> Vec<Rect> {
    ctx.graphics(|graphics| {
        graphics
            .get(LayerId::background())
            .map(|list| {
                list.all_entries()
                    .filter_map(|entry| match &entry.shape {
                        Shape::Mesh(mesh) => {
                            let clear = mesh.vertices.iter().any(|vertex| vertex.color.a() == 0);
                            let solid = mesh.vertices.iter().any(|vertex| vertex.color.a() == 255);
                            (clear && solid).then(|| entry.shape.visual_bounding_rect())
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// A few quiet frames, answering with what the last one painted: a click takes effect on the
/// frame after the one that reports it, and the geometry map has to settle either way.
fn frames(ctx: &Context, state: &mut DockState<String>, style: &Style) -> Frame {
    let mut painted = Frame::default();
    for _ in 0..4 {
        painted = frame(ctx, state, style, Vec::new());
    }
    painted
}

/// Press and release at `at`, then let the layout settle, answering with what is painted after.
fn click(ctx: &Context, state: &mut DockState<String>, style: &Style, at: Pos2) -> Frame {
    for pressed in [true, false] {
        frame(
            ctx,
            state,
            style,
            vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }],
        );
    }
    frames(ctx, state, style)
}

fn layout_of(ctx: &Context) -> DockLayout {
    DockLayout::load(ctx, Id::new(DOCK_ID))
}

fn rect_of(layout: &DockLayout, node: NodeId) -> Rect {
    layout.rect(path(node)).expect("the node was laid out")
}

/// What the strip at `node` painted, in the order it painted it.
///
/// Selected by *where it is allowed to show* rather than by where its glyphs are: a name is drawn
/// whole and clipped to its slot, so the clip is what says which panel this name belongs to. A
/// name whose slot ended up outside the strip is not in this list, and every assertion below that
/// counts names would fail. The sibling leaf paints its own tab bar and body in the same frame.
fn names_in(painted: &[Painted], strip: Rect) -> Vec<Painted> {
    painted
        .iter()
        .filter(|item| strip.expand(TOLERANCE).contains_rect(item.clip))
        .cloned()
        .collect()
}

fn texts(names: &[Painted]) -> Vec<String> {
    names.iter().map(|item| item.text.clone()).collect()
}

/// A leaf of three tabs beside an ordinary one, with the first collapsed sideways into a strip.
fn a_collapsed_leaf_beside_a_column() -> (DockState<String>, NodeId, NodeId) {
    let mut state = DockState::new(vec![tab("open")]);
    let open = state.main_surface().root().unwrap();
    let [_, strip] = state.split(
        path(open),
        Split::Left,
        0.5,
        Node::leaf_with(vec![tab("Geology"), tab("Trajectory"), tab("Schema")]),
    );
    state.main_surface_mut().set_leaf_collapsed(strip, true);
    (state, strip, open)
}

/// Three stacked leaves beside an ordinary one, the stack stowed away as a unit.
fn a_stowed_side_of_three() -> (DockState<String>, NodeId, [NodeId; 3]) {
    let mut state = DockState::new(vec![tab("open")]);
    let open = state.main_surface().root().unwrap();
    let [_, first] = state.split(path(open), Split::Left, 0.5, Node::leaf(tab("Tuning")));
    let [_, second] = state.split(path(first), Split::Below, 0.5, Node::leaf(tab("Debug")));
    let [_, third] = state.split(path(second), Split::Below, 0.5, Node::leaf(tab("Legend")));

    // The side is the child of the root that holds all three, however deep the stack goes.
    let side = state
        .main_surface()
        .top_level_ancestor(first)
        .expect("the stack is a side of the root");
    state.main_surface_mut().set_split_stowed(side, true);
    (state, side, [first, second, third])
}

/// A collapsed leaf's strip carries its own tabs' names, turned a quarter turn, inside itself.
#[test]
fn a_collapsed_leaf_names_its_own_tabs() {
    let style = style();
    let (mut state, strip, _open) = a_collapsed_leaf_beside_a_column();

    let ctx = Context::default();
    let frame = frames(&ctx, &mut state, &style);
    let painted = frame.names.clone();
    let strip_rect = rect_of(&layout_of(&ctx), strip);
    let names = names_in(&painted, strip_rect);

    assert_eq!(
        texts(&names),
        vec!["Geology", "Trajectory", "Schema"],
        "the strip should name every tab it put away, in tab order"
    );
    for name in &names {
        assert!(
            name.angle != 0.0,
            "{:?} was drawn horizontally in a strip one tab bar wide",
            name.text
        );
    }
}

/// A side stowed as a unit names every leaf inside it — all three, in tree order.
#[test]
fn a_stowed_side_names_every_leaf_inside_it() {
    let style = style();
    let (mut state, side, _inside) = a_stowed_side_of_three();

    let ctx = Context::default();
    let frame = frames(&ctx, &mut state, &style);
    let painted = frame.names.clone();
    let strip_rect = rect_of(&layout_of(&ctx), side);

    assert_eq!(
        texts(&names_in(&painted, strip_rect)),
        vec!["Tuning", "Debug", "Legend"],
        "a stowed side should name what is inside it, top to bottom"
    );
}

/// A side stowed under a *vertical* parent is a horizontal bar, and it names its tabs the plain
/// way round — no quarter turn, because there is nothing to turn for.
///
/// The other axis of the same code, and the one the vertical scenes cannot speak for: `side` is
/// `None` here, which is how the layout says "this is a bar". An implementation that turned every
/// name would draw this one sideways in something one tab bar *tall*.
#[test]
fn a_bar_names_its_tabs_the_plain_way_round() {
    let style = style();

    let mut state = DockState::new(vec![tab("top")]);
    let top = state.main_surface().root().unwrap();
    let [_, first] = state.split(path(top), Split::Below, 0.5, Node::leaf(tab("Tuning")));
    state.split(path(first), Split::Right, 0.5, Node::leaf(tab("Debug")));
    let side = state
        .main_surface()
        .parent(first)
        .expect("the two side-by-side leaves have a parent");
    state.main_surface_mut().set_split_stowed(side, true);

    let ctx = Context::default();
    let frame = frames(&ctx, &mut state, &style);
    let painted = frame.names.clone();
    let bar_rect = rect_of(&layout_of(&ctx), side);
    let names = names_in(&painted, bar_rect);

    assert_eq!(
        texts(&names),
        vec!["Tuning", "Debug"],
        "a bar should name what is inside it, left to right"
    );
    for name in &names {
        assert_eq!(
            name.angle, 0.0,
            "{:?} was turned on its side in a bar one tab bar tall",
            name.text
        );
    }
}

/// Clicking a name brings the panel back *and* makes that tab the one showing.
///
/// The tab clicked is deliberately not the active one: an implementation that only expands would
/// pass an assertion made on the tab that was already active.
#[test]
fn clicking_a_name_brings_the_panel_back_showing_that_tab() {
    let style = style();
    let (mut state, strip, _open) = a_collapsed_leaf_beside_a_column();
    assert!(
        state
            .main_surface()
            .leaf(strip)
            .unwrap()
            .is_active(TabIndex(0)),
        "the scene starts with the first tab active, which is what makes the third a test"
    );

    let ctx = Context::default();
    let frame = frames(&ctx, &mut state, &style);
    let painted = frame.names.clone();
    let strip_rect = rect_of(&layout_of(&ctx), strip);
    let schema = names_in(&painted, strip_rect)
        .into_iter()
        .find(|name| name.text == "Schema")
        .expect("the strip names every tab");

    click(&ctx, &mut state, &style, schema.rect.center());

    let leaf = state.main_surface().leaf(strip).unwrap();
    assert!(
        !state.main_surface()[strip].is_collapsed(),
        "clicking a name should bring the panel back"
    );
    assert!(
        leaf.is_active(TabIndex(2)),
        "clicking a name should show the tab that was named, not whatever was active before"
    );
}

/// The arrow keeps meaning "come back as you were": it expands and changes nothing else.
#[test]
fn the_arrow_brings_the_panel_back_as_it_was() {
    let style = style();
    let (mut state, strip, _open) = a_collapsed_leaf_beside_a_column();
    state
        .main_surface_mut()
        .set_active_tab(strip, TabIndex(1))
        .unwrap();

    let ctx = Context::default();
    frames(&ctx, &mut state, &style);
    let strip_rect = rect_of(&layout_of(&ctx), strip);

    // 8 px into the arrow's own square, which is the top of the strip.
    let arrow = Pos2::new(
        strip_rect.left() + 8.0,
        strip_rect.top() + style.tab_bar.height / 2.0,
    );
    click(&ctx, &mut state, &style, arrow);

    assert!(
        !state.main_surface()[strip].is_collapsed(),
        "the arrow should still expand the panel"
    );
    assert!(
        state
            .main_surface()
            .leaf(strip)
            .unwrap()
            .is_active(TabIndex(1)),
        "the arrow should bring back the tab that was showing, not the first one"
    );
}

/// A name longer than the strip is kept inside it and faded out where it runs past, rather than
/// drawn out over the panel beside it. The rotation is what makes this worth stating: a quarter
/// turn the wrong way round draws the same text, off the edge.
#[test]
fn a_name_too_long_for_the_strip_fades_out_inside_it() {
    let style = style();
    let mut state = DockState::new(vec![tab("open")]);
    let open = state.main_surface().root().unwrap();
    // Long enough that no plausible strip could hold it: the screen is 900 px tall, and this is
    // some 3000 px of text. A name that merely *nearly* fits would state nothing — it would pass
    // whether or not anything was cut at all.
    let long = &"Geology, trajectory, schema, map, graph, and everything else. ".repeat(8);
    let [_, strip] = state.split(path(open), Split::Left, 0.5, Node::leaf(tab(long)));
    state.main_surface_mut().set_leaf_collapsed(strip, true);

    let ctx = Context::default();
    let frame = frames(&ctx, &mut state, &style);
    let painted = frame.names.clone();
    let strip_rect = rect_of(&layout_of(&ctx), strip);

    let names = names_in(&painted, strip_rect);
    assert_eq!(
        names.len(),
        1,
        "the one tab should be named once, inside the strip"
    );
    assert!(
        names[0].cut(),
        "a name that cannot fit should be shown in part, not in full"
    );

    // What says it was cut: the fade over the end of it. Without one the name would simply stop
    // dead, which is the thing an ellipsis used to be there to avoid.
    assert!(
        frame
            .fades
            .iter()
            .any(|fade| strip_rect.expand(TOLERANCE).contains_rect(*fade)),
        "a cut name should fade out inside its strip: {:?}",
        frame.fades
    );
}

/// A strip with more names than room squeezes *all* of them before dropping any.
///
/// The scene is twelve names that cannot all be drawn in full and can all be drawn cut: an
/// implementation that gives each name what it asks for until the room runs out draws about half
/// of them and loses the rest without saying so.
#[test]
fn names_are_squeezed_before_any_of_them_is_dropped() {
    let style = style();
    let mut state = DockState::new(vec![tab("open")]);
    let open = state.main_surface().root().unwrap();
    let tabs: Vec<String> = (0..12)
        .map(|i| tab(&format!("Panel number {i} with a name of some length")))
        .collect();
    let [_, strip] = state.split(path(open), Split::Left, 0.5, Node::leaf_with(tabs));
    state.main_surface_mut().set_leaf_collapsed(strip, true);

    let ctx = Context::default();
    let frame = frames(&ctx, &mut state, &style);
    let painted = frame.names.clone();
    let strip_rect = rect_of(&layout_of(&ctx), strip);
    let inside = names_in(&painted, strip_rect);

    assert_eq!(
        inside.len(),
        12,
        "every tab should still be named: {:?}",
        texts(&inside)
    );
    assert!(
        inside.iter().all(|name| name.text != ELLIPSIS),
        "nothing was dropped, so nothing should stand in for it: {:?}",
        texts(&inside)
    );
    assert!(
        inside.iter().all(Painted::cut),
        "names this long cannot fit twelve to a strip uncut: {:?}",
        texts(&inside)
    );
}

/// What the strip cannot hold even squeezed is stood for by an ellipsis at the end of it —
/// never by silence, which would claim those tabs are not there at all.
///
/// The scene is forty tabs in one strip: no arrangement fits them, so some are dropped, and the
/// mark that says so is the last thing drawn. Nothing spills past the bottom of the panel either.
#[test]
fn what_the_strip_cannot_hold_is_stood_for_by_an_ellipsis() {
    let style = style();
    let mut state = DockState::new(vec![tab("open")]);
    let open = state.main_surface().root().unwrap();
    let tabs: Vec<String> = (0..40).map(|i| tab(&format!("Panel number {i}"))).collect();
    let [_, strip] = state.split(path(open), Split::Left, 0.5, Node::leaf_with(tabs));
    state.main_surface_mut().set_leaf_collapsed(strip, true);

    let ctx = Context::default();
    let frame = frames(&ctx, &mut state, &style);
    let painted = frame.names.clone();
    let strip_rect = rect_of(&layout_of(&ctx), strip);

    let inside = names_in(&painted, strip_rect);
    assert!(
        inside.len() < 40,
        "40 names cannot fit in {} px: something was drawn that has no room",
        strip_rect.height()
    );
    assert!(
        !inside.is_empty(),
        "the strip should still name what it can"
    );

    // A bare ellipsis, and not merely a name that happens to end in one: the mark stands for the
    // tabs that were dropped, so it carries no name of its own.
    let last = inside.last().expect("the strip drew something");
    assert_eq!(
        last.text,
        ELLIPSIS,
        "the strip should end by saying there is more: {:?}",
        texts(&inside)
    );

    // Nothing the strip draws may be *shown* outside it. Names run past their slots by design
    // now, so what has to stay inside is what the painter lets through: every piece of text whose
    // clip touches the strip must have that clip wholly within it.
    let escaping: Vec<&Painted> = painted
        .iter()
        .filter(|item| item.clip.intersects(strip_rect.shrink(TOLERANCE)))
        .filter(|item| !strip_rect.expand(TOLERANCE).contains_rect(item.clip))
        .collect();
    assert!(
        escaping.is_empty(),
        "every name the strip drew has to be shown inside the strip: {escaping:?}"
    );
}
