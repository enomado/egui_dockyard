//! Taking hold of a floating window makes it the focused surface.
//!
//! # Why this is a test file of its own
//!
//! Focus is what an application's own keyboard shortcuts are aimed at: `Ctrl+W` asks
//! [`DockState::find_active_focused`] which tab to close, and that reads `focused_surface`. Every
//! way of *pointing at* a surface therefore has to write it, or a shortcut silently addresses the
//! surface the hand left behind.
//!
//! Clicking already does — a click inside a leaf's body queues the focus, and so does a click on a
//! tab. A **drag does not**, and for a floating window that is the whole gesture: egui builds it
//! with no title bar, so a press anywhere over its body is a window move (see
//! `a_moved_window_says_so.rs`), and `any_click()` is never true of a press that travelled. So a
//! window could be dragged clear across the screen, be the only thing the hand had touched, and
//! `Ctrl+W` would still close a tab in the main surface — reported from the application as
//! "closing a window's tab closes its neighbour instead".
//!
//! What this file states, on the two gestures a window as a whole answers to:
//!
//! * moving it by its body focuses it;
//! * resizing it by an edge focuses it too — it is the same "I am working on this window";
//! * and the scene is not degenerate: the focus really was somewhere else first, and the window
//!   really did move under the hand.

use egui::{
    Atoms, CentralPanel, Context, Event, Id, Modifiers, PointerButton, Pos2, RawInput, Rect, Ui,
    Vec2,
};
use egui_dockyard::{DockArea, DockLayout, DockState, NodePath, Style, SurfaceIndex, TabViewer};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_window_the_hand_moves_takes_the_focus";

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

/// A dock with one floating window in it, and the window's index.
fn dock_with_a_window() -> (DockState<String>, SurfaceIndex) {
    let mut state = DockState::new(vec!["main".to_owned()]);
    let window = state.add_window(vec!["floating".to_owned()]);
    (state, window)
}

/// One headless frame with whatever pointer events the step needs.
fn frame(ctx: &Context, state: &mut DockState<String>, id: Id, events: Vec<Event>) {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ctx| {
        CentralPanel::default().show(ctx, |ui| {
            DockArea::new(state)
                .id(id)
                .style(Style::from_egui(ui.style().as_ref()))
                .show_inside(ui, &mut Viewer)
                .apply(ui.ctx(), state, &mut Viewer);
        });
    });
    // Headless harness, no GPU backend to hand the delta to.
    output.textures_delta.clear();
}

fn moved_to(at: Pos2) -> Vec<Event> {
    vec![Event::PointerMoved(at)]
}

fn button(at: Pos2, pressed: bool) -> Vec<Event> {
    vec![
        Event::PointerMoved(at),
        Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
        },
    ]
}

/// A press and a release in the same place: a click, which is the gesture that already focuses.
fn click(ctx: &Context, state: &mut DockState<String>, id: Id, at: Pos2) {
    frame(ctx, state, id, button(at, true));
    frame(ctx, state, id, button(at, false));
}

/// The content rectangle of a surface's root — what the dock laid out this frame.
fn root_rect(ctx: &Context, id: Id, state: &DockState<String>, surface: SurfaceIndex) -> Rect {
    let root = state[surface].root().expect("the surface has a root");
    DockLayout::load(ctx, id)
        .rect(NodePath::new(surface, root))
        .expect("the surface was laid out this frame")
}

/// The window's **outer** frame, the rectangle egui hangs its eight resize handles off — not the
/// content rectangle `DockLayout` tracks, which is inset from it by the frame's margin and stroke.
/// The same id and the same reasoning as `a_resized_window_says_so.rs`.
fn window_outer_rect(ctx: &Context, window: SurfaceIndex) -> Rect {
    let SurfaceIndex::Window(index) = window else {
        panic!("not a window surface");
    };
    let area_id = Id::new(format!("window SurfaceIndex({})", index.0 + 1));
    ctx.memory(|mem| mem.area_rect(area_id))
        .expect("the window surface was laid out this frame")
}

/// A point inside a window that no widget of the dock's answers to: low in the leaf's body, below
/// the tab bar and away from every button, so the press is one egui hands to the window itself.
fn empty_body_of(rect: Rect) -> Pos2 {
    rect.center() + Vec2::new(0.0, rect.height() * 0.25)
}

/// A point in the main surface's body that the floating window does not cover.
///
/// Asserted rather than assumed: a point under the window would be a click egui gives to the
/// window, and the "focus started on the main surface" half of every test here would be testing
/// nothing.
fn main_body_point(ctx: &Context, id: Id, state: &DockState<String>, window: SurfaceIndex) -> Pos2 {
    let main = root_rect(ctx, id, state, SurfaceIndex::Main);
    let over_the_window = window_outer_rect(ctx, window);
    let at = main.left_bottom() + Vec2::new(40.0, -40.0);
    assert!(
        main.contains(at) && !over_the_window.contains(at),
        "the scene needs a spot of the main surface the window does not cover; \
         main {main:?}, window {over_the_window:?}"
    );
    at
}

/// Four frames with nothing happening: the window has finished finding out how big it is.
fn settled(ctx: &Context, state: &mut DockState<String>, id: Id) {
    for _ in 0..4 {
        frame(ctx, state, id, Vec::new());
    }
}

