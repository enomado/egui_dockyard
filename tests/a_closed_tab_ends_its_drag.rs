//! A tab that is closed while it is being dragged takes its drag with it.
//!
//! # The gesture
//!
//! Press the left button on a tab, pull it out of the tab row, bring it back — the drag is
//! live all the while — and, without letting go, middle-click to close that same tab. The tab
//! *is* still a tab in the bar while it is dragged, so the close lands, and the dock is left
//! holding a drag whose subject no longer exists.
//!
//! Reported against the fork; it panicked on the very next frame.
//!
//! # Why it is a file of its own
//!
//! The drag is the only piece of dock state that **outlives the frame that created it**, and it
//! was addressed by *position*: `TabIndex` inside a `TabPath`, stored in `State::dnd` and read
//! again one or more frames later. Every position in a leaf is renumbered by every removal, so
//! the stored address decays in two different ways, and the scene decides which:
//!
//! * the closed tab was the **last** position — the address now names nothing, and indexing with
//!   it panicked (`index out of bounds: the len is 1 but the index is 1`, `show/leaf.rs`);
//! * the closed tab was **not** last — the address silently names its neighbour, and the gesture
//!   went on dragging a tab nobody grabbed. No panic, no message, a tab moved.
//!
//! The second is the reason this cannot be tested as "does it panic": a fix that only stops the
//! crash leaves the quiet half standing. So each scenario below asserts what the dock *holds*
//! afterwards, not merely that it survived.
//!
//! A third address is in play, and it belongs to egui rather than to the dock: a tab is drawn
//! under an id built from its *position* (`tab_widget_id`), and egui remembers that id as the
//! thing being dragged. Closing a tab therefore hands its id — and any live drag attached to it —
//! to whichever tab slides into the vacated slot. Ending the dock's drag without ending egui's
//! leaves the neighbour dragged by a hand that never grabbed it.
//!
//! # Two routes, and why both are here
//!
//! Which halves of that fire depends on *how* the tab was closed, and neither route covers the
//! other:
//!
//! * **middle click** (the report). The middle button's release is a release as far as egui is
//!   concerned, so egui drops its own drag on the spot — measured, not assumed:
//!   `dragged_id()` is `None` right after the click. What is left over is the dock's `State::dnd`,
//!   read on the next frame, and that is the panic.
//! * **`TabViewer::force_close`** — the application closing a tab under the hand, no button
//!   released, egui's drag still live. Here the id *is* inherited and the gesture goes on: before
//!   the fix, releasing over the other leaf dropped `Tab 3` — a tab the hand never grabbed —
//!   into it, with nothing raised anywhere.

use std::collections::HashSet;

use egui::{
    CentralPanel, Context, Event, Id, PointerButton, Pos2, RawInput, Rect, Ui, Vec2, WidgetText,
};
use egui_dock::{
    DockArea, DockLayout, DockState, NodePath, Style, SurfaceIndex, TabIndex, TabViewer,
    tab_widget_id,
};

const SCREEN: Vec2 = Vec2::new(1000.0, 700.0);
const DOCK_ID: &str = "a_closed_tab_ends_its_drag";

#[derive(Default)]
struct Viewer {
    /// Tabs the *application* wants gone, closed by the dock on the next pass that draws them.
    ///
    /// This is the second route into the bug and the one with no user gesture in it at all: a
    /// tab can leave the tree while a drag carries it without the hand doing anything.
    force_close: HashSet<String>,
}

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        tab.clone().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        ui.label(tab.as_str());
    }

    fn force_close(&mut self, tab: &mut Self::Tab) -> bool {
        self.force_close.contains(tab)
    }
}

/// What the dock holds, in tree order: one entry per leaf, with its tabs in bar order.
///
/// The assertions below are written against this rather than against a tab count, because the
/// quiet half of the bug moves a tab without changing how many there are.
type Contents = Vec<(NodePath, Vec<String>)>;

struct Sim {
    ctx: Context,
    state: DockState<String>,
    viewer: Viewer,
    frame: u32,
}

impl Sim {
    fn new(state: DockState<String>) -> Self {
        let mut sim = Self {
            ctx: Context::default(),
            state,
            viewer: Viewer::default(),
            frame: 0,
        };
        // Gestures are aimed with geometry, and there is no geometry until a pass has run.
        sim.run(vec![]);
        sim
    }

    fn run(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            // egui's drag detection is time-based; a clock that never ticks is its own kind of
            // unreal scene.
            time: Some(f64::from(self.frame) / 60.0),
            events,
            ..Default::default()
        };
        self.frame += 1;

