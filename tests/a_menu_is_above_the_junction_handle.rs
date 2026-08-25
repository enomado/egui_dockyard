//! A menu opened over a separator is above the junction handle — in the paint and in the press.
//!
//! # Why this needs a scene of its own
//!
//! The handle is not an ordinary widget of the dock's: it is drawn into a layer of its own, and
//! that layer is what stops the separator underneath from taking the hover back (see
//! `draw_one_handle`). Which layer it is decides two different questions, and egui answers them
//! by two different rules:
//!
//! * **the press** is resolved by [`egui::Memory::areas`]' order, where a layer that is not an
//!   [`egui::Area`] — and the handle's is not — compares *below* every area of the same
//!   [`egui::Order`], because `None < Some(i)`;
//! * **the paint** is resolved by `GraphicLayers::drain`, which walks the areas of an order first
//!   and then sweeps up every layer of that order it has not seen — so a non-area layer is painted
//!   **last** in its tier, on top of the areas it was ranked under.
//!
//! So one `Order` can put the handle under a menu for the pointer and over it for the eye, which
//! is exactly the state Стас reported on 2026-08-10: a menu opened over a separator has a square
//! sitting on it. Both halves are asserted here, because fixing the paint by lowering the tier is
//! the kind of change that can quietly take the press with it.
//!
//! The menu here is a plain [`egui::Area`] in [`egui::Order::Foreground`] with a distinctive fill,
//! because that is exactly what egui's own menus and popups are (`Popup`'s `Menu | Popup =>
//! Order::Foreground`). Using a real menu would drag its own open/close state into a test that is
//! about layers.

use egui::{
    Area, Color32, Context, Event, Frame, Id, Modifiers, Order, PointerButton, Pos2, RawInput,
    Rect, Sense, Ui, Vec2, WidgetText,
};
use egui_dockyard::{
    DockArea, DockLayout, DockState, DragInFlight, DragSubject, NodeId, NodePath, Style,
    SurfaceIndex, TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_menu_is_above_the_junction_handle";

/// The menu's own fill, unlike anything the dock's style paints, so its shapes can be told from
/// everything else in a flat list.
const MENU_FILL: Color32 = Color32::from_rgb(3, 5, 7);

/// How far the menu's left edge sits from the junction, in points.
///
/// Inside the drawn square (half of `CrossSplitToggleStyle::size`, 7 by default) so the handle
/// really does overlap the menu, and far enough from the catch zone's edge (13 by default) to
/// leave room for a pointer that is *outside* the menu and still on the handle.
const MENU_OVERLAP: f32 = 6.0;

struct Viewer;

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> WidgetText {
        tab.clone().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        ui.label(tab.as_str());
    }
}

/// What a menu was allowed to be this frame: nothing, a bare rectangle, or a rectangle that
/// answers to a press.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    None,
    /// Painted and interactable to nothing: the scene for the paint order.
    Bare,
    /// One button filling it, the scene for the press.
    WithButton,
}

/// What one frame is asked to report back.
struct Frame1 {
    /// What the dock said it was holding when the pass ended.
    held: Option<DragInFlight>,
    /// Whether the menu's button was clicked this frame (always `false` without one).
    menu_clicked: bool,
    /// Everything painted, in the order it will be drawn in.
    ///
    /// `FullOutput::shapes` is the flattened list `GraphicLayers::drain` produced, so its *order*
    /// is the paint order — which is the whole question here, and the one thing a per-layer read
    /// mid-frame cannot answer.
    shapes: Vec<(Rect, Option<Color32>)>,
}

/// A three-leaf dock: the root split left/right, and the left column split again, so a divider
/// ends on the vertical line and the two make a tee.
fn a_tee() -> (DockState<String>, NodePath, NodePath, NodePath) {
    let mut state = DockState::new(vec!["Tab 1".to_owned()]);
    let root = state.main_surface().root().unwrap();
    let [left, right] = state
        .main_surface_mut()
        .split_right(root, 0.5, vec!["Right".to_owned()]);
    let [left, below] = state
        .main_surface_mut()
        .split_below(left, 0.5, vec!["Below".to_owned()]);
    let path = |node: NodeId| NodePath::new(SurfaceIndex::main(), node);
    (state, path(left), path(below), path(right))
}

