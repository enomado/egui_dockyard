//! What `DockArea::collapse_sideways` does to the layout, stated on the rectangles.
//!
//! # Why this is a test file of its own
//!
//! Collapsing spends *height*: a collapsed leaf is a tab bar and nothing else. Under a
//! horizontal split there is nobody to give that height to — the sibling is a column beside it
//! — so a leaf shrunk to a bar would leave an area with no tab bar, no body and no owner. That
//! is why a collapsed leaf beside a column keeps its column, pinned next door in
//! `a_collapsed_leaf_is_one_row.rs`.
//!
//! `collapse_sideways` is the other answer to that same problem: spend **width** instead, which
//! the sibling column *can* take. So the property to state here is not "the strip is narrow" —
//! it is that **nothing is left over**: the two children plus their divider still add up to
//! their parent, exactly as before, with the sibling holding everything the strip gave up.
//!
//! Two cases would bring the hole back, and each one is a test below rather than a comment: a
//! *vertically* collapsed split (its subtree is rows of tab bars, which do not fit in a strip),
//! and the knob being off (the old behaviour, which saved layouts still depend on).
//!
//! A third used to be here — "two collapsed siblings, nobody to take the width" — and it was
//! wrong about a row of three or more. A binary tree writes `a | b | c` as `H(a, H(b, c))`, so
//! collapsing two panels of the row makes them siblings, each reading the other as "nobody",
//! while the open column that could hold the width sits one level out. Collapsing the second
//! panel of a row therefore un-stripped the first as well (Стас, 30.08.2026: "если у нас 3
//! полоски — то сворачиваются 2 вместе"). What the pair-shaped rule stood for is kept by
//! `strip_columns` instead: a side is given exactly the strips it needs, and only when *both*
//! sides are strips — the whole row collapsed, nobody left to take anything — is there space
//! left over, which is then empty by decision rather than by accident.

use egui::{CentralPanel, Context, Id, Pos2, RawInput, Rect, Ui, Vec2, WidgetText};
use egui_dockyard::{
    DockArea, DockLayout, DockState, Node, NodeId, NodePath, SideStrip, Split, Style, SurfaceIndex,
    TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_collapsed_leaf_can_hide_sideways";

/// Half a device pixel at the default scale: every boundary is snapped to whole pixels, so an
/// exact comparison would be reporting the snapping rather than the property.
const TOLERANCE: f32 = 0.5;

struct Viewer;

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> WidgetText {
        tab.clone().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &Self::Tab) {
        ui.label(tab.as_str());
    }
}

fn tab(name: &str) -> String {
    name.to_owned()
}

fn style() -> Style {
    Style::from_egui(&egui::Style::default())
}

/// A few headless frames in a context of your own, for when the *same* context has to see two
/// states in a row — the geometry map lives in its memory and outlives a frame.
fn frames(ctx: &Context, state: &mut DockState<String>, style: &Style, sideways: bool) {
    let id = Id::new(DOCK_ID);
    for _ in 0..4 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            CentralPanel::default().show(ui, |ui| {
                DockArea::new(state)
                    .id(id)
                    .style(style.clone())
                    .show_leaf_collapse_buttons(true)
                    .collapse_sideways(sideways)
                    .show_inside(ui, &mut Viewer);
            });
        });
        output.textures_delta.clear();
    }
}

/// A few headless frames, and the geometry they settled on.
fn run(state: &mut DockState<String>, style: &Style, sideways: bool) -> DockLayout {
    let ctx = Context::default();
    frames(&ctx, state, style, sideways);
    DockLayout::load(&ctx, Id::new(DOCK_ID))
}

fn rect_of(layout: &DockLayout, node: NodeId) -> Rect {
    layout
        .rect(NodePath::new(SurfaceIndex::main(), node))
        .expect("the node was laid out")
}

fn side_strip_of(layout: &DockLayout, node: NodeId) -> Option<SideStrip> {
    layout.side_strip(NodePath::new(SurfaceIndex::main(), node))
}

