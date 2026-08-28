//! What stowing a whole side does to the layout, stated on the rectangles.
//!
//! # Why this is a test file of its own
//!
//! `collapse_sideways` (v1, pinned next door in `a_collapsed_leaf_can_hide_sideways.rs`) can
//! squeeze a collapsed **leaf** against the edge of its split and hand the width to the sibling
//! column. It deliberately refuses to do that for a **split**: a collapsed split is rows of tab
//! bars, one per leaf, and rows do not fit in a strip one tab bar wide.
//!
//! Stowing is the answer to that refusal, and it is a different question rather than a relaxation
//! of the same one. A stowed split is not "all of its leaves happen to be collapsed" — it is one
//! object put away behind one arrow, drawing a single bar for whatever it contains, and keeping
//! its insides exactly as they were for when it comes back. So the rows are not there to be
//! fitted, and the strip has one thing to draw.
//!
//! What that makes true, and what this file states:
//!
//! * the side becomes a strip and the sibling takes the width — the subtree version of
//!   `a_collapsed_leaf_beside_a_column_becomes_a_strip`;
//! * **nothing inside it is laid out at all** — not laid out smaller, not laid out off-screen:
//!   there is no rectangle, and no divider between children that are not on screen. Everything
//!   downstream is written to ask the layout rather than work the answer out for itself, so an
//!   entry left over from before the side was put away *is* what drawing would believe;
//! * under a *vertical* parent it is one bar rather than a strip, which is the row count of a
//!   stowed split (1, whatever it contains) arriving at the layout;
//! * and it all comes back.

use egui::{
    CentralPanel, Context, Event, Id, PointerButton, Pos2, RawInput, Rect, Ui, Vec2, WidgetText,
};
use egui_dockyard::{
    DockArea, DockLayout, DockState, Node, NodeId, NodePath, SideStrip, Split, Style, SurfaceIndex,
    TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_side_can_be_stowed";

/// Half a device pixel at the default scale: every boundary is snapped to whole pixels, so an
/// exact comparison would be reporting the snapping rather than the property.
const TOLERANCE: f32 = 0.5;

/// Records which tab bodies were drawn, so a frame can be asked what it *painted* rather than
/// what the tree said afterwards. "Nothing inside a stowed side is drawn" is a statement about
/// the frame, and the tree cannot answer it: the leaves are still there, still expanded.
#[derive(Default)]
struct Viewer {
    drawn: Vec<String>,
}

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> WidgetText {
        tab.clone().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &Self::Tab) {
        self.drawn.push(tab.clone());
        ui.label(tab.as_str());
    }
}

fn tab(name: &str) -> String {
    name.to_owned()
}

fn style() -> Style {
    Style::from_egui(&egui::Style::default())
}

fn path(node: NodeId) -> NodePath {
    NodePath::new(SurfaceIndex::main(), node)
}

/// One headless frame carrying `events`, answering with the tab bodies it painted.
fn frame(
    ctx: &Context,
    state: &mut DockState<String>,
    style: &Style,
    sideways: bool,
    events: Vec<Event>,
) -> Vec<String> {
    let mut viewer = Viewer::default();
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        CentralPanel::default().show(ui, |ui| {
            DockArea::new(state)
                .id(Id::new(DOCK_ID))
                .style(style.clone())
                .show_leaf_collapse_buttons(true)
                .collapse_sideways(sideways)
                .show_inside(ui, &mut viewer);
        });
    });
    output.textures_delta.clear();
    viewer.drawn
}

/// A few quiet frames in a context of your own, for when the *same* context has to see two
/// states in a row — the geometry map lives in its memory and outlives a frame, which is the
/// only way "the entry from before is gone" can be asked at all. Answers with what the last of
/// them painted, once the layout has settled.
fn frames(
    ctx: &Context,
    state: &mut DockState<String>,
    style: &Style,
    sideways: bool,
) -> Vec<String> {
    let mut drawn = Vec::new();
    for _ in 0..4 {
        drawn = frame(ctx, state, style, sideways, Vec::new());
    }
    drawn
}

/// Press and release at `at`, answering with what the *release* frame painted — the frame that
/// answers the click, and still paints the picture the click asked to change (see
/// `a_click_that_changes_a_leaf_lands_next_frame.rs`).
fn click(
    ctx: &Context,
    state: &mut DockState<String>,
    style: &Style,
    sideways: bool,
    at: Pos2,
) -> Vec<String> {
    for pressed in [true, false] {
        let event = Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let drawn = frame(ctx, state, style, sideways, vec![event]);
        if !pressed {
            return drawn;
        }
    }
    unreachable!("the release frame returns")
}

fn layout_of(ctx: &Context) -> DockLayout {
    DockLayout::load(ctx, Id::new(DOCK_ID))
}

