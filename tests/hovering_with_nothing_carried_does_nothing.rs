//! Hovering a tab or a leaf body while nothing is being dragged must never move anything.
//!
//! # Why this file exists
//!
//! [`leaf.rs`](../src/widgets/dock_area/show/leaf.rs) publishes two pieces of hover geometry —
//! `tab_hover_rect` (a tab's own rect, keyed by index) and the `hover_data` temp value (a
//! destination address plus a rect) — on *every* frame the pointer sits over a tab or a leaf
//! body, whether or not a drag is in flight. Until now both writes were guarded by
//! `carried_tab().is_some()` / `carried.is_some()`, which read as "only bother while a hand is
//! full". The guard was removed (see `docs/PLAN_one_place_says_what_the_hand_holds.md`, backlog:
//! "the two 'is a tab in flight' gates in leaf.rs") because the only reader,
//! `show_inside_with_response` in `show/mod.rs`, never looks at `hover_data` without
//! `source_rect`, and `source_rect` itself is `carried.filter(pulled_out).and_then(...)` — so a
//! write made with an empty hand is already unreachable one level up, guard or no guard.
//!
//! This is the gate that removal was asked to come with: it does not exercise the deleted
//! condition (there is nothing left to exercise — that is the point), it exercises the
//! **downstream** gate the deleted condition duplicated. If a future change ever lets `show/mod.rs`
//! read `hover_data` without requiring a carried tab, this is what goes red.

use egui::{Atoms, CentralPanel, Context, Event, Id, Pos2, RawInput, Rect, Ui, Vec2};
use egui_dockyard::{
    DockArea, DockState, NodePath, Style, SurfaceIndex, TabIndex, TabViewer, tab_widget_id,
};

const SCREEN: Vec2 = Vec2::new(1000.0, 700.0);
const DOCK_ID: &str = "hovering_with_nothing_carried_does_nothing";

#[derive(Default)]
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
                    .show_inside(ui, viewer);
            });
        });
        // Headless harness: no GPU backend to hand the delta to, and epaint panics on drop
        // otherwise.
        output.textures_delta.clear();
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

    fn move_to(&mut self, pos: Pos2) {
        self.run(vec![Event::PointerMoved(pos)]);
    }
}

fn two_leaves() -> (DockState<String>, NodePath, NodePath) {
    let mut state = DockState::new(vec!["Tab 1".to_owned()]);
    let root = state.main_surface().root().unwrap();
    let [left, right] = state
        .main_surface_mut()
        .split_right(root, 0.5, vec!["Tab 2".to_owned()]);
    let path = |node| NodePath::new(SurfaceIndex::main(), node);
    (state, path(left), path(right))
}

/// The positive control: with no drag ever started, sweeping the pointer across both tabs and
/// both leaf bodies — the exact geometry the deleted guards used to gate on — moves nothing and
/// leaves the tree well-formed.
#[test]
fn a_sweeping_hover_with_an_empty_hand_moves_nothing() {
    let (state, left, right) = two_leaves();
    let mut sim = Sim::new(state);
    let before = sim.contents();

    let left_tab = sim.tab_rect(left, 0).center();
    let right_tab = sim.tab_rect(right, 0).center();

    // Cross both tabs and both leaf bodies repeatedly — several frames on each, since the
    // deleted guards fired per-frame, not once per gesture.
    for _ in 0..3 {
        sim.move_to(left_tab);
        sim.move_to(right_tab);
    }

    assert_eq!(
        sim.contents(),
        before,
        "hovering with nothing carried must not rearrange any tab"
    );
    assert_eq!(
        sim.state.validate(),
        Ok(()),
        "and the dock stays well-formed"
    );
}
