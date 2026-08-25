//! [`a_menu_is_above_the_junction_handle`]'s scene, with the dock hosted in a floating window
//! instead of a panel — the residue `handle_layer`'s doc names: a window surface already draws at
//! [`Order::Middle`], so its handles sit at [`Order::Foreground`] alongside menus, and a bare
//! layer at that tier was ranked under real [`egui::Area`]s for the press and painted over them
//! regardless (see [`draw_one_handle`]'s doc on `# It claims no space` and `handle_layer`'s own).
//!
//! The scene is exactly [`a_menu_is_above_the_junction_handle`]'s three, with one difference: the
//! tee lives inside a window surface built with `DockState::add_window`, given a fixed position
//! and size so the aiming points below stay put across a settling pass. Everything else — the
//! menu, the events, the assertions — is unchanged, because the fix (`draw_one_handle` drawing the
//! handle as a real `Area`) is not supposed to know or care which surface it is drawn over.
//!
//! [`a_menu_is_above_the_junction_handle`]: mod@self
//! [`draw_one_handle`]: egui_dockyard::DockArea

use egui::{
    Area, Color32, Context, Event, Frame, Id, Modifiers, Order, PointerButton, Pos2, RawInput,
    Rect, Sense, Ui, Vec2, WidgetText,
};
use egui_dockyard::core::geom::{Point, Size};
use egui_dockyard::{
    DockArea, DockLayout, DockState, DragInFlight, DragSubject, NodePath, Style, TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_menu_is_above_the_junction_handle_in_a_window";

/// The menu's own fill, unlike anything the dock's style paints, so its shapes can be told from
/// everything else in a flat list.
const MENU_FILL: Color32 = Color32::from_rgb(3, 5, 7);

/// How far the menu's left edge sits from the junction, in points. Same value as the panel scene
/// — inside the drawn square, off the catch zone's edge — so the two scenes aim the same way.
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
    shapes: Vec<(Rect, Option<Color32>)>,
}

/// A three-leaf dock inside a floating window: the window's root split left/right, and the left
/// column split again, so a divider ends on the vertical line and the two make a tee — the same
/// shape [`a_menu_is_above_the_junction_handle`]'s `a_tee` builds on the main surface.
///
/// Position and size are fixed rather than left to the window's own auto-sizing, so the tee's
/// on-screen place does not depend on a settling pass the caller has to guess the length of.
fn a_tee_in_a_window() -> (DockState<String>, NodePath, NodePath, NodePath) {
    let mut state = DockState::new(vec!["Tab 1".to_owned()]);
    let window = state.add_window(vec!["Right".to_owned()]);
    state
        .get_window_state_mut(window)
        .expect("the window was just added")
        .set_position(Point::new(80.0, 80.0))
        .set_size(Size::new(500.0, 400.0));

    let root = state[window].root().unwrap();
    let [left, right] = state.split(
        NodePath::new(window, root),
        egui_dockyard::Split::Right,
        0.5,
        egui_dockyard::Node::leaf("Right2".to_owned()),
    );
    let [left, below] = state.split(
        NodePath::new(window, left),
        egui_dockyard::Split::Below,
        0.5,
        egui_dockyard::Node::leaf("Below".to_owned()),
    );
    let path = |node| NodePath::new(window, node);
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

/// A settled scene, with the pointer parked away from every boundary and the window's own
/// auto-sizing pass behind it.
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
/// press there really is a junction gesture, inside a window the same as inside a panel.
#[test]
fn the_tee_offers_a_handle_where_the_scenes_press() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let (mut state, left, below, right) = a_tee_in_a_window();
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

/// The eye: nothing of the dock's is painted over the menu, even though the window surface draws
/// at the same tier ([`Order::Middle`]) the handle's own tier is derived from — the case
/// `handle_layer`'s doc calls the residue.
#[test]
fn a_menu_over_a_junction_in_a_window_is_painted_last() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let (mut state, left, below, right) = a_tee_in_a_window();
    settled(&ctx, &mut state, id);
    let tee = tee_of(&ctx, id, left, below, right);
    let menu = menu_over(tee);

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

/// The hand: a press inside the menu is the menu's, not the handle's, inside a window too.
#[test]
fn a_press_on_the_menu_in_a_window_is_not_a_press_on_the_handle() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let (mut state, left, below, right) = a_tee_in_a_window();
    settled(&ctx, &mut state, id);
    let tee = tee_of(&ctx, id, left, below, right);
    let menu = menu_over(tee);

    let contested = tee + Vec2::new(4.0, 0.0);
    assert!(
        menu.contains(contested),
        "the aiming point {contested:?} must be on the menu {menu:?}"
    );

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