fn run(
    ctx: &Context,
    state: &mut DockState<String>,
    id: Id,
    events: Vec<Event>,
    menu: Menu,
    menu_rect: Rect,
) -> Frame1 {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
        events,
        ..Default::default()
    };
    let mut held = None;
    let mut menu_clicked = false;
    let mut output = ctx.run_ui(input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            held = DockArea::new(state)
                .id(id)
                .style(Style::from_egui(ui.style().as_ref()))
                .show_inside_with_response(ui, &mut Viewer)
                .dragging;
        });

        if menu != Menu::None {
            // Same shape egui gives a menu: a foreground area at a fixed place.
            Area::new(Id::new("the menu"))
                .order(Order::Foreground)
                .fixed_pos(menu_rect.min)
                .constrain(false)
                // A fading area paints its fill with the opacity of the moment, and the scene
                // below tells the menu from everything else *by* that fill.
                .fade_in(false)
                .show(ui.ctx(), |ui| {
                    Frame::NONE.fill(MENU_FILL).show(ui, |ui| {
                        ui.set_min_size(menu_rect.size());
                        if menu == Menu::WithButton {
                            menu_clicked = ui
                                .interact(menu_rect, Id::new("the menu item"), Sense::click())
                                .clicked();
                        }
                    });
                });
        }
    });
    output.textures_delta.clear();

    let shapes = output
        .shapes
        .iter()
        .map(|clipped| {
            let fill = match &clipped.shape {
                egui::Shape::Rect(rect) => Some(rect.fill),
                _ => None,
            };
            (clipped.shape.visual_bounding_rect(), fill)
        })
        .collect();

    Frame1 {
        held,
        menu_clicked,
        shapes,
    }
}

fn moved_to(at: Pos2) -> Vec<Event> {
    vec![Event::PointerMoved(at)]
}

fn button(at: Pos2, pressed: bool) -> Vec<Event> {
    vec![
        Event::PointerMoved(at),
        Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
        },
    ]
}

/// Where the tee sits: the vertical line the two columns share, at the height of the divider that
/// ends on it.
fn tee_of(ctx: &Context, id: Id, left: NodePath, below: NodePath, right: NodePath) -> Pos2 {
    let layout = DockLayout::load(ctx, id);
    let rect = |path| {
        layout
            .rect(path)
            .expect("the surface was laid out this frame")
    };
    let (left, below, right) = (rect(left), rect(below), rect(right));
    Pos2::new(
        (left.max.x + right.min.x) / 2.0,
        (left.max.y + below.min.y) / 2.0,
    )
}

/// A settled scene, with the pointer parked away from every boundary.
fn settled(ctx: &Context, state: &mut DockState<String>, id: Id) {
    for _ in 0..4 {
        run(
            ctx,
            state,
            id,
            moved_to(Pos2::new(20.0, 20.0)),
            Menu::None,
            Rect::NOTHING,
        );
    }
}

/// The menu the scene opens over the tee: covering the junction and the right half of the handle,
/// with its left edge close enough for a pointer to sit on the handle and off the menu.
fn menu_over(tee: Pos2) -> Rect {
    Rect::from_min_size(
        Pos2::new(tee.x - MENU_OVERLAP, tee.y - 60.0),
        Vec2::new(220.0, 120.0),
    )
}

/// Positive control: the handle really is offered at the point the two scenes below aim at, and a
/// press there really is a junction gesture. Without this, a menu winning is not news — a scene
/// with no handle in it would say the same.
#[test]
fn the_tee_offers_a_handle_where_the_scenes_press() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let (mut state, left, below, right) = a_tee();
    settled(&ctx, &mut state, id);
    let tee = tee_of(&ctx, id, left, below, right);

    run(
        &ctx,
        &mut state,
        id,
        moved_to(tee),
        Menu::None,
        Rect::NOTHING,
    );
    run(
        &ctx,
        &mut state,
        id,
        button(tee, true),
        Menu::None,
        Rect::NOTHING,
    );
    let held = run(
        &ctx,
        &mut state,
        id,
        moved_to(tee + Vec2::new(30.0, 0.0)),
        Menu::None,
        Rect::NOTHING,
    )
    .held
    .expect("a press on the tee's handle is a gesture the dock names");
    assert!(
        matches!(held.subject, DragSubject::Junction { .. }),
        "the scene's aiming point is a junction handle, not {:?}",
        held.subject
    );
    run(
        &ctx,
        &mut state,
        id,
        button(tee + Vec2::new(30.0, 0.0), false),
        Menu::None,
        Rect::NOTHING,
    );
}

