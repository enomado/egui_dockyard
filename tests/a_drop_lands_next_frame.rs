//! Letting go of a dragged tab takes effect on the frame after the release.
//!
//! The drop is the last edit drawing made to the tree by itself: the overlay resolved where a
//! release would land and `move_tab` ran right there, *before* any surface was drawn, so the frame
//! of the release already painted the tab in its new home. It is queued now, like every other edit
//! a pass asks for, and applied by the render epilogue — which is one call after every surface has
//! been visited. The release frame therefore paints the arrangement as it was, and the tab appears
//! in its new leaf on the next repaint.
//!
//! This file exists for the same reason
//! [`a_click_that_changes_a_leaf_lands_next_frame`](a_click_that_changes_a_leaf_lands_next_frame.rs)
//! does, and it is worth saying twice: the shift is **invisible** to every other gate in the
//! network. `a_dead_drop_destination_is_not_a_drop.rs` and the `dst.rs` sweep both ask what the
//! tree holds *after* the release frame — and the epilogue has run by then, so they read the moved
//! tab either way. Only what a single frame *painted* tells the two behaviours apart.
//!
//! The second test is not about the shift but about what the shift costs: with the move deferred,
//! everything else the frame asked for is applied *before* it, against a tree that has not been
//! edited yet — so the drop, and only the drop, can find its destination taken away by its own
//! frame. See `DockMutation::MoveTab`.

use std::collections::HashSet;

use egui::{
    Atoms, CentralPanel, Context, Event, Id, PointerButton, Pos2, RawInput, Rect, Ui, Vec2,
};
use egui_dockyard::{
    DockArea, DockLayout, DockState, NodePath, Style, SurfaceIndex, TabIndex, TabViewer,
    tab_widget_id,
};

const SCREEN: Vec2 = Vec2::new(1000.0, 700.0);
const DOCK_ID: &str = "a_drop_lands_next_frame";

/// Records which tab bodies a frame painted, so the frame can be asked what it drew rather than
/// what the tree said once it was over.
#[derive(Default)]
struct Viewer {
    drawn: Vec<String>,
    /// Tabs the application wants gone, closed by the dock on the next pass that draws them.
    force_close: HashSet<String>,
}

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> Atoms<'static> {
        Atoms::new(tab.clone())
    }

    fn ui(&mut self, ui: &mut Ui, tab: &Self::Tab) {
        self.drawn.push(tab.clone());
        ui.label(tab.as_str());
    }

    fn force_close(&mut self, tab: &Self::Tab) -> bool {
        self.force_close.contains(tab)
    }
}

/// What the dock holds, in tree order: one entry per leaf, with its tabs in bar order.
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

    /// Runs one frame and answers with the tab bodies it painted, in draw order.
    fn run(&mut self, events: Vec<Event>) -> Vec<String> {
        self.viewer.drawn.clear();
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            // egui's drag detection and the overlay's preference lock are both time-based; a
            // clock that never ticks would make either mechanism unreal.
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
                    .show_inside(ui, viewer)
                    .apply(ui.ctx(), state);
            });
        });
        // Headless harness: no GPU backend to hand the delta to, and epaint panics on drop
        // otherwise.
        output.textures_delta.clear();
        self.viewer.drawn.clone()
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

    fn tab_rect(&self, leaf: NodePath, tab: usize) -> Rect {
        let tab = self
            .state
            .leaf(leaf)
            .unwrap()
            .tab_id_at(TabIndex(tab))
            .expect("the caller asked about a tab this leaf has");
        self.ctx
            .read_response(tab_widget_id(Id::new(DOCK_ID), leaf, tab))
            .expect("the tab was drawn last frame")
            .rect
    }

    fn is_dragging(&self, leaf: NodePath, tab: usize) -> bool {
        let tab_id = self
            .state
            .leaf(leaf)
            .unwrap()
            .tab_id_at(TabIndex(tab))
            .unwrap();
        self.ctx
            .is_being_dragged(tab_widget_id(Id::new(DOCK_ID), leaf, tab_id))
    }

    fn move_to(&mut self, pos: Pos2) {
        self.run(vec![Event::PointerMoved(pos)]);
    }

    fn button(&mut self, pos: Pos2, pressed: bool) -> Vec<String> {
        self.run(vec![Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }])
    }
}

