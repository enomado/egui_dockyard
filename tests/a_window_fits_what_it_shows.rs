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
//! rectangles through [`DockLayout`], and a headless pass leaves behind every shape it painted,
//! under the clip rectangle it was painted with and in the layer that painted it. What the eye
//! found, the numbers can keep.

use egui::{
    CentralPanel, Context, CornerRadius, Frame, Id, LayerId, Pos2, RawInput, Rect, Shape, Stroke,
    Ui, Vec2, WidgetText, epaint::ClippedShape,
};
use egui_dockyard::{
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
    /// Everything painted this frame, grouped by the layer that painted it.
    ///
    /// `FullOutput::shapes` is one flat list — `end_pass` drains every layer into it and the
    /// layer is gone by the time the frame returns. Read out mid-frame it is still there, and
    /// that is what makes "outside its window" a statement about the *dock's* painting rather
    /// than about whatever else the frame drew.
    by_layer: Vec<(LayerId, Vec<ClippedShape>)>,
    /// The area the `DockArea` was given — the main surface's border is drawn against this.
    given: Rect,
}

impl Painted {
    /// Everything painted into the layer of a floating window surface.
    ///
    /// A window's layer holds its frame *and* the dock it hosts, which is exactly the set that
    /// has to stay inside the window.
    fn window_layer(&self, window: SurfaceIndex) -> &[ClippedShape] {
        let id = window_area_id(window);
        self.by_layer
            .iter()
            .find(|(layer, _)| layer.id == id)
            .map(|(_, shapes)| shapes.as_slice())
            .expect("the window was painted this frame")
    }
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
    let mut by_layer = Vec::new();
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
        // Still inside the pass: the paint lists exist until `end_pass` drains them, and
        // draining is what loses the layer each shape was painted into.
        by_layer = layers_painted(ui.ctx());
    });
    // Load-bearing: `TexturesDelta` panics when dropped with deltas nobody applied, and there is
    // no GPU backend here to apply them. The shapes in `output` are the flat, layerless list —
    // everything this harness reads came from `by_layer` above.
    output.textures_delta.clear();
    Painted { by_layer, given }
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
) -> (DockState<String>, SurfaceIndex, Vec<egui_dockyard::NodeId>) {
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
/// A leaf that cannot hold its own tab bar must show a *cut* tab bar; nothing may escape the
/// rectangle the layout gave it, and nothing may escape the window around that. What used to
/// happen instead is that the bottom row was painted straight through the window's border and
/// out onto the desktop, because `Ui::set_clip_rect` **replaces** the clip rectangle rather than
/// intersecting it: the tab bar's own clip silently undid the leaf's.
///
/// # The scene has to make the clip do work
///
/// This started life on a window squeezed below the height its collapsed rows need — and that
/// scene stopped meaning anything the moment the strip arithmetic was fixed, because a collapsed
/// window is *sized from its strip*: asking for 40 px gets a 63 px window that fits its rows, and
/// there is nothing left to cut. Reintroducing the clip bug left it green. A scene has to rest on
/// something the fix does not remove, so this one squeezes the leaf from the other side — a tab
/// bar taller than half the window it lives in. The window keeps the size it was given, the leaf
/// keeps its half of it, and the tab bar wants more than either: [`SQUEEZED_TAB_BAR`]. The
/// premise is asserted below rather than assumed, so that a layout which ever *does* refuse to
/// make a leaf that short says so, instead of passing for free.
///
/// # Everything, not just the text
///
/// The check used to be on *text*, because a galley carries its string and an escapee could be
/// named rather than described as "something at y = 612" — and because `FullOutput::shapes` is
/// one flat list with no layer on it, so "this rectangle is outside that window" could not be
/// told apart from a rectangle that has every right to be there. The tab bar's fill, the buttons,
/// the body's stroke all went unchecked, which is most of what the bug actually drew.
///
/// Attribution turned out to be there already: `end_pass` is what flattens the layers, and until
/// it runs, `Context::graphics` still hands back a paint list *per layer*. A window surface's
/// layer holds its frame and the dock inside it and nothing else, so the property can be stated
/// over the whole of it — see [`layers_painted`].
///
/// The one thing painted in that layer and meant to fall outside is the window's **shadow**,
/// which is a blurred rectangle by construction; blur is what identifies it, not a name.
#[test]
fn a_window_paints_nothing_outside_itself() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let mut style = style();
    style.tab_bar.height = SQUEEZED_TAB_BAR;

    let mut state = DockState::new(vec![tab("main")]);
    let window = state.add_window(vec![tab("row 0")]);
    let top = state[window].root().unwrap();
    let [_, bottom] = state.split(
        NodePath::new(window, top),
        Split::Below,
        0.5,
        Node::leaf(tab("row 1")),
    );
    state
        .get_window_state_mut(window)
        .unwrap()
        .set_size(egui_dockyard::geom::Size::new(320.0, SQUEEZED_WINDOW));

    let painted_frame = settle(&ctx, &mut state, id, &style);
    let outer = window_area_rect(&ctx, window).expand(TOLERANCE);

    // The premise: both leaves really are shorter than the tab bar they have to draw.
    let layout = DockLayout::load(&ctx, id);
    for leaf in [top, bottom] {
        let rect = layout
            .rect(NodePath::new(window, leaf))
            .expect("the leaf was laid out");
        assert!(
            rect.height() + TOLERANCE < style.tab_bar.height,
            "{leaf:?} came out {} px tall and its tab bar is {} px, so nothing has to be cut \
             and the scan below would pass however the clip behaved",
            rect.height(),
            style.tab_bar.height
        );
    }

    let painted = visible_shapes(painted_frame.window_layer(window));
    // A scanner has to say how much it read: the loop below holds vacuously over an empty list,
    // and a layer read after the drain, or read under the wrong id, is empty.
    assert!(
        painted.len() >= 4,
        "the window's layer held {} shapes, and two leaves cannot be drawn in that few — the \
         layer was read empty, and the loop below proved nothing: {painted:#?}",
        painted.len()
    );
    println!(
        "checked {} shapes painted in the window's layer",
        painted.len()
    );

    for (what, rect) in &painted {
        assert!(
            outer.contains_rect(*rect),
            "{what} was painted at {rect:?}, outside its window at {outer:?}"
        );
    }
}