/// The eye: nothing of the dock's is painted over the menu.
#[test]
fn a_menu_over_a_junction_is_painted_last() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let (mut state, left, below, right) = a_tee();
    settled(&ctx, &mut state, id);
    let tee = tee_of(&ctx, id, left, below, right);
    let menu = menu_over(tee);

    // The pointer sits on the handle and *outside* the menu — that is what makes the handle paint
    // at all. A pointer under the menu would be hovering the menu, the handle would draw nothing,
    // and the scene would prove nothing about paint order.
    let on_the_handle = tee - Vec2::new(11.0, 0.0);
    assert!(
        !menu.contains(on_the_handle),
        "the aiming point {on_the_handle:?} must be off the menu {menu:?}"
    );
    run(
        &ctx,
        &mut state,
        id,
        moved_to(on_the_handle),
        Menu::Bare,
        menu,
    );
    let frame = run(
        &ctx,
        &mut state,
        id,
        moved_to(on_the_handle),
        Menu::Bare,
        menu,
    );

    let last_menu_shape = frame
        .shapes
        .iter()
        .rposition(|(_, fill)| *fill == Some(MENU_FILL))
        .expect("the menu was painted, or there is nothing for anything to be painted over");
    let over_the_menu: Vec<Rect> = frame.shapes[last_menu_shape + 1..]
        .iter()
        .map(|(rect, _)| *rect)
        .filter(|rect| {
            let shared = rect.intersect(menu);
            shared.is_positive() && shared.area() > 1.0
        })
        .collect();
    assert!(
        over_the_menu.is_empty(),
        "the menu is at {menu:?}, and {} shape(s) were painted on top of it: {over_the_menu:?} \
         — the handle's layer is the only thing in this scene that reaches there",
        over_the_menu.len()
    );
}

/// The hand: a press inside the menu is the menu's, not the handle's.
#[test]
fn a_press_on_the_menu_is_not_a_press_on_the_handle() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let (mut state, left, below, right) = a_tee();
    settled(&ctx, &mut state, id);
    let tee = tee_of(&ctx, id, left, below, right);
    let menu = menu_over(tee);

    // Inside the menu *and* inside the handle's catch zone: the one point where the two claims
    // overlap, and so the only point that tells them apart.
    let contested = tee + Vec2::new(4.0, 0.0);
    assert!(
        menu.contains(contested),
        "the aiming point {contested:?} must be on the menu {menu:?}"
    );

    // The menu has to be *open* before the press, and not by one frame: egui hit-tests against
    // the widgets of the previous pass and ranks layers by an order list a new area only joins at
    // the end of the frame it first appears in. A press on the frame the menu opens reaches
    // whatever was underneath it, which is a fact about egui's own timing rather than about who
    // owns the point.
    for _ in 0..3 {
        run(
            &ctx,
            &mut state,
            id,
            moved_to(contested),
            Menu::WithButton,
            menu,
        );
    }
    let pressed = run(
        &ctx,
        &mut state,
        id,
        button(contested, true),
        Menu::WithButton,
        menu,
    );
    assert!(
        pressed.held.is_none(),
        "the press landed on the menu, so the dock is holding nothing — it says {:?}",
        pressed.held.map(|drag| drag.subject)
    );

    // And the menu got it: a press nobody answers would satisfy the assertion above too.
    let released = run(
        &ctx,
        &mut state,
        id,
        button(contested, false),
        Menu::WithButton,
        menu,
    );
    assert!(
        released.menu_clicked,
        "the menu item was pressed and released and did not register the click"
    );
    assert!(released.held.is_none());
}
