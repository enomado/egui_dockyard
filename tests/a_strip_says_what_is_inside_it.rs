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
    /// The glyphs actually laid out — **not** `Galley::text()`, which answers with the whole
    /// string the layout job was given whether or not it fitted. Truncation is a fact about the
    /// rows, so a test that reads the job text cannot see it happen (this one did, and passed a
    /// scene where nothing was truncated at all).
    text: String,
    rect: Rect,
    /// Radians clockwise. Zero for ordinary horizontal text, so "was this turned?" is a question
    /// about the shape rather than about its shape's proportions.
    angle: f32,
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
                            angle: text.angle,
                        }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// One headless frame carrying `events`, answering with the text it painted.
fn frame(
    ctx: &Context,
    state: &mut DockState<String>,
    style: &Style,
    events: Vec<Event>,
) -> Vec<Painted> {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
        events,
        ..Default::default()
    };
    let mut painted = Vec::new();
    let mut output = ctx.run_ui(input, |ui| {
        CentralPanel::default().show(ui, |ui| {
            DockArea::new(state)
                .id(Id::new(DOCK_ID))
                .style(style.clone())
                .show_leaf_collapse_buttons(true)
                .collapse_sideways(true)
                .show_inside(ui, &mut Viewer);
        });
        painted = painted_text(ui.ctx());
    });
    // `TexturesDelta` panics when dropped with deltas nobody applied, and there is no backend here.
    output.textures_delta.clear();
    painted
}

/// A few quiet frames, answering with what the last one painted: a click takes effect on the
/// frame after the one that reports it, and the geometry map has to settle either way.
fn frames(ctx: &Context, state: &mut DockState<String>, style: &Style) -> Vec<Painted> {
    let mut painted = Vec::new();
    for _ in 0..4 {
        painted = frame(ctx, state, style, Vec::new());
    }
    painted
}

/// Press and release at `at`, then let the layout settle, answering with what is painted after.
fn click(ctx: &Context, state: &mut DockState<String>, style: &Style, at: Pos2) -> Vec<Painted> {
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
/// Selected by rectangle rather than by asking the crate, which is the point: a name that ended
/// up outside the panel it belongs to is not in this list, and every assertion below that counts
/// names would fail. The sibling leaf paints its own tab bar and body in the same frame.
fn names_in(painted: &[Painted], strip: Rect) -> Vec<Painted> {
    painted
        .iter()
        .filter(|item| strip.expand(TOLERANCE).contains_rect(item.rect))
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
    let painted = frames(&ctx, &mut state, &style);
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
    let painted = frames(&ctx, &mut state, &style);
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
    let painted = frames(&ctx, &mut state, &style);
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
    let painted = frames(&ctx, &mut state, &style);
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

/// A name longer than the strip is truncated into it, rather than drawn out over the panel
/// beside it. The rotation is what makes this worth stating: a quarter turn the wrong way round
/// draws the same text, off the edge.
#[test]
fn a_name_too_long_for_the_strip_is_truncated_into_it() {
    let style = style();
    let mut state = DockState::new(vec![tab("open")]);
    let open = state.main_surface().root().unwrap();
    // Long enough that no plausible strip could hold it: the screen is 900 px tall, and this is
    // some 3000 px of text. A name that merely *nearly* fits would state nothing — it would pass
    // whether or not truncation happened at all.
    let long = &"Geology, trajectory, schema, map, graph, and everything else. ".repeat(8);
    let [_, strip] = state.split(path(open), Split::Left, 0.5, Node::leaf(tab(long)));
    state.main_surface_mut().set_leaf_collapsed(strip, true);

    let ctx = Context::default();
    let painted = frames(&ctx, &mut state, &style);
    let strip_rect = rect_of(&layout_of(&ctx), strip);

    let names = names_in(&painted, strip_rect);
    assert_eq!(
        names.len(),
        1,
        "the one tab should be named once, inside the strip"
    );
    assert!(
        names[0].text.len() < long.len(),
        "a name that cannot fit should be truncated, not drawn in full"
    );
    assert!(
        names[0].text.ends_with('…'),
        "a truncated name should say that it was cut: {:?}",
        names[0].text
    );
}

/// Names that have no room left are not drawn at all, and nothing is drawn outside the strip.
///
/// The scene is a short strip and more tabs than fit: what the last ones must *not* do is spill
/// past the bottom of the panel.
#[test]
fn a_name_with_no_room_left_is_not_drawn() {
    let style = style();
    let mut state = DockState::new(vec![tab("open")]);
    let open = state.main_surface().root().unwrap();
    let tabs: Vec<String> = (0..40).map(|i| tab(&format!("Panel number {i}"))).collect();
    let [_, strip] = state.split(path(open), Split::Left, 0.5, Node::leaf_with(tabs));
    state.main_surface_mut().set_leaf_collapsed(strip, true);

    let ctx = Context::default();
    let painted = frames(&ctx, &mut state, &style);
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

    // Nothing the strip drew may land outside it: `names_in` filters by rectangle, so this
    // compares what is inside against every piece of text that overlaps the strip at all.
    let overlapping = painted
        .iter()
        .filter(|item| item.rect.intersects(strip_rect.shrink(TOLERANCE)))
        .count();
    assert_eq!(
        overlapping,
        inside.len(),
        "every name the strip drew has to be inside the strip"
    );
}
