//! The overlay's preference lock lasts as long as the style says it does.
//!
//! # What the lock is
//!
//! While a tab is being dragged, the dock remembers the target the pointer arrived on and
//! *refuses to look at another one* until `overlay.feel.max_preference_time` has passed
//! (`State::set_drag_and_drop` declines to overwrite the hover while `is_locked`). It is a
//! flicker guard for a hand that sweeps across two leaves on its way to a third: without it the
//! drop resolves against whatever the pointer happened to cross last.
//!
//! # Why this file exists
//!
//! The DST sweep drags hundreds of tabs, and every one of them exercises the lock — but it runs
//! with `max_preference_time` cut to 0.05 s so that a drag need not rest twenty frames, and its
//! pause is computed *from the same number*. That is a sound trade for the sweep (the drop still
//! lands where the step aimed, and coverage came out identical), and it means the sweep can say
//! nothing about the lock's **duration**: shrink the number to zero, stretch it to a minute, and
//! the sweep is equally green because it waits exactly as long as it is told.
//!
//! So the property that nothing else states: the lock is a *duration*, and the same number of
//! frames falls on either side of it depending on what that duration is. Both cases below run
//! the identical gesture, frame for frame; only the style differs.
//!
//! Nothing here needs a screen — the destination is read off the tree afterwards.