/// A window short enough that half of it cannot hold [`SQUEEZED_TAB_BAR`].
///
/// Both numbers are far enough apart that the overflow is tens of pixels rather than a rounding
/// argument: the window's content is about 106 px, so each of the two leaves gets about 53, and
/// a tab bar that wants 90 reaches some 30 px past the window's own border.
const SQUEEZED_WINDOW: f32 = 120.0;
const SQUEEZED_TAB_BAR: f32 = 90.0;

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
///
/// The scene rounds **one** corner, which is what makes the bound two-sided. That the content
/// clears the arc says the clearance is big enough; that the three square corners cost nothing
/// beyond the stroke says it is not simply the largest radius charged to all four sides — a
/// rounded title corner should not levy a strip along the bottom of the window.
#[test]
fn a_surface_does_not_cover_the_border_it_draws() {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let mut style = style();
    let stroke = 3.0;
    let radius = 14.0_f32;
    style.main_surface_border_stroke = Stroke::new(stroke, egui::Color32::RED);
    style.main_surface_border_rounding = CornerRadius {
        nw: radius as u8,
        ne: 0,
        sw: 0,
        se: 0,
    };

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

    let rounded = stroke + radius * (1.0 - 1.0 / 2.0_f32.sqrt());
    assert!(
        content.min.x + TOLERANCE >= border.min.x + rounded
            && content.min.y + TOLERANCE >= border.min.y + rounded,
        "the content starts at {:?}, inside a {radius} px rounded corner at {:?} that needs \
         {rounded} px of clearance",
        content.min,
        border.min
    );
    let square = border.max - Vec2::splat(stroke);
    assert!(
        (content.max.x - square.x).abs() <= TOLERANCE
            && (content.max.y - square.y).abs() <= TOLERANCE,
        "the content ends at {:?} where the two square corners there ask for {square:?}: either \
         it is painting over the stroke, or it is paying for an arc at the other end of the \
         rectangle",
        content.max
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
    leaf: egui_dockyard::NodeId,
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

/// The id `show_window_surface` gives a window surface's `Area`.
///
/// Deliberately frozen at the shape the old positional `SurfaceIndex` printed, because egui
/// persists window geometry under it. It names both the area (its rectangle) and the layer
/// (what was painted into it).
fn window_area_id(window: SurfaceIndex) -> Id {
    let index = match window {
        SurfaceIndex::Window(index) => index.0 + 1,
        SurfaceIndex::Main => panic!("the main surface is not an egui window"),
    };
    Id::new(format!("window SurfaceIndex({index})"))
}

/// The outer rectangle egui remembers for a window surface, frame and all.
fn window_area_rect(ctx: &Context, window: SurfaceIndex) -> Rect {
    ctx.memory(|memory| memory.area_rect(window_area_id(window)))
        .expect("the window was shown this frame")
}

/// Everything a paint list covers on screen: each shape's own rectangle cut down by the clip
/// rectangle it was painted under, named well enough for a failure to be read.
///
/// Two kinds of shape are left out, and neither is a hole in the property:
///
/// * a **blurred** rectangle is a drop shadow — it is drawn outside its window on purpose, and
///   blur is what says so (nothing else in the dock paints with it);
/// * a shape that covers nothing (`Shape::Noop`, a transparent rectangle, or one clipped away
///   entirely) has no position to be wrong about.
fn visible_shapes(shapes: &[ClippedShape]) -> Vec<(String, Rect)> {
    fn kind(shape: &Shape) -> String {
        match shape {
            Shape::Text(text) => format!("the text {:?}", text.galley.text()),
            Shape::Rect(rect) => format!(
                "a rectangle (fill {:?}, stroke {} px)",
                rect.fill, rect.stroke.width
            ),
            Shape::LineSegment { .. } => "a line".to_owned(),
            Shape::Path(_) => "a path".to_owned(),
            Shape::Circle(_) => "a circle".to_owned(),
            Shape::Mesh(_) => "a mesh".to_owned(),
            other => format!("{other:?}"),
        }
    }

    fn walk(shape: &Shape, clip: Rect, out: &mut Vec<(String, Rect)>) {
        match shape {
            Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, clip, out);
                }
            }
            Shape::Noop => (),
            // The window's own shadow, drawn around the outside of the frame by design.
            Shape::Rect(rect) if rect.blur_width > 0.0 => (),
            shape => {
                let rect = shape.visual_bounding_rect().intersect(clip);
                if rect.is_positive() {
                    out.push((kind(shape), rect));
                }
            }
        }
    }

    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, clipped.clip_rect, &mut out);
    }
    out
}
