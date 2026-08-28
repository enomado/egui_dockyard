//! What `DockArea::collapse_sideways` does to the layout, stated on the rectangles.
//!
//! # Why this is a test file of its own
//!
//! Collapsing spends *height*: a collapsed leaf is a tab bar and nothing else. Under a
//! horizontal split there is nobody to give that height to — the sibling is a column beside it
//! — so a leaf shrunk to a bar would leave an area with no tab bar, no body and no owner. That
//! is why a collapsed leaf beside a column keeps its column, pinned next door in
//! `a_collapsed_leaf_is_one_row.rs`.
//!
//! `collapse_sideways` is the other answer to that same problem: spend **width** instead, which
//! the sibling column *can* take. So the property to state here is not "the strip is narrow" —
//! it is that **nothing is left over**: the two children plus their divider still add up to
//! their parent, exactly as before, with the sibling holding everything the strip gave up.
//!
//! Three cases would bring the hole back, and each one is a test below rather than a comment:
//! two collapsed siblings (nobody to take the width), a collapsed *split* (its subtree is rows
//! of tab bars, which do not fit in a strip), and the knob being off (the old behaviour, which
//! saved layouts still depend on).

use egui::{CentralPanel, Context, Id, Pos2, RawInput, Rect, Ui, Vec2, WidgetText};
use egui_dockyard::{
    DockArea, DockLayout, DockState, Node, NodeId, NodePath, SideStrip, Split, Style, SurfaceIndex,
    TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_collapsed_leaf_can_hide_sideways";

/// Half a device pixel at the default scale: every boundary is snapped to whole pixels, so an
/// exact comparison would be reporting the snapping rather than the property.
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

/// A few headless frames in a context of your own, for when the *same* context has to see two
/// states in a row — the geometry map lives in its memory and outlives a frame.
fn frames(ctx: &Context, state: &mut DockState<String>, style: &Style, sideways: bool) {
    let id = Id::new(DOCK_ID);
    for _ in 0..4 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            CentralPanel::default().show(ui, |ui| {
                DockArea::new(state)
                    .id(id)
                    .style(style.clone())
                    .show_leaf_collapse_buttons(true)
                    .collapse_sideways(sideways)
                    .show_inside(ui, &mut Viewer);
            });
        });
        output.textures_delta.clear();
    }
}

/// A few headless frames, and the geometry they settled on.
fn run(state: &mut DockState<String>, style: &Style, sideways: bool) -> DockLayout {
    let ctx = Context::default();
    frames(&ctx, state, style, sideways);
    DockLayout::load(&ctx, Id::new(DOCK_ID))
}

fn rect_of(layout: &DockLayout, node: NodeId) -> Rect {
    layout
        .rect(NodePath::new(SurfaceIndex::main(), node))
        .expect("the node was laid out")
}

fn side_strip_of(layout: &DockLayout, node: NodeId) -> Option<SideStrip> {
    layout.side_strip(NodePath::new(SurfaceIndex::main(), node))
}

/// Two leaves side by side, the named one collapsed. Returns (parent, left, right).
fn two_columns(collapse: Collapse) -> (DockState<String>, NodeId, NodeId, NodeId) {
    let mut state = DockState::new(vec![tab("left")]);
    let left = state.main_surface().root().unwrap();
    let [_, right] = state.split(
        NodePath::new(SurfaceIndex::main(), left),
        Split::Right,
        0.5,
        Node::leaf(tab("right")),
    );
    match collapse {
        Collapse::Left => state.main_surface_mut().set_leaf_collapsed(left, true),
        Collapse::Right => state.main_surface_mut().set_leaf_collapsed(right, true),
        Collapse::Both => {
            state.main_surface_mut().set_leaf_collapsed(left, true);
            state.main_surface_mut().set_leaf_collapsed(right, true);
        }
    }
    let parent = state
        .main_surface()
        .parent(left)
        .expect("the two leaves have a parent");
    (state, parent, left, right)
}

enum Collapse {
    Left,
    Right,
    Both,
}

