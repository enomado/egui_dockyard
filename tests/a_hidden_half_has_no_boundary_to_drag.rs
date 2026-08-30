//! A split whose half hid sideways has no divider to grab, and its ratio is left alone.
//!
//! # Why this is its own file
//!
//! `a_collapsed_leaf_can_hide_sideways.rs` states where the two children *land*. It cannot state
//! this: a divider that lies across the sibling changes nobody's rectangle, so every assertion
//! there passes while a grabbable line hangs over the layout. The bug this file pins was found
//! by eye, in the application, with that whole file green — "a stick you can drag, painted over
//! the panels, attached to nothing".
//!
//! The property is therefore about a *gesture*, not a rectangle: pressing where the divider
//! would have been and dragging must not move anything. Two things ride on it. The visible one
//! is the line. The one that outlives the frame is the ratio: a divider drag *writes*
//! `SplitNode::fraction`, and that fraction is precisely what the hidden half is keeping for
//! when it comes back — edit it while the half is off screen and expanding returns the leaf to
//! a width the user never chose.
//!
//! Where the divider "would have been" is not a guess: it is read from a run with both halves
//! open, which is the same ratio the buggy code drew at.

use egui::{
    CentralPanel, Context, Event, Id, PointerButton, Pos2, RawInput, Rect, Ui, Vec2, WidgetText,
};
use egui_dockyard::{
    DockArea, DockLayout, DockState, Node, NodeId, NodePath, SideStrip, Split, Style, SurfaceIndex,
    TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_hidden_half_has_no_boundary_to_drag";

/// Half a device pixel at the default scale: boundaries are snapped to whole pixels, so an exact
/// comparison would be reporting the snapping rather than the property.
const TOLERANCE: f32 = 0.5;

/// How far the drag pulls. Far enough that a boundary that *did* move could not be mistaken for
/// snapping noise, and well inside the band a separator drag is clamped to.
const PULL: f32 = 150.0;

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

/// One dock, one context, kept across frames — the geometry map and the drag in flight both live
/// in that context's memory, so a gesture cannot be simulated with a fresh one per frame.
struct Sim {
    ctx: Context,
    state: DockState<String>,
    parent: NodeId,
    left: NodeId,
    right: NodeId,
}

impl Sim {
    /// Two leaves side by side, split down the middle.
    fn two_columns() -> Self {
        let mut state = DockState::new(vec![tab("left")]);
        let left = state.main_surface().root().unwrap();
        let [_, right] = state.split(
            NodePath::new(SurfaceIndex::main(), left),
            Split::Right,
            0.5,
            Node::leaf(tab("right")),
        );
        let parent = state
            .main_surface()
            .parent(left)
            .expect("the two leaves have a parent");

        let mut sim = Sim {
            ctx: Context::default(),
            state,
            parent,
            left,
            right,
        };
        sim.settle();
        sim
    }

    /// A column beside two stacked panels: `H(V(top, bottom), right)`.
    ///
    /// The smallest scene with a **junction** in it — the root's line runs down the middle, and
    /// the boundary between the two stacked panels ends on it. Answers with the split over the
    /// stack and its two leaves; `parent` is the root, and `left` is the stack itself, so
    /// [`Sim::seam`] still names the line down the middle.
    fn a_column_beside_two_rows() -> (Self, NodeId, NodeId, NodeId) {
        let mut state = DockState::new(vec![tab("top")]);
        let top = state.main_surface().root().unwrap();
        let [_, right] = state.split(
            NodePath::new(SurfaceIndex::main(), top),
            Split::Right,
            0.5,
            Node::leaf(tab("right")),
        );
        let [_, bottom] = state.split(
            NodePath::new(SurfaceIndex::main(), top),
            Split::Below,
            0.5,
            Node::leaf(tab("bottom")),
        );
        let rows = state
            .main_surface()
            .parent(top)
            .expect("the two stacked leaves have a parent");
        let parent = state
            .main_surface()
            .parent(rows)
            .expect("the stack sits beside a column");

        let mut sim = Sim {
            ctx: Context::default(),
            state,
            parent,
            left: rows,
            right,
        };
        sim.settle();
        (sim, rows, top, bottom)
    }

    /// Enough frames for the layout pass to settle and publish every rectangle.
    fn settle(&mut self) {
        for _ in 0..4 {
            self.frame(Vec::new());
        }
    }

    fn frame(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            events,
            ..Default::default()
        };
        let state = &mut self.state;
        let mut output = self.ctx.run_ui(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                DockArea::new(state)
                    .id(Id::new(DOCK_ID))
                    .style(Style::from_egui(ui.style().as_ref()))
                    .show_leaf_collapse_buttons(true)
                    .collapse_sideways(true)
                    .show_inside(ui, &mut Viewer);
            });
        });
        output.textures_delta.clear();
    }

    /// Press at `from`, drag to `from + by`, release — as a real hand does it, over several
    /// frames, because a separator drag reads `drag_delta()` per frame and commits on release.
    fn drag(&mut self, from: Pos2, by: Vec2) {
        self.frame(vec![Event::PointerMoved(from)]);
        self.frame(vec![Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        }]);
        // Two moves rather than one: the first frame of a drag is where egui decides a press
        // became a drag at all, and a boundary that follows the cursor would move on the second.
        self.frame(vec![Event::PointerMoved(from + by * 0.5)]);
        self.frame(vec![Event::PointerMoved(from + by)]);
        self.frame(vec![Event::PointerButton {
            pos: from + by,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]);
        self.settle();
    }

    fn collapse(&mut self, node: NodeId, collapsed: bool) {
        self.state
            .main_surface_mut()
            .set_leaf_collapsed(node, collapsed);
        self.settle();
    }

    fn layout(&self) -> DockLayout {
        DockLayout::load(&self.ctx, Id::new(DOCK_ID))
    }

    fn rect_of(&self, node: NodeId) -> Rect {
        self.layout()
            .rect(NodePath::new(SurfaceIndex::main(), node))
            .expect("the node was laid out")
    }

    fn side_strip_of(&self, node: NodeId) -> Option<SideStrip> {
        self.layout()
            .side_strip(NodePath::new(SurfaceIndex::main(), node))
    }

    /// The ratio the split is cut at — the thing a divider drag writes, and the thing a hidden
    /// half is keeping for when it expands.
    fn fraction(&self) -> f32 {
        self.fraction_of(self.parent)
    }

    fn fraction_of(&self, node: NodeId) -> f32 {
        self.state.main_surface()[node]
            .get_row()
            .expect("the node is a split")
            .fraction()
    }

    /// Where the divider sits while both halves are open: the seam between the two rectangles.
    /// Read rather than computed, so the test aims at the line the code actually drew.
    fn seam(&self) -> Pos2 {
        self.seam_between(self.left, self.right)
    }

    /// The seam between two nodes laid out side by side or one above the other: the midpoint of
    /// the gap along whichever axis they are separated on, and the middle of the overlap on the
    /// other. Read rather than computed from a ratio, for the reason [`Sim::seam`] is.
    fn seam_between(&self, first: NodeId, second: NodeId) -> Pos2 {
        let (first, second) = (self.rect_of(first), self.rect_of(second));
        if (first.center().x - second.center().x).abs()
            > (first.center().y - second.center().y).abs()
        {
            Pos2::new(
                (first.max.x + second.min.x) * 0.5,
                (first.min.y + first.max.y) * 0.5,
            )
        } else {
            Pos2::new(
                (first.min.x + first.max.x) * 0.5,
                (first.max.y + second.min.y) * 0.5,
            )
        }
    }
}