fn rect_of(layout: &DockLayout, node: NodeId) -> Rect {
    layout.rect(path(node)).expect("the node was laid out")
}

fn side_strip_of(layout: &DockLayout, node: NodeId) -> Option<SideStrip> {
    layout.side_strip(path(node))
}

/// A row of two: one half an ordinary leaf, the other a **split** of two stacked leaves — the
/// shape v1 refuses to squeeze. Nothing is stowed yet; the tests do that themselves, because the
/// interesting scene is the transition.
struct Scene {
    state: DockState<String>,
    /// The split over the two halves — the row the widths have to add up to.
    root: NodeId,
    /// The half that is a split of two leaves: the side that gets put away.
    side: NodeId,
    /// The two leaves inside it, top then bottom.
    inside: [NodeId; 2],
    /// The ordinary leaf beside it, which is supposed to take the width.
    open: NodeId,
}

/// `side_on_left` picks which half is the split, because the layout has a branch for each edge
/// and they were written twice — the arrangement in which one of the two quietly says something
/// slightly different.
fn a_side_beside_a_column(side_on_left: bool) -> Scene {
    let mut state = DockState::new(vec![tab("open")]);
    let open = state.main_surface().root().unwrap();
    let [_, first] = state.split(
        path(open),
        if side_on_left {
            Split::Left
        } else {
            Split::Right
        },
        0.5,
        Node::leaf(tab("side top")),
    );
    let [_, second] = state.split(
        path(first),
        Split::Below,
        0.5,
        Node::leaf(tab("side bottom")),
    );

    let side = state
        .main_surface()
        .parent(first)
        .expect("the two stacked leaves have a parent");
    let root = state
        .main_surface()
        .parent(side)
        .expect("the stack sits beside a column");
    Scene {
        state,
        root,
        side,
        inside: [first, second],
        open,
    }
}

/// The side becomes a strip one tab bar thick, and everything it gave up went to its sibling.
///
/// The same property as for a single leaf, and asserted the same way — *nothing is left over* —
/// because that is what the feature is for. What differs is only what the strip stands for.
#[test]
fn a_stowed_side_beside_a_column_becomes_a_strip() {
    let style = style();

    for side_on_left in [true, false] {
        let mut scene = a_side_beside_a_column(side_on_left);
        scene
            .state
            .main_surface_mut()
            .set_split_stowed(scene.side, true);

        let ctx = Context::default();
        frames(&ctx, &mut scene.state, &style, true);
        let layout = layout_of(&ctx);

        let (root_rect, side_rect, open_rect) = (
            rect_of(&layout, scene.root),
            rect_of(&layout, scene.side),
            rect_of(&layout, scene.open),
        );

        assert!(
            (side_rect.width() - style.tab_bar.height).abs() <= TOLERANCE,
            "a stowed side got {} px for a {} px strip",
            side_rect.width(),
            style.tab_bar.height
        );

        // No hole: the strip, its sibling and the divider are still the whole row.
        let covered = side_rect.width() + open_rect.width() + style.separator.width;
        assert!(
            (covered - root_rect.width()).abs() <= TOLERANCE,
            "the stowed side, its sibling and the divider cover {} px of a {} px row: the \
             difference is an area with no tab bar, no body and no owner",
            covered,
            root_rect.width()
        );

        // And it is pressed against the edge it belongs to, not floating in the middle.
        let (outer_edge, strip_edge) = if side_on_left {
            (root_rect.min.x, side_rect.min.x)
        } else {
            (root_rect.max.x, side_rect.max.x)
        };
        assert!(
            (outer_edge - strip_edge).abs() <= TOLERANCE,
            "the strip sits at {} but the edge of its split is at {}",
            strip_edge,
            outer_edge
        );

        assert_eq!(
            side_strip_of(&layout, scene.side),
            Some(if side_on_left {
                SideStrip::Left
            } else {
                SideStrip::Right
            }),
            "drawing reads the side off the layout, so the layout has to have said it"
        );
    }
}

