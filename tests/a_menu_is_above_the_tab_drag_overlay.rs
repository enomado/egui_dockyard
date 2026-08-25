//! A menu open while a tab is being dragged is above the drop overlay — in the paint.
//!
//! # Why this file exists
//!
//! The junction handle's square was painted over open menus, and the reason was structural rather
//! than local: a layer that is not an [`egui::Area`] is *ranked* below every area of its tier by
//! `Areas::compare_order` (`None < Some(i)`) and *painted* after them by `GraphicLayers::drain`,
//! which sweeps up every layer of a tier it has not already walked. The fix was to make the
//! handle a real `Area`.
//!
//! `drag_and_drop.rs`'s overlay had the same shape — one bare [`egui::LayerId`] in
//! [`Order::Foreground`], shared by every painter the drop overlay draws through — so the same
//! question had to be asked of it, and the backlog asked it in this order: **can a menu be open
//! while a tab is in flight at all?** If it cannot, there is no scene and no item.
//!
//! It can, and this file is the reason the answer is not a matter of opinion. egui's own menus
//! close on a press outside themselves, so a menu the *user* opened is indeed gone by the time a
//! tab drag begins. But a menu is not the only thing in `Order::Foreground`: `Popup`'s own mapping
//! is `Menu | Popup => Order::Foreground`, and an application draws popups it keeps open itself —
//! a pinned inspector, a palette, a toast with a button, anything shown from state rather than
//! from a click that must first land somewhere else. Such a thing coexists with a drag by
//! construction, and it is the same layer a menu is. So the scene here opens the foreground area
//! from the harness's own state, exactly the way an application would, and exactly the way the
//! junction scenes already do.
//!
//! # What is deliberately *not* asked
//!
//! **The press.** Unlike the handle, the overlay is paint and nothing else: it never asks egui for
//! a widget (`make_overlay_painter` hands out a [`Painter`](egui::Painter), and the drop buttons
//! are hit-tested against the pointer's position arithmetically). There is no press for a menu to
//! win or lose, so the pair of questions the junction scenes ask — the eye and the hand — has one
//! half here.
//!
//! **The carried tab.** The tab that follows the cursor is drawn in [`Order::Tooltip`], which is
//! *above* `Foreground` on purpose and is the same tier egui puts its own drag payload preview in.
//! A thing that follows the hand belongs over everything, so the scene keeps the pointer away from
//! the menu rather than exempting that layer: the witness is the drop overlay, which is painted
//! where the *destination* is, not where the hand is.

use egui::{
    Area, CentralPanel, Context, Event, Frame, Id, Order, PointerButton, Pos2, RawInput, Rect,
    Sense, Ui, Vec2, WidgetText,
};
use egui_dockyard::{
    DockArea, DockLayout, DockState, DragSubject, NodePath, Style, SurfaceIndex, TabIndex,
    TabViewer, tab_widget_id,
};

const SCREEN: Vec2 = Vec2::new(1000.0, 700.0);
const DOCK_ID: &str = "a_menu_is_above_the_tab_drag_overlay";

/// The menu's own fill, unlike anything the dock's style paints, so its shapes can be told from
/// everything else in the frame's shape list.
const MENU_FILL: egui::Color32 = egui::Color32::from_rgb(3, 7, 11);

/// Frames enough for the overlay's preference lock (0.3 s by default) to lapse, so the overlay
/// has settled on the leaf the pointer is actually over.
const PREFERENCE_FRAMES: u32 = 30;

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

/// One frame's worth of what the scene needs to judge: what the dock says it is holding, and every
/// shape the frame painted, in paint order, with the fill of the ones that have one.
struct Painted {
    held: Option<DragSubject>,
    shapes: Vec<(Rect, Option<egui::Color32>)>,
}

struct Sim {
    ctx: Context,
    state: DockState<String>,
    /// Where the foreground area sits this frame, or `None` for no menu at all. Held by the
    /// harness rather than by a click, because that is the shape of the only menu that can still
    /// be open during a drag — see the module doc.
    menu: Option<Rect>,
    frame: u32,
}

impl Sim {
    /// Two leaves side by side: three tabs on the left to drag one out of, one on the right to be
    /// the destination the overlay draws over.
    fn new() -> (Self, NodePath, NodePath) {
        let mut state = DockState::new(vec![
            "Tab 1".to_owned(),
            "Tab 2".to_owned(),
            "Tab 3".to_owned(),
        ]);
        let root = state.main_surface().root().unwrap();
        let [left, right] =
            state
                .main_surface_mut()
                .split_right(root, 0.5, vec!["Elsewhere".to_owned()]);

        let mut sim = Self {
            ctx: Context::default(),
            state,
            menu: None,
            frame: 0,
        };
        // Gestures are aimed with geometry, and there is no geometry until a pass has run.
        sim.run(vec![]);
        (
            sim,
            NodePath::new(SurfaceIndex::main(), left),
            NodePath::new(SurfaceIndex::main(), right),
        )
    }

    fn run(&mut self, events: Vec<Event>) -> Painted {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            // egui's drag detection is time-based; a clock that never ticks is its own kind of
            // unreal scene.
            time: Some(f64::from(self.frame) / 60.0),
            events,
            ..Default::default()
        };
        self.frame += 1;

