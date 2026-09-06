//! A panel may ask the tree about itself while it is being drawn.
//!
//! # What this is a gate on
//!
//! "Is that tab already open?" is a question a tab body asks — a button that opens a view wants
//! to know whether to open one or focus the one there is. The dock used to make that question
//! unaskable: [`DockArea`] took the tree **mutably** for the length of the frame, so an
//! application whose tree lives beside the rest of its state had nothing left to read it with.
//! What that produced in practice was not a compile error but a panic — the tree went behind a
//! `RefCell`, the frame borrowed it mutably, and the first panel to look at it during draw hit
//! `already mutably borrowed`.
//!
//! Drawing takes the tree by shared reference now, so the question compiles: the viewer below
//! holds the same tree the [`DockArea`] is showing, and reads it from inside
//! [`TabViewer::ui`]. **That it compiles at all is the gate** — before, `&tree` in two places at
//! once was the thing the type system refused. The assertion afterwards is only there so that a
//! test which stopped drawing anything could not pass quietly.
//!
//! # Why the reading viewer is not the one `apply` gets
//!
//! [`DockDraw::apply`](egui_dockyard::DockDraw::apply) still takes a `TabViewer`, because closing
//! a tab asks the application for permission (`on_close`, `is_closeable`) and for a successor.
//! A viewer holding `&DockState` therefore cannot be the one handed to `apply`, which needs the
//! tree mutably — so this file drops it first and applies with a plain one. Removing those
//! callbacks is the next step of the track (D5); when it lands, one viewer will do both halves
//! and this comment goes with it.

use egui::{
    Atoms, CentralPanel, Context, Id, Pos2, RawInput, Rect, Ui, Vec2,
};
use egui_dockyard::{DockArea, DockState, Style, TabViewer};

const SCREEN: Vec2 = Vec2::new(800.0, 600.0);
const DOCK_ID: &str = "a_tab_body_can_read_the_tree_it_is_drawn_in";

/// Draws nothing but a question about the tree it is inside.
struct Peek<'tree> {
    tree: &'tree DockState<String>,
    /// One entry per body drawn: what that body saw the tree holding.
    seen: Vec<(String, usize)>,
}

impl TabViewer for Peek<'_> {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> Atoms<'static> {
        Atoms::new(tab.clone())
    }

    fn ui(&mut self, ui: &mut Ui, tab: &Self::Tab) {
        // The whole point: the tree being drawn, read from inside the drawing.
        let open = self.tree.iter_all_tabs().count();
        self.seen.push((tab.clone(), open));
        ui.label(tab.as_str());
    }
}

/// What `apply` is given, since the reading viewer cannot be — see the module docs.
struct Plain;

impl TabViewer for Plain {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> Atoms<'static> {
        Atoms::new(tab.clone())
    }

    fn ui(&mut self, ui: &mut Ui, tab: &Self::Tab) {
        ui.label(tab.as_str());
    }
}

#[test]
fn a_tab_body_can_read_the_tree_it_is_drawn_in() {
    let ctx = Context::default();
    let mut tree = DockState::new(vec!["one".to_owned(), "two".to_owned()]);

    let mut seen = Vec::new();
    for _ in 0..2 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                // Both borrows shared, and of the same tree: this is the line that used not to
                // compile.
                let mut peek = Peek {
                    tree: &tree,
                    seen: Vec::new(),
                };
                let drawn = DockArea::new(&tree)
                    .id(Id::new(DOCK_ID))
                    .style(Style::from_egui(ui.style().as_ref()))
                    .show_inside(ui, &mut peek);
                seen = peek.seen;
                // `peek` is done with the tree; the frame's edits can have it mutably.
                drawn.apply(ui.ctx(), &mut tree, &mut Plain);
            });
        });
        output.textures_delta.clear();
    }

    assert_eq!(
        seen,
        vec![("one".to_owned(), 2)],
        "the active body was drawn, and it saw both tabs of the tree it is in"
    );
}
