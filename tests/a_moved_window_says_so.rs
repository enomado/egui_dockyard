//! A floating window being dragged is a gesture the dock can name.
//!
//! # Why this is a test file of its own
//!
//! Every other gesture in the vocabulary is the dock's own widget: a tab, a separator, a junction
//! handle are all allocated and sensed by `egui_dockyard`, so "does the field know about it" is a
//! question about code that is right there. A window move is not — it belongs to
//! [`egui::Window`], which the dock builds with no title bar, so egui resolves it to a
//! drag-from-anywhere over the window's body. The dock takes none of that over. It reads the
//! response `Window::show` hands back, which it used to drop on the floor.
//!
//! That makes this the one variant of [`DragSubject`] whose honesty is not visible in the crate's
//! own source: if egui stopped reporting the move on that response — a different drag mode, a
//! title bar, a version that senses the body some other way — the field would go quietly empty
//! while a window slid across the screen under the hand, and nothing in the crate would fail to
//! compile. The scene here is the check: press inside a window, move, and ask both holders of a
//! drag — egui and the dock — what they think is happening.
//!
//! The frame sweep in `dst.rs` reaches this gesture too (`SubjectWatch::window`), but only as
//! coverage: it counts frames in which the dock said "a window", and a count cannot say *which*
//! window or that the thing on screen actually moved. Both are asserted here.

use egui::{
    CentralPanel, Context, Event, Id, Modifiers, PointerButton, Pos2, RawInput, Rect, Ui, Vec2,
    WidgetText,
};
use egui_dockyard::{
    DockArea, DockLayout, DockState, DragInFlight, DragSubject, NodePath, Style, SurfaceIndex,
    TabViewer, drag_in_flight,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_moved_window_says_so";

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

/// A dock with one floating window in it, and the window's index.
fn dock_with_a_window() -> (DockState<String>, SurfaceIndex) {
    let mut state = DockState::new(vec!["main".to_owned()]);
    let window = state.add_window(vec!["floating".to_owned()]);
    (state, window)
}

/// One headless frame, with whatever pointer events the step needs, and what the dock said it
/// was holding when that frame ended.
///
/// The answer is taken from [`DockAreaResponse::dragging`] rather than from
/// [`drag_in_flight`] afterwards, and the difference is not cosmetic: the two go through the same
/// liveness filter, but between frames the pass number has moved on, so a gesture that the dock
/// failed to end is filtered out as a leftover and the failure becomes invisible. Inside the
/// frame it is still live, and a release that does not empty the hand can be seen.
///
/// [`DockAreaResponse::dragging`]: egui_dockyard::dock_area::DockAreaResponse::dragging
fn frame(
    ctx: &Context,
    state: &mut DockState<String>,
    id: Id,
    events: Vec<Event>,
) -> Option<DragInFlight> {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
        events,
        ..Default::default()
    };
    let mut held = None;
    let mut output = ctx.run_ui(input, |ctx| {
        CentralPanel::default().show(ctx, |ui| {
            held = DockArea::new(state)
                .id(id)
                .style(Style::from_egui(ui.style().as_ref()))
                .show_inside_with_response(ui, &mut Viewer)
                .dragging;
        });
    });
    // Headless harness, no GPU backend to hand the delta to.
    output.textures_delta.clear();
    held
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

/// Where the window's content sits this frame — the surface's root rectangle.
fn window_rect(ctx: &Context, id: Id, state: &DockState<String>, window: SurfaceIndex) -> Rect {
    let root = state[window].root().expect("the window has a root");
    DockLayout::load(ctx, id)
        .rect(NodePath::new(window, root))
        .expect("the window surface was laid out this frame")
}

/// A point inside the window that no widget of the dock's answers to.
///
/// Low in the leaf's body, below the tab bar and away from every button: a press here is one egui
/// hands to the window itself, which is the gesture under test. Aiming at the tab bar instead
/// would start a *tab* drag, which is a different subject and already has its own oracles.
fn empty_body_of(rect: Rect) -> Pos2 {
    rect.center() + Vec2::new(0.0, rect.height() * 0.25)
}

/// A settled scene: the window has finished finding out how big it is.
fn settled(ctx: &Context, state: &mut DockState<String>, id: Id) {
    for _ in 0..4 {
        frame(ctx, state, id, Vec::new());
    }
}

/// The two holders of a drag agree, by name, that a window is what the hand is moving.
#[test]
fn a_window_dragged_by_its_body_is_named_by_the_dock() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let (mut state, window) = dock_with_a_window();
    settled(&ctx, &mut state, id);

    let before = window_rect(&ctx, id, &state, window);
    let from = empty_body_of(before);
    frame(&ctx, &mut state, id, button(from, true));

    // egui calls a press a drag once it has travelled far enough; one long move is enough.
    let to = from + Vec2::new(60.0, 40.0);
    let held = frame(&ctx, &mut state, id, moved_to(to))
        .expect("the dock is holding the window being moved");
    assert_eq!(
        drag_in_flight(&ctx, id).map(|drag| drag.subject),
        Some(held.subject),
        "the two ways out of the field — the pass's response and the reader between frames — \
         are one value seen twice, so they cannot disagree about a live gesture"
    );
    assert_eq!(
        held.subject,
        DragSubject::Window {
            surface: window,
            edge: None
        },
        "the hand is moving {window:?}, and that is what the field must say"
    );
    assert_eq!(
        Some(held.widget),
        ctx.dragged_id(),
        "the dock names the gesture by egui's own widget, or the two holders cannot be compared"
    );
    assert!(
        held.moved,
        "the pointer travelled {:?}, so the gesture has done something",
        to - from
    );

    // Not a degenerate scene: the press really did reach the window's own drag, and the thing on
    // screen moved. Without this the assertions above would still pass if the window sat still
    // and the field were filled by something that merely watched the pointer.
    let during = window_rect(&ctx, id, &state, window);
    assert!(
        (during.min - before.min).length() > 1.0,
        "the window did not move: {before:?} -> {during:?}"
    );

    let after_release = frame(&ctx, &mut state, id, button(to, false));
    assert!(
        after_release.is_none(),
        "the hand opened, so by the end of that very frame it holds nothing — and asking in the \
         frame is the whole of the check: a pass later a gesture that was never ended is dropped \
         as a leftover, which would make forgetting to end it unobservable"
    );
    assert!(drag_in_flight(&ctx, id).is_none());
}