/// Two leaves side by side, the named one collapsed. Returns (parent, left, right).
fn two_columns(collapse: Collapse) -> (DockState<String>, NodeId, NodeId, NodeId) {
    let mut state = DockState::new(vec![tab("left")]);
    let left = state.main_surface().root().unwrap();
    let [_, right] = state.split(
        NodePath::new(SurfaceIndex::main(), left),
        Split::Right,
        0.5,
        Node::leaf(tab("right")),
    );
    match collapse {
        Collapse::Left => state.main_surface_mut().set_leaf_collapsed(left, true),
        Collapse::Right => state.main_surface_mut().set_leaf_collapsed(right, true),
        Collapse::Both => {
            state.main_surface_mut().set_leaf_collapsed(left, true);
            state.main_surface_mut().set_leaf_collapsed(right, true);
        }
    }
    let parent = state
        .main_surface()
        .parent(left)
        .expect("the two leaves have a parent");
    (state, parent, left, right)
}

enum Collapse {
    Left,
    Right,
    Both,
}

/// Three leaves in a row, left to right — one row of three.
///
/// It used to be two fixtures, `RowShape::RightHeavy` (`H(a, H(b, c))`) and `LeftHeavy`
/// (`H(H(a, b), c)`), and every row test here ran against both: the nesting was *not* something
/// the user chose — it recorded the order the panels were split off in — and a rule that read the
/// tree pair by pair passed for one spelling and failed for the other. That is exactly how the
/// bug this file is about stayed hidden.
///
/// Since stage 7 of `docs/PLAN_a_row_holds_many_panels.md` the two spellings **are the same
/// tree**: splitting the same way twice joins the row rather than nesting inside it. The
/// property the double fixture was defending is now held by construction, and keeping it would
/// mean running every scene twice on one shape while claiming to run it on two.
fn three_columns() -> (DockState<String>, [NodeId; 3]) {
    let mut state = DockState::new(vec![tab("a")]);
    let a = state.main_surface().root().unwrap();
    let [_, b] = state.split(
        NodePath::new(SurfaceIndex::main(), a),
        Split::Right,
        0.5,
        Node::leaf(tab("b")),
    );
    let [_, c] = state.split(
        NodePath::new(SurfaceIndex::main(), b),
        Split::Right,
        0.5,
        Node::leaf(tab("c")),
    );
    (state, [a, b, c])
}

fn row_rect(layout: &DockLayout, state: &DockState<String>) -> Rect {
    rect_of(layout, state.main_surface().root().unwrap())
}

/// The strip is one tab bar thick, and everything it gave up went to its sibling.
///
/// Both sides, because the layout has a branch for each — "the strip is the left edge of the
/// node" and "the strip is the right edge of it". They are mirror images and were written
/// twice, which is the arrangement in which one of the two quietly says something slightly
/// different; here, which side of the boundary the divider is taken out of.
#[test]
fn a_collapsed_leaf_beside_a_column_becomes_a_strip() {
    let style = style();

    for collapse_left in [true, false] {
        let (mut state, parent, left, right) = two_columns(if collapse_left {
            Collapse::Left
        } else {
            Collapse::Right
        });
        let layout = run(&mut state, &style, true);

        let (parent_rect, left_rect, right_rect) = (
            rect_of(&layout, parent),
            rect_of(&layout, left),
            rect_of(&layout, right),
        );
        let (strip, open) = if collapse_left {
            (left_rect, right_rect)
        } else {
            (right_rect, left_rect)
        };

        assert!(
            (strip.width() - style.tab_bar.height).abs() <= TOLERANCE,
            "a leaf collapsed sideways got {} px for a {} px strip",
            strip.width(),
            style.tab_bar.height
        );

        // The whole point: no hole. What the strip gave up is held by the sibling, and the two
        // of them plus the divider are still the parent.
        let covered = strip.width() + open.width() + style.separator.width;
        assert!(
            (covered - parent_rect.width()).abs() <= TOLERANCE,
            "the strip, its sibling and the divider cover {} px of a {} px row: the difference \
             is an area with no tab bar, no body and no owner",
            covered,
            parent_rect.width()
        );

        // And it is pressed against the edge it collapsed towards, not floating in the middle.
        let (outer_edge, strip_edge) = if collapse_left {
            (parent_rect.min.x, strip.min.x)
        } else {
            (parent_rect.max.x, strip.max.x)
        };
        assert!(
            (outer_edge - strip_edge).abs() <= TOLERANCE,
            "the strip sits at {} but the edge of its split is at {}",
            strip_edge,
            outer_edge
        );

        let expected_side = if collapse_left {
            SideStrip::Left
        } else {
            SideStrip::Right
        };
        let (collapsed_node, open_node) = if collapse_left {
            (left, right)
        } else {
            (right, left)
        };
        assert_eq!(
            side_strip_of(&layout, collapsed_node),
            Some(expected_side),
            "drawing reads the side off the layout, so the layout has to have said it"
        );
        assert_eq!(
            side_strip_of(&layout, open_node),
            None,
            "the open sibling is not a strip"
        );
    }
}