/// The strip is one tab bar thick, and everything it gave up went to its sibling.
///
/// Both sides, because the layout has a branch for each — "the strip is the left edge of the
/// node" and "the strip is the right edge of it". They are mirror images and were written
/// twice, which is the arrangement in which one of the two quietly says something slightly
/// different; here, which side of the boundary the divider is taken out of.
#[test]
fn a_collapsed_leaf_beside_a_column_becomes_a_strip() {
    let style = style();

    for collapse_left in [true, false] {
        let (mut state, parent, left, right) = two_columns(if collapse_left {
            Collapse::Left
        } else {
            Collapse::Right
        });
        let layout = run(&mut state, &style, true);

        let (parent_rect, left_rect, right_rect) = (
            rect_of(&layout, parent),
            rect_of(&layout, left),
            rect_of(&layout, right),
        );
        let (strip, open) = if collapse_left {
            (left_rect, right_rect)
        } else {
            (right_rect, left_rect)
        };

        assert!(
            (strip.width() - style.tab_bar.height).abs() <= TOLERANCE,
            "a leaf collapsed sideways got {} px for a {} px strip",
            strip.width(),
            style.tab_bar.height
        );

        // The whole point: no hole. What the strip gave up is held by the sibling, and the two
        // of them plus the divider are still the parent.
        let covered = strip.width() + open.width() + style.separator.width;
        assert!(
            (covered - parent_rect.width()).abs() <= TOLERANCE,
            "the strip, its sibling and the divider cover {} px of a {} px row: the difference \
             is an area with no tab bar, no body and no owner",
            covered,
            parent_rect.width()
        );

        // And it is pressed against the edge it collapsed towards, not floating in the middle.
        let (outer_edge, strip_edge) = if collapse_left {
            (parent_rect.min.x, strip.min.x)
        } else {
            (parent_rect.max.x, strip.max.x)
        };
        assert!(
            (outer_edge - strip_edge).abs() <= TOLERANCE,
            "the strip sits at {} but the edge of its split is at {}",
            strip_edge,
            outer_edge
        );

        let expected_side = if collapse_left {
            SideStrip::Left
        } else {
            SideStrip::Right
        };
        let (collapsed_node, open_node) = if collapse_left {
            (left, right)
        } else {
            (right, left)
        };
        assert_eq!(
            side_strip_of(&layout, collapsed_node),
            Some(expected_side),
            "drawing reads the side off the layout, so the layout has to have said it"
        );
        assert_eq!(
            side_strip_of(&layout, open_node),
            None,
            "the open sibling is not a strip"
        );
    }
}

/// Two collapsed siblings keep their columns: there is nobody to hand the width to.
///
/// Squeeze both and the width they gave up belongs to no node — the very hole this whole
/// feature exists to avoid. So the pair is left alone, and each draws an ordinary tab bar.
#[test]
fn two_collapsed_siblings_keep_their_columns() {
    let style = style();
    let (mut state, parent, left, right) = two_columns(Collapse::Both);
    let layout = run(&mut state, &style, true);

    let (parent_rect, left_rect, right_rect) = (
        rect_of(&layout, parent),
        rect_of(&layout, left),
        rect_of(&layout, right),
    );

    for (name, rect) in [("left", left_rect), ("right", right_rect)] {
        assert!(
            rect.width() > style.tab_bar.height * 2.0,
            "the {name} leaf was squeezed to {} px even though its sibling is collapsed too, \
             and the width has nowhere to go",
            rect.width()
        );
    }
    let covered = left_rect.width() + right_rect.width() + style.separator.width;
    assert!(
        (covered - parent_rect.width()).abs() <= TOLERANCE,
        "two collapsed siblings cover {} px of a {} px row",
        covered,
        parent_rect.width()
    );
    assert_eq!(side_strip_of(&layout, left), None);
    assert_eq!(side_strip_of(&layout, right), None);
}