        let state = &mut self.state;
        let menu = self.menu;
        let mut held = None;
        let mut output = self.ctx.run_ui(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                held = DockArea::new(state)
                    .id(Id::new(DOCK_ID))
                    .style(Style::from_egui(ui.style().as_ref()))
                    .show_inside_with_response(ui, &mut Viewer)
                    .dragging
                    .map(|drag| drag.subject);
            });

            if let Some(menu) = menu {
                // Same shape egui gives a menu or a popup: a foreground area at a fixed place.
                Area::new(Id::new("the menu"))
                    .order(Order::Foreground)
                    .fixed_pos(menu.min)
                    .constrain(false)
                    // A fading area paints its fill with the opacity of the moment, and the scene
                    // below tells the menu from everything else *by* that fill.
                    .fade_in(false)
                    .sense(Sense::hover())
                    .show(ctx, |ui| {
                        Frame::NONE.fill(MENU_FILL).show(ui, |ui| {
                            ui.set_min_size(menu.size());
                        });
                    });
            }
        });
        // Headless harness: no GPU backend to hand the delta to, and epaint panics on drop
        // otherwise.
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

        Painted { held, shapes }
    }

    fn layout(&self) -> DockLayout {
        DockLayout::load(&self.ctx, Id::new(DOCK_ID))
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

    fn move_to(&mut self, at: Pos2) -> Painted {
        self.run(vec![Event::PointerMoved(at)])
    }

    fn button(&mut self, at: Pos2, pressed: bool) -> Painted {
        self.run(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }])
    }

    /// Moves the pointer in steps, one frame each, and rests at the far end long enough for the
    /// overlay's preference lock to settle on what is under it.
    fn sweep(&mut self, from: Pos2, to: Pos2) -> Painted {
        for step in 1..=8u8 {
            self.move_to(from + (to - from) * (f32::from(step) / 8.0));
        }
        let mut last = self.move_to(to);
        for _ in 0..PREFERENCE_FRAMES {
            last = self.move_to(to);
        }
        last
    }

    /// Grabs a tab out of the left leaf and carries it to `to`, button still down. Answers the
    /// frame it ends on.
    fn carry_a_tab_to(&mut self, leaf: NodePath, tab: usize, to: Pos2) -> Painted {
        let home = self.tab_rect(leaf, tab).center();
        self.move_to(home);
        self.button(home, true);
        // Out of the tab row first — the dock calls a press a drag only once it has travelled.
        let out = home + Vec2::new(0.0, 160.0);
        self.sweep(home, out);
        self.sweep(out, to)
    }
}

/// Where the harness's foreground area sits: inside the destination leaf, so the overlay drawn for
/// that leaf reaches it, and well away from the corner the pointer rests in, so the carried tab —
/// which is in `Order::Tooltip` and belongs over everything — is not what the assertion catches.
fn menu_over(destination: Rect) -> Rect {
    Rect::from_min_size(
        destination.min + Vec2::new(40.0, 120.0),
        Vec2::new(200.0, 120.0),
    )
}

/// Where the hand rests while the overlay draws: the far corner of the destination leaf, inside it
/// (so the overlay resolves to that leaf) and far from the menu.
fn resting_corner(destination: Rect) -> Pos2 {
    destination.max - Vec2::new(30.0, 30.0)
}

/// Positive control, and the one that decides whether this file has a subject at all: with a tab
/// in flight over the destination leaf, the overlay really does paint into the rectangle the menu
/// occupies. Without this, "nothing was painted over the menu" is satisfied by a scene where
/// nothing was painted there at all.
#[test]
fn the_overlay_reaches_where_the_menu_is() {
    let (mut sim, left, right) = Sim::new();
    let destination = sim.layout().rect(right).expect("laid out this frame");
    let menu = menu_over(destination);

    // No menu in this scene: the question is only whether the overlay covers that rectangle.
    let frame = sim.carry_a_tab_to(left, 1, resting_corner(destination));
    assert!(
        matches!(frame.held, Some(DragSubject::Tab(_))),
        "the scene has to be a live tab drag, or it is not the gesture under test — the dock says \
         {:?}",
        frame.held
    );

    let reaching: Vec<Rect> = frame
        .shapes
        .iter()
        .map(|(rect, _)| *rect)
        .filter(|rect| {
            let shared = rect.intersect(menu);
            shared.is_positive() && shared.area() > 1.0
        })
        .collect();
    assert!(
        !reaching.is_empty(),
        "the drop overlay for {right:?} must paint into {menu:?} for the order test to mean \
         anything, and nothing did"
    );
}

/// The eye: nothing of the dock's is painted over the open foreground area.
#[test]
fn a_menu_over_the_drop_overlay_is_painted_last() {
    let (mut sim, left, right) = Sim::new();
    let destination = sim.layout().rect(right).expect("laid out this frame");
    let menu = menu_over(destination);
    let rest = resting_corner(destination);
    assert!(
        !menu.contains(rest),
        "the hand must rest off the menu {menu:?}, and it is at {rest:?} — the carried tab is \
         drawn at the pointer and is allowed above everything"
    );

    // Open before the drag, and not by one frame: a new area only joins egui's order list at the
    // end of the frame it first appears in.
    sim.menu = Some(menu);
    sim.run(vec![]);
    sim.run(vec![]);

    let frame = sim.carry_a_tab_to(left, 1, rest);
    assert!(
        matches!(frame.held, Some(DragSubject::Tab(_))),
        "the scene has to be a live tab drag, or it is not the gesture under test — the dock says \
         {:?}",
        frame.held
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
        "the menu is at {menu:?}, and {} shape(s) were painted on top of it: {over_the_menu:?} — \
         the drop overlay is what reaches there while a tab is in flight",
        over_the_menu.len()
    );
}
