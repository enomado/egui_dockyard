//! What a tab body paints stays inside the leaf that hosts it — including once the body has been
//! scrolled sideways.
//!
//! # Why this is a test file of its own
//!
//! `nothing_widens_its_clip` is a *static* gate: it reads `src/` and asserts that nobody calls
//! `Ui::set_clip_rect` except the helper that narrows. That catches the one way a `Ui` frees
//! itself by hand, and it says nothing at all about the tab body — which is not built by
//! narrowing a parent, but by `Ui::new`, from scratch (`show/leaf.rs`). A `Ui` made that way
//! takes its clip rectangle from its own `max_rect` and never sees the parent's, so whether the
//! body stays inside its leaf is a statement about `body_rect` and about everything the hosted
//! tab does inside it — not about who called which method.
//!
//! The difference is what a user reports. A dock hands each tab body a two-axis `ScrollArea`
//! (`TabViewer::scroll_bars`, `[true, true]` by default), and a tab whose content is wider than
//! its leaf therefore scrolls sideways. If anything in that path paints outside the leaf, what
//! the user sees is one panel's content drawn over its neighbour — which reads as a broken panel,
//! not as a clipping bug, and cost a session to attribute (2026-08-23).
//!
//! None of this needs a screen. A headless pass leaves every shape it painted behind, each under
//! the clip rectangle it was painted with, and `DockLayout` publishes the rectangle each leaf was
//! given. Visible ink is `shape ∩ clip_rect`, and the question is whether that lies inside the
//! leaf.
//!
//! # The positive control
//!
//! A body that never scrolled would satisfy the assertion trivially, and a scene that painted
//! nothing would satisfy it even better. So the test first pins the marked shape's position, then
//! scrolls, then asserts the shape actually moved before asserting where it ended up: a green run
//! means the body really was scrolled sideways and really did stay inside its leaf.

use egui::{
    CentralPanel, Color32, Context, CornerRadius, Event, Id, LayerId, Modifiers, MouseWheelUnit,
    Pos2, RawInput, Rect, Sense, Shape, TouchPhase, Ui, Vec2, WidgetText, epaint::ClippedShape,
};
use egui_dockyard::{
    DockArea, DockLayout, DockState, Node, NodePath, Split, Style, SurfaceIndex, TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_tab_body_stays_in_its_leaf";

/// Far wider and taller than any leaf in this scene, so the body has something to scroll and
/// something to spill.
const CONTENT: Vec2 = Vec2::new(2000.0, 2000.0);

/// The colour that marks what the hosted tab painted.
///
/// The scene is read back out of a flat pile of shapes that also holds the dock's own furniture —
/// tab bars, borders, separators. A colour nothing else in the default style uses is what makes
/// "this shape came from the tab body" a fact rather than a guess about draw order.
const MARK: Color32 = Color32::from_rgb(7, 191, 3);

/// Half a device pixel at the default scale: layout boundaries are snapped to whole pixels, so an
/// exact comparison would be reporting the snapping. The bug this file is about is hundreds of
/// pixels wide.
const TOLERANCE: f32 = 0.5;

/// The tab that paints the marked block; every other tab paints only a label.
const WIDE: &str = "wide";

/// The tab that builds a layout of its own out of `Panel`s and then paints the marked block in
/// what is left — the shape of a real host panel (a list on the left, detail in the middle).
const PANELLED: &str = "panelled";

struct Viewer;

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> WidgetText {
        tab.clone().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        match tab.as_str() {
            // Deliberately larger than the body: the point of the scene is content that does not
            // fit, which is what puts the body's `ScrollArea` to work.
            WIDE => {
                let (rect, _) = ui.allocate_exact_size(CONTENT, Sense::hover());
                ui.painter().rect_filled(rect, CornerRadius::ZERO, MARK);
            }
            // A host panel that lays itself out: a side list, then the rest. `Panel` sizes itself
            // against the `Ui`'s `max_rect`, and inside the body's `ScrollArea` that rectangle is
            // the virtual size of the content rather than the visible viewport — which is the
            // arrangement this file exists to keep an eye on.
            PANELLED => {
                egui::Panel::left("inner_list").show(ui, |ui| {
                    ui.label("list");
                });
                egui::CentralPanel::default().show(ui, |ui| {
                    let (rect, _) = ui.allocate_exact_size(CONTENT, Sense::hover());
                    ui.painter().rect_filled(rect, CornerRadius::ZERO, MARK);
                });
            }
            _ => {
                ui.label(tab.as_str());
            }
        }
    }
}

fn tab(name: &str) -> String {
    name.to_owned()
}

fn style() -> Style {
    Style::from_egui(&egui::Style::default())
}

/// One headless frame, driven by `events`, returning everything painted in it by layer.
fn frame_with(
    ctx: &Context,
    state: &mut DockState<String>,
    id: Id,
    style: &Style,
    events: Vec<Event>,
) -> Vec<(LayerId, Vec<ClippedShape>)> {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
        events,
        ..Default::default()
    };
    let mut by_layer = Vec::new();
    let mut output = ctx.run_ui(input, |ui| {
        CentralPanel::default().show(ui, |ui| {
            DockArea::new(state)
                .id(id)
                .style(style.clone())
                .show_inside(ui, &mut Viewer);
        });
        // Still inside the pass: the paint lists exist until `end_pass` drains them, and draining
        // is what loses the layer each shape was painted into.
        by_layer = layers_painted(ui.ctx());
    });
    // Load-bearing: `TexturesDelta` panics when dropped with deltas nobody applied, and there is
    // no GPU backend here to apply them.
    output.textures_delta.clear();
    by_layer
}

