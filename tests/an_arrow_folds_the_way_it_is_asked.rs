//! One arrow, two axes: a plain click spends the leaf's height, `Ctrl` spends its width.
//!
//! # Why this is a test file of its own
//!
//! Folding used to take its direction from the parent split — vertical parent, a bar; horizontal
//! parent, a strip (behind `collapse_sideways`). That reads as a rule right up to the day it is
//! the wrong one, and then there is nothing to ask with: a user who wanted the column left
//! standing had to turn the knob off for the whole application. It was reported as exactly that
//! — "the column hides sideways on a plain click, and that should be the Ctrl one".
//!
//! So [`Fold`] is state now, chosen by the gesture, and this file is where the gesture is stated:
//!
//! * a plain click folds into a **bar** — the leaf keeps its column and empties it;
//! * `Ctrl` + click folds into a **strip** — the leaf gives up its width and the sibling takes it;
//! * `Ctrl` on a leaf already a strip brings it back, and on a bar re-folds it the other way,
//!   without a trip through "open";
//! * under a **vertical** parent there is no width to give up, so `Ctrl` adds nothing and the
//!   press is the plain fold — the same shape as the modifier that does nothing on a leaf which
//!   is already its own side.
//!
//! The layout is asked, not the tree, wherever the question is "what does this look like": a
//! narrow rectangle is not a strip, and [`DockLayout::side_strip`] is what tells them apart.

use egui::{
    Atoms, CentralPanel, Context, Event, Id, Modifiers, PointerButton, Pos2, RawInput, Rect, Ui,
    Vec2,
};
use egui_dockyard::{
    DockArea, DockLayout, DockState, Fold, Node, NodeId, NodePath, SideStrip, Split, Style,
    SurfaceIndex, TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "an_arrow_folds_the_way_it_is_asked";

struct Viewer;

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> Atoms<'static> {
        Atoms::new(tab.clone())
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

/// One frame with `modifiers` held for the whole of it — the button reads them off the input
/// state, the way a *held* key is read, so they are announced rather than only attached to the
/// click. Same idiom as `a_side_can_be_stowed.rs`.
fn frame(
    ctx: &Context,
    state: &mut DockState<String>,
    style: &Style,
    sideways: bool,
    events: Vec<Event>,
    modifiers: Modifiers,
) {
    let mut held = vec![Event::ModifiersChanged(modifiers)];
    held.extend(events);
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
        events: held,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        CentralPanel::default().show(ui, |ui| {
            DockArea::new(state)
                .id(Id::new(DOCK_ID))
                .style(style.clone())
                .show_leaf_collapse_buttons(true)
                .collapse_sideways(sideways)
                .show_inside(ui, &mut Viewer);
        });
    });
    output.textures_delta.clear();
}

/// A few quiet frames, so the layout has settled before anything is asked of it.
fn settle(ctx: &Context, state: &mut DockState<String>, style: &Style, sideways: bool) {
    for _ in 0..4 {
        frame(ctx, state, style, sideways, Vec::new(), Modifiers::NONE);
    }
}

/// Press and release at `at` with `modifiers` held, then let the queued edit land: a fold is
/// queued while drawing and applied at the end of that pass, so the picture it asks for is the
/// next frame's.
fn click(
    ctx: &Context,
    state: &mut DockState<String>,
    style: &Style,
    sideways: bool,
    at: Pos2,
    modifiers: Modifiers,
) {
    for pressed in [true, false] {
        let event = Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers,
        };
        frame(ctx, state, style, sideways, vec![event], modifiers);
    }
    settle(ctx, state, style, sideways);
}

fn layout_of(ctx: &Context) -> DockLayout {
    DockLayout::load(ctx, Id::new(DOCK_ID))
}

fn rect_of(layout: &DockLayout, node: NodeId) -> Rect {
    layout.rect(path(node)).expect("the node was laid out")
}

/// The top-left corner of a node's own collapse arrow — 8 px in, comfortably inside the button
/// and clear of the tab-bar margin.
fn collapse_arrow_of(layout: &DockLayout, node: NodeId, style: &Style) -> Pos2 {
    let rect = rect_of(layout, node);
    Pos2::new(rect.left() + 8.0, rect.top() + style.tab_bar.height / 2.0)
}

fn fold_of(state: &DockState<String>, node: NodeId) -> Fold {
    state[path(node)].fold()
}

/// Two leaves side by side: (state, left, right).
fn two_columns() -> (DockState<String>, NodeId, NodeId) {
    let mut state = DockState::new(vec![tab("left")]);
    let left = state.main_surface().root().unwrap();
    let [_, right] = state.split(
        path(left),
        Split::Right,
        0.5,
        Node::leaf(tab("right")),
    );
    (state, left, right)
}

/// Two leaves stacked: (state, top, bottom).
fn two_rows() -> (DockState<String>, NodeId, NodeId) {
    let mut state = DockState::new(vec![tab("top")]);
    let top = state.main_surface().root().unwrap();
    let [_, bottom] = state.split(
        path(top),
        Split::Below,
        0.5,
        Node::leaf(tab("bottom")),
    );
    (state, top, bottom)
}

