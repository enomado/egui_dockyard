//! A floating window is big enough for what the dock draws inside it — and what the dock
//! draws stays inside it.
//!
//! # Why this is a test file of its own
//!
//! Collapsing every leaf of a floating window turns it into a strip of tab bars, one row per
//! collapsed leaf, and the window is sized to fit exactly that strip. Two different numbers meet
//! there, and both were wrong:
//!
//! * the height the rows *need* — `rows * tab_bar.height` forgot the dividers between the rows,
//!   and forgot that a divider takes its width out of the rows on either side of it;
//! * the height the window is *set to* — `Window::min_height` / `max_height` name the **outer**
//!   size of a window (egui says so on the method: "including frame margins, stroke, and the
//!   title bar"), and the number handed to them was measured in the *content* area inside that
//!   frame. Two frame margins short, every time.
//!
//! So the last row did not fit, and — because a `Ui`'s clip rectangle is *replaced* rather than
//! intersected — it was not cut off either: the tab bar of the bottom row was painted straight
//! over the window's own border and out into the desktop below it.
//!
//! Both halves are geometry, so none of this needs a screen: the layout pass publishes its
//! rectangles through [`DockLayout`], and `Context::run_ui` hands back the shapes that were
//! painted, with the clip rectangle each was painted under. What the eye found, the numbers can
//! keep.

use egui::{
    CentralPanel, Context, CornerRadius, Frame, Id, Pos2, RawInput, Rect, Shape, Stroke, Ui, Vec2,
    WidgetText, epaint::ClippedShape,
};
use egui_dock::{
    DockArea, DockLayout, DockState, Node, NodePath, Split, Style, SurfaceIndex, TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_window_fits_what_it_shows";

/// Half a device pixel at the default scale: every boundary in the layout pass is snapped to
/// whole pixels, so an exact comparison would be reporting the snapping, not the bug it is
/// looking for. The bugs this file is about are 14 px and a whole tab bar row tall.
const TOLERANCE: f32 = 0.5;

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

/// What one headless frame produced: the shapes it painted, and the rectangle the dock was
/// handed to draw the whole of itself in.
struct Painted {
    shapes: Vec<ClippedShape>,
    /// The area the `DockArea` was given — the main surface's border is drawn against this.
    given: Rect,
}

/// One headless frame, with no input.
fn frame(ctx: &Context, state: &mut DockState<String>, id: Id, style: &Style) -> Painted {
    frame_with(ctx, state, id, style, Vec::new())
}

/// One headless frame, driven by `events`.
fn frame_with(
    ctx: &Context,
    state: &mut DockState<String>,
    id: Id,
    style: &Style,
    events: Vec<egui::Event>,
) -> Painted {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
        events,
        ..Default::default()
    };
    let mut given = Rect::NOTHING;
    let mut output = ctx.run_ui(input, |ui| {
        CentralPanel::default().show(ui, |ui| {
            given = ui.available_rect_before_wrap();
            DockArea::new(state)
                .id(id)
                .style(style.clone())
                .show_leaf_collapse_buttons(true)
                .show_close_buttons(true)
                .show_add_buttons(true)
                .show_inside(ui, &mut Viewer);
        });
    });
    // Headless harness, no GPU backend to hand the delta to.
    output.textures_delta.clear();
    Painted {
        shapes: output.shapes,
        given,
    }
}

/// Settle, then one more frame, which is the one under test.
///
/// A floating window finds its size over the first frames — the state carries "move me" and
/// "resize me" requests that are consumed one frame at a time — so a single frame would be
/// measuring the window mid-thought.
fn settle(ctx: &Context, state: &mut DockState<String>, id: Id, style: &Style) -> Painted {
    for _ in 0..6 {
        frame(ctx, state, id, style);
    }
    frame(ctx, state, id, style)
}

/// The default style, which is what every scene here is measured against.
fn style() -> Style {
    Style::from_egui(&egui::Style::default())
}

/// A window holding `rows` leaves stacked vertically, every one of them collapsed.
///
/// This is what a user gets by clicking the collapse arrow on every leaf of a floating window:
/// the window shrinks to a strip of tab bars, one row per leaf.
fn window_of_collapsed_rows(
    rows: usize,
) -> (DockState<String>, SurfaceIndex, Vec<egui_dock::NodeId>) {
    assert!(rows >= 1);
    let mut state = DockState::new(vec![tab("main")]);
    let window = state.add_window(vec![tab("row 0")]);

    let mut leaves = vec![state[window].root().unwrap()];
    for row in 1..rows {
        let last = *leaves.last().unwrap();
        let [_, new] = state.split(
            NodePath::new(window, last),
            Split::Below,
            0.5,
            Node::leaf(tab(&format!("row {row}"))),
        );
        leaves.push(new);
    }
    for &leaf in &leaves {
        state[window].set_leaf_collapsed(leaf, true);
    }

    (state, window, leaves)
}