/// Stowing a side takes its insides off the map: no rectangle, and no divider between them.
///
/// Asked of a context that has already drawn the side **open**, which is the only arrangement in
/// which it can fail. From a fresh context the subtree has no entries to begin with and every
/// assertion below passes without the layout doing anything.
///
/// Both halves matter. A leftover *rectangle* is where drawing would put a tab body — it asks the
/// layout instead of deciding for itself, so a stale entry is not an unread number, it is the
/// answer. A leftover *divider* is worse than stale: the line between two children of a split
/// that is now one strip lies across the strip, visible and grabbable, and grabbing it writes the
/// fraction the side is keeping for when it comes back. That is the bug this crate has already
/// paid for once, in `a_hidden_half_has_no_boundary_to_drag`.
#[test]
fn stowing_a_side_takes_its_insides_off_the_map() {
    let style = style();
    let mut scene = a_side_beside_a_column(false);
    let ctx = Context::default();

    frames(&ctx, &mut scene.state, &style, true);
    let layout = layout_of(&ctx);
    // The scene this test is about was actually reached: the insides were on the map before.
    for node in scene.inside {
        assert!(
            layout.get(path(node)).is_some(),
            "the leaves inside the side have to be laid out before stowing can take them off"
        );
    }
    assert!(
        layout.divider(path(scene.side)).is_some(),
        "and the side has to have had a divider to lose"
    );

    scene
        .state
        .main_surface_mut()
        .set_split_stowed(scene.side, true);
    frames(&ctx, &mut scene.state, &style, true);
    let layout = layout_of(&ctx);

    for node in scene.inside {
        assert_eq!(
            layout.get(path(node)),
            None,
            "a leaf inside a stowed side kept its geometry from before it was put away, and \
             drawing believes the layout: its tab body lands inside the strip"
        );
    }
    assert_eq!(
        layout.divider(path(scene.side)),
        None,
        "the stowed side kept the divider between two children that are no longer on screen — a \
         line lying across the strip, which moves nothing and writes the fraction when dragged"
    );
    // The side itself is emphatically still on the map: it is the strip.
    assert!(
        (rect_of(&layout, scene.side).width() - style.tab_bar.height).abs() <= TOLERANCE,
        "the side itself must keep its rectangle — it is what is drawn"
    );
}

/// A side stowed under a *vertical* parent is a single bar, not a strip.
///
/// It costs one row whatever it contains, which is `update_split_collapsed` answering 1 for a
/// stowed split; this is that number arriving at the layout. The distinction from the case above
/// is the parent's orientation and nothing else — a collapsed thing spends height under a
/// vertical split and width under a horizontal one.
#[test]
fn a_stowed_side_under_a_vertical_parent_is_one_bar() {
    let style = style();

    let mut state = DockState::new(vec![tab("top")]);
    let top = state.main_surface().root().unwrap();
    let [_, first] = state.split(path(top), Split::Below, 0.5, Node::leaf(tab("side left")));
    state.split(
        path(first),
        Split::Right,
        0.5,
        Node::leaf(tab("side right")),
    );

    let side = state
        .main_surface()
        .parent(first)
        .expect("the two side-by-side leaves have a parent");
    let root = state
        .main_surface()
        .parent(side)
        .expect("the pair sits under the top leaf");
    state.main_surface_mut().set_split_stowed(side, true);

    let ctx = Context::default();
    frames(&ctx, &mut state, &style, true);
    let layout = layout_of(&ctx);

    let (root_rect, side_rect, top_rect) = (
        rect_of(&layout, root),
        rect_of(&layout, side),
        rect_of(&layout, top),
    );

    assert!(
        (side_rect.height() - style.tab_bar.height).abs() <= TOLERANCE,
        "a stowed side under a vertical parent got {} px for a {} px bar: it draws one bar for \
         whatever it contains, so it costs one row",
        side_rect.height(),
        style.tab_bar.height
    );
    let covered = side_rect.height() + top_rect.height() + style.separator.width;
    assert!(
        (covered - root_rect.height()).abs() <= TOLERANCE,
        "the bar, the leaf above it and the divider cover {} px of a {} px column",
        covered,
        root_rect.height()
    );
    assert_eq!(
        side_strip_of(&layout, side),
        None,
        "a bar is not a sideways strip: the arrow on it points the other way"
    );
}

/// Bringing the side back lays its insides out again, where they were.
///
/// The drawn half of `round_trip_keeps_a_subtree_stowed_and_its_insides_untouched`, which states
/// the model half. Taking the geometry off the map is not losing anything: it is derived from the
/// tree every frame, and the tree kept the fraction.
#[test]
fn bringing_a_stowed_side_back_lays_its_insides_out_again() {
    let style = style();
    let mut scene = a_side_beside_a_column(false);
    let ctx = Context::default();

    frames(&ctx, &mut scene.state, &style, true);
    let before: Vec<Rect> = scene
        .inside
        .iter()
        .map(|node| rect_of(&layout_of(&ctx), *node))
        .collect();

    scene
        .state
        .main_surface_mut()
        .set_split_stowed(scene.side, true);
    frames(&ctx, &mut scene.state, &style, true);
    assert_eq!(
        layout_of(&ctx).get(path(scene.inside[0])),
        None,
        "the side has to have been put away before bringing it back proves anything"
    );

    scene
        .state
        .main_surface_mut()
        .set_split_stowed(scene.side, false);
    frames(&ctx, &mut scene.state, &style, true);
    let layout = layout_of(&ctx);

    for (node, was) in scene.inside.iter().zip(before) {
        let now = rect_of(&layout, *node);
        assert!(
            (now.min - was.min).length() <= TOLERANCE && (now.max - was.max).length() <= TOLERANCE,
            "a leaf inside the side came back at {now:?} having been at {was:?}"
        );
    }
    assert_eq!(
        side_strip_of(&layout, scene.side),
        None,
        "an unstowed side is still marked as a strip: the flag went stale"
    );
    assert!(
        layout.divider(path(scene.side)).is_some(),
        "and its two children have a boundary between them again"
    );
}

