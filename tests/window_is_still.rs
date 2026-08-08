//! A floating window nobody touches does not move.
//!
//! # Why this is a test file of its own
//!
//! A `DockArea` draws a lot of things at positions the layout pass has already decided: tab
//! bars, close and collapse and add buttons, scroll bars, the cross-split toggle. None of them
//! may *claim* space in the `Ui` the dock is drawn into — and one of them did. `Ui::scope_builder`
//! folds the child's `min_rect` into the parent and advances its cursor, which is the right thing
//! for a widget laid out in a flow and exactly the wrong thing for one painted at an absolute
//! position; the cross-split toggle used it, and every button on screen grew the dock's `Ui` by
//! one item spacing per frame.
//!
//! On the main surface that is invisible: a panel's size does not come from its content, so the
//! growth goes nowhere. Inside an [`egui::Window`] the content's `min_rect` **is** the size the
//! window asks for, so the growth is real — the window swelled downwards until
//! `Window::constrain_to` ran out of screen, and from then on it slid up it, frame after frame,
//! with nobody touching anything.
//!
//! So the window is the instrument, and the property is the weakest one there is: a scene under
//! no input does not move. What makes it worth a file is the *scene* — every ornament the dock
//! knows how to draw, on one window at once — because the rule ("draw over the layout, claim no
//! space") is not something a compiler or a lint can hold, and the next ornament will be written
//! by someone who has never heard of it.
//!
//! The rule is not mechanical, which is why it cannot be a lint: `show_leaf`'s dragged tab uses
//! the very same `scope_builder` and genuinely wants its slot in the tab bar. What tells the two
//! apart is whether the layout has already accounted for the space, and only a human knows that.

use egui::{CentralPanel, Context, Id, Pos2, RawInput, Rect, Ui, Vec2, WidgetText};
use egui_dock::{
    DockArea, DockLayout, DockState, Node, NodePath, Split, Style, SurfaceIndex, TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "window_is_still_dock";

struct Viewer;

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        tab.clone().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        ui.label(tab.as_str());
    }
}

/// One headless frame with every ornament the `DockArea` can draw switched on.
///
/// Defaults would do for most of them, but naming them is the point of the file: a flag left at
/// its default is a flag nobody chose, and this scene exists precisely to have chosen all of
/// them.
fn frame(ctx: &Context, state: &mut DockState<String>, id: Id, pointer: Option<Pos2>) {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
        events: pointer.map(egui::Event::PointerMoved).into_iter().collect(),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ctx| {
        CentralPanel::default().show(ctx, |ui| {
            DockArea::new(state)
                .id(id)
                .style(Style::from_egui(ui.style().as_ref()))
                .show_add_buttons(true)
                .show_add_popup(true)
                .show_close_buttons(true)
                .show_leaf_close_all_buttons(true)
                .show_leaf_collapse_buttons(true)
                .show_cross_split_toggle(true)
                .show_tab_name_on_hover(true)
                .show_secondary_button_hint(true)
                .show_inside(ui, &mut Viewer);
        });
    });
    // Headless harness, no GPU backend to hand the delta to.
    output.textures_delta.clear();
}

/// A floating window carrying a 2x2 cross, one leaf of which holds enough tabs to overflow its
/// tab bar and bring out the scroll bar.
fn ornamented_window() -> (DockState<String>, SurfaceIndex) {
    let tab = |name: &str| name.to_owned();
    let mut state = DockState::new(vec![tab("main")]);

    // Enough tabs that the bar cannot show them all in a 600px window: the overflow scroll bar
    // is one more thing drawn at a position the layout chose.
    let crowded: Vec<String> = (0..12).map(|i| tab(&format!("crowded {i}"))).collect();
    let window = state.add_window(crowded);

    let top_left = state[window].root().unwrap();
    let [_, top_right] = state.split(
        NodePath::new(window, top_left),
        Split::Right,
        0.5,
        Node::leaf(tab("top right")),
    );
    state.split(
        NodePath::new(window, top_left),
        Split::Below,
        0.5,
        Node::leaf(tab("bottom left")),
    );
    state.split(
        NodePath::new(window, top_right),
        Split::Below,
        0.5,
        Node::leaf(tab("bottom right")),
    );

    (state, window)
}

/// The window surface's root rectangle, which is the window's content area.
fn window_rect(ctx: &Context, id: Id, state: &DockState<String>, window: SurfaceIndex) -> Rect {
    let root = state[window].root().expect("the window has a root");
    DockLayout::load(ctx, id)
        .rect(NodePath::new(window, root))
        .expect("the window surface was laid out this frame")
}

/// Twenty quiet frames, and the window is where it was.
///
/// Twenty rather than two because the failure this guards against is *cumulative*: the creep was
/// three points a frame, which is nothing to look at once and 60 points to look at twenty times.
/// A test that took two frames would have called the bug a rounding error.
#[test]
fn an_untouched_window_does_not_move() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let (mut state, window) = ornamented_window();

    // Settle: a window is auto-sized from its content, so the first frames are it finding out
    // how big it is. What must not happen is that it never stops finding out.
    for _ in 0..4 {
        frame(&ctx, &mut state, id, None);
    }
    let settled = window_rect(&ctx, id, &state, window);

    for step in 1..=20 {
        frame(&ctx, &mut state, id, None);
        let now = window_rect(&ctx, id, &state, window);
        assert!(
            (settled.min - now.min).length() < 0.5 && (settled.max - now.max).length() < 0.5,
            "quiet frame {step} moved the window from {settled:?} to {now:?}"
        );
    }
}

/// And it does not move while the pointer wanders over it either.
///
/// Hovering is what brings the ornaments to life — highlights, cursors, the toggle growing to
/// meet the pointer — and a widget that claims space only when hovered would slip past the test
/// above. The pointer is walked across the window's diagonal, so it passes the tab bar, the
/// buttons at both ends of it, the separators and the crossing.
#[test]
fn a_hovered_window_does_not_move() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let (mut state, window) = ornamented_window();

    for _ in 0..4 {
        frame(&ctx, &mut state, id, None);
    }
    let settled = window_rect(&ctx, id, &state, window);

    for step in 0..=20u8 {
        let t = f32::from(step) / 20.0;
        let at = settled.min + (settled.max - settled.min) * t;
        frame(&ctx, &mut state, id, Some(at));
        let now = window_rect(&ctx, id, &state, window);
        assert!(
            (settled.min - now.min).length() < 0.5 && (settled.max - now.max).length() < 0.5,
            "the pointer at {at:?} moved the window from {settled:?} to {now:?}"
        );
    }
}