/// Two collapsed siblings are two strips, side by side against the near edge, and the width
/// they gave up is left empty.
///
/// This is the one place in the feature where a hole is the answer: with the whole row
/// collapsed there is no open column to hand the width to, and the alternatives are worse —
/// keeping the columns means collapsing the second panel visibly *undoes* the first (the bug
/// this replaced), and stretching the last strip to fill the row means the thing labelled "a
/// strip" is not one. Decision of 30.08.2026, Стас: strips for everyone, the rest empty.
///
/// The strips go against the *near* edge, one after the other, so the empty part stands beside
/// them instead of separating them from each other.
#[test]
fn two_collapsed_siblings_become_two_strips() {
    let style = style();
    let (mut state, parent, left, right) = two_columns(Collapse::Both);
    let layout = run(&mut state, &style, true);

    let (parent_rect, left_rect, right_rect) = (
        rect_of(&layout, parent),
        rect_of(&layout, left),
        rect_of(&layout, right),
    );

    for (name, rect) in [("left", left_rect), ("right", right_rect)] {
        assert!(
            (rect.width() - style.tab_bar.height).abs() <= TOLERANCE,
            "the {name} leaf of a fully collapsed row got {} px for a {} px strip",
            rect.width(),
            style.tab_bar.height
        );
    }
    assert!(
        (left_rect.min.x - parent_rect.min.x).abs() <= TOLERANCE,
        "the first strip sits at {} but its row starts at {}",
        left_rect.min.x,
        parent_rect.min.x
    );
    assert!(
        (right_rect.min.x - (left_rect.max.x + style.separator.width)).abs() <= TOLERANCE,
        "the second strip starts at {} rather than right after the first ({} + a divider): the \
         empty part of the row got in between them",
        right_rect.min.x,
        left_rect.max.x
    );
    // And the leftover really is leftover — this is the scene the decision is about, so it has
    // to be reached rather than assumed.
    assert!(
        right_rect.max.x < parent_rect.max.x - style.tab_bar.height,
        "nothing was left over: the two strips end at {} of a row ending at {}",
        right_rect.max.x,
        parent_rect.max.x
    );
    assert_eq!(side_strip_of(&layout, left), Some(SideStrip::Left));
    assert_eq!(side_strip_of(&layout, right), Some(SideStrip::Left));
}

