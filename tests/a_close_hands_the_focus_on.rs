//! Closing the tab you are looking at lands on the tab you came from — or wherever the
//! application says.
//!
//! # What decides
//!
//! Two rules, in order:
//!
//! 1. [`TabViewer::successor_on_close`], if the application answers it. Only the application
//!    knows when the answer is not a matter of where the user has been — a tab that owns the
//!    one being closed, a pinned tab, an order of its own.
//! 2. otherwise the leaf's **focus history**: the tab last left, then the one before that,
//!    and the left neighbour only once the history is exhausted.
//!
//! # Why the history is a stack and why that needs a test
//!
//! It used to be a single slot, which survives exactly one close: the second close in a row
//! found nothing to consult and fell back to the positional rule — in the one situation where
//! the user has demonstrably been moving around and *has* a history worth following.
//!
//! A scene only shows that if the two rules disagree, so the one below is built so that they
//! do: each close has a history answer and a left-neighbour answer that are different tabs. A
//! scene where they coincide passes under either rule and proves nothing.
//!
//! Everything here goes through the rendered tab bar — clicking a title to activate, a middle
//! click to close — because the chokepoints being tested are the ones the UI calls.

use egui::{
    Atoms, CentralPanel, Context, Event, Id, PointerButton, Pos2, RawInput, Rect, Ui, Vec2,
};
use egui_dockyard::{
    DockArea, DockState, LeafNode, NodePath, Style, SurfaceIndex, TabId, TabIndex, TabViewer,
    tab_widget_id,
};

const SCREEN: Vec2 = Vec2::new(1000.0, 700.0);
const DOCK_ID: &str = "a_close_hands_the_focus_on";

#[derive(Default)]
struct Viewer {
    /// Title of the tab this application insists on landing on, if it insists at all.
    pinned: Option<String>,
}

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> Atoms<'static> {
        Atoms::new(tab.clone())
    }

    fn ui(&mut self, ui: &mut Ui, tab: &Self::Tab) {
        ui.label(tab.as_str());
    }

    fn successor_on_close(
        &mut self,
        leaf: &LeafNode<Self::Tab>,
        closing: TabIndex,
    ) -> Option<TabId> {
        let pinned = self.pinned.as_ref()?;
        leaf.iter_tabs_indexed()
            .find(|(index, tab)| *index != closing && *tab == pinned)
            .and_then(|(index, _)| leaf.tab_id_at(index))
    }
}

struct Sim {
    ctx: Context,
    state: DockState<String>,
    viewer: Viewer,
    frame: u32,
}

impl Sim {
    fn new(tabs: &[&str]) -> Self {
        let mut sim = Self {
            ctx: Context::default(),
            state: DockState::new(tabs.iter().map(|tab| (*tab).to_owned()).collect()),
            viewer: Viewer::default(),
            frame: 0,
        };
        sim.run(vec![]);
        sim
    }

    fn run(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
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
        output.textures_delta.clear();
    }

    fn leaf(&self) -> NodePath {
        NodePath::new(
            SurfaceIndex::main(),
            self.state.main_surface().root().unwrap(),
        )
    }

    fn tabs(&self) -> Vec<String> {
        self.state
            .leaf(self.leaf())
            .unwrap()
            .iter_tabs()
            .map(String::to_owned)
            .collect()
    }

    /// The title of the active tab — what the assertions below are written in, because a
    /// position means something different after every close.
    fn active(&self) -> String {
        let leaf = self.state.leaf(self.leaf()).unwrap();
        leaf[leaf
            .active_index()
            .expect("a non-empty leaf has an active tab")]
        .clone()
    }

    fn tab_centre(&self, title: &str) -> Pos2 {
        let leaf_path = self.leaf();
        let leaf = self.state.leaf(leaf_path).unwrap();
        let (index, _) = leaf
            .iter_tabs_indexed()
            .find(|(_, tab)| *tab == title)
            .unwrap_or_else(|| panic!("no tab titled {title} to aim at"));
        let id = tab_widget_id(Id::new(DOCK_ID), leaf_path, leaf.tab_id_at(index).unwrap());
        let rect = self
            .ctx
            .read_response(id)
            .expect("the tab was drawn last frame")
            .rect;
        // The title's left edge, not the tab's centre: a hovered tab draws a close button at
        // its right end, and a press in the middle of a short title lands on *that* — the
        // button answers the click and the tab never sees it. Found by a scene that clicked
        // and changed nothing.
        Pos2::new(rect.left() + 4.0, rect.center().y)
    }

