//! A boundary drag can take room from more than its two neighbours — when the hand says so.
//!
//! # What this file is
//!
//! Stage 0 of [`PLAN_a_drag_chooses_who_pays_for_it`](../docs/PLAN_a_drag_chooses_who_pays_for_it.md):
//! the oracle written *before* the work and red on arrival. The plan is done when the
//! `#[ignore]`s below come off and it passes.
//!
//! # The property
//!
//! Three panels side by side, and a drag on the first boundary. What that drag is allowed to do
//! depends on what is held:
//!
//! ```text
//!     ┌─────┬─────┬─────┐   nothing held → Chain: the near neighbour pays until it hits its
//!     │  a  │  b  │  c  │   minimum, then the one behind it does — the line keeps following
//!     └─────┴──▲──┴─────┘   the cursor instead of stopping at b.
//!            grab
//!                           Shift → Pair: b pays, c never moves. What the crate does today.
//!                           Ctrl  → Proportional: b and c both pay, in proportion, from the
//!                                   first pixel — no minimum has to be reached first.
//! ```
//!
//! # Why the crate cannot keep it today
//!
//! `Pair` is not the default here so much as the only thing expressible. `RowNode::set_boundary`
//! rewrites exactly the two weights beside the gap; `DockArea::nudge_boundary` clamps the write
//! into `SeparatorBand::between(lo, hi, …)`, where `lo` and `hi` are *the neighbouring
//! boundaries*; and `DockMutation::SetBoundary` carries one boundary, which a chain drag does not
//! have. All three have to grow before either ignored test below can pass.
//!
//! # The scene, and why these numbers
//!
//! A 1200 px row of three equal panels, 400 px each, and `SeparatorStyle::extra` is 175 px — the
//! margin every child keeps. So panel `b` has 225 px to give before it is at its minimum, and a
//! 400 px pull asks for 175 px more than it has. Under `Chain` that 175 px comes from `c`; under
//! `Pair` the line simply stops. The two answers differ by more than a third of a panel, which is
//! why this is a scene and not a tolerance argument.
//!
//! # What is not ignored
//!
//! Two positive controls run today and must stay green: the gesture reaches the divider at all,
//! and a `Pair` drag leaves the far panel alone. Without the first, every assertion here would
//! also pass if the drag never arrived; without the second, "the far panel moved" could be
//! reported by a crate that had simply started moving everything, which is a different bug
//! wearing this feature's clothes.

use egui::{
    CentralPanel, Context, Event, Id, Modifiers, PointerButton, Pos2, RawInput, Rect, Ui, Vec2,
    WidgetText,
};
use egui_dockyard::{
    DockArea, DockLayout, DockState, Node, NodeId, NodePath, Split, Style, SurfaceIndex, TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_drag_chooses_who_pays_for_it";

/// Half a device pixel at the default scale: boundaries are snapped to whole pixels, so an exact
/// comparison would be reporting the snapping rather than the property.
const TOLERANCE: f32 = 0.5;

/// A pull that asks for more than the near neighbour has. Panel `b` starts at 400 px and keeps
/// 175, so 225 px is all it can give; the remaining 175 is the part only a chain can deliver.
const HARD_PULL: f32 = 400.0;

/// A pull well inside the near neighbour's own room. Under `Chain` nothing behind `b` is touched
/// — which is what makes `Proportional` distinguishable here rather than a second name for the
/// same picture.
const SOFT_PULL: f32 = 150.0;

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

fn path(node: NodeId) -> NodePath {
    NodePath::new(SurfaceIndex::main(), node)
}

/// One dock, one context, kept across frames — the geometry map and the drag in flight both live
/// in that context's memory, so a gesture cannot be simulated with a fresh context per frame.
struct Sim {
    ctx: Context,
    state: DockState<String>,
    /// The three leaves in screen order.
    panels: [NodeId; 3],
}

impl Sim {
    /// Three equal columns: one row of three, which is what a same-axis split builds since
    /// stage 7 of the n-ary plan.
    fn row_of_three() -> Self {
        let mut state = DockState::new(vec![tab("a")]);
        let a = state.main_surface().root().unwrap();
        let [_, b] = state.split(path(a), Split::Right, 1.0 / 3.0, Node::leaf(tab("b")));
        let [_, c] = state.split(path(b), Split::Right, 0.5, Node::leaf(tab("c")));

        let mut sim = Sim {
            ctx: Context::default(),
            state,
            panels: [a, b, c],
        };
        sim.settle();
        sim
    }

    /// Enough frames for the layout pass to settle and publish every rectangle.
    fn settle(&mut self) {
        for _ in 0..4 {
            self.frame(Vec::new(), Modifiers::default());
        }
    }

    /// One frame with `modifiers` held for the whole of it.
    ///
    /// Declared every frame with `ModifiersChanged` rather than only on the press event: a drag
    /// is read over several frames, and what a hand holds it holds throughout. (egui keeps the
    /// last declared set between frames, so re-declaring is redundant but honest — a frame that
    /// says what is held cannot be read as one that inherited it by accident.)
    fn frame(&mut self, events: Vec<Event>, modifiers: Modifiers) {
        let mut all = vec![Event::ModifiersChanged(modifiers)];
        all.extend(events);
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            events: all,
            ..Default::default()
        };
        let state = &mut self.state;
        let mut output = self.ctx.run_ui(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                DockArea::new(state)
                    .id(Id::new(DOCK_ID))
                    .style(Style::from_egui(ui.style().as_ref()))
                    .show_inside(ui, &mut Viewer);
            });
        });
        output.textures_delta.clear();
    }

    /// Press at `from`, drag right by `by` with `modifiers` held, release — as a real hand does
    /// it, over several frames, because a separator drag reads `drag_delta()` per frame and
    /// commits on release.
    fn drag(&mut self, from: Pos2, by: f32, modifiers: Modifiers) {
        let by = Vec2::new(by, 0.0);
        self.frame(vec![Event::PointerMoved(from)], modifiers);
        self.frame(
            vec![Event::PointerButton {
                pos: from,
                button: PointerButton::Primary,
                pressed: true,
                modifiers,
            }],
            modifiers,
        );
        // Two moves rather than one: the first frame of a drag is where egui decides a press
        // became a drag at all, and a boundary that follows the cursor would move on the second.
        self.frame(vec![Event::PointerMoved(from + by * 0.5)], modifiers);
        self.frame(vec![Event::PointerMoved(from + by)], modifiers);
        self.frame(
            vec![Event::PointerButton {
                pos: from + by,
                button: PointerButton::Primary,
                pressed: false,
                modifiers,
            }],
            modifiers,
        );
        self.settle();
    }

    fn rect_of(&self, node: NodeId) -> Rect {
        DockLayout::load(&self.ctx, Id::new(DOCK_ID))
            .rect(path(node))
            .expect("the node was laid out")
    }

    fn width_of(&self, panel: usize) -> f32 {
        self.rect_of(self.panels[panel]).width()
    }

    /// Where boundary `gap` sits: the seam between panels `gap` and `gap + 1`.
    ///
    /// Read off the panels rather than computed from the weights, so the test aims at the line
    /// the code actually drew.
    fn boundary(&self, gap: usize) -> f32 {
        let before = self.rect_of(self.panels[gap]);
        let after = self.rect_of(self.panels[gap + 1]);
        0.5 * (before.max.x + after.min.x)
    }

    /// A point on boundary `gap`, half way down the row — where a hand would grab it.
    fn grab_point(&self, gap: usize) -> Pos2 {
        let rect = self.rect_of(self.panels[gap]);
        Pos2::new(self.boundary(gap), 0.5 * (rect.min.y + rect.max.y))
    }
}