use egui::{
    Atoms, CentralPanel, Context, Event, Id, Modifiers, PointerButton, Pos2, RawInput, Rect, Ui,
    Vec2,
};
use egui_dockyard::{
    DockArea, DockLayout, DockState, Node, NodePath, Split, Style, SurfaceIndex, TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "the_preference_lock_is_a_duration";

/// The lock the crate ships with, and the one the app therefore runs on.
const DEFAULT_PREFERENCE_TIME: f32 = 0.3;

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

/// Where the dragged tab ended up.
#[derive(Debug, PartialEq, Eq)]
enum Landed {
    /// Torn off into a new floating window — which is what "the preference is still held"
    /// looks like from the outside.
    ///
    /// The overlay was still offering the *window's* buttons, drawn over the window the pointer
    /// had already left, and the pointer was nowhere near them. `resolve_icon_based` starts from
    /// `TabDestination::Window` and only replaces it when a button answers, so a release with no
    /// button under the pointer tears the tab off. That is the flicker guard doing its job: the
    /// drop deliberately does **not** go to the leaf the pointer is over.
    TornOff,
    /// Appended into the main-surface leaf the pointer was actually over: the preference had
    /// expired, the hover followed the pointer, and its centre button answered.
    MainRight,
}

/// One headless dock: a main surface split in two, and a floating window over the left half.
///
/// The window is placed by hand so that the pointer's path is unambiguous — it must be possible
/// to be over the window, and then over the right-hand leaf, without the two overlapping.
struct Scene {
    ctx: Context,
    state: DockState<String>,
    style: Style,
    frame: u32,
    window: SurfaceIndex,
    right: egui_dockyard::NodeId,
    left: egui_dockyard::NodeId,
}

impl Scene {
    fn new(preference_time: f32) -> Self {
        let mut style = Style::from_egui(&egui::Style::default());
        style.overlay.feel.max_preference_time = preference_time;

        let mut state = DockState::new(vec!["left".to_owned()]);
        let root = state.main_surface().root().unwrap();
        let [left, right] = state.split(
            NodePath::new(SurfaceIndex::main(), root),
            Split::Right,
            0.5,
            Node::leaf("right".to_owned()),
        );
        let window = state.add_window(vec!["floating".to_owned()]);
        {
            let window_state = state.get_window_state_mut(window).unwrap();
            window_state.set_position(egui_dockyard::geom::Point::new(60.0, 60.0));
            window_state.set_size(egui_dockyard::geom::Size::new(300.0, 240.0));
        }

        let mut scene = Self {
            ctx: Context::default(),
            state,
            style,
            frame: 0,
            window,
            left,
            right,
        };
        // The window consumes its "move me"/"resize me" requests one frame at a time, and every
        // gesture below aims with geometry that does not exist until a pass has run.
        for _ in 0..8 {
            scene.run_frame(vec![]);
        }
        scene
    }

    /// One frame. Time advances, because everything this file is about is a time comparison.
    fn run_frame(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            time: Some(f64::from(self.frame) / 60.0),
            events,
            ..Default::default()
        };
        self.frame += 1;

        let state = &mut self.state;
        let style = &self.style;
        let mut output = self.ctx.run_ui(input, |ui| {
            CentralPanel::default().show(ui, |ui| {
                DockArea::new(state)
                    .id(Id::new(DOCK_ID))
                    .style(style.clone())
                    .show_inside(ui, &mut Viewer)
                    .apply(ui.ctx(), state);
            });
        });
        // No GPU backend here to apply them, and `TexturesDelta` panics if dropped unapplied.
        output.textures_delta.clear();
    }

    fn leaf_rect(&self, surface: SurfaceIndex, node: egui_dockyard::NodeId) -> Rect {
        DockLayout::load(&self.ctx, Id::new(DOCK_ID))
            .rect(NodePath::new(surface, node))
            .expect("the leaf was laid out")
    }

    /// Where the first tab of a leaf was drawn — the point a drag has to start from.
    fn first_tab(&self, surface: SurfaceIndex, node: egui_dockyard::NodeId) -> Rect {
        let id = Id::new(DOCK_ID)
            .with((surface, "surface"))
            .with((node, "node"))
            .with((0usize, "tab"));
        self.ctx
            .read_response(id)
            .expect("the tab was drawn last frame")
            .rect
    }

    /// Drag the left leaf's tab across the floating window, on to the right leaf, and let go
    /// after resting `rest_frames` frames there.
    ///
    /// The pointer *passes over the window and leaves it* — that is the whole gesture. What the
    /// lock decides is whether the release, some frames later, still counts as aimed at the
    /// window.
    fn drag_over_the_window_and_release(&mut self, rest_frames: u32) -> Landed {
        let window_leaf = self.state[self.window].root().unwrap();
        let over_window = self.leaf_rect(self.window, window_leaf).center();
        self.drag_via(over_window, rest_frames)
    }

    /// The gesture itself: grab the left leaf's only tab, cross `over`, come to rest on the
    /// right leaf for `rest_frames`, let go.
    ///
    /// `over` is the only thing that differs between the scenes, and the frame counts are fixed,
    /// so two runs differ in exactly what the test says they differ in.
    fn drag_via(&mut self, over: Pos2, rest_frames: u32) -> Landed {
        let grab = self.first_tab(SurfaceIndex::main(), self.left).center();
        let destination = self.leaf_rect(SurfaceIndex::main(), self.right).center();

        self.run_frame(vec![Event::PointerMoved(grab)]);
        self.run_frame(vec![Event::PointerButton {
            pos: grab,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        // Into the crossed leaf, and long enough for the dock to take it as the target: egui
        // only calls a press a drag once the pointer has travelled, and the hover is read per
        // frame.
        for step in 1..=4u8 {
            let t = f32::from(step) / 4.0;
            self.run_frame(vec![Event::PointerMoved(grab + (over - grab) * t)]);
        }
        for _ in 0..3 {
            self.run_frame(vec![Event::PointerMoved(over)]);
        }
        // Out of it in one step, so that the frames spent over the destination are the only
        // thing that varies between the runs.
        self.run_frame(vec![Event::PointerMoved(destination)]);
        for _ in 0..rest_frames {
            self.run_frame(vec![Event::PointerMoved(destination)]);
        }
        self.run_frame(vec![Event::PointerButton {
            pos: destination,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        // Removals and detachments are applied at the end of the pass after the drop.
        self.run_frame(vec![Event::PointerMoved(destination)]);

        self.landed()
    }

    /// Split a third leaf out of the left one, so the pointer has a main-surface leaf to cross
    /// on its way to the right-hand one.
    fn split_off_a_middle_leaf(&mut self) -> egui_dockyard::NodeId {
        let [_, middle] = self.state.split(
            NodePath::new(SurfaceIndex::main(), self.left),
            Split::Right,
            0.5,
            Node::leaf("middle".to_owned()),
        );
        for _ in 0..4 {
            self.run_frame(vec![]);
        }
        middle
    }

    /// The same gesture as [`Scene::drag_over_the_window_and_release`], with a main-surface leaf
    /// in the middle instead of the floating window.
    fn drag_over_a_main_leaf_and_release(
        &mut self,
        crossed: egui_dockyard::NodeId,
        rest_frames: u32,
    ) -> Landed {
        let over = self.leaf_rect(SurfaceIndex::main(), crossed).center();
        self.drag_via(over, rest_frames)
    }

    /// Read the destination off the tree, and refuse to guess.
    ///
    /// The source leaf holds exactly one tab, so it is removed either way and the main surface
    /// ends up with one leaf; what separates the two outcomes is whether a *third* surface
    /// appeared. Both numbers are read, so a scene that did neither cannot pass as either.
    fn landed(&self) -> Landed {
        let surfaces = self.state.iter_surfaces().count();
        let in_right = self.state.main_surface()[self.right].iter_tabs().count();
        match (surfaces, in_right) {
            // Main + the floating window + one more that this release tore off.
            (3, 1) => Landed::TornOff,
            (2, 2) => Landed::MainRight,
            other => panic!(
                "the drag landed somewhere this test cannot name: (surfaces, tabs in right) = \
                 {other:?}. The gesture missed, and every assertion about the lock would be \
                 about a scene that never happened."
            ),
        }
    }
}

/// The same gesture, the same number of frames, two different locks — and two different drops.
///
/// Twelve frames is 0.2 s on this clock, which is inside a 0.3 s lock and outside a 0.1 s one.
/// Neither case is a boundary: each is a tenth of a second clear of its threshold, so this is
/// not a test of `<` versus `<=`.
///
/// A frame count would give both cases the same answer. A duration does not — which is exactly
/// what the DST sweep, whose pause is computed *from* `max_preference_time`, cannot say.
#[test]
fn the_same_pause_falls_on_either_side_of_two_different_locks() {
    const REST_FRAMES: u32 = 12; // 0.2 s at 60 fps

    let long = Scene::new(0.3).drag_over_the_window_and_release(REST_FRAMES);
    let short = Scene::new(0.1).drag_over_the_window_and_release(REST_FRAMES);

    assert_eq!(
        long,
        Landed::TornOff,
        "with a 0.3 s lock, a release 0.2 s after leaving the window is still aimed at the \
         window: the preference had not expired, so the leaf under the pointer was not even \
         offered"
    );
    assert_eq!(
        short,
        Landed::MainRight,
        "with a 0.1 s lock, the same 0.2 s pause is past the preference, and the drop belongs \
         to the leaf the pointer is actually over"
    );
}

/// And the number the crate ships with behaves the same way, so the property is about the dock
/// as delivered and not only about styles a test invented.
#[test]
fn the_default_lock_holds_a_target_for_the_time_it_promises() {
    // Just inside: a quarter of a second is less than 0.3 s.
    let inside = Scene::new(DEFAULT_PREFERENCE_TIME).drag_over_the_window_and_release(15);
    // And clear of the other side, with a frame to spare — the comparison is `<` on elapsed
    // time, so landing exactly on the boundary would be a coin toss dressed as a test.
    let outside = Scene::new(DEFAULT_PREFERENCE_TIME)
        .drag_over_the_window_and_release((DEFAULT_PREFERENCE_TIME * 60.0).ceil() as u32 + 2);

    assert_eq!(inside, Landed::TornOff, "0.25 s is inside a 0.3 s lock");
    assert_eq!(
        outside,
        Landed::MainRight,
        "past 0.3 s the dock must look at the pointer again"
    );
}

/// Between two leaves of the **main** surface the preference does not outlive the frame.
///
/// This one is a characterisation, not a promise: it records what the dock does, because what
/// it does is not what the guard's name suggests. `update_lock` clears the lock the moment the
/// pointer is off the hovered rectangle —
///
/// ```text
/// let window_hold = if !self.hover.dst.surface_address().is_main() { self.is_locked(..) }
///                   else { false };
/// if target_state == LockState::Unlocked && !window_hold { self.locked = None }
/// ```
///
/// — and `window_hold` is `false` whenever the held target is on the main surface. So the
/// duration is consulted only for a *window*: between main-surface leaves the preference lasts
/// exactly the one frame that `set_drag_and_drop` was refused, whatever `max_preference_time`
/// says. The gesture below rests two frames on the destination, some 0.28 s inside a 0.3 s
/// lock, and the tab lands where the pointer is.
///
/// Whether that asymmetry is intended is an open question — it is written up in `ORIGIN.md`. If it
/// is ever made symmetric, this is the test that will say so, and it should be *changed* rather
/// than deleted.
#[test]
fn between_main_surface_leaves_the_preference_does_not_outlive_a_frame() {
    let mut scene = Scene::new(DEFAULT_PREFERENCE_TIME);
    let middle = scene.split_off_a_middle_leaf();

    let landed = scene.drag_over_a_main_leaf_and_release(middle, 2);

    assert_eq!(
        landed,
        Landed::MainRight,
        "two frames is well inside a {DEFAULT_PREFERENCE_TIME} s lock, and yet the drop \
         followed the pointer: on the main surface the preference is dropped as soon as the \
         pointer leaves the leaf it was held for"
    );
}
