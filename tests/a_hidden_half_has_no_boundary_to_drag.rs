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
        self.state.main_surface()[self.parent]
            .get_split()
            .expect("the parent is a split")
            .fraction
    }

    /// Where the divider sits while both halves are open: the seam between the two rectangles.
    /// Read rather than computed, so the test aims at the line the code actually drew.
    fn seam(&self) -> Pos2 {
        let left = self.rect_of(self.left);
        let right = self.rect_of(self.right);
        Pos2::new(
            (left.max.x + right.min.x) * 0.5,
            (left.min.y + left.max.y) * 0.5,
        )
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
