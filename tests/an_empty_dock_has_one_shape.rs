//! Gate: there is exactly one empty dock, however the dock got there.
//!
//! # The two shapes of "empty"
//!
//! A dock can arrive at holding no tabs in four ways, and until this gate they did not all
//! agree on what the result *is*:
//!
//! * `DockState::new(vec![])` built a tree whose root is a leaf with no tabs;
//! * closing the last tab (`remove_tab`), an in-place sweep (`retain_tabs`) and a copying one
//!   (`filter_tabs`) all left a tree with **no root at all**.
//!
//! The difference is not bookkeeping. `Tree::is_empty` asks about the root, so the first shape
//! answers "not empty", and the renderer branches on exactly that: a tree with no root paints
//! nothing and allocates the whole area as a drop target ([`TabDestination::EmptySurface`]),
//! while a tree with an empty root leaf goes down the ordinary path and paints a leaf — a strip
//! of empty tab bar, and a drop target the size of that leaf rather than of the surface. The
//! same "the dock is empty" state therefore looked and behaved differently depending on whether
//! the user had closed the last tab or the application had started that way.
//!
//! It also cost an exception, written down four times: the validator exempted the root from
//! `EmptyLeaf`, `Tree::split` carried a branch to fill an empty leaf instead of splitting it,
//! and the persistence layer repeated the exemption on both the current and the legacy form.
//!
//! # What is pinned here
//!
//! *Structure*: all four ways give a tree with no root, no tabs, and a clean `validate`.
//!
//! *Rendering*: one headless frame over each of them publishes no node geometry at all
//! ([`DockLayout::is_empty`]) and paints the same shapes — the assertion that the empty tab bar
//! strip is gone, stated without a screen.

use egui::{CentralPanel, Context, Id, LayerId, Pos2, RawInput, Rect, Ui, Vec2, WidgetText};
use egui_dock::{DockArea, DockLayout, DockState, Style, TabViewer};

const SCREEN: Vec2 = Vec2::new(800.0, 600.0);

struct Viewer;

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        tab.clone().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        ui.label(tab.as_str());
    }
}

fn tab(name: &str) -> String {
    name.to_owned()
}

/// Every way a dock can come to hold no tabs, named by how it got there.
///
/// The names are what the failure message reads, so they say the route, not the shape.
fn every_empty_dock() -> Vec<(&'static str, DockState<String>)> {
    let built_empty = DockState::new(Vec::new());

    let mut last_tab_closed = DockState::new(vec![tab("only")]);
    let path = last_tab_closed.find_tab(&tab("only")).unwrap();
    last_tab_closed.remove_tab(path);

    let mut swept_in_place = DockState::new(vec![tab("a"), tab("b")]);
    swept_in_place.retain_tabs(|_| false);

    let swept_by_copy = DockState::new(vec![tab("a"), tab("b")]).filter_tabs(|_| false);

    vec![
        ("built empty", built_empty),
        ("last tab closed", last_tab_closed),
        ("swept in place", swept_in_place),
        ("swept by copy", swept_by_copy),
    ]
}

#[test]
fn every_way_to_empty_gives_the_same_tree() {
    for (route, state) in every_empty_dock() {
        let main = state.main_surface();
        assert_eq!(
            main.root(),
            None,
            "{route}: an empty dock has no root — the empty root leaf is not a second way to be empty"
        );
        assert!(main.is_empty(), "{route}: and says so");
        assert_eq!(main.len(), 0, "{route}: with no nodes left behind either");
        assert_eq!(state.iter_all_tabs().count(), 0, "{route}: and no tabs");
        assert_eq!(state.validate(), Ok(()), "{route}: and is well-formed");
    }
}

#[test]
fn every_way_to_empty_draws_the_same_nothing() {
    let ctx = Context::default();
    let style = Style::from_egui(&egui::Style::default());

    let mut painted: Option<(&str, Vec<String>)> = None;
    for (route, mut state) in every_empty_dock() {
        let id = Id::new("an_empty_dock_has_one_shape").with(route);
        let shapes = frame(&ctx, &mut state, id, &style);

        let layout = DockLayout::load(&ctx, id);
        assert!(
            layout.is_empty(),
            "{route}: an empty dock publishes no node geometry, but {} node(s) got a rectangle",
            layout.len()
        );
        match &painted {
            None => painted = Some((route, shapes)),
            Some((first_route, first)) => assert_eq!(
                &shapes, first,
                "{route} paints something {first_route} does not — the empty dock has two looks again"
            ),
        }
    }
}

/// One headless frame, returning what the dock's own layer painted, described so that two
/// frames of two different `DockState`s can be compared.
///
/// The shapes are read *inside* the pass (`Context::graphics`), because `end_pass` drains every
/// layer into one flat list and the layer each shape belongs to is gone by the time the frame
/// returns — and "what the dock painted" is a statement about the dock's layer.
fn frame(ctx: &Context, state: &mut DockState<String>, id: Id, style: &Style) -> Vec<String> {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
        ..Default::default()
    };
    let mut shapes = Vec::new();
    let mut output = ctx.run_ui(input, |ui| {
        CentralPanel::default().show(ui, |ui| {
            DockArea::new(state)
                .id(id)
                .style(style.clone())
                .show_inside(ui, &mut Viewer);
        });
        shapes = background_shapes(ui.ctx());
    });
    // `TexturesDelta` panics when dropped with deltas nobody applied, and there is no backend
    // here to apply them.
    output.textures_delta.clear();
    shapes
}

/// The background layer's shapes, as text: the discriminant plus the rectangle each shape
/// covers, rounded to whole pixels.
///
/// Text rather than the shapes themselves because `Shape` is not `PartialEq`, and because a
/// failure has to be readable — the point of the comparison is "the tab bar strip is back",
/// which a list of names and rectangles says out loud.
fn background_shapes(ctx: &Context) -> Vec<String> {
    let layer = LayerId::background();
    ctx.graphics(|graphics| {
        graphics
            .get(layer)
            .map(|list| {
                list.all_entries()
                    .map(|entry| {
                        let rect = entry.shape.visual_bounding_rect();
                        format!(
                            "{} [{:.0} {:.0} {:.0} {:.0}]",
                            shape_kind(&entry.shape),
                            rect.min.x,
                            rect.min.y,
                            rect.max.x,
                            rect.max.y
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn shape_kind(shape: &egui::Shape) -> &'static str {
    match shape {
        egui::Shape::Noop => "noop",
        egui::Shape::Vec(_) => "vec",
        egui::Shape::Circle(_) => "circle",
        egui::Shape::Ellipse(_) => "ellipse",
        egui::Shape::LineSegment { .. } => "line",
        egui::Shape::Path(_) => "path",
        egui::Shape::Rect(_) => "rect",
        egui::Shape::Text(_) => "text",
        egui::Shape::Mesh(_) => "mesh",
        egui::Shape::QuadraticBezier(_) => "quadratic",
        egui::Shape::CubicBezier(_) => "cubic",
        egui::Shape::Callback(_) => "callback",
    }
}
