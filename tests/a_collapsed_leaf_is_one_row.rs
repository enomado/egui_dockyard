//! What collapsing a leaf does to the layout, stated on the rectangles.
//!
//! # Why this is a test file of its own
//!
//! A collapsed leaf draws a tab bar and nothing else, so it should be given a tab bar's height
//! and no more. That sentence has three cases in it, and only one of them had ever been looked
//! at:
//!
//! * a collapsed row **above** an open one — the branch that fires when a user collapses the top
//!   half of a stack;
//! * a collapsed row **below** an open one — the mirror image, a separate branch in
//!   `compute_rect_sizes`, and mirrored code is exactly where an asymmetry hides;
//! * a collapsed leaf **beside** a column, under a *horizontal* split — where the sentence does
//!   not apply at all, and the layout deliberately does something else. That case had no test
//!   and no comment, which is the same thing as not having decided it.
//!
//! The strip of rows a floating window shrinks to is the same arithmetic seen from outside, and
//! it lives in `a_window_fits_what_it_shows.rs`.

use egui::{CentralPanel, Context, Id, Pos2, RawInput, Rect, Ui, Vec2, WidgetText};
use egui_dockyard::{
    DockArea, DockLayout, DockState, Node, NodeId, NodePath, Split, Style, SurfaceIndex, TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_collapsed_leaf_is_one_row";

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

/// A few headless frames, and the geometry they settled on.
fn run(state: &mut DockState<String>, style: &Style) -> (Context, DockLayout) {
    let ctx = Context::default();
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
                    .show_inside(ui, &mut Viewer);
            });
        });
        output.textures_delta.clear();
    }
    let layout = DockLayout::load(&ctx, id);
    (ctx, layout)
}

fn rect_of(layout: &DockLayout, node: NodeId) -> Rect {
    layout
        .rect(NodePath::new(SurfaceIndex::main(), node))
        .expect("the node was laid out")
}

/// A collapsed row takes a tab bar's height, whichever side of the stack it is on.
///
/// Both orders, because the layout has a branch for each: "the strip is the top of the node" and
/// "the strip is the bottom of it". They are mirror images and were written twice, which is the
/// arrangement in which one of the two quietly says something slightly different — here, which
/// side of the boundary the divider is taken out of.
#[test]
fn a_collapsed_row_beside_an_open_one_takes_exactly_a_tab_bar() {
    let style = style();

    for collapse_top in [true, false] {
        let mut state = DockState::new(vec![tab("top")]);
        let top = state.main_surface().root().unwrap();
        let [_, bottom] = state.split(
            NodePath::new(SurfaceIndex::main(), top),
            Split::Below,
            0.5,
            Node::leaf(tab("bottom")),
        );
        let (collapsed, open) = if collapse_top {
            (top, bottom)
        } else {
            (bottom, top)
        };
        state.main_surface_mut().set_leaf_collapsed(collapsed, true);

        let (_ctx, layout) = run(&mut state, &style);
        let (collapsed, open) = (rect_of(&layout, collapsed), rect_of(&layout, open));

        assert!(
            (collapsed.height() - style.tab_bar.height).abs() <= TOLERANCE,
            "a collapsed row {} an open one got {} px for a {} px tab bar",
            if collapse_top { "above" } else { "below" },
            collapsed.height(),
            style.tab_bar.height
        );
        // And the divider between the two comes out of the space *around* the row, not out of
        // the row: the two rectangles are exactly one divider apart.
        let gap = if collapse_top {
            open.min.y - collapsed.max.y
        } else {
            collapsed.min.y - open.max.y
        };
        assert!(
            (gap - style.separator.width).abs() <= TOLERANCE,
            "the gap between the rows is {gap} px, and the divider is {} px wide",
            style.separator.width
        );
    }
}

/// Beside a column, a collapsed leaf keeps the whole column — and that is the decision, not an
/// oversight.
///
/// Collapsing means "give up your body and be a tab bar", and the height given up has to go
/// *somewhere*. Under a vertical split it goes to the sibling above or below, which is what the
/// user is asking for. Under a horizontal one the sibling is a column *beside* it: it cannot
/// grow into the space, nothing else can either, and a leaf shrunk to a bar would leave a hole
/// belonging to no node — an area with no tab bar, no body and no owner, that the dock would
/// nonetheless have to keep hit-testing.
///
/// So a collapsed leaf in a row of columns is drawn as a tab bar at the top of its own column,
/// with its column's full width and height still its own. Pinned here because "whichever branch
/// happens to run" is not a decision, and because the alternative — the hole — is the kind of
/// thing that looks like an obvious improvement until it is drawn.
#[test]
fn a_collapsed_leaf_beside_a_column_keeps_the_column() {
    let style = style();

    let mut state = DockState::new(vec![tab("left")]);
    let left = state.main_surface().root().unwrap();
    let [_, right] = state.split(
        NodePath::new(SurfaceIndex::main(), left),
        Split::Right,
        0.5,
        Node::leaf(tab("right top")),
    );
    state.split(
        NodePath::new(SurfaceIndex::main(), right),
        Split::Below,
        0.5,
        Node::leaf(tab("right bottom")),
    );
    state.main_surface_mut().set_leaf_collapsed(left, true);

    let (_ctx, layout) = run(&mut state, &style);
    let outer = state
        .main_surface()
        .parent(left)
        .expect("left has a parent");
    let (outer, left) = (rect_of(&layout, outer), rect_of(&layout, left));

    assert!(
        (left.height() - outer.height()).abs() <= TOLERANCE,
        "a collapsed leaf beside a column got {} px of a {} px row: something took its height \
         away, and nothing on screen can use it",
        left.height(),
        outer.height()
    );
}