/// A collapsed row is a tab bar and nothing else, so it needs exactly one tab bar's height.
///
/// It used to get less: the divider between two rows is `separator.width` wide and is cut out
/// of the rows around it, so a strip of `n` rows was `n - 1` dividers short of what it drew.
/// With the default one-pixel divider that is a hairline on the second row and half a row on
/// the twentieth — the bug is not the size of the number, it is that the number is missing.
#[test]
fn every_collapsed_row_gets_a_whole_tab_bar() {
    let style = style();
    for rows in 1..=4 {
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        let (mut state, window, leaves) = window_of_collapsed_rows(rows);
        settle(&ctx, &mut state, id, &style);

        let layout = DockLayout::load(&ctx, id);
        for (row, leaf) in leaves.iter().enumerate() {
            let rect = layout
                .rect(NodePath::new(window, *leaf))
                .expect("every leaf of a shown surface was laid out");
            assert!(
                rect.height() + TOLERANCE >= style.tab_bar.height,
                "in a window of {rows} collapsed rows, row {row} got {} px for a tab bar \
                 that is {} px tall",
                rect.height(),
                style.tab_bar.height
            );
        }
    }
}

/// And the strip of rows fits inside the window's frame.
///
/// The window is sized by `Window::min_height` / `max_height`, which name its **outer** height:
/// frame margin, stroke and all. The dock's rectangles live in the content area inside that
/// frame, so the two are one frame margin apart at each end — and the number was handed over
/// unconverted. The window came out 14 px short at the default style, which is most of a tab
/// bar row.
#[test]
fn the_rows_of_a_collapsed_window_fit_inside_its_frame() {
    let style = style();
    let margin = Frame::window(&egui::Style::default())
        .total_margin()
        .sum()
        .y;

    for rows in 1..=4 {
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        let (mut state, window, _) = window_of_collapsed_rows(rows);
        settle(&ctx, &mut state, id, &style);

        // What the strip needs: a tab bar per row, and a divider between every two of them.
        let strip = rows as f32 * style.tab_bar.height + (rows - 1) as f32 * style.separator.width;
        let outer = window_area_rect(&ctx, window);

        assert!(
            outer.height() + TOLERANCE >= strip + margin,
            "a window of {rows} collapsed rows is {} px tall, and a {strip} px strip inside \
             a {margin} px frame needs {}",
            outer.height(),
            strip + margin
        );
    }
}

/// Nothing a window paints leaves the window.
///
/// The scene is a window deliberately squeezed below what its rows need, because the property
/// has to hold when the geometry *cannot* be satisfied — that is the only interesting case. A
/// row that does not fit must be cut off by the frame, not painted through it: `Ui::set_clip_rect`
/// **replaces** the clip rectangle rather than intersecting it, so the tab bar's own clip
/// silently undid the leaf's, and the label of the bottom row was drawn out in the open, below
/// the window's border, over whatever happened to be there.
///
/// Text is the probe because it is unambiguous: every galley carries its string, so the shape
/// that escaped can be *named* in the failure rather than described as "something at y = 612".
#[test]
fn a_window_paints_no_text_outside_itself() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let style = style();
    let (mut state, window, _) = window_of_collapsed_rows(3);

    // Half the height the three rows need: the window is now too small by construction, and the
    // question is only whether the overflow is cut or spilled.
    state
        .get_window_state_mut(window)
        .unwrap()
        .set_size(egui_dock::geom::Size::new(320.0, 40.0));

    let painted_frame = settle(&ctx, &mut state, id, &style);
    let outer = window_area_rect(&ctx, window).expand(TOLERANCE);

    for (text, painted) in visible_texts(&painted_frame.shapes) {
        if !text.starts_with("row ") {
            continue;
        }
        assert!(
            outer.contains_rect(painted),
            "the label {text:?} was painted at {painted:?}, outside its window at {outer:?}"
        );
    }
}

/// A surface does not cover the border it just drew around itself.
///
/// The border is painted `StrokeKind::Inside` the surface's rectangle and may be rounded; the
/// content then starts at the very same rectangle, so with any rounding at all the first thing
/// drawn — the tab bar, a filled rectangle with square corners — paints over the arc. On the
/// main surface that is the dock's own border being erased at all four corners.
///
/// Clearing an arc of radius `r` costs `r - r / sqrt(2)` on each axis: that is how far the arc
/// bulges inwards from the corner of the rectangle. The property is stated on the rectangles,
/// not on pixels, so it holds for whatever rounding a style asks for.
#[test]
fn a_surface_does_not_cover_the_border_it_draws() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let mut style = style();
    style.main_surface_border_stroke = Stroke::new(3.0, egui::Color32::RED);
    style.main_surface_border_rounding = CornerRadius::same(14);

    let mut state = DockState::new(vec![tab("main")]);
    let painted_frame = settle(&ctx, &mut state, id, &style);

    let layout = DockLayout::load(&ctx, id);
    let root = state.main_surface().root().unwrap();
    let content = layout
        .rect(NodePath::new(SurfaceIndex::main(), root))
        .expect("the main surface was laid out");

    // The rectangle the border was drawn around: what the dock was given, minus its padding.
    let mut border = painted_frame.given;
    if let Some(padding) = style.dock_area_padding {
        border.min += padding.left_top();
        border.max -= padding.right_bottom();
    }

    let radius = f32::from(style.main_surface_border_rounding.nw);
    let clearance = style.main_surface_border_stroke.width + radius * (1.0 - 1.0 / 2.0_f32.sqrt());
    assert!(
        content.min.x + TOLERANCE >= border.min.x + clearance
            && content.min.y + TOLERANCE >= border.min.y + clearance,
        "the content starts at {:?}, inside a {radius} px rounded border at {:?} that needs \
         {clearance} px of clearance",
        content.min,
        border.min
    );
}