/// A press that never travels is not a gesture that moved anything.
///
/// `moved` is the commit gate every subject answers, and for a window — whose position lives in
/// egui's area memory rather than in the dock's tree — it is the *only* thing that separates
/// "the layout is being edited right now" from a click that landed on a window and did nothing.
#[test]
fn a_window_pressed_and_released_moved_nothing() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let (mut state, window) = dock_with_a_window();
    settled(&ctx, &mut state, id);

    let before = window_rect(&ctx, id, &state, window);
    let at = empty_body_of(before);
    frame(&ctx, &mut state, id, button(at, true));
    // Frames pass with the button down and the pointer still. egui may or may not call this a
    // drag — that is its business — but nothing has moved, and the field must not claim otherwise.
    for _ in 0..3 {
        if let Some(held) = frame(&ctx, &mut state, id, moved_to(at)) {
            assert_eq!(
                held.subject,
                DragSubject::Window {
                    surface: window,
                    edge: None
                }
            );
            assert!(!held.moved, "the pointer never left {at:?}");
        }
    }

    frame(&ctx, &mut state, id, button(at, false));
    let after = window_rect(&ctx, id, &state, window);
    assert!(
        (after.min - before.min).length() < 0.5,
        "a still press moved the window from {before:?} to {after:?}"
    );
    assert!(drag_in_flight(&ctx, id).is_none());
}