/// Nothing inside a stowed side is drawn.
///
/// The frame has to be asked, not the tree: the leaves inside are still there and still
/// expanded — that is the whole point of stowing as a unit — so `is_collapsed` on any of them
/// says no. What changed is that they are not on screen, and only what was painted can say so.
#[test]
fn nothing_inside_a_stowed_side_is_drawn() {
    let style = style();
    let mut scene = a_side_beside_a_column(false);
    let ctx = Context::default();

    let drawn = frames(&ctx, &mut scene.state, &style, true);
    assert_eq!(
        drawn.len(),
        3,
        "all three leaves draw a body before the side is put away, or the scene below proves \
         nothing: painted {drawn:?}"
    );

    scene
        .state
        .main_surface_mut()
        .set_split_stowed(scene.side, true);
    let drawn = frames(&ctx, &mut scene.state, &style, true);

    assert_eq!(
        drawn,
        vec!["open".to_owned()],
        "a stowed side painted {drawn:?}: its leaves are expanded and its subtree is off the \
         map, so anything drawn for them landed on top of the strip or of its sibling"
    );
    // And the leaves really are untouched — this is not "everything got collapsed".
    for node in scene.inside {
        assert!(
            !scene.state[path(node)].is_collapsed(),
            "stowing collapsed a leaf inside the side, which is the spelling this feature exists \
             to avoid: it would come back expanded"
        );
    }
}

/// The arrow on the strip brings the side back.
///
/// One arrow for the whole side, and it is the same `tab_collapse` button as everywhere else —
/// given the split's path, and queuing the edit that suits a split. Which is the point of the
/// click going through a mutation the caller hands in: `set_leaf_collapsed` panics on a split,
/// so a button that decided for itself would have had to learn what it was sitting on.
#[test]
fn the_arrow_on_a_stowed_side_brings_it_back() {
    let style = style();
    let mut scene = a_side_beside_a_column(false);
    scene
        .state
        .main_surface_mut()
        .set_split_stowed(scene.side, true);
    let ctx = Context::default();
    frames(&ctx, &mut scene.state, &style, true);

    // The arrow sits at the top of the strip, one `TAB_COLLAPSE_BUTTON_SIZE` square — which is
    // the strip's whole width. Its exact size is private to the crate; 8 px in is comfortably
    // inside it.
    let strip = rect_of(&layout_of(&ctx), scene.side);
    let target = Pos2::new(strip.left() + 8.0, strip.top() + style.tab_bar.height / 2.0);

    let during = click(&ctx, &mut scene.state, &style, true, target);
    assert_eq!(
        during,
        vec!["open".to_owned()],
        "the frame the click is answered in still paints the side away, as every other queued \
         edit does — see a_click_that_changes_a_leaf_lands_next_frame.rs"
    );
    assert!(
        !scene.state[path(scene.side)].is_stowed(),
        "clicking the arrow on the strip did not bring the side back"
    );

    let drawn = frames(&ctx, &mut scene.state, &style, true);
    assert_eq!(
        drawn.len(),
        3,
        "the next repaint shows the side again, insides and all: painted {drawn:?}"
    );
}

/// With `collapse_sideways` off, a stowed side keeps its column.
///
/// The positive control. Without it every assertion above would still pass if the knob were
/// ignored and the strip were simply always drawn — and stowing would then change the layout of
/// saved sessions that never asked for it. The side is still *stowed*: one bar's worth of it is
/// drawn, the rest of the column is empty. That is the old behaviour of a collapsed split, which
/// is what the knob being off means.
#[test]
fn the_knob_off_keeps_the_column_for_a_stowed_side() {
    let style = style();
    let mut scene = a_side_beside_a_column(false);
    scene
        .state
        .main_surface_mut()
        .set_split_stowed(scene.side, true);

    let ctx = Context::default();
    frames(&ctx, &mut scene.state, &style, false);
    let layout = layout_of(&ctx);

    let (root_rect, side_rect) = (rect_of(&layout, scene.root), rect_of(&layout, scene.side));
    assert!(
        side_rect.width() > style.tab_bar.height * 2.0,
        "with `collapse_sideways` off a stowed side got {} px of a {} px row: the knob was not \
         read, and layouts that rely on the old behaviour have changed under them",
        side_rect.width(),
        root_rect.width()
    );
    assert_eq!(side_strip_of(&layout, scene.side), None);
}
