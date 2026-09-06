//! The application has the last word on a close, and hears which closes happened.
//!
//! # What this is a gate on
//!
//! Closing used to be a conversation held *inside* the dock: it called back into the
//! application for permission (`on_close`), for whether a tab could be closed at all, and for a
//! successor — from the middle of the one moment the tree is held mutably. That is the moment
//! nothing can be asked, which is why those callbacks had to go.
//!
//! What replaces them is the same conversation, moved to where it can be had: the frame says
//! what it was asked to close, the application answers
//! ([`DockDraw::settle_closes`](egui_dockyard::DockDraw::settle_closes)) while the tree is still
//! only being read, and the removal is then simply carried out. Afterwards the response carries
//! the tabs that actually went, which is what a close callback was really for — an application
//! keeping anything alongside a tab needs to hear that the tab is gone, and now hears it about
//! closes that *happened* rather than closes that were asked for.
//!
//! Three answers, three scenes. Every close here is a middle click on a title, because the
//! chokepoint being tested is the one the UI calls.

use egui::{Atoms, CentralPanel, Context, Event, Id, PointerButton, Pos2, RawInput, Rect, Ui, Vec2};
use egui_dockyard::{
    CloseVerdict, DockArea, DockState, NodePath, Style, SurfaceIndex, TabViewer, tab_widget_id,
};

const SCREEN: Vec2 = Vec2::new(1000.0, 700.0);
const DOCK_ID: &str = "a_close_the_application_refuses_does_not_happen";

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

struct Sim {
    ctx: Context,
    state: DockState<String>,
    /// The one answer this application gives to every close it is asked about, or `None` for an
    /// application that never settles at all — which has to behave as if it had said
    /// `Close { successor: None }`.
    answer: Option<CloseVerdict>,
    /// Every tab the dock has handed back as closed, across all frames.
    closed: Vec<String>,
    frame: u32,
}

impl Sim {
    fn new(tabs: &[&str], answer: Option<CloseVerdict>) -> Self {
        let mut sim = Self {
            ctx: Context::default(),
            state: DockState::new(tabs.iter().map(|tab| (*tab).to_owned()).collect()),
            answer,
            closed: Vec::new(),
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
        let answer = self.answer;
        let mut closed = Vec::new();
        let mut output = self.ctx.run_ui(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                let mut drawn = DockArea::new(state)
                    .id(Id::new(DOCK_ID))
                    .style(Style::from_egui(ui.style().as_ref()))
                    .show_close_buttons(true)
                    .show_inside(ui, &mut Viewer);
                if let Some(answer) = answer {
                    drawn.settle_closes(|_removal| answer);
                }
                closed = drawn.apply(ui.ctx(), state).closed;
            });
        });
        output.textures_delta.clear();
        self.closed.extend(closed);
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
        // The title's left edge: a hovered tab draws a close button at its right end, and the
        // middle of a short title lands on that instead.
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

    /// Asks to close a tab the way a user does: a middle click on its title.
    fn ask_close(&mut self, title: &str) {
        for _ in 0..60 {
            self.run(vec![]);
        }
        let at = self.tab_centre(title);
        self.run(vec![Event::PointerMoved(at)]);
        self.button(at, PointerButton::Middle, true);
        self.button(at, PointerButton::Middle, false);
        // The request is settled and applied at the end of the pass that saw the click.
        self.run(vec![]);
    }
}

/// `Ignore`: nothing happens at all — the tab stays, and nobody is told anything closed.
#[test]
fn a_refused_close_leaves_the_tab_where_it_was() {
    let mut sim = Sim::new(&["A", "B", "C"], Some(CloseVerdict::Ignore));

    sim.ask_close("B");

    assert_eq!(sim.tabs(), ["A", "B", "C"], "the tab was not taken out");
    assert_eq!(sim.active(), "A", "and nothing moved to it either");
    assert!(
        sim.closed.is_empty(),
        "a close that did not happen is not reported as one: {:?}",
        sim.closed
    );
}

/// `Focus`: the answer for "this tab has something to show you first" — it is landed on
/// instead of being taken out.
#[test]
fn a_close_answered_with_focus_lands_on_the_tab_instead() {
    let mut sim = Sim::new(&["A", "B", "C"], Some(CloseVerdict::Focus));
    assert_eq!(sim.active(), "A", "the scene starts elsewhere, or it proves nothing");

    sim.ask_close("B");

    assert_eq!(sim.tabs(), ["A", "B", "C"], "the tab was not taken out");
    assert_eq!(sim.active(), "B", "it was landed on instead");
    assert!(sim.closed.is_empty(), "nothing closed: {:?}", sim.closed);
}

/// An application that never settles closes what it was asked to — and gets the tab back.
#[test]
fn an_unsettled_close_happens_and_hands_the_tab_over() {
    let mut sim = Sim::new(&["A", "B", "C"], None);

    sim.ask_close("B");

    assert_eq!(sim.tabs(), ["A", "C"]);
    assert_eq!(
        sim.closed,
        ["B"],
        "the tab itself comes back, which is what the close callback used to be for"
    );
}