/// The bug, stated: with the left half hidden sideways, the gesture that used to move the
/// boundary finds nothing to move.
///
/// Everything is asserted, not just the ratio, because the three could come apart: a divider
/// could be gone from the screen and still hit-testable, or writable without repainting.
#[test]
fn dragging_where_the_divider_was_does_nothing() {
    let mut sim = Sim::two_columns();
    let seam = sim.seam();

    sim.collapse(sim.left, true);
    assert_eq!(
        sim.side_strip_of(sim.left),
        Some(SideStrip::Left),
        "precondition: the collapsed leaf hid sideways, so there is a strip to speak of"
    );

    let fraction_before = sim.fraction();
    let (strip_before, sibling_before) = (sim.rect_of(sim.left), sim.rect_of(sim.right));

    sim.drag(seam, Vec2::new(PULL, 0.0));

    assert!(
        (sim.fraction() - fraction_before).abs() < f32::EPSILON,
        "the hidden half keeps the ratio it had: {} became {}",
        fraction_before,
        sim.fraction()
    );
    assert!(
        (sim.rect_of(sim.left).width() - strip_before.width()).abs() < TOLERANCE
            && (sim.rect_of(sim.right).min.x - sibling_before.min.x).abs() < TOLERANCE,
        "neither the strip nor the sibling moved: {:?} / {:?} became {:?} / {:?}",
        strip_before,
        sibling_before,
        sim.rect_of(sim.left),
        sim.rect_of(sim.right)
    );
    assert_eq!(
        sim.side_strip_of(sim.left),
        Some(SideStrip::Left),
        "and it is still a strip afterwards"
    );
}