/// Grabs `tab` of `leaf`, pulls it out of the tab row and back — left button still down.
///
/// Out and back rather than straight to the destination: the drag has to be recognised as one
/// (`DragInFlight::moved`), which is what pulling it clear of the bar does.
fn grab(sim: &mut Sim, leaf: NodePath, tab: usize) {
    let home = sim.tab_rect(leaf, tab).center();
    sim.move_to(home);
    sim.button(home, true);

    let out = home + Vec2::new(0.0, 200.0);
    for step in 1..=8u8 {
        sim.move_to(home + (out - home) * (f32::from(step) / 8.0));
    }
    assert!(
        sim.is_dragging(leaf, tab),
        "the scene has to be a live drag, or it is not the gesture under test"
    );
}

/// Moves the hand onto `leaf`'s body and gives the overlay two frames to settle its preference on
/// it — one for the leaf to publish that it is under the pointer, one for the dock to pick that up.
fn settle_onto(sim: &mut Sim, leaf: NodePath) -> Pos2 {
    let body = sim.layout().viewport(leaf).unwrap().center();
    sim.move_to(body);
    sim.move_to(body);
    body
}

/// `left` holds the tab that is dragged and one that stays behind; `right` is where it is dropped.
fn two_leaves() -> (DockState<String>, NodePath, NodePath) {
    let mut state = DockState::new(vec!["Tab 1".to_owned(), "Tab 2".to_owned()]);
    let root = state.main_surface().root().unwrap();
    let [left, right] = state
        .main_surface_mut()
        .split_right(root, 0.5, vec!["Target".to_owned()]);
    let path = |node| NodePath::new(SurfaceIndex::main(), node);
    (state, path(left), path(right))
}

#[test]
fn the_release_frame_still_paints_the_tab_where_it_was() {
    let (state, left, right) = two_leaves();
    let mut sim = Sim::new(state);

    grab(&mut sim, left, 0);
    let target_body = settle_onto(&mut sim, right);

    let during = sim.button(target_body, false);

    assert_eq!(
        during,
        vec!["Tab 1".to_owned(), "Target".to_owned()],
        "the frame the release is answered in paints both leaves as they were: the dragged tab is \
         still open in the leaf it came from, and the destination still shows its own tab"
    );
    assert_eq!(
        sim.contents(),
        vec![
            (left, vec!["Tab 2".to_owned()]),
            (right, vec!["Target".to_owned(), "Tab 1".to_owned()]),
        ],
        "the epilogue of that same frame applied the move"
    );
    assert_eq!(
        sim.run(vec![]),
        vec!["Tab 2".to_owned(), "Tab 1".to_owned()],
        "the next repaint paints the tab in the leaf it was dropped into"
    );
    assert_eq!(sim.state.validate(), Ok(()), "and the dock is well-formed");
}

#[test]
fn a_destination_closed_by_the_same_frame_is_not_dropped_onto() {
    let (state, left, right) = two_leaves();
    let mut sim = Sim::new(state);

    grab(&mut sim, left, 0);
    let target_body = settle_onto(&mut sim, right);

    // The application asks for the destination's only tab to go on the very frame the hand lets
    // go of the dragged one. Both land in the same request list, and the removal is applied first
    // — which is what leaves the drop pointing at a leaf that is no longer there.
    sim.viewer.force_close.insert("Target".to_owned());
    sim.button(target_body, false);
    sim.viewer.force_close.clear();
    sim.run(vec![]);

    assert_eq!(
        sim.contents(),
        vec![(left, vec!["Tab 1".to_owned(), "Tab 2".to_owned()])],
        "the destination was closed by its own frame, so the release had nowhere to land: the \
         dragged tab stays where it was, and neither leaf lost anything else"
    );
    assert_eq!(sim.state.validate(), Ok(()), "and the dock is well-formed");
    let _ = right;
}