/// The reported bug, stated the way it was reported: the plain click leaves the column standing.
#[test]
fn a_plain_click_folds_into_a_bar_and_keeps_the_column() {
    let ctx = Context::default();
    let style = style();
    let (mut state, left, right) = two_columns();
    settle(&ctx, &mut state, &style, true);

    let before = rect_of(&layout_of(&ctx), right);
    let open_before = rect_of(&layout_of(&ctx), left).width();
    let at = collapse_arrow_of(&layout_of(&ctx), right, &style);
    click(&ctx, &mut state, &style, true, at, Modifiers::NONE);

    assert_eq!(
        fold_of(&state, right),
        Fold::Bar,
        "a plain click spends height, whatever the parent"
    );
    let layout = layout_of(&ctx);
    assert_eq!(
        layout.side_strip(path(right)),
        None,
        "a bar is not a strip, however the parent is split"
    );
    let after = rect_of(&layout, right);
    assert!(
        (after.width() - before.width()).abs() < 1.0,
        "the column is exactly as wide as it was: {before:?} -> {after:?}"
    );
    assert!(
        (rect_of(&layout, left).width() - open_before).abs() < 1.0,
        "and the sibling did not take anything"
    );
}

/// The other axis, on the other key — and the sibling really does take the width, which is the
/// half of `collapse_sideways` a bar cannot reach.
#[test]
fn ctrl_click_folds_into_a_strip_and_hands_over_the_width() {
    let ctx = Context::default();
    let style = style();
    let (mut state, left, right) = two_columns();
    settle(&ctx, &mut state, &style, true);

    let open_before = rect_of(&layout_of(&ctx), left).width();
    let at = collapse_arrow_of(&layout_of(&ctx), right, &style);
    click(&ctx, &mut state, &style, true, at, Modifiers::COMMAND);

    assert_eq!(fold_of(&state, right), Fold::Strip);
    let layout = layout_of(&ctx);
    assert_eq!(
        layout.side_strip(path(right)),
        Some(SideStrip::Right),
        "the strip hugs the edge of its own split"
    );
    assert!(
        rect_of(&layout, left).width() > open_before + 1.0,
        "the sibling takes the width the strip gave up"
    );
}

/// The two folds are one gesture away from each other, in both directions, without a trip
/// through "open" — which is what makes the axis a *choice* rather than a mode you have to
/// unfold out of.
#[test]
fn the_modifier_moves_a_fold_between_the_two_axes() {
    let ctx = Context::default();
    let style = style();
    let (mut state, _left, right) = two_columns();
    settle(&ctx, &mut state, &style, true);

    let at = collapse_arrow_of(&layout_of(&ctx), right, &style);
    click(&ctx, &mut state, &style, true, at, Modifiers::NONE);
    assert_eq!(fold_of(&state, right), Fold::Bar);

    // The arrow of a bar sits where it always did; a strip's is at the top of the strip, which
    // is the same corner of the same rectangle.
    let at = collapse_arrow_of(&layout_of(&ctx), right, &style);
    click(&ctx, &mut state, &style, true, at, Modifiers::COMMAND);
    assert_eq!(
        fold_of(&state, right),
        Fold::Strip,
        "Ctrl on a bar re-folds it sideways rather than opening it"
    );

    let at = collapse_arrow_of(&layout_of(&ctx), right, &style);
    click(&ctx, &mut state, &style, true, at, Modifiers::COMMAND);
    assert_eq!(
        fold_of(&state, right),
        Fold::Open,
        "and on a strip it takes it back"
    );
}

/// A plain click still opens whatever is folded, either way round: the arrow on a strip is the
/// way back, exactly as it is on a bar.
#[test]
fn a_plain_click_opens_a_strip_too() {
    let ctx = Context::default();
    let style = style();
    let (mut state, _left, right) = two_columns();
    settle(&ctx, &mut state, &style, true);

    let at = collapse_arrow_of(&layout_of(&ctx), right, &style);
    click(&ctx, &mut state, &style, true, at, Modifiers::COMMAND);
    assert_eq!(fold_of(&state, right), Fold::Strip);

    let at = collapse_arrow_of(&layout_of(&ctx), right, &style);
    click(&ctx, &mut state, &style, true, at, Modifiers::NONE);
    assert_eq!(fold_of(&state, right), Fold::Open);
}

/// Under a vertical parent there is no width to hand anybody, so the modifier adds nothing and
/// the press is the plain fold — the same answer the crate gives for a modifier whose gesture
/// does not apply, rather than a fold nobody can see the point of.
#[test]
fn the_modifier_adds_nothing_under_a_vertical_parent() {
    let ctx = Context::default();
    let style = style();
    let (mut state, _top, bottom) = two_rows();
    settle(&ctx, &mut state, &style, true);

    let at = collapse_arrow_of(&layout_of(&ctx), bottom, &style);
    click(&ctx, &mut state, &style, true, at, Modifiers::COMMAND);

    assert_eq!(
        fold_of(&state, bottom),
        Fold::Bar,
        "the press went through as the plain fold"
    );
    assert_eq!(layout_of(&ctx).side_strip(path(bottom)), None);
}

/// With the knob off there are no strips at all, so the modifier has nothing to offer and the
/// arrow keeps its one meaning.
///
/// The positive control for the knob: without it, every assertion above would pass on a dock that
/// ignored `collapse_sideways` entirely.
#[test]
fn the_knob_off_leaves_the_modifier_meaningless() {
    let ctx = Context::default();
    let style = style();
    let (mut state, _left, right) = two_columns();
    settle(&ctx, &mut state, &style, false);

    let at = collapse_arrow_of(&layout_of(&ctx), right, &style);
    click(&ctx, &mut state, &style, false, at, Modifiers::COMMAND);

    assert_eq!(fold_of(&state, right), Fold::Bar);
    assert_eq!(layout_of(&ctx).side_strip(path(right)), None);
}