/// Collapsing the second panel of a row of three does not un-collapse the first: two strips
/// stand beside the one open column, whichever of the three stayed open and whichever way the
/// tree was written.
///
/// The regression this file's header describes. Two of three collapsed always makes some pair
/// of them siblings — which pair depends on the shape and on which one stayed open — and the
/// old rule refused a strip to both members of any such pair, so the first panel visibly came
/// back when the second was collapsed. Six scenes rather than one because the failure needs
/// *a* pair to exist, and each shape hides it for a different choice of open panel.
#[test]
fn two_of_three_collapsed_are_two_strips_beside_one_column() {
    let style = style();

    {
        for open in 0..3 {
            let (mut state, nodes) = three_columns();
            for (i, node) in nodes.iter().enumerate() {
                if i != open {
                    state.main_surface_mut().set_leaf_collapsed(*node, true);
                }
            }
            let layout = run(&mut state, &style, true);
            let row = row_rect(&layout, &state);
            let rects = nodes.map(|node| rect_of(&layout, node));

            for (i, rect) in rects.iter().enumerate() {
                if i == open {
                    continue;
                }
                assert!(
                    (rect.width() - style.tab_bar.height).abs() <= TOLERANCE,
                    "panel {open} open: panel {i} got {} px instead of a {} px strip \
                     — collapsing one panel of a row took the strip away from another",
                    rect.width(),
                    style.tab_bar.height
                );
                assert!(
                    side_strip_of(&layout, nodes[i]).is_some(),
                    "panel {open} open: panel {i} is narrow but the layout did not \
                     call it a strip, so it will draw a tab bar with no room for one"
                );
            }

            // No hole: the open column holds everything the two strips gave up.
            let covered =
                rects.iter().map(|rect| rect.width()).sum::<f32>() + 2.0 * style.separator.width;
            assert!(
                (covered - row.width()).abs() <= TOLERANCE,
                "panel {open} open: the row covers {covered} px of {} px",
                row.width()
            );
            assert_eq!(
                side_strip_of(&layout, nodes[open]),
                None,
                "the open panel {open} is not a strip"
            );
            // Left to right, as they were built: a strip belongs to the panel it collapsed
            // from, and a row that reorders itself when two of it collapse is a different bug
            // wearing the same numbers.
            assert!(
                rects[0].min.x < rects[1].min.x && rects[1].min.x < rects[2].min.x,
                "panel {open} open: the row came out in the order {:?}",
                rects.map(|rect| rect.min.x)
            );
        }
    }
}

/// A row with *everything* collapsed is a row of strips against its near edge, and the rest of
/// it is empty.
///
/// The deliberate hole — see `two_collapsed_siblings_become_two_strips` for why it is the
/// answer here. Stated on three panels as well as two because the strips are laid out by a
/// recursion: the second one is placed by a split that was itself given exactly its strips, and
/// a rule that only lines up a pair would leave the third one somewhere else entirely.
#[test]
fn a_fully_collapsed_row_is_a_row_of_strips() {
    let style = style();

    {
        let (mut state, nodes) = three_columns();
        for node in nodes {
            state.main_surface_mut().set_leaf_collapsed(node, true);
        }
        let layout = run(&mut state, &style, true);
        let row = row_rect(&layout, &state);
        let rects = nodes.map(|node| rect_of(&layout, node));

        for (i, rect) in rects.iter().enumerate() {
            assert!(
                (rect.width() - style.tab_bar.height).abs() <= TOLERANCE,
                "panel {i} of a fully collapsed row got {} px instead of a {} px strip",
                rect.width(),
                style.tab_bar.height
            );
            assert!(
                side_strip_of(&layout, nodes[i]).is_some(),
                "panel {i} of a fully collapsed row is not marked as a strip"
            );
        }
        assert!(
            (rects[0].min.x - row.min.x).abs() <= TOLERANCE,
            "the row of strips starts at {} rather than at its row's edge {}",
            rects[0].min.x,
            row.min.x
        );
        for i in 1..3 {
            assert!(
                (rects[i].min.x - (rects[i - 1].max.x + style.separator.width)).abs() <= TOLERANCE,
                "strip {i} starts at {} rather than right after strip {} ({} + a \
                 divider) — the empty part of the row got in between the strips",
                rects[i].min.x,
                i - 1,
                rects[i - 1].max.x
            );
        }
        assert!(
            rects[2].max.x < row.max.x - style.tab_bar.height,
            "nothing was left empty — three strips ended at {} of a row ending at {}",
            rects[2].max.x,
            row.max.x
        );
    }
}