/// Every layer egui knows about this frame, with the shapes painted into it so far.
fn layers_painted(ctx: &Context) -> Vec<(LayerId, Vec<ClippedShape>)> {
    let layers: Vec<LayerId> = ctx.memory(|memory| memory.layer_ids().collect());
    ctx.graphics(|graphics| {
        layers
            .into_iter()
            .map(|layer| {
                let shapes = graphics
                    .get(layer)
                    .map(|list| list.all_entries().cloned().collect())
                    .unwrap_or_default();
                (layer, shapes)
            })
            .collect()
    })
}

/// The *visible* rectangles the hosted tab painted: its shape intersected with the clip rectangle
/// it was painted under, which is the ink a user would actually see.
fn marked_ink(by_layer: &[(LayerId, Vec<ClippedShape>)]) -> Vec<Rect> {
    let mut out = Vec::new();
    for (_, shapes) in by_layer {
        for ClippedShape { clip_rect, shape } in shapes {
            if let Shape::Rect(rect_shape) = shape
                && rect_shape.fill == MARK
            {
                let visible = shape.visual_bounding_rect().intersect(*clip_rect);
                if visible.is_positive() {
                    out.push(visible);
                }
            }
        }
    }
    out
}

/// The leftmost edge of everything the hosted tab painted — how the scene reports where the body
/// has been scrolled to.
fn left_edge(ink: &[Rect]) -> f32 {
    ink.iter().map(|rect| rect.min.x).fold(f32::MAX, f32::min)
}

/// The scene both tests share, and the assertions they both make.
///
/// Two leaves side by side, the right one holding `hosted`, whose content is wider than the leaf.
/// Scrolling that body right moves its content left — straight at the neighbouring leaf, which is
/// exactly where a clip rectangle that does not hold would show.
fn a_sideways_scrolled_body_stays_inside_its_leaf(hosted: &str) {
    let style = style();
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);

    let mut state = DockState::new(vec![tab("narrow")]);
    let left = state.main_surface().root().unwrap();
    let [_, right] = state.split(
        NodePath::new(SurfaceIndex::main(), left),
        Split::Right,
        0.5,
        Node::leaf(tab(hosted)),
    );

    // Settle: the first frames are where the layout finds its rectangles.
    for _ in 0..4 {
        frame_with(&ctx, &mut state, id, &style, Vec::new());
    }

    let layout = DockLayout::load(&ctx, id);
    let leaf = layout
        .rect(NodePath::new(SurfaceIndex::main(), right))
        .expect("the leaf holding the wide tab was laid out");

    let before = marked_ink(&frame_with(&ctx, &mut state, id, &style, Vec::new()));
    assert!(
        !before.is_empty(),
        "the scene painted nothing to measure: no shape of the marked colour reached the frame, \
         so everything below this would pass by vacancy"
    );
    let before_edge = left_edge(&before);

    // Scroll the body sideways. The pointer has to be over the *body* — the wheel over a tab bar
    // scrolls the tab strip instead, which is a different mechanism entirely.
    let events = vec![
        Event::PointerMoved(leaf.center()),
        Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            // Negative X: egui reads `delta` as "move the content this way", so a negative value
            // moves the content left — which is what scrolling right does, and what aims it at
            // the neighbouring leaf.
            delta: Vec2::new(-300.0, 0.0),
            phase: TouchPhase::Move,
            modifiers: Modifiers::default(),
        },
    ];
    frame_with(&ctx, &mut state, id, &style, events);
    let after = marked_ink(&frame_with(&ctx, &mut state, id, &style, Vec::new()));
    assert!(!after.is_empty(), "the scrolled body painted nothing");
    let after_edge = left_edge(&after);

    // Positive control, before the assertion that matters: if the body did not move, the scene
    // never reached the state under test and a green run would mean nothing.
    assert!(
        after_edge < before_edge - TOLERANCE,
        "the body did not scroll sideways at all (left edge {before_edge} → {after_edge}), so \
         this run never tested what it claims to"
    );

    let bounds = leaf.expand(TOLERANCE);
    for ink in &after {
        assert!(
            bounds.contains_rect(*ink),
            "the tab body painted at {ink:?}, outside the leaf it lives in ({leaf:?}): a user \
             sees this as one panel's content drawn over its neighbour"
        );
    }
}

/// The plain case: a tab that paints one oversized block straight into the body.
#[test]
fn a_sideways_scrolled_tab_body_paints_inside_its_leaf() {
    a_sideways_scrolled_body_stays_inside_its_leaf(WIDE);
}

/// The case a host application actually has: the tab lays *itself* out with `Panel`s and paints
/// into what is left.
///
/// This is worth its own run because the two arrangements measure themselves differently. A
/// `Panel` takes its size from the `Ui`'s `max_rect`, and inside the body's `ScrollArea` that is
/// the virtual size of the content, not the visible viewport — so a panelled body can size itself
/// against a rectangle far larger than the leaf, and does not find out. Whether that also escapes
/// the leaf is the question; either answer is worth having in writing, because "the panel drew
/// over its neighbour" was reported from a running app and attributed to the dock by elimination
/// (2026-08-23), which is not the same as knowing.
#[test]
fn a_body_that_hosts_panels_paints_inside_its_leaf() {
    a_sideways_scrolled_body_stays_inside_its_leaf(PANELLED);
}