/// Positive control: the gesture reaches the divider at all.
///
/// If this goes red, every other test in the file is vacuous — a drag that never arrived moves
/// no panel, which reads exactly like "the far panel was left alone".
#[test]
fn the_drag_moves_the_boundary_it_grabbed() {
    let mut sim = Sim::row_of_three();
    let before = sim.boundary(0);
    let grab = sim.grab_point(0);

    sim.drag(grab, SOFT_PULL, Modifiers::default());

    let moved = sim.boundary(0) - before;
    assert!(
        moved > SOFT_PULL * 0.5,
        "a {SOFT_PULL} px pull moved the boundary by {moved} px — the gesture did not reach the \
         divider, so nothing else in this file means anything"
    );
}

/// Positive control, and the behaviour this plan must not lose: with Shift held, the far panel
/// is not part of the gesture.
///
/// Green today for the uninteresting reason that Shift is not read yet and `Pair` is all the
/// crate can do. It must still be green when Shift *is* read, which is the point of writing it
/// now: it pins the mode rather than the accident.
#[test]
fn a_pair_drag_leaves_the_far_panel_alone() {
    let mut sim = Sim::row_of_three();
    let far_before = sim.width_of(2);
    let grab = sim.grab_point(0);

    sim.drag(grab, HARD_PULL, Modifiers::SHIFT);

    let far_after = sim.width_of(2);
    assert!(
        (far_after - far_before).abs() <= TOLERANCE,
        "Shift means the pair: panel c went from {far_before} to {far_after} px"
    );
}

/// With nothing held, a pull larger than the near neighbour's room keeps going: `b` sits at its
/// minimum and `c` pays the rest.
///
/// Red today — the clamp stops the line at `b`'s margin and `c` never hears about it.
#[test]
#[ignore = "stage 0 of PLAN_a_drag_chooses_who_pays_for_it: red until the plan lands"]
fn a_plain_drag_pushes_past_the_neighbour_that_ran_out() {
    let mut sim = Sim::row_of_three();
    let near_before = sim.width_of(1);
    let far_before = sim.width_of(2);
    let grab = sim.grab_point(0);

    sim.drag(grab, HARD_PULL, Modifiers::default());

    let near_after = sim.width_of(1);
    let far_after = sim.width_of(2);
    // `b` had 225 px to give and was asked for 400; the 175 px it could not give is `c`'s to
    // pay. Asserted as "well over a hundred" rather than exactly 175 so the test survives a
    // pixel of snapping, while still being far outside anything `Pair` could produce.
    assert!(
        far_before - far_after > 100.0,
        "a chain drag stopped at the neighbour: b {near_before} → {near_after}, \
         c {far_before} → {far_after} px"
    );
}

/// With Ctrl held, both panels ahead of the boundary pay from the first pixel, in proportion to
/// what they have — no minimum has to be reached first.
///
/// Red today, and red for a second reason on top of the clamp: the mutation carries one boundary,
/// and this gesture moves two.
#[test]
#[ignore = "stage 0 of PLAN_a_drag_chooses_who_pays_for_it: red until the plan lands"]
fn a_proportional_drag_moves_every_boundary_of_the_row() {
    let mut sim = Sim::row_of_three();
    let second_before = sim.boundary(1);
    let grab = sim.grab_point(0);

    sim.drag(grab, SOFT_PULL, Modifiers::COMMAND);

    let second_after = sim.boundary(1);
    // Equal panels, so a 150 px pull is 75 px from each of the two ahead of it, which moves the
    // second boundary by that 75. A chain drag on this same soft pull moves it by nothing at
    // all — that is what separates the two modes here.
    let moved = second_after - second_before;
    assert!(
        moved > 25.0,
        "Ctrl means proportional: the second boundary moved by {moved} px \
         ({second_before} → {second_after})"
    );
}