/// A row of strips is marked leaf by leaf, and the split holding them is not marked at all.
///
/// Drawing asks the layout "am I a strip?" and answers by drawing one bar with one arrow. A row
/// holding collapsed panels is their *width* but not their *bar*: mark it and the row draws a
/// single arrow for two panels — which is what stowing means, and stowing is a state the user
/// sets deliberately, not something a pair of ordinary collapses should turn into behind their
/// back.
#[test]
fn a_row_of_strips_marks_its_leaves_not_the_split() {
    let style = style();

    let (mut state, [a, b, c]) = three_columns();
    state.main_surface_mut().set_leaf_collapsed(b, true);
    state.main_surface_mut().set_leaf_collapsed(c, true);
    let row = state
        .main_surface()
        .parent(b)
        .expect("the panels have a parent");

    let layout = run(&mut state, &style, true);

    assert_eq!(
        side_strip_of(&layout, row),
        None,
        "the row holding two strips was marked as a strip itself, so it draws one bar and one \
         arrow for both panels — silently the same as stowing it"
    );
    // `Right`, and this is where the mark **changed at stage 7**. The two used to sit in a
    // nested pair of their own, and inside that pair — which was fully collapsed — they were the
    // *leading* run, so both were marked `Left`. On the flat row they are what they always were
    // on screen: the trailing run, stacked from the far edge, with the open column taking the
    // width to their left. The rule (`Run::Trailing` → `Right`) did not move; the tree stopped
    // mis-stating which run they are in.
    assert_eq!(side_strip_of(&layout, b), Some(SideStrip::Right));
    assert_eq!(side_strip_of(&layout, c), Some(SideStrip::Right));

    // The two of them together are exactly two strips and the divider between them, which is
    // the arithmetic `collapsed_strip_width` gained a `columns` parameter for. Measured on the
    // strips rather than on the node above them, which the flat row no longer has.
    let strips = rect_of(&layout, b).union(rect_of(&layout, c));
    let expected = 2.0 * style.tab_bar.height + style.separator.width;
    assert!(
        (strips.width() - expected).abs() <= TOLERANCE,
        "the pair of strips was given {} px where two strips and a divider need {expected}",
        strips.width()
    );
    let whole = rect_of(&layout, row);
    assert!(
        (strips.max.x - whole.max.x).abs() <= TOLERANCE,
        "the strips are stacked from the far edge: they end at {} of a row ending at {}",
        strips.max.x,
        whole.max.x
    );
    assert!(
        rect_of(&layout, a).width() > style.tab_bar.height * 2.0,
        "the open column did not take the width the two strips gave up"
    );
}

/// A *vertically* collapsed — but not *stowed* — split beside a column keeps the column.
///
/// A vertically collapsed split is a stack of collapsed leaves, i.e. rows of tab bars. A strip
/// is one tab bar thick measured the other way, so those rows have nowhere to be drawn; the
/// subtree keeps its column and each of its leaves stays a row, exactly as without the knob.
///
/// The axis is the whole of it, and this is the test that says so: a *horizontally* collapsed
/// split is a row of strips and does become one (`a_row_of_strips_marks_its_leaves_not_the_split`),
/// because side by side is how strips stack. Get the two confused and a stack of collapsed tab
/// bars is squeezed into a column one bar wide, where all but the first of them is cut off.
///
/// Since `SplitNode::stowed` there is a second way for a split to be collapsed, and it becomes a
/// strip whichever way it is split (`a_side_can_be_stowed.rs`) — because a side put away as a
/// unit draws one bar for whatever it contains, so there are no rows to fit. Stowed and
/// collapsed-leaf-by-leaf are told apart by how they got that way, not by how they look to
/// `is_collapsed`.
#[test]
fn a_vertically_collapsed_split_beside_a_column_keeps_the_column() {
    let style = style();

    let mut state = DockState::new(vec![tab("top")]);
    let top = state.main_surface().root().unwrap();
    state.split(
        NodePath::new(SurfaceIndex::main(), top),
        Split::Right,
        0.5,
        Node::leaf(tab("right")),
    );
    let [_, bottom] = state.split(
        NodePath::new(SurfaceIndex::main(), top),
        Split::Below,
        0.5,
        Node::leaf(tab("bottom")),
    );
    // The whole left-hand subtree is collapsed, so the split above these two is itself
    // collapsed — and it is a split, not a leaf.
    state.main_surface_mut().set_leaf_collapsed(top, true);
    state.main_surface_mut().set_leaf_collapsed(bottom, true);

    let inner = state
        .main_surface()
        .parent(top)
        .expect("the two stacked leaves have a parent");
    let outer = state
        .main_surface()
        .parent(inner)
        .expect("the stack sits beside a column");

    let layout = run(&mut state, &style, true);
    let (outer_rect, inner_rect) = (rect_of(&layout, outer), rect_of(&layout, inner));

    assert!(
        inner_rect.width() > style.tab_bar.height * 2.0,
        "a collapsed split was squeezed to {} px of a {} px row, and its rows of tab bars have \
         nowhere to be drawn",
        inner_rect.width(),
        outer_rect.width()
    );
    assert_eq!(side_strip_of(&layout, inner), None);
    assert_eq!(side_strip_of(&layout, top), None);
}