        let state = &mut self.state;
        let viewer = &mut self.viewer;
        let mut output = self.ctx.run_ui(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                DockArea::new(state)
                    .id(Id::new(DOCK_ID))
                    .style(Style::from_egui(ui.style().as_ref()))
                    .show_close_buttons(true)
                    .show_inside(ui, viewer);
            });
        });
        // Headless harness: no GPU backend to hand the delta to, and epaint panics on drop
        // otherwise.
        output.textures_delta.clear();
    }

    fn contents(&self) -> Contents {
        self.state
            .iter_leaves()
            .map(|(path, leaf)| {
                (
                    path,
                    leaf.iter_tabs().map(String::to_owned).collect::<Vec<_>>(),
                )
            })
            .collect()
    }

    fn layout(&self) -> DockLayout {
        DockLayout::load(&self.ctx, Id::new(DOCK_ID))
    }

    /// Where a tab was drawn during the last frame.
    ///
    /// Aiming at a guessed offset into the tab bar was tried first and missed the tabs
    /// entirely — the bar has leading buttons — so the address comes from the dock itself.
    fn tab_rect(&self, leaf: NodePath, tab: usize) -> Rect {
        self.ctx
            .read_response(tab_widget_id(Id::new(DOCK_ID), leaf, TabIndex(tab)))
            .expect("the tab was drawn last frame")
            .rect
    }

    fn is_dragging(&self, leaf: NodePath, tab: usize) -> bool {
        self.ctx
            .is_being_dragged(tab_widget_id(Id::new(DOCK_ID), leaf, TabIndex(tab)))
    }

    fn move_to(&mut self, pos: Pos2) {
        self.run(vec![Event::PointerMoved(pos)]);
    }

    /// Moves the pointer from `from` to `to` in steps, one frame each, and rests at the far
    /// end long enough for the overlay's preference lock to settle on what is under it.
    fn sweep(&mut self, from: Pos2, to: Pos2) {
        for step in 1..=8u8 {
            self.move_to(from + (to - from) * (f32::from(step) / 8.0));
        }
        for _ in 0..PREFERENCE_FRAMES {
            self.move_to(to);
        }
    }

    fn button(&mut self, pos: Pos2, button: PointerButton, pressed: bool) {
        self.run(vec![Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Default::default(),
        }]);
    }

    /// A middle click: press and release, in two frames, exactly as a mouse delivers it.
    fn middle_click(&mut self, pos: Pos2) {
        self.button(pos, PointerButton::Middle, true);
        self.button(pos, PointerButton::Middle, false);
        // Removals are applied at the end of the pass, so the state that follows the click is
        // the state of the frame *after* it.
        self.run(vec![]);
    }
}

/// Frames enough for `Style::overlay.feel.max_preference_time` (0.3 s by default) to lapse,
/// plus a margin: the overlay keeps a preference for the target the pointer arrived on and
/// ignores later ones until it does, so a drop released too soon resolves against a leaf the
/// pointer merely crossed.
const PREFERENCE_FRAMES: u32 = 30;

/// Grabs `tab` of `leaf` and pulls it out of the tab row and back, left button still down.
///
/// Returns where the tab sat in the bar — where the hand is when this returns, and where a
/// middle click has to land to hit that tab.
fn grab(sim: &mut Sim, leaf: NodePath, tab: usize) -> Pos2 {
    let home = sim.tab_rect(leaf, tab).center();
    sim.move_to(home);
    sim.button(home, PointerButton::Primary, true);

    // Out of the tab row — far enough that the dock calls it a drag rather than a click — and
    // back to where it started, the button never released.
    let out = home + Vec2::new(0.0, 200.0);
    sim.sweep(home, out);
    assert!(
        sim.is_dragging(leaf, tab),
        "the scene has to be a live drag, or it is not the gesture under test"
    );
    sim.sweep(out, home);
    assert!(
        sim.is_dragging(leaf, tab),
        "and still a live drag on the way back"
    );
    home
}

/// Two tabs, and the second one — the last position in the bar — is closed mid-drag. This is
/// the reported crash: the stored address named a position that stopped existing.
#[test]
fn closing_the_dragged_tab_ends_the_drag() {
    let mut sim = Sim::new(DockState::new(vec!["Tab 1".to_owned(), "Tab 2".to_owned()]));
    let leaf = NodePath::new(
        SurfaceIndex::main(),
        sim.state.main_surface().root().unwrap(),
    );

    let home = grab(&mut sim, leaf, 1);
    sim.middle_click(home);

    assert_eq!(
        sim.contents(),
        vec![(leaf, vec!["Tab 1".to_owned()])],
        "the closed tab is gone and the other one is untouched"
    );
    assert_eq!(sim.state.validate(), Ok(()), "and the dock is well-formed");
    assert!(
        sim.ctx.dragged_id().is_none(),
        "nothing is being dragged any more — not the closed tab, not its neighbour"
    );

    // The hand is still down. Whatever it does now is not a drop: it has nothing to drop.
    let body = sim.layout().viewport(leaf).unwrap().center();
    sim.sweep(home, body);
    sim.button(body, PointerButton::Primary, false);
    sim.run(vec![]);

    assert_eq!(
        sim.contents(),
        vec![(leaf, vec!["Tab 1".to_owned()])],
        "and releasing the button moves nothing"
    );
    assert_eq!(sim.state.validate(), Ok(()));
}

