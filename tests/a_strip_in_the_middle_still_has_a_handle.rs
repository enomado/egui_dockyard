//! A panel collapsed sideways in the *middle* of a row does not take the row's resizing with it.
//!
//! # The property
//!
//! ```text
//!     ┌─────────┬─┬─────────┐   The middle leaf is collapsed: it is one strip wide and keeps
//!     │    a    │▮│    c    │   that width whatever anyone drags. `a` and `c` divide what it
//!     └─────────┴─┴─────────┘   leaves — and that division is a boundary, so it has a handle.
//!                ▲ ▲
//!                grab either: both are the same line, and the strip slides along with it.
//! ```
//!
//! # Where it comes from
//!
//! Стас, clicking through the dock with a collapsed *Hydrodynamics* panel between two others:
//!
//! > есть панель свёрнутая посередине. это хорошо, но так получилось что у неё нет ручки чтобы
//! > ресайзить сплиты
//!
//! And there was not. A strip is cut at its own edges rather than at a ratio, so the rule "a
//! divider lies between two open neighbours" answered *no* to both of the strip's gaps at once —
//! and the two open columns either side of it were left with no line between them anywhere. At
//! the row's **end** that answer is right and stays: one open child has nobody to trade with.
//!
//! # What is a control here
//!
//! `the_two_open_columns_can_be_resized` is the property; everything else exists so that it
//! cannot pass for the wrong reason. Without `a_strip_at_the_edge_grows_no_handle` the fix could
//! be "every gap beside a strip gets a line"; without `the_strip_keeps_its_own_width` it could be
//! "the strip is just an ordinary child again"; and without `the_strip_keeps_the_width_it_is_
//! holding` a drag could be quietly spending the width the hidden panel gets back when it opens
//! — which is the defect this feature was fixed for once already, from the other side.

use egui::{
    Atoms, CentralPanel, Context, Event, Id, Modifiers, PointerButton, Pos2, RawInput, Rect, Ui,
    Vec2,
};
use egui_dockyard::{DockArea, DockLayout, DockState, Fold, GapIndex, GapPath, Node, NodeId, NodePath, Split, Style, SurfaceIndex, TabViewer};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_strip_in_the_middle_still_has_a_handle";

/// Half a device pixel at the default scale: every edge is snapped to a whole pixel, so an exact
/// comparison would be reporting the snapping rather than the property.
const TOLERANCE: f32 = 0.5;

/// A pull well inside what either open column can spare. `SeparatorStyle::extra` is 175 px and
/// each column starts near 585, so nothing here is testing a clamp.
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

/// One dock, one context, kept across frames — the geometry map and the drag in flight both live
/// in that context's memory, so a gesture cannot be simulated with a fresh context per frame.
struct Sim {
    ctx: Context,
    state: DockState<String>,
    /// The row holding all three.
    row: NodeId,
    /// The three leaves in screen order.
    panels: [NodeId; 3],
}

impl Sim {
    /// Three equal columns with the middle one collapsed into a sideways strip.
    fn strip_between_two_columns() -> Self {
        let mut sim = Self::row_of_three();
        sim.state
            .main_surface_mut()
            .set_leaf_fold(sim.panels[1], Fold::Strip);
        sim.settle();
        sim
    }

    /// Three equal columns, nothing collapsed.
    fn row_of_three() -> Self {
        let mut state = DockState::new(vec![tab("a")]);
        let a = state.main_surface().root().unwrap();
        let [_, b] = state.split(path(a), Split::Right, 1.0 / 3.0, Node::leaf(tab("b")));
        let [_, c] = state.split(path(b), Split::Right, 0.5, Node::leaf(tab("c")));
        let row = state.main_surface().root().unwrap();

        let mut sim = Sim {
            ctx: Context::default(),
            state,
            row,
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

    /// One frame with `modifiers` held for the whole of it — a drag is read over several frames,
    /// and what a hand holds it holds throughout.
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
                    // The whole scene: without the knob a collapsed leaf keeps its column and
                    // there is no strip to have a handle beside.
                    .collapse_sideways(true)
                    .show_inside(ui, &mut Viewer);
            });
        });
        output.textures_delta.clear();
    }

    /// Press at `from`, drag right by `by`, release — as a real hand does it, over several
    /// frames, because a separator drag reads `drag_delta()` per frame.
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
        // became a drag at all.
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

    fn layout(&self) -> DockLayout {
        DockLayout::load(&self.ctx, Id::new(DOCK_ID))
    }

    fn rect_of(&self, node: NodeId) -> Rect {
        self.layout()
            .rect(path(node))
            .expect("the node was laid out")
    }

    fn width_of(&self, panel: usize) -> f32 {
        self.rect_of(self.panels[panel]).width()
    }

    /// Where the divider of `gap` was drawn, or [`None`] if the pass drew none there.
    fn divider(&self, gap: usize) -> Option<Rect> {
        self.layout()
            .divider(GapPath::new(path(self.row), GapIndex(gap)))
    }

    /// A point on the divider of `gap`, half way down the row — where a hand would grab it.
    fn grab_point(&self, gap: usize) -> Pos2 {
        let divider = self.divider(gap).expect("there is a line to grab");
        divider.center()
    }

    /// This row's stored weights — what the layout is derived *from*, as opposed to what it came
    /// out as. A strip's own weight is not spent on screen, so only this can say whether a drag
    /// spent it anyway.
    fn shares(&self) -> Vec<f32> {
        self.state.main_surface()[self.row]
            .get_row()
            .expect("the root is a row")
            .shares()
            .iter()
            .map(|share| share.0)
            .collect()
    }
}

