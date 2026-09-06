//! A floating window's edge, dragged to resize it, is a gesture the dock can name.
//!
//! # Why this is a test file of its own, and not a case added to the move test
//!
//! [`a_moved_window_says_so`] already makes the case for why a window's gesture needs a test at
//! all: it is not the dock's own widget, so nothing here fails to *compile* if the field stops
//! seeing it. A resize is the same risk one level worse — a move is at least read off a
//! `Response` `Window::show` hands back; a resize is read off a widget id the dock reconstructs
//! by hand (`WINDOW_EDGES` in `window_surface.rs`) to match one egui builds and consumes entirely
//! inside itself, surfacing nothing to the caller. If egui ever renamed that id's salts, or moved
//! the resize widgets to a different layer, this file is the only thing that would notice: the
//! crate would keep compiling, and the field would go quietly empty while a window resized under
//! the hand.
//!
//! See `tests/a_moved_window_says_so.rs`.

use egui::{
    Atoms, CentralPanel, Context, Event, Id, Modifiers, PointerButton, Pos2, RawInput, Rect, Ui,
    Vec2,
};
use egui_dockyard::{
    DockArea, DockState, DragInFlight, DragSubject, Style, SurfaceIndex, TabViewer, WindowEdge,
    drag_in_flight,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_resized_window_says_so";

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

/// One headless frame, with whatever pointer events the step needs, and what the dock said it
/// was holding when that frame ended.
///
/// See `tests/a_moved_window_says_so.rs` for why the answer is taken from
/// [`DockAreaResponse::dragging`] and not from [`drag_in_flight`] afterwards.
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
                .show_inside(ui, &mut Viewer)
                .apply(ui.ctx(), state, &mut Viewer)
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

/// The window's outer frame — the same rectangle egui's own `do_resize_interaction` builds its
/// eight edge/corner grab zones from (shrunk by half the frame's stroke width, which the grab
/// radius comfortably covers). This is *not* the content rectangle `DockLayout` tracks: that one
/// is inset from this by the window's frame margin and stroke, and a press aimed at its edge
/// would land inside the body, not on a resize handle.
fn window_outer_rect(ctx: &Context, window_area_id: Id) -> Rect {
    ctx.memory(|mem| mem.area_rect(window_area_id))
        .expect("the window surface was laid out this frame")
}

/// The same id `show_window_surface` builds for the window's area — frozen format, see the
/// comment at its construction site in `window_surface.rs`.
fn window_area_id(window: SurfaceIndex) -> Id {
    let SurfaceIndex::Window(index) = window else {
        panic!("not a window surface");
    };
    Id::new(format!("window SurfaceIndex({})", index.0 + 1))
}

/// A settled scene: the window has finished finding out how big it is.
fn settled(ctx: &Context, state: &mut DockState<String>, id: Id) {
    for _ in 0..4 {
        frame(ctx, state, id, Vec::new());
    }
}

/// The two holders of a drag agree, by name, that a window's right edge is what the hand is
/// resizing, and the window really does grow under it.
#[test]
fn a_window_dragged_by_its_right_edge_is_named_by_the_dock() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let (mut state, window) = dock_with_a_window();
    settled(&ctx, &mut state, id);

    let area_id = window_area_id(window);
    let before = window_outer_rect(&ctx, area_id);
    let from = before.right_center();
    frame(&ctx, &mut state, id, button(from, true));

    // egui calls a press a drag once it has travelled far enough; one long move is enough, and
    // outward (growing the window) keeps it away from the min-size clamp.
    let to = from + Vec2::new(60.0, 0.0);
    let held = frame(&ctx, &mut state, id, moved_to(to))
        .expect("the dock is holding the edge being dragged");
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
            edge: Some(WindowEdge::Right)
        },
        "the hand is resizing {window:?} by its right edge, and that is what the field must say"
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

    // Not a degenerate scene: the press really did reach egui's resize widget, and the window on
    // screen actually grew. Without this the assertions above would still pass if the field were
    // filled by something that merely watched the pointer near an edge.
    let during = window_outer_rect(&ctx, area_id);
    assert!(
        during.width() > before.width() + 1.0,
        "the window did not grow: {before:?} -> {during:?}"
    );

    let after_release = frame(&ctx, &mut state, id, button(to, false));
    assert!(
        after_release.is_none(),
        "the hand opened, so by the end of that very frame it holds nothing"
    );
    assert!(drag_in_flight(&ctx, id).is_none());
}

/// A press on the right edge that never travels is not a gesture that resized anything.
#[test]
fn a_window_edge_pressed_and_released_resized_nothing() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let (mut state, window) = dock_with_a_window();
    settled(&ctx, &mut state, id);

    let area_id = window_area_id(window);
    let before = window_outer_rect(&ctx, area_id);
    let at = before.right_center();
    frame(&ctx, &mut state, id, button(at, true));
    for _ in 0..3 {
        if let Some(held) = frame(&ctx, &mut state, id, moved_to(at)) {
            assert_eq!(
                held.subject,
                DragSubject::Window {
                    surface: window,
                    edge: Some(WindowEdge::Right)
                }
            );
            assert!(!held.moved, "the pointer never left {at:?}");
        }
    }

    frame(&ctx, &mut state, id, button(at, false));
    let after = window_outer_rect(&ctx, area_id);
    assert!(
        (after.width() - before.width()).abs() < 0.5,
        "a still press resized the window from {before:?} to {after:?}"
    );
    assert!(drag_in_flight(&ctx, id).is_none());
}