/// The closed tab is *not* the last one in the bar, and the close comes from the application
/// rather than from the hand: `TabViewer::force_close` while the drag is in flight — a tab that
/// finished loading and closed itself, a document the app dropped, a panel switched off from a
/// menu. The hand never lets go, so this is the route where **egui's** drag survives the removal
/// too, and its id — built from the tab's position — is now the neighbour's id.
///
/// This is the quiet half of the bug: nothing panics, `Tab 3` is simply carried off by a
/// gesture that grabbed `Tab 2`, and lands wherever the hand happens to let go.
///
/// The right-hand leaf is the instrument — somewhere for a hijacked drag to land.
#[test]
fn the_neighbour_does_not_inherit_the_drag() {
    let mut state = DockState::new(vec![
        "Tab 1".to_owned(),
        "Tab 2".to_owned(),
        "Tab 3".to_owned(),
    ]);
    let root = state.main_surface().root().unwrap();
    let [left, right] =
        state
            .main_surface_mut()
            .split_right(root, 0.5, vec!["Elsewhere".to_owned()]);
    let (left, right) = (
        NodePath::new(SurfaceIndex::main(), left),
        NodePath::new(SurfaceIndex::main(), right),
    );
    let mut sim = Sim::new(state);

    let home = grab(&mut sim, left, 1);

    // The application closes the dragged tab under the hand. One pass to see the request, and
    // the removal lands at the end of it.
    sim.viewer.force_close.insert("Tab 2".to_owned());
    sim.run(vec![]);
    sim.viewer.force_close.clear();
    sim.run(vec![]);

    let settled = vec![
        (left, vec!["Tab 1".to_owned(), "Tab 3".to_owned()]),
        (right, vec!["Elsewhere".to_owned()]),
    ];
    assert_eq!(sim.contents(), settled, "only the closed tab left");
    assert!(
        sim.ctx.dragged_id().is_none(),
        "the neighbour that inherited the closed tab's id did not inherit its drag"
    );

    // Carry the still-pressed hand over to the other leaf and let go. A drag that survived the
    // close would drop `Tab 3` here.
    let target = sim.layout().viewport(right).unwrap().center();
    sim.sweep(home, target);
    sim.button(target, PointerButton::Primary, false);
    sim.run(vec![]);

    assert_eq!(
        sim.contents(),
        settled,
        "and the release drops nothing on the leaf the pointer ended over"
    );
    assert_eq!(sim.state.validate(), Ok(()));
}

/// The closed tab was the only one in its leaf, so the removal takes the leaf — and the node
/// the drag's address is anchored to — out of the tree with it. The address then names neither
/// a tab nor a node, which is a second way for it to decay and a second place it was indexed.
#[test]
fn closing_the_last_tab_of_a_leaf_mid_drag_leaves_the_dock_standing() {
    let mut state = DockState::new(vec!["Tab 1".to_owned()]);
    let root = state.main_surface().root().unwrap();
    let [left, right] = state
        .main_surface_mut()
        .split_right(root, 0.5, vec!["Tab 2".to_owned()]);
    let (left, right) = (
        NodePath::new(SurfaceIndex::main(), left),
        NodePath::new(SurfaceIndex::main(), right),
    );
    let mut sim = Sim::new(state);

    let home = grab(&mut sim, right, 0);
    sim.middle_click(home);

    assert_eq!(
        sim.contents(),
        vec![(left, vec!["Tab 1".to_owned()])],
        "the emptied leaf went with its last tab, and the split collapsed onto the other one"
    );
    assert_eq!(sim.state.validate(), Ok(()));
    assert!(sim.ctx.dragged_id().is_none(), "the drag is over");

    // The leaf the drag was anchored to is gone; releasing over what is left must be a no-op.
    let body = sim.layout().viewport(left).unwrap().center();
    sim.sweep(home, body);
    sim.button(body, PointerButton::Primary, false);
    sim.run(vec![]);

    assert_eq!(sim.contents(), vec![(left, vec!["Tab 1".to_owned()])]);
    assert_eq!(sim.state.validate(), Ok(()));
}