/// The property: the two open columns either side of the strip can be resized against each other.
///
/// Both of the strip's edges are handles on that one boundary, and they answer the same, which is
/// what "one line with two handles" means — the assertion runs the same drag from each.
#[test]
fn the_two_open_columns_can_be_resized() {
    for gap in [0, 1] {
        let mut sim = Sim::strip_between_two_columns();
        let (left_before, right_before) = (sim.width_of(0), sim.width_of(2));

        sim.drag(sim.grab_point(gap), PULL, Modifiers::default());

        let left = sim.width_of(0) - left_before;
        let right = sim.width_of(2) - right_before;
        assert!(
            (left - PULL).abs() <= TOLERANCE * 4.0,
            "grabbing the strip's edge {gap}: the left column was to grow by {PULL} px and grew \
             by {left}"
        );
        assert!(
            (right + PULL).abs() <= TOLERANCE * 4.0,
            "and the right one to give the same up, but it changed by {right}"
        );
    }
}

/// The strip keeps its own width through the drag: it is not a child that trades, it is a child
/// that rides along.
#[test]
fn the_strip_keeps_its_own_width() {
    let mut sim = Sim::strip_between_two_columns();
    let before = sim.rect_of(sim.panels[1]);

    sim.drag(sim.grab_point(0), PULL, Modifiers::default());

    let after = sim.rect_of(sim.panels[1]);
    assert!(
        (after.width() - before.width()).abs() <= TOLERANCE,
        "the strip was {} px wide and is now {}",
        before.width(),
        after.width()
    );
    assert!(
        (after.min.x - before.min.x - PULL).abs() <= TOLERANCE * 4.0,
        "and it moved with the line, from {} to {}",
        before.min.x,
        after.min.x
    );
}

/// The width the hidden panel is *holding* — its stored weight — is not what the drag spends.
///
/// Aimed at the ratio rather than at the picture, because the picture cannot tell: a strip is
/// given its own width whatever weight it carries, so a drag that quietly rewrote that weight
/// would look exactly like this one until the panel was expanded again.
#[test]
fn the_strip_keeps_the_width_it_is_holding() {
    let mut sim = Sim::strip_between_two_columns();
    let before = sim.shares();

    sim.drag(sim.grab_point(0), PULL, Modifiers::default());

    let after = sim.shares();
    assert_eq!(
        before[1], after[1],
        "the strip's stored weight was {} and is now {} ({before:?} → {after:?})",
        before[1], after[1]
    );
    assert!(
        after[0] > before[0] && after[2] < before[2],
        "control: the two open columns did trade ({before:?} → {after:?})"
    );
}

/// A strip against the row's **end** grows no handle: there is one open child beside it and
/// nothing for it to trade with.
///
/// The control that keeps the fix from being "any gap beside a strip gets a line".
#[test]
fn a_strip_at_the_edge_grows_no_handle() {
    let mut sim = Sim::row_of_three();
    sim.state
        .main_surface_mut()
        .set_leaf_fold(sim.panels[0], Fold::Strip);
    sim.settle();

    assert_eq!(
        sim.divider(0),
        None,
        "no line between the strip and the column beside it"
    );
    assert!(
        sim.divider(1).is_some(),
        "control: the two open columns still have theirs"
    );
}

/// With nothing collapsed the row has exactly the lines it always had — one per gap, each between
/// two open neighbours. The control that keeps the fix from being "draw two lines everywhere".
#[test]
fn an_open_row_is_untouched() {
    let sim = Sim::row_of_three();
    assert!(sim.divider(0).is_some() && sim.divider(1).is_some());
    let (first, second) = (sim.divider(0).unwrap(), sim.divider(1).unwrap());
    assert!(
        first.max.x < second.min.x,
        "and they are two different lines, one per boundary: {first:?} and {second:?}"
    );
}
