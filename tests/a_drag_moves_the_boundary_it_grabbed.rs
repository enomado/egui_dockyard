//! Dragging one boundary of a row must not move the others — which a binary tree cannot promise.
//!
//! # What this file is
//!
//! Stage 0 of [`PLAN_a_row_holds_many_panels`](../docs/PLAN_a_row_holds_many_panels.md): the
//! oracle written *before* the work, stating what the pair-shaped tree costs, and red on
//! arrival. It is the acceptance test of that plan — stage 7 is done when the `#[ignore]`s
//! below come off and it passes.
//!
//! # The property
//!
//! Three panels side by side have two boundaries. Grab one and pull: the other one is not part
//! of the gesture and has no business moving. On screen that is the whole of it.
//!
//! # Why a binary tree cannot keep it
//!
//! A row of three is not a row in this model — it is two nested pairs, and there are two ways
//! to write the same three columns:
//!
//! ```text
//!     H(a, H(b, c))                    H(H(a, b), c)
//!     ┌───┬───┬───┐                    ┌───┬───┬───┐
//!     │ a │ b │ c │                    │ a │ b │ c │
//!     └───┴───┴───┘                    └───┴───┴───┘
//!       ↑   └── inner                    inner ──┘ ↑
//!       outer                                  outer
//! ```
//!
//! A split's fraction is a share *of its own rectangle*. Dragging the **outer** boundary
//! changes the rectangle the inner split is a fraction of, so the inner boundary slides along
//! with it — by half the pull, at an inner ratio of 0.5. Dragging the **inner** one changes
//! nothing outside itself, so it is well behaved.
//!
//! So one of the two boundaries infects the other, and *which one* depends on how the tree
//! happens to spell the row. That is the same signature as the strip bug fixed on 30.08 (the
//! two spellings disagreed about which panel loses its strip), and the reason both spellings
//! are tested here: a rule checked against one of them looks finished.
//!
//! # What is not ignored
//!
//! `the_drag_moves_the_boundary_it_grabbed` runs today and must stay green. Without it every
//! assertion here would also pass if the drag simply never reached the dock — wrong
//! coordinates, too few frames, a press that never became a drag — and the file would be
//! pinning nothing at all. It is the positive control, and it is what makes the red red.