/// A collapsed — but not *stowed* — split beside a column keeps the column.
///
/// A collapsed split is a stack of collapsed leaves, i.e. rows of tab bars. A strip is one tab
/// bar thick measured the other way, so those rows have nowhere to be drawn; the subtree keeps
/// its column and each of its leaves stays a row, exactly as without the knob.
///
/// Since `SplitNode::stowed` there is a second way for a split to be collapsed, and it *does*
/// become a strip (`a_side_can_be_stowed.rs`) — because a side put away as a unit draws one bar
/// for whatever it contains, so there are no rows to fit. The two are told apart by how they got
/// that way, not by how they look to `is_collapsed`, and this test is the half that says the
/// leaf-at-a-time spelling still keeps its column.
#[test]
fn a_collapsed_split_beside_a_column_keeps_the_column() {
    let style = style();

    let mut state = DockState::new(vec![tab("top")]);
    let top = state.main_surface().root().unwrap();
    state.split(
        NodePath::new(SurfaceIndex::main(), top),
        Split::Right,
        0.5,
        Node::leaf(tab("right")),
    );
    let [_, bottom] = state.split(
        NodePath::new(SurfaceIndex::main(), top),
        Split::Below,
        0.5,
        Node::leaf(tab("bottom")),
    );
    // The whole left-hand subtree is collapsed, so the split above these two is itself
    // collapsed — and it is a split, not a leaf.
    state.main_surface_mut().set_leaf_collapsed(top, true);
    state.main_surface_mut().set_leaf_collapsed(bottom, true);

    let inner = state
        .main_surface()
        .parent(top)
        .expect("the two stacked leaves have a parent");
    let outer = state
        .main_surface()
        .parent(inner)
        .expect("the stack sits beside a column");

    let layout = run(&mut state, &style, true);
    let (outer_rect, inner_rect) = (rect_of(&layout, outer), rect_of(&layout, inner));

    assert!(
        inner_rect.width() > style.tab_bar.height * 2.0,
        "a collapsed split was squeezed to {} px of a {} px row, and its rows of tab bars have \
         nowhere to be drawn",
        inner_rect.width(),
        outer_rect.width()
    );
    assert_eq!(side_strip_of(&layout, inner), None);
    assert_eq!(side_strip_of(&layout, top), None);
}

/// Expanding a strip takes the strip back, in the same context that drew it.
///
/// The geometry map is kept in egui memory and entries outlive the frame that wrote them, so
/// "this leaf is a strip" is a flag that can go *stale*: set it once and never clear it, and
/// the leaf keeps drawing an arrow and no body forever, having been expanded. The clearing
/// happens in `DockLayout::set_rect`, which every laid-out node goes through on every pass —
/// and this is the test that says so, because a fresh context per run cannot see it.
#[test]
fn expanding_a_strip_takes_it_back() {
    let style = style();
    let (mut state, parent, left, _right) = two_columns(Collapse::Left);
    let ctx = Context::default();

    frames(&ctx, &mut state, &style, true);
    let layout = DockLayout::load(&ctx, Id::new(DOCK_ID));
    // The scene this test is about was actually reached — without this the rest passes on a
    // leaf that was never a strip to begin with.
    assert_eq!(
        side_strip_of(&layout, left),
        Some(SideStrip::Left),
        "the leaf has to be a strip before expanding it can prove anything"
    );

    state.main_surface_mut().set_leaf_collapsed(left, false);
    frames(&ctx, &mut state, &style, true);
    let layout = DockLayout::load(&ctx, Id::new(DOCK_ID));

    assert_eq!(
        side_strip_of(&layout, left),
        None,
        "an expanded leaf is still marked as a strip: the flag went stale and it will draw an \
         arrow and no body forever"
    );
    let (parent_rect, left_rect) = (rect_of(&layout, parent), rect_of(&layout, left));
    assert!(
        left_rect.width() > style.tab_bar.height * 2.0,
        "an expanded leaf got {} px of a {} px row",
        left_rect.width(),
        parent_rect.width()
    );
}

/// With the knob off, nothing changes — the old decision still holds.
///
/// The positive control of this file. Without it every assertion above would still pass if the
/// knob were ignored and sideways collapsing were simply always on, which is exactly the
/// mistake an experimental feature must not be able to make quietly.
#[test]
fn the_knob_off_keeps_the_whole_column() {
    let style = style();
    let (mut state, parent, left, _right) = two_columns(Collapse::Left);
    let layout = run(&mut state, &style, false);

    let (parent_rect, left_rect) = (rect_of(&layout, parent), rect_of(&layout, left));

    assert!(
        left_rect.width() > style.tab_bar.height * 2.0,
        "with `collapse_sideways` off a collapsed leaf got {} px of a {} px row: the knob was \
         not read, and layouts that rely on the old behaviour have changed under them",
        left_rect.width(),
        parent_rect.width()
    );
    assert!(
        (left_rect.height() - parent_rect.height()).abs() <= TOLERANCE,
        "and it keeps the full height of its column, as `a_collapsed_leaf_is_one_row` states"
    );
    assert_eq!(side_strip_of(&layout, left), None);
}