/// Expanding a strip takes the strip back, in the same context that drew it.
///
/// The geometry map is kept in egui memory and entries outlive the frame that wrote them, so
/// "this leaf is a strip" is a flag that can go *stale*: set it once and never clear it, and
/// the leaf keeps drawing an arrow and no body forever, having been expanded. The clearing
/// happens in `DockLayout::set_rect`, which every laid-out node goes through on every pass —
/// and this is the test that says so, because a fresh context per run cannot see it.
#[test]
fn expanding_a_strip_takes_it_back() {
    let style = style();
    let (mut state, parent, left, _right) = two_columns(Collapse::Left);
    let ctx = Context::default();

    frames(&ctx, &mut state, &style, true);
    let layout = DockLayout::load(&ctx, Id::new(DOCK_ID));
    // The scene this test is about was actually reached — without this the rest passes on a
    // leaf that was never a strip to begin with.
    assert_eq!(
        side_strip_of(&layout, left),
        Some(SideStrip::Left),
        "the leaf has to be a strip before expanding it can prove anything"
    );

    state.main_surface_mut().set_leaf_collapsed(left, false);
    frames(&ctx, &mut state, &style, true);
    let layout = DockLayout::load(&ctx, Id::new(DOCK_ID));

    assert_eq!(
        side_strip_of(&layout, left),
        None,
        "an expanded leaf is still marked as a strip: the flag went stale and it will draw an \
         arrow and no body forever"
    );
    let (parent_rect, left_rect) = (rect_of(&layout, parent), rect_of(&layout, left));
    assert!(
        left_rect.width() > style.tab_bar.height * 2.0,
        "an expanded leaf got {} px of a {} px row",
        left_rect.width(),
        parent_rect.width()
    );
}

/// With the knob off, nothing changes — the old decision still holds.
///
/// The positive control of this file. Without it every assertion above would still pass if the
/// knob were ignored and sideways collapsing were simply always on, which is exactly the
/// mistake an experimental feature must not be able to make quietly.
#[test]
fn the_knob_off_keeps_the_whole_column() {
    let style = style();
    let (mut state, parent, left, _right) = two_columns(Collapse::Left);
    let layout = run(&mut state, &style, false);

    let (parent_rect, left_rect) = (rect_of(&layout, parent), rect_of(&layout, left));

    assert!(
        left_rect.width() > style.tab_bar.height * 2.0,
        "with `collapse_sideways` off a collapsed leaf got {} px of a {} px row: the knob was \
         not read, and layouts that rely on the old behaviour have changed under them",
        left_rect.width(),
        parent_rect.width()
    );
    assert!(
        (left_rect.height() - parent_rect.height()).abs() <= TOLERANCE,
        "and it keeps the full height of its column, as `a_collapsed_leaf_is_one_row` states"
    );
    assert_eq!(side_strip_of(&layout, left), None);
}