use egui::{
    Atoms, CentralPanel, Context, Event, Id, PointerButton, Pos2, RawInput, Rect, Ui, Vec2,
};
use egui_dockyard::{
    DockArea, DockLayout, DockState, Node, NodeId, NodePath, Split, Style, SurfaceIndex, TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_drag_moves_the_boundary_it_grabbed";

/// Half a device pixel at the default scale: boundaries are snapped to whole pixels, so an
/// exact comparison would be reporting the snapping rather than the property.
const TOLERANCE: f32 = 0.5;

/// How far the drag pulls. Far enough that a boundary that *did* move could not be mistaken for
/// snapping noise, and well inside the band a separator drag is clamped to on a 400 px column.
const PULL: f32 = 150.0;

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

fn tab(name: &str) -> String {
    name.to_owned()
}

fn path(node: NodeId) -> NodePath {
    NodePath::new(SurfaceIndex::main(), node)
}

/// How the same three columns are spelled in the tree.
///
/// Both draw one row of three equal panels; they differ only in which pair is nested inside
/// which, and that is exactly what must not be visible to the hand.
#[derive(Clone, Copy, Debug)]
enum Spelling {
    /// `H(a, H(b, c))` — the second pair is the nested one.
    RightLeaning,
    /// `H(H(a, b), c)` — the first pair is.
    LeftLeaning,
}

/// One dock, one context, kept across frames — the geometry map and the drag in flight both
/// live in that context's memory, so a gesture cannot be simulated with a fresh one per frame.
struct Sim {
    ctx: Context,
    state: DockState<String>,
    /// The three leaves in screen order.
    panels: [NodeId; 3],
}

impl Sim {
    /// Three equal columns, spelled either way round.
    ///
    /// The fractions are chosen so both spellings put the boundaries in the *same* places —
    /// thirds — because a difference in where the lines start would be a difference the test
    /// could accidentally be measuring instead of the property.
    fn row_of_three(spelling: Spelling) -> Self {
        let mut state = DockState::new(vec![tab("a")]);
        let a = state.main_surface().root().unwrap();

        let panels = match spelling {
            Spelling::RightLeaning => {
                // a | b, at a third; then b | c inside the right two thirds.
                let [_, b] = state.split(path(a), Split::Right, 1.0 / 3.0, Node::leaf(tab("b")));
                let [_, c] = state.split(path(b), Split::Right, 0.5, Node::leaf(tab("c")));
                [a, b, c]
            }
            Spelling::LeftLeaning => {
                // ab | c, at two thirds; then a | b inside the left two thirds.
                let [_, c] = state.split(path(a), Split::Right, 2.0 / 3.0, Node::leaf(tab("c")));
                let [_, b] = state.split(path(a), Split::Right, 0.5, Node::leaf(tab("b")));
                [a, b, c]
            }
        };

        let mut sim = Sim {
            ctx: Context::default(),
            state,
            panels,
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
                    .show_inside(ui, &mut Viewer)
                    .apply(ui.ctx(), state, &mut Viewer);
            });
        });
        output.textures_delta.clear();
    }

    /// Press at `from`, drag right by `by`, release — as a real hand does it, over several
    /// frames, because a separator drag reads `drag_delta()` per frame and commits on release.
    fn drag(&mut self, from: Pos2, by: f32) {
        let by = Vec2::new(by, 0.0);
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

    fn rect_of(&self, node: NodeId) -> Rect {
        DockLayout::load(&self.ctx, Id::new(DOCK_ID))
            .rect(path(node))
            .expect("the node was laid out")
    }

    /// Where boundary `gap` sits: the seam between panels `gap` and `gap + 1`.
    ///
    /// Read off the panels rather than computed from the fractions, so the test aims at the
    /// line the code actually drew — and so that it is spelled the same way for both shapes,
    /// which store their ratios on different nodes.
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

/// The positive control, and the only test here that runs today: the gesture works at all.
///
/// Both spellings, both boundaries — four drags, each of which must move the line it grabbed.
/// If this ever goes red, everything below is vacuous and says nothing about rows.
#[test]
fn the_drag_moves_the_boundary_it_grabbed() {
    for spelling in [Spelling::RightLeaning, Spelling::LeftLeaning] {
        for gap in 0..2 {
            let mut sim = Sim::row_of_three(spelling);
            let before = sim.boundary(gap);
            let grab = sim.grab_point(gap);

            sim.drag(grab, PULL);

            let moved = sim.boundary(gap) - before;
            assert!(
                moved > PULL * 0.5,
                "{spelling:?}, boundary {gap}: a {PULL} px pull moved it by {moved} px — \
                 the gesture did not reach the divider, so nothing else in this file means \
                 anything"
            );
        }
    }
}

/// The property, stated for the boundary that is nested *inside* the one being dragged.
///
/// Red today, in one spelling out of two: dragging the outer boundary rescales the rectangle
/// the inner split takes its fraction of, so the inner line slides by half the pull. Which of
/// the two boundaries is the outer one depends on the spelling, which is the whole complaint.
#[test]
fn dragging_one_boundary_leaves_the_other_where_it_was() {
    let mut failures = Vec::new();

    for spelling in [Spelling::RightLeaning, Spelling::LeftLeaning] {
        for gap in 0..2 {
            let other = 1 - gap;
            let mut sim = Sim::row_of_three(spelling);
            let other_before = sim.boundary(other);
            let grab = sim.grab_point(gap);

            sim.drag(grab, PULL);

            let drift = sim.boundary(other) - other_before;
            if drift.abs() > TOLERANCE {
                failures.push(format!(
                    "{spelling:?}: dragging boundary {gap} by {PULL} px moved boundary {other} \
                     by {drift} px (from {other_before} to {})",
                    sim.boundary(other)
                ));
            }
        }
    }

    // Collected rather than asserted one at a time: the interesting fact is *which* of the four
    // cases drift, and stopping at the first would hide that it is one per spelling.
    assert!(
        failures.is_empty(),
        "a boundary moved that the hand never grabbed:\n  {}",
        failures.join("\n  ")
    );
}

/// The same complaint from the other end, and the one a user words as "the panels jump".
///
/// Dragging a boundary changes the width of the two panels it lies between. Every *other*
/// panel of the row is not part of the gesture and must keep its width.
///
/// Separate from the boundary test because it can fail on its own: a model that moved both
/// boundaries by the same amount would keep every width and still be wrong, and one that kept
/// the far boundary while resizing the far panel is not a shape this tree can produce today but
/// is exactly what a careless n-ary implementation would produce tomorrow.
#[test]
fn dragging_a_boundary_resizes_only_the_two_panels_it_lies_between() {
    let mut failures = Vec::new();

    for spelling in [Spelling::RightLeaning, Spelling::LeftLeaning] {
        for gap in 0..2 {
            // The panel that is not adjacent to this boundary: for gap 0 that is panel 2, for
            // gap 1 it is panel 0.
            let bystander = if gap == 0 { 2 } else { 0 };
            let mut sim = Sim::row_of_three(spelling);
            let width_before = sim.rect_of(sim.panels[bystander]).width();
            let grab = sim.grab_point(gap);

            sim.drag(grab, PULL);

            let width_after = sim.rect_of(sim.panels[bystander]).width();
            if (width_after - width_before).abs() > TOLERANCE {
                failures.push(format!(
                    "{spelling:?}: dragging boundary {gap} resized panel {bystander} from \
                     {width_before} to {width_after}"
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "a panel was resized that the hand never touched:\n  {}",
        failures.join("\n  ")
    );
}