/// The junction handle is the *other* way to drag that boundary, and a hidden half has none of it
/// either.
///
/// Two separators meet where the line down the middle carries the boundary between the two stacked
/// panels, and the dock puts a handle there that drags both at once. Fold the top panel away and
/// the second of those two boundaries is gone — no line to paint, none to hit-test — so there is
/// nothing left for a handle to be made of.
///
/// The detector used to disagree with itself about that, and the way it failed is worth keeping:
/// it read the boundaries off the *rectangles*, which are still where they were, and offered a
/// handle; the press was answered and the drag began; and then the frame that follows a live
/// junction drag asks the layout for the same rectangle, finds none, draws nothing, and the dock
/// drops the gesture with the button still down. On screen the corner answers the hand and then
/// goes dead until it is released — and what it would have been dragging is the very ratio the
/// folded panel is keeping for its return, which is what this whole file is about. Found by the
/// DST sweep at seed 5, the pass it learned to fold.
#[test]
fn a_junction_on_a_hidden_boundary_is_not_offered() {
    let (mut sim, rows, top, bottom) = Sim::a_column_beside_two_rows();
    sim.collapse(top, true);
    // Read **after** the fold, and that is the point of it: folding moves the stack's boundary up
    // against the bar, so the crossing "where it used to be" is nowhere near where a handle would
    // wrongly be offered now. A test aimed at the old place presses plain separator either way and
    // passes whatever the detector does — measured, on the very mutant this test exists to kill.
    let junction = Pos2::new(sim.seam().x, sim.seam_between(top, bottom).y);
    assert!(
        sim.layout()
            .divider(NodePath::new(SurfaceIndex::main(), rows))
            .is_none(),
        "precondition: the stack's own boundary is gone, because one of its panels folded away"
    );

    let (rows_before, root_before) = (sim.fraction_of(rows), sim.fraction());
    let bottom_before = sim.rect_of(bottom);

    // Diagonally, which is what a junction drag is for: one leg along each of the two lines it is
    // supposed to be made of.
    sim.drag(junction, Vec2::new(PULL, PULL));

    assert!(
        (sim.fraction_of(rows) - rows_before).abs() < f32::EPSILON,
        "the folded panel keeps the ratio it had: {} became {}",
        rows_before,
        sim.fraction_of(rows)
    );
    assert!(
        (sim.rect_of(bottom).min.y - bottom_before.min.y).abs() < TOLERANCE
            && (sim.rect_of(bottom).max.y - bottom_before.max.y).abs() < TOLERANCE,
        "and nothing in the stack moved vertically: {:?} became {:?}",
        bottom_before,
        sim.rect_of(bottom)
    );
    // The line down the middle *is* still there, so the press landed on it and it must have
    // followed the hand **all the way**. This is the assertion that tells the fix from the bug,
    // and the weaker "it moved at all" does not: a handle offered here answers the press itself,
    // and its first frame's travel is applied through the same path a live drag takes before the
    // gesture is dropped. So the bug does not leave the line where it was — it leaves it short,
    // stranded wherever the pointer had reached when the dock let go.
    let landed = sim.seam_between(sim.left, sim.right).x;
    assert!(
        (landed - (junction.x + PULL)).abs() <= TOLERANCE,
        "the line the press landed on has to end up under the pointer, at {}, and it stopped at \
         {landed} (the ratio went {root_before} -> {}). A line that stops short is the corner \
         answering the press and then going dead",
        junction.x + PULL,
        sim.fraction()
    );
}

