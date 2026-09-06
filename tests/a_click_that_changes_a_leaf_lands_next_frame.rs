//! Clicking a tab, or the collapse button, takes effect on the frame after the click.
//!
//! Both edits are queued while drawing and applied by the render epilogue, so the frame the
//! click arrives in still paints the leaf as it was: the body shows the previously active tab,
//! and a leaf being collapsed still draws its body one last time. The new state appears on the
//! next repaint.
//!
//! This is the accepted cost of a draw pass that does not mutate the tree (decision of
//! 2026-08-26), and the reason it is a test rather than a comment is that it is *invisible* to
//! every other gate: they assert the outcome after several frames, where the shift has already
//! settled, so they would stay green whether the edit lands this frame or the next. Pinning it
//! here means the day someone removes the shift — `DockMutation`'s doc comment describes how —
//! this file says so out loud instead of the change passing unnoticed.

use egui::{
    Atoms, CentralPanel, Context, Event, Id, PointerButton, Pos2, RawInput, Rect, Ui, Vec2,
};
use egui_dockyard::{
    DockArea, DockLayout, DockState, NodePath, Style, SurfaceIndex, TabIndex, TabViewer,
    tab_widget_id,
};

const SCREEN: Vec2 = Vec2::new(800.0, 600.0);
const DOCK_ID: &str = "a_click_that_changes_a_leaf_lands_next_frame";

/// Records which tab bodies were drawn, so a frame can be asked what it painted rather than
/// what the tree said afterwards.
#[derive(Default)]
struct Viewer {
    drawn: Vec<String>,
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
}

struct Sim {
    ctx: Context,
    state: DockState<String>,
    viewer: Viewer,
}

impl Sim {
    fn new(tabs: &[&str]) -> Self {
        let mut sim = Self {
            ctx: Context::default(),
            state: DockState::new(tabs.iter().map(|tab| (*tab).to_owned()).collect()),
            viewer: Viewer::default(),
        };
        // Enough frames for the layout pass to settle and publish every tab's hit rectangle.
        for _ in 0..4 {
            sim.frame(Vec::new());
        }
        sim
    }

    /// Runs one frame and answers with the tab bodies it painted.
    fn frame(&mut self, events: Vec<Event>) -> Vec<String> {
        self.viewer.drawn.clear();
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            events,
            ..Default::default()
        };
        let (state, viewer) = (&mut self.state, &mut self.viewer);
        let mut output = self.ctx.run_ui(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                DockArea::new(state)
                    .id(Id::new(DOCK_ID))
                    .style(Style::from_egui(ui.style().as_ref()))
                    .show_leaf_collapse_buttons(true)
                    .show_inside(ui, viewer)
                    .apply(ui.ctx(), state, viewer);
            });
        });
        output.textures_delta.clear();
        self.viewer.drawn.clone()
    }

    fn button(&mut self, at: Pos2, pressed: bool) -> Vec<String> {
        self.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }])
    }

    fn leaf_path(&self) -> NodePath {
        NodePath::new(
            SurfaceIndex::main(),
            self.state.main_surface().root().unwrap(),
        )
    }

    fn active(&self) -> String {
        let leaf = self.state.leaf(self.leaf_path()).unwrap();
        leaf[leaf.active_index().expect("a non-empty leaf is open")].clone()
    }

    fn collapsed(&self) -> bool {
        self.state[self.leaf_path()].is_collapsed()
    }

    /// The left edge of a tab's title, not its centre: a hovered tab draws a close button over
    /// its middle, and that button would answer the click instead of the tab.
    fn tab_left_edge(&self, index: usize) -> Pos2 {
        let path = self.leaf_path();
        let leaf = self.state.leaf(path).unwrap();
        let id = tab_widget_id(
            Id::new(DOCK_ID),
            path,
            leaf.tab_id_at(TabIndex(index)).unwrap(),
        );
        let rect = self
            .ctx
            .read_response(id)
            .expect("the tab was drawn last frame")
            .rect;
        Pos2::new(rect.left() + 4.0, rect.center().y)
    }

    /// The collapse button occupies the left end of the tab bar, inside the leaf's rectangle.
    /// Its exact width is private to the crate; 8 px in is comfortably inside it and clear of
    /// the tab-bar margin.
    fn collapse_button(&self) -> Pos2 {
        let rect = DockLayout::load(&self.ctx, Id::new(DOCK_ID))
            .rect(self.leaf_path())
            .expect("the leaf was laid out");
        let height = Style::from_egui(&egui::Style::default()).tab_bar.height;
        Pos2::new(rect.left() + 8.0, rect.top() + height / 2.0)
    }
}

#[test]
fn the_body_of_the_click_frame_still_shows_the_old_tab() {
    let mut sim = Sim::new(&["A", "B", "C"]);
    assert_eq!(sim.active(), "A", "a fresh leaf opens on its first tab");

    let target = sim.tab_left_edge(2);
    sim.button(target, true);
    let during = sim.button(target, false);

    assert_eq!(
        during,
        vec!["A".to_owned()],
        "the frame the click is answered in still paints the tab that was open"
    );
    assert_eq!(
        sim.active(),
        "C",
        "the epilogue of that same frame applied the activation"
    );
    assert_eq!(
        sim.frame(Vec::new()),
        vec!["C".to_owned()],
        "the next repaint shows the clicked tab"
    );
}

#[test]
fn the_click_frame_of_a_collapse_still_draws_the_body() {
    let mut sim = Sim::new(&["A", "B"]);
    assert!(!sim.collapsed(), "a fresh leaf is open");

    let target = sim.collapse_button();
    sim.button(target, true);
    let during = sim.button(target, false);

    assert_eq!(
        during,
        vec!["A".to_owned()],
        "the frame the collapse is requested in still draws the body once more"
    );
    assert!(
        sim.collapsed(),
        "the epilogue of that same frame collapsed the leaf"
    );
    assert!(
        sim.frame(Vec::new()).is_empty(),
        "a collapsed leaf draws no body"
    );
}