/// Which tab an application's `Ctrl+W` would close right now — the very door `main_app` uses.
fn what_a_shortcut_would_reach(state: &mut DockState<String>) -> Option<String> {
    state.find_active_focused().cloned()
}

/// A scene with the focus parked on the main surface, and the window still untouched.
fn focus_parked_on_main() -> (Context, Id, DockState<String>, SurfaceIndex) {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let (mut state, window) = dock_with_a_window();
    settled(&ctx, &mut state, id);

    let at = main_body_point(&ctx, id, &state, window);
    click(&ctx, &mut state, id, at);
    assert_eq!(
        state.focused_leaf().map(|leaf| leaf.surface),
        Some(SurfaceIndex::Main),
        "the positive control: without the focus starting somewhere else, every assertion below \
         would pass on a dock that never moves the focus at all"
    );
    assert_eq!(
        what_a_shortcut_would_reach(&mut state).as_deref(),
        Some("main")
    );

    (ctx, id, state, window)
}

/// Dragging a window by its body makes it the focused surface — the reported bug, stated on the
/// door a shortcut reads.
#[test]
fn a_window_moved_by_its_body_becomes_the_focused_surface() {
    let (ctx, id, mut state, window) = focus_parked_on_main();

    let before = root_rect(&ctx, id, &state, window);
    let from = empty_body_of(before);
    frame(&ctx, &mut state, id, button(from, true));
    // egui calls a press a drag once it has travelled far enough; one long move is enough.
    let to = from + Vec2::new(60.0, 40.0);
    frame(&ctx, &mut state, id, moved_to(to));

    // Not a degenerate scene: the press really did reach the window's own drag. Without this the
    // assertion below would pass on a press that landed somewhere else entirely.
    let during = root_rect(&ctx, id, &state, window);
    assert!(
        (during.min - before.min).length() > 1.0,
        "the window did not move: {before:?} -> {during:?}"
    );

    // Mid-gesture the focus has not moved yet, and that is the contract rather than an accident:
    // the focus is part of the tree a consumer saves, so a live frame of a drag may not write it —
    // that would be one undo entry per frame of a window sliding across the screen.
    assert_eq!(
        state.focused_leaf().map(|leaf| leaf.surface),
        Some(SurfaceIndex::Main),
        "a drag in flight commits nothing, focus included"
    );

    frame(&ctx, &mut state, id, button(to, false));
    assert_eq!(
        state.focused_leaf().map(|leaf| leaf.surface),
        Some(window),
        "the hand let go of {window:?}, so that is the surface a shortcut addresses"
    );
    assert_eq!(
        what_a_shortcut_would_reach(&mut state).as_deref(),
        Some("floating"),
        "Ctrl+W closes the tab of the window that was dragged, not the one the focus was on before"
    );
}

/// A press inside a window that never travels is not the window's gesture: it belongs to whatever
/// was pressed, and the focus stays where it was.
///
/// The negative half of the rule above, and the one that keeps the fix honest — egui reports a
/// window's drag-from-anywhere for presses aimed at the dock's own widgets inside it as well, so
/// a focus taken at the *press* would fire on every one of them, and would name the window's leaf
/// rather than whatever the press was actually about.
#[test]
fn a_window_pressed_without_moving_leaves_the_focus_alone() {
    let (ctx, id, mut state, window) = focus_parked_on_main();

    let at = empty_body_of(root_rect(&ctx, id, &state, window));
    frame(&ctx, &mut state, id, button(at, true));
    for _ in 0..3 {
        frame(&ctx, &mut state, id, moved_to(at));
    }
    assert_eq!(
        state.focused_leaf().map(|leaf| leaf.surface),
        Some(SurfaceIndex::Main),
        "nothing moved, so nothing about the window was asked for"
    );

    // The release is a *click* on the window's body, which is the dock's own way of focusing a
    // leaf — so the focus does land, and it lands because of the click rather than because of the
    // window.
    frame(&ctx, &mut state, id, button(at, false));
    assert_eq!(
        state.focused_leaf().map(|leaf| leaf.surface),
        Some(window),
        "a click inside a leaf's body focuses it, window or not"
    );
}

/// Resizing a window by an edge focuses it too: it is the same "I am working on this window", and
/// the edge widgets are egui's, so nothing about the move gesture covers them.
#[test]
fn a_window_resized_by_its_edge_becomes_the_focused_surface() {
    let (ctx, id, mut state, window) = focus_parked_on_main();

    let before = window_outer_rect(&ctx, window);
    let from = before.right_center();
    frame(&ctx, &mut state, id, button(from, true));
    // Outward, which grows the window and keeps the drag away from the min-size clamp.
    let to = from + Vec2::new(60.0, 0.0);
    frame(&ctx, &mut state, id, moved_to(to));

    let during = window_outer_rect(&ctx, window);
    assert!(
        during.width() > before.width() + 1.0,
        "the window did not grow: {before:?} -> {during:?}"
    );

    frame(&ctx, &mut state, id, button(to, false));
    assert_eq!(
        state.focused_leaf().map(|leaf| leaf.surface),
        Some(window),
        "the hand resized {window:?}, so that is the surface a shortcut addresses"
    );
    assert_eq!(
        what_a_shortcut_would_reach(&mut state).as_deref(),
        Some("floating")
    );
}