/// The positive control for the test above: with both panels open, that very point is a junction,
/// and one drag moves **both** boundaries.
///
/// Without it "nothing moved vertically" is satisfied by a point that was never a handle in the
/// first place — a couple of pixels off, a scene with no crossing in it — and the test would be
/// pinning its own arithmetic rather than the dock.
#[test]
fn the_same_press_moves_both_boundaries_while_both_panels_are_open() {
    let (mut sim, rows, top, bottom) = Sim::a_column_beside_two_rows();
    let junction = Pos2::new(sim.seam().x, sim.seam_between(top, bottom).y);
    let (rows_before, root_before) = (sim.fraction_of(rows), sim.fraction());

    sim.drag(junction, Vec2::new(PULL, PULL));

    assert!(
        sim.fraction() > root_before,
        "the line down the middle followed the hand: {} became {}",
        root_before,
        sim.fraction()
    );
    assert!(
        sim.fraction_of(rows) > rows_before,
        "and so did the boundary that ends on it — that is what makes this point a junction and \
         not just a separator: {} became {}",
        rows_before,
        sim.fraction_of(rows)
    );
}

/// A split that is drawn has a divider recorded for it, and a leaf never does.
///
/// Cheap, and it is what makes the `None` in the test above mean something: without it, a
/// `divider` field that was simply never filled would satisfy every "there is no line here"
/// assertion in this file.
#[test]
fn only_a_split_has_a_divider() {
    let sim = Sim::two_columns();
    let path = |node| NodePath::new(SurfaceIndex::main(), node);

    assert!(
        sim.layout().divider(path(sim.parent)).is_some(),
        "the split was cut at its ratio, so there is a line between its children"
    );
    assert_eq!(
        sim.layout().divider(path(sim.left)),
        None,
        "a leaf divides nothing"
    );
    assert_eq!(sim.layout().divider(path(sim.right)), None);
}

/// The positive control. Without it every assertion above would also pass if the drag simply
/// never reached the dock — wrong coordinates, too few frames, a press that never became a
/// drag — and the file would be pinning nothing at all.
#[test]
fn the_same_drag_moves_the_boundary_when_both_halves_are_open() {
    let mut sim = Sim::two_columns();
    let seam = sim.seam();
    let fraction_before = sim.fraction();

    sim.drag(seam, Vec2::new(PULL, 0.0));

    assert!(
        sim.fraction() > fraction_before + 0.05,
        "an open split follows the same gesture: {} should have grown well past it, got {}",
        fraction_before,
        sim.fraction()
    );
}

/// What the ratio is *for*: the half comes back the width it left at, even after the gesture
/// that used to edit it while it was away.
#[test]
fn expanding_returns_the_width_the_half_left_at() {
    let mut sim = Sim::two_columns();
    let seam = sim.seam();

    // Not the default 0.5, so a bug that resets the ratio to the middle is a failure here
    // rather than a coincidence.
    sim.drag(seam, Vec2::new(-200.0, 0.0));
    let fraction_chosen = sim.fraction();
    let width_chosen = sim.rect_of(sim.left).width();
    assert!(
        fraction_chosen < 0.45,
        "precondition: the user narrowed the left half to {fraction_chosen}"
    );

    // Where the divider is *now*, read before the half hides — after that the seam is the edge
    // of the strip, which is somewhere else entirely, and a drag aimed there would sail past
    // the line under test and pass against the bug.
    let divider = sim.seam();
    sim.collapse(sim.left, true);
    sim.drag(divider, Vec2::new(PULL, 0.0));
    sim.collapse(sim.left, false);

    assert!(
        (sim.fraction() - fraction_chosen).abs() < f32::EPSILON,
        "the ratio survived hiding: {fraction_chosen} became {}",
        sim.fraction()
    );
    assert!(
        (sim.rect_of(sim.left).width() - width_chosen).abs() < TOLERANCE,
        "and so did the width it buys: {width_chosen} became {}",
        sim.rect_of(sim.left).width()
    );
}