    fn button(&mut self, pos: Pos2, button: PointerButton, pressed: bool) {
        self.run(vec![Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Default::default(),
        }]);
    }

    /// Frames enough for egui to forget the last click.
    ///
    /// Without it the gestures below arrive inside egui's double-click window and are counted
    /// as one multi-click rather than two separate presses — a scene that looks like it acts
    /// and does not, which is the "green for free" this file exists to avoid.
    fn pause(&mut self) {
        for _ in 0..60 {
            self.run(vec![]);
        }
    }

    /// Activates a tab the way a user does: a click on its title.
    fn click(&mut self, title: &str) {
        self.pause();
        let at = self.tab_centre(title);
        self.run(vec![Event::PointerMoved(at)]);
        self.button(at, PointerButton::Primary, true);
        self.button(at, PointerButton::Primary, false);
        self.run(vec![]);
    }

    /// Closes a tab the way a user does: a middle click on its title.
    fn close(&mut self, title: &str) {
        self.pause();
        let at = self.tab_centre(title);
        self.run(vec![Event::PointerMoved(at)]);
        self.button(at, PointerButton::Middle, true);
        self.button(at, PointerButton::Middle, false);
        // Removals are applied at the end of the pass that saw the click.
        self.run(vec![]);
    }
}

/// Two closes in a row, each with a history answer that is *not* the left neighbour.
///
/// Under the single slot the first close was right and the second fell back to the neighbour:
/// `C` would have been closed onto `B`. Under the stack it walks back to `A`, which is where
/// the user actually came from.
#[test]
fn a_second_close_still_follows_the_history() {
    let mut sim = Sim::new(&["A", "B", "C", "D", "E"]);

    sim.click("C"); // A -> C, history [A]
    sim.click("E"); // C -> E, history [A, C]
    assert_eq!(sim.active(), "E");

    sim.close("E");
    assert_eq!(sim.tabs(), ["A", "B", "C", "D"]);
    assert_eq!(sim.active(), "C", "the tab last left, not D next to it");

    sim.close("C");
    assert_eq!(sim.tabs(), ["A", "B", "D"]);
    assert_eq!(
        sim.active(),
        "A",
        "one step deeper into the history, not B next to it"
    );
}

/// Closing a tab nobody is looking at moves nothing — neither the focus nor the history.
#[test]
fn closing_an_inactive_tab_leaves_the_focus_where_it_was() {
    let mut sim = Sim::new(&["A", "B", "C", "D"]);

    sim.click("B"); // history [A]
    sim.click("D"); // history [A, B]

    sim.close("C");
    assert_eq!(sim.tabs(), ["A", "B", "D"]);
    assert_eq!(sim.active(), "D", "still the tab that was open");

    // ...and the history is intact underneath, which the next close shows.
    sim.close("D");
    assert_eq!(sim.active(), "B");
}

/// The application overrules the history, and is asked only about the tab that has the focus.
#[test]
fn the_application_can_name_the_successor() {
    let mut sim = Sim::new(&["A", "B", "Pinned", "D"]);
    sim.viewer.pinned = Some("Pinned".to_owned());

    sim.click("A"); // history [] -> A was already active
    sim.click("B"); // history [A]
    sim.click("D"); // history [A, B]
    assert_eq!(sim.active(), "D");

    sim.close("D");
    assert_eq!(sim.tabs(), ["A", "B", "Pinned"]);
    assert_eq!(
        sim.active(),
        "Pinned",
        "the application's answer, not the history's B"
    );

    // The history it passed over is still there: closing the pinned tab falls back into it.
    sim.close("Pinned");
    assert_eq!(sim.active(), "B");
}

/// A viewer that answers is still not asked about a tab that is not active, so its answer
/// cannot move a focus that was not going anywhere.
#[test]
fn the_application_is_not_asked_about_a_tab_nobody_is_looking_at() {
    let mut sim = Sim::new(&["A", "B", "Pinned", "D"]);
    sim.viewer.pinned = Some("Pinned".to_owned());

    sim.click("D");
    sim.close("B");

    assert_eq!(sim.tabs(), ["A", "Pinned", "D"]);
    assert_eq!(sim.active(), "D", "the focus never moved, so nothing chose");
}