/// Collapsing a window and opening it again gives back the window that was there.
///
/// The third place a window's height is decided, and the one that shows the units apart most
/// plainly: `WindowState::expanded_height` records how tall the *dock* was when the window
/// collapsed, and hands that number to `Window::max_height`, which is an **outer** height. Every
/// round trip therefore lost one window frame — 14 px at the default style — and the loss
/// accumulates, because the next collapse records the shrunken height as the new truth. Three
/// round trips and the window has lost a tab bar; a dozen and there is nothing left to click.
///
/// Driven through the collapse button rather than through `set_leaf_collapsed`, because it is
/// the button that records the height: the model call alone leaves that path untouched, and a
/// test written that way would pass no matter what the arithmetic did.
#[test]
fn a_window_collapsed_and_expanded_is_the_height_it_was() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let style = style();

    let mut state = DockState::new(vec![tab("main")]);
    let window = state.add_window(vec![tab("row 0")]);
    let top = state[window].root().unwrap();
    let [_, bottom] = state.split(
        NodePath::new(window, top),
        Split::Below,
        0.5,
        Node::leaf(tab("row 1")),
    );

    settle(&ctx, &mut state, id, &style);
    let before = window_area_rect(&ctx, window).height();

    for round in 1..=3 {
        for leaf in [top, bottom] {
            click_collapse_button(&ctx, &mut state, id, &style, window, leaf);
            assert!(
                state[window][leaf].is_collapsed(),
                "round {round}: the click on the collapse button of {leaf} did not collapse it \
                 — the gesture missed, and everything below this line would pass for free"
            );
        }
        for leaf in [top, bottom] {
            click_collapse_button(&ctx, &mut state, id, &style, window, leaf);
            assert!(
                !state[window][leaf].is_collapsed(),
                "round {round}: the click on the collapse button of {leaf} did not open it again"
            );
        }

        settle(&ctx, &mut state, id, &style);
        let after = window_area_rect(&ctx, window).height();
        assert!(
            (after - before).abs() <= TOLERANCE,
            "after {round} collapse-and-expand round trip(s) the window is {after} px tall, \
             where it started at {before}"
        );
    }
}

/// Click the collapse button of `leaf`, which sits at the left end of its tab bar.
fn click_collapse_button(
    ctx: &Context,
    state: &mut DockState<String>,
    id: Id,
    style: &Style,
    window: SurfaceIndex,
    leaf: egui_dock::NodeId,
) {
    let rect = DockLayout::load(ctx, id)
        .rect(NodePath::new(window, leaf))
        .expect("the leaf was laid out");
    // The button is `TAB_COLLAPSE_BUTTON_SIZE` wide and as tall as the tab bar, at the very
    // start of the bar; the middle of the row is comfortably inside it whatever the margins.
    let at = rect.min + Vec2::new(10.0, style.tab_bar.height / 2.0);

    frame_with(ctx, state, id, style, vec![egui::Event::PointerMoved(at)]);
    frame_with(
        ctx,
        state,
        id,
        style,
        vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    frame_with(
        ctx,
        state,
        id,
        style,
        vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    settle(ctx, state, id, style);
}

/// The outer rectangle egui remembers for a window surface, frame and all.
///
/// The id is the one `show_window_surface` builds — deliberately frozen at the shape the old
/// positional `SurfaceIndex` printed, because egui persists window geometry under it.
fn window_area_rect(ctx: &Context, window: SurfaceIndex) -> Rect {
    let index = match window {
        SurfaceIndex::Window(index) => index.0 + 1,
        SurfaceIndex::Main => panic!("the main surface is not an egui window"),
    };
    let id = Id::new(format!("window SurfaceIndex({index})"));
    ctx.memory(|memory| memory.area_rect(id))
        .expect("the window was shown this frame")
}

/// Every text painted this frame, with the rectangle it actually covers on screen — its own
/// rectangle cut down by the clip rectangle it was painted under.
fn visible_texts(shapes: &[ClippedShape]) -> Vec<(String, Rect)> {
    fn walk(shape: &Shape, clip: Rect, out: &mut Vec<(String, Rect)>) {
        match shape {
            Shape::Text(text) => {
                let rect = text.visual_bounding_rect().intersect(clip);
                if rect.is_positive() {
                    out.push((text.galley.text().to_owned(), rect));
                }
            }
            Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, clip, out);
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, clipped.clip_rect, &mut out);
    }
    out
}
