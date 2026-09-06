//! An overflowing tab bar scrolls under the wheel without growing its own scroll widget.

use egui::{
    Atoms, CentralPanel, Context, Event, Id, Modifiers, MouseWheelUnit, Pos2, RawInput, Rect,
    TouchPhase, Ui, Vec2,
};
use egui_dockyard::{DockArea, DockState, NodePath, Style, SurfaceIndex, TabViewer};

const SCREEN: Vec2 = Vec2::new(400.0, 300.0);
const DOCK_ID: &str = "tab_bar_wheel_scrolls";

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

fn frame(ctx: &Context, state: &mut DockState<String>, events: Vec<Event>) {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ctx| {
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

fn tab_scroll(state: &DockState<String>) -> f32 {
    let root = state
        .main_surface()
        .root()
        .expect("the tabbed dock has a root");
    state[NodePath::new(SurfaceIndex::main(), root)]
        .get_leaf()
        .expect("the root is a leaf")
        .scroll
}

#[test]
fn wheel_over_overflowing_tabs_moves_the_leaf_scroll() {
    let ctx = Context::default();
    let mut state = DockState::new((0..20).map(|i| format!("long tab title {i}")).collect());

    // First publish the tab bar's hit rectangle, then put the pointer over that rectangle and
    // turn the wheel. A negative delta moves the tab contents left and reveals later tabs.
    frame(&ctx, &mut state, Vec::new());
    let before = tab_scroll(&state);
    frame(
        &ctx,
        &mut state,
        vec![
            Event::PointerMoved(Pos2::new(100.0, 12.0)),
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -120.0),
                phase: TouchPhase::Move,
                modifiers: Modifiers::default(),
            },
        ],
    );

    assert!(
        tab_scroll(&state) < before,
        "the leaf, rather than a rendered ScrollArea, owns tab-bar wheel scrolling"
    );
}
